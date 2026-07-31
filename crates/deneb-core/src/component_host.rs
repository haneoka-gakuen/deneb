//! Sandboxed build-time host for `haneoka:deneb-plugin/build-plugin`.
//!
//! The linker intentionally contains only Deneb's five host functions. It does
//! not link WASI, sockets, clocks, environment variables, or process APIs.

use crate::diagnostics::{Diagnostic, Severity};
use crate::distribution::{DistributionScope, enforce_distribution_policy_bytes};
use crate::format::{MAX_PROJECT_BYTES, parse_project};
use crate::model::VegaProject;
use crate::plugin_lock::{PluginLock, PluginSource};
use crate::plugins::{
    PluginBundleError, PluginExecutionRecord, PluginExecutionStatus, load_path_plugin, normal_build_grants,
};
use crate::validate::{validate_portable_relative_path, validate_project};
use camino::{Utf8Path, Utf8PathBuf};
use deneb_plugin_api::{Capability, Hook, HostLimits};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use wasmtime::component::{Component, HasSelf, Linker, ResourceAny};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "build-plugin",
        trappable_imports: true,
        require_store_data_send: true,
    });
}

use bindings::exports::haneoka::deneb_plugin::lifecycle as guest;
use bindings::haneoka::deneb_plugin::{host as host_abi, types as abi};

/// Wasm linear memory available to one build plugin session.
pub const MAX_PLUGIN_MEMORY_BYTES: usize = 64 * 1024 * 1024;
/// Deterministic instruction budget replenished for every boundary call.
pub const MAX_PLUGIN_FUEL_PER_CALL: u64 = 50_000_000;
const MAX_PLUGIN_ASSET_READ_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PLUGIN_TEXT_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ComponentHostError {
    #[error("plugin dependency graph is invalid: {0}")]
    Dependency(String),
    #[error("required plugin `{id}` failed during {stage}: {message}")]
    Required { id: String, stage: String, message: String },
    #[error("plugin host initialization failed: {0}")]
    Runtime(String),
    #[error(transparent)]
    Bundle(#[from] PluginBundleError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Default)]
struct OutputUsage {
    files: u32,
    bytes: u64,
    diagnostics: u32,
    paths: BTreeSet<String>,
}

struct HostState {
    plugin_id: String,
    project_root: Utf8PathBuf,
    output_root: Utf8PathBuf,
    namespace: String,
    project_bytes: Vec<u8>,
    asset_sources: BTreeMap<String, String>,
    grants: BTreeSet<Capability>,
    limits: HostLimits,
    usage: OutputUsage,
    pending_files: BTreeMap<String, Vec<u8>>,
    pending_diagnostics: Vec<abi::Diagnostic>,
    store_limits: StoreLimits,
}

impl HostState {
    fn prepare_hook(&mut self, project: &VegaProject) -> Result<(), ComponentHostError> {
        self.project_bytes = serde_json::to_vec(project)?;
        self.asset_sources = project
            .assets
            .iter()
            .map(|(key, asset)| (key.clone(), asset.source.clone()))
            .collect();
        self.pending_files.clear();
        self.pending_diagnostics.clear();
        Ok(())
    }

    fn has(&self, capability: Capability) -> bool {
        self.grants.contains(&capability)
    }

    fn denied(capability: &str) -> String {
        format!("capability `{capability}` was not granted")
    }

    fn validate_text(value: &str, label: &str) -> Result<(), String> {
        if value.len() > MAX_PLUGIN_TEXT_BYTES || value.chars().any(|character| character == '\0') {
            return Err(format!("{label} exceeds the host text limit"));
        }
        Ok(())
    }

    fn reserve_pending_file(&self, path: &str, bytes: usize) -> Result<(), String> {
        validate_portable_relative_path(path).map_err(|_| format!("unsafe generated path `{path}`"))?;
        if self.usage.paths.contains(path) || self.pending_files.contains_key(path) {
            return Err(format!("generated path `{path}` was emitted more than once"));
        }
        let files = self
            .usage
            .files
            .saturating_add(u32::try_from(self.pending_files.len()).unwrap_or(u32::MAX))
            .saturating_add(1);
        if files > self.limits.max_generated_files {
            return Err("generated-file count limit exceeded".into());
        }
        let pending_bytes = self
            .pending_files
            .values()
            .fold(0_u64, |total, bytes| total.saturating_add(bytes.len() as u64));
        let total = self
            .usage
            .bytes
            .saturating_add(pending_bytes)
            .saturating_add(bytes as u64);
        if total > self.limits.max_generated_bytes {
            return Err("generated-byte limit exceeded".into());
        }
        Ok(())
    }

    fn reserve_pending_diagnostic(&self) -> Result<(), String> {
        let diagnostics = self
            .usage
            .diagnostics
            .saturating_add(u32::try_from(self.pending_diagnostics.len()).unwrap_or(u32::MAX))
            .saturating_add(1);
        if diagnostics > self.limits.max_diagnostics {
            return Err("diagnostic limit exceeded".into());
        }
        Ok(())
    }

    fn read_asset_bytes(&self, key: &str) -> Result<Vec<u8>, String> {
        let source = self
            .asset_sources
            .get(key)
            .ok_or_else(|| format!("unknown asset key `{key}`"))?;
        if source.starts_with("http://") || source.starts_with("https://") {
            return Err("remote assets are not available to the offline component host".into());
        }
        if source.starts_with("data:") {
            return decode_data_uri(source);
        }
        validate_portable_relative_path(source).map_err(|_| "asset path is not portable".to_owned())?;
        let requested = self.project_root.join(source);
        let canonical = canonical_utf8(&requested).map_err(|error| error.to_string())?;
        if !canonical.starts_with(&self.project_root) || !canonical.is_file() {
            return Err("asset resolves outside the project or is not a regular file".into());
        }
        let size = fs::metadata(&canonical).map_err(|error| error.to_string())?.len();
        if size > MAX_PLUGIN_ASSET_READ_BYTES {
            return Err(format!(
                "asset exceeds the {MAX_PLUGIN_ASSET_READ_BYTES}-byte plugin read limit"
            ));
        }
        fs::read(canonical).map_err(|error| error.to_string())
    }

    fn commit_pending(&mut self) -> Result<Vec<Diagnostic>, ComponentHostError> {
        let pending = std::mem::take(&mut self.pending_files);
        let pending_diagnostics = std::mem::take(&mut self.pending_diagnostics);
        let diagnostics = pending_diagnostics
            .into_iter()
            .map(|value| convert_diagnostic(&self.plugin_id, value))
            .collect::<Result<Vec<_>, _>>()?;
        let mut writes = Vec::with_capacity(pending.len());
        for (path, bytes) in pending {
            let destination = self
                .output_root
                .join("generated/plugins")
                .join(&self.namespace)
                .join(&path);
            enforce_distribution_policy_bytes(DistributionScope::PublicPluginPackage, &destination, &bytes)
                .map_err(|error| ComponentHostError::Runtime(error.to_string()))?;
            writes.push((path, bytes, destination));
        }

        let mut written = Vec::with_capacity(writes.len());
        for (_, bytes, destination) in &writes {
            let parent = destination
                .parent()
                .ok_or_else(|| ComponentHostError::Runtime("generated path has no parent".into()))?;
            if let Err(error) = fs::create_dir_all(parent) {
                rollback_generated_files(&written);
                return Err(error.into());
            }
            written.push(destination.clone());
            if let Err(error) = fs::write(destination, bytes) {
                rollback_generated_files(&written);
                return Err(error.into());
            }
        }

        for (path, bytes, _) in writes {
            self.usage.files = self.usage.files.saturating_add(1);
            self.usage.bytes = self.usage.bytes.saturating_add(bytes.len() as u64);
            self.usage.paths.insert(path);
        }
        self.usage.diagnostics = self
            .usage
            .diagnostics
            .saturating_add(u32::try_from(diagnostics.len()).unwrap_or(u32::MAX));
        Ok(diagnostics)
    }

    fn discard_pending(&mut self) {
        self.pending_files.clear();
        self.pending_diagnostics.clear();
    }
}

fn rollback_generated_files(paths: &[Utf8PathBuf]) {
    for path in paths.iter().rev() {
        let _ = fs::remove_file(path);
    }
}

impl abi::Host for HostState {}

impl host_abi::Host for HostState {
    fn log(&mut self, level: String, message: String) -> wasmtime::Result<()> {
        Self::validate_text(&level, "log level").map_err(anyhow::Error::msg)?;
        Self::validate_text(&message, "log message").map_err(anyhow::Error::msg)?;
        self.reserve_pending_diagnostic().map_err(anyhow::Error::msg)?;
        self.pending_diagnostics.push(abi::Diagnostic {
            severity: match level.as_str() {
                "error" => "error",
                "warn" | "warning" => "warning",
                _ => "info",
            }
            .into(),
            code: "plugin.log".into(),
            message,
            path: None,
        });
        Ok(())
    }

    fn read_project(&mut self) -> wasmtime::Result<Result<Vec<u8>, String>> {
        if !self.has(Capability::ReadProject) {
            return Ok(Err(Self::denied("read-project")));
        }
        Ok(Ok(self.project_bytes.clone()))
    }

    fn read_asset(&mut self, key: String) -> wasmtime::Result<Result<Vec<u8>, String>> {
        if !self.has(Capability::ReadAssets) {
            return Ok(Err(Self::denied("read-assets")));
        }
        Self::validate_text(&key, "asset key").map_err(anyhow::Error::msg)?;
        Ok(self.read_asset_bytes(&key))
    }

    fn write_generated(&mut self, path: String, bytes: Vec<u8>) -> wasmtime::Result<Result<(), String>> {
        if !self.has(Capability::WriteGenerated) {
            return Ok(Err(Self::denied("write-generated")));
        }
        if let Err(error) = self.reserve_pending_file(&path, bytes.len()) {
            return Ok(Err(error));
        }
        self.pending_files.insert(path, bytes);
        Ok(Ok(()))
    }

    fn emit_diagnostic(&mut self, value: abi::Diagnostic) -> wasmtime::Result<()> {
        validate_abi_diagnostic(&value).map_err(anyhow::Error::msg)?;
        self.reserve_pending_diagnostic().map_err(anyhow::Error::msg)?;
        self.pending_diagnostics.push(value);
        Ok(())
    }
}

struct ComponentSession {
    id: String,
    required: bool,
    hooks: BTreeSet<Hook>,
    configuration: Vec<u8>,
    engine: Engine,
    store: Store<HostState>,
    guest: bindings::BuildPlugin,
    resource: Option<ResourceAny>,
    active: bool,
    completed_hooks: BTreeSet<Hook>,
    disposed: bool,
    failure: Option<String>,
}

impl ComponentSession {
    #[allow(clippy::too_many_arguments)]
    fn instantiate(
        id: &str,
        version: &str,
        required: bool,
        configuration: &Value,
        project: &VegaProject,
        project_root: &Utf8Path,
        output_root: &Utf8Path,
        build_context_id: &str,
        limits: &HostLimits,
        component_bytes: &[u8],
        manifest_capabilities: &BTreeSet<Capability>,
        manifest_hooks: &BTreeSet<Hook>,
    ) -> Result<Self, ComponentHostError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).map_err(runtime_error)?;
        let component = Component::new(&engine, component_bytes).map_err(runtime_error)?;
        let namespace = plugin_namespace(id);
        let safe_grants = normal_build_grants(manifest_capabilities);
        let state = HostState {
            plugin_id: id.into(),
            project_root: project_root.to_owned(),
            output_root: output_root.to_owned(),
            namespace,
            project_bytes: serde_json::to_vec(project)?,
            asset_sources: project
                .assets
                .iter()
                .map(|(key, asset)| (key.clone(), asset.source.clone()))
                .collect(),
            grants: safe_grants.clone(),
            limits: limits.clone(),
            usage: OutputUsage::default(),
            pending_files: BTreeMap::new(),
            pending_diagnostics: Vec::new(),
            store_limits: StoreLimitsBuilder::new()
                .memory_size(MAX_PLUGIN_MEMORY_BYTES)
                .instances(16)
                .memories(4)
                .tables(4)
                .build(),
        };
        let mut store = Store::new(&engine, state);
        store.limiter(|state| &mut state.store_limits);
        let mut linker = Linker::new(&engine);
        bindings::BuildPlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state).map_err(runtime_error)?;

        let guest = guarded_call(
            &engine,
            &mut store,
            limits.max_execution_millis,
            "instantiate",
            |store| bindings::BuildPlugin::instantiate(store, &component, &linker),
        )?;
        let metadata = guarded_call(&engine, &mut store, limits.max_execution_millis, "metadata", |store| {
            guest.haneoka_deneb_plugin_lifecycle().call_metadata(store)
        })?;
        validate_metadata(&metadata, id, version, manifest_capabilities, manifest_hooks)?;
        let context = abi::SessionContext {
            abi_version: deneb_plugin_api::DENEB_PLUGIN_ABI_VERSION,
            build_id: build_context_id.into(),
            session_id: format!("{build_context_id}/{id}"),
            granted_capabilities: safe_grants.iter().map(capability_to_abi).collect(),
            limits: abi::Limits {
                max_generated_files: limits.max_generated_files,
                max_generated_bytes: limits.max_generated_bytes,
                max_diagnostics: limits.max_diagnostics,
                max_execution_millis: limits.max_execution_millis,
            },
        };
        let resource = guarded_call(&engine, &mut store, limits.max_execution_millis, "create", |store| {
            guest.haneoka_deneb_plugin_lifecycle().call_create(store, &context)
        })?
        .map_err(|error| ComponentHostError::Runtime(format!("create returned {error}")))?;

        Ok(Self {
            id: id.into(),
            required,
            hooks: manifest_hooks.clone(),
            configuration: serde_json::to_vec(configuration)?,
            engine,
            store,
            guest,
            resource: Some(resource),
            active: true,
            completed_hooks: BTreeSet::new(),
            disposed: false,
            failure: None,
        })
    }

    fn run(
        &mut self,
        hook: Hook,
        project: &VegaProject,
    ) -> Result<(Vec<Diagnostic>, Option<Vec<u8>>), ComponentHostError> {
        if !self.active || !self.hooks.contains(&hook) {
            return Ok((Vec::new(), None));
        }
        self.store.data_mut().prepare_hook(project)?;
        let resource = self
            .resource
            .ok_or_else(|| ComponentHostError::Runtime("plugin session resource is absent".into()))?;
        let abi_hook = hook_to_abi(hook);
        let limit = self.store.data().limits.max_execution_millis;
        let output = guarded_call(&self.engine, &mut self.store, limit, hook_name(hook), |store| {
            self.guest.haneoka_deneb_plugin_lifecycle().session().call_run(
                store,
                resource,
                abi_hook,
                &self.configuration,
            )
        })
        .and_then(|result| result.map_err(|error| ComponentHostError::Runtime(format!("{error}"))));
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                self.store.data_mut().discard_pending();
                return Err(error);
            }
        };
        if output.project_patch.is_some()
            && (hook != Hook::Transform || !self.store.data().has(Capability::TransformProject))
        {
            self.store.data_mut().discard_pending();
            return Err(ComponentHostError::Runtime(
                "project patches require the transform hook and transform-project capability".into(),
            ));
        }
        for file in output.generated_files {
            if !self.store.data().has(Capability::WriteGenerated) {
                self.store.data_mut().discard_pending();
                return Err(ComponentHostError::Runtime(HostState::denied("write-generated")));
            }
            if let Err(error) = self.store.data().reserve_pending_file(&file.path, file.bytes.len()) {
                self.store.data_mut().discard_pending();
                return Err(ComponentHostError::Runtime(error));
            }
            self.store.data_mut().pending_files.insert(file.path, file.bytes);
        }
        for diagnostic in output.diagnostics {
            validate_abi_diagnostic(&diagnostic).map_err(ComponentHostError::Runtime)?;
            self.store
                .data()
                .reserve_pending_diagnostic()
                .map_err(ComponentHostError::Runtime)?;
            self.store.data_mut().pending_diagnostics.push(diagnostic);
        }
        let diagnostics = self.store.data_mut().commit_pending()?;
        self.completed_hooks.insert(hook);
        Ok((diagnostics, output.project_patch))
    }

    fn dispose(&mut self) -> Result<Vec<Diagnostic>, ComponentHostError> {
        if !self.active {
            return Ok(Vec::new());
        }
        self.active = false;
        let Some(resource) = self.resource.take() else {
            return Ok(Vec::new());
        };
        let limit = self.store.data().limits.max_execution_millis;
        let dispose_result = guarded_call(&self.engine, &mut self.store, limit, "dispose", |store| {
            self.guest
                .haneoka_deneb_plugin_lifecycle()
                .session()
                .call_dispose(store, resource)
        })
        .and_then(|result| result.map_err(|error| ComponentHostError::Runtime(format!("{error}"))));
        let drop_result = guarded_call(&self.engine, &mut self.store, limit, "resource-drop", |store| {
            resource.resource_drop(store)
        });
        if let Err(error) = dispose_result {
            self.store.data_mut().discard_pending();
            let _ = drop_result;
            self.failure.get_or_insert_with(|| error.to_string());
            return Err(error);
        }
        if let Err(error) = drop_result {
            self.failure.get_or_insert_with(|| error.to_string());
            return Err(error);
        }
        self.disposed = true;
        self.store.data_mut().commit_pending()
    }

    fn execution_record(&self) -> PluginExecutionRecord {
        PluginExecutionRecord {
            status: if self.failure.is_some() {
                PluginExecutionStatus::Failed
            } else if self.disposed {
                PluginExecutionStatus::Executed
            } else {
                PluginExecutionStatus::NotRun
            },
            completed_hooks: self.completed_hooks.clone(),
            disposed: self.disposed,
            generated_root: Some(format!("generated/plugins/{}", plugin_namespace(&self.id))),
            failure: self.failure.clone(),
        }
    }
}

pub(crate) struct ComponentBuildHost {
    sessions: Vec<ComponentSession>,
    lock: PluginLock,
    diagnostics: Vec<Diagnostic>,
    skipped: BTreeMap<String, PluginExecutionRecord>,
}

impl ComponentBuildHost {
    #[allow(clippy::too_many_arguments)]
    pub fn instantiate(
        project: &VegaProject,
        lock: &PluginLock,
        project_root: &Utf8Path,
        output_root: &Utf8Path,
        build_context_id: &str,
        limits: &HostLimits,
        release: bool,
    ) -> Result<Self, ComponentHostError> {
        let order = dependency_order(project, lock)?;
        let requested = project
            .plugins
            .iter()
            .map(|plugin| (plugin.id.as_str(), plugin))
            .collect::<BTreeMap<_, _>>();
        let mut sessions = Vec::new();
        let mut diagnostics = Vec::new();
        let mut skipped = BTreeMap::new();
        for id in order {
            let plugin = requested
                .get(id.as_str())
                .ok_or_else(|| ComponentHostError::Dependency(format!("missing requested plugin `{id}`")))?;
            let locked = lock
                .plugins
                .get(&id)
                .ok_or_else(|| ComponentHostError::Dependency(format!("missing locked plugin `{id}`")))?;
            let path = match &locked.source {
                PluginSource::Path { path } => path,
                PluginSource::Registry { .. } | PluginSource::Git { .. } => {
                    let message = if release {
                        "release build plugin is not materialized as an offline path source"
                    } else {
                        "build plugin is unresolved and cannot execute"
                    };
                    if plugin.required {
                        return Err(ComponentHostError::Required {
                            id,
                            stage: "resolve".into(),
                            message: message.into(),
                        });
                    }
                    diagnostics.push(optional_failure(&id, "resolve", message));
                    skipped.insert(
                        id.clone(),
                        PluginExecutionRecord {
                            status: PluginExecutionStatus::Unresolved,
                            completed_hooks: BTreeSet::new(),
                            disposed: false,
                            generated_root: None,
                            failure: Some(message.into()),
                        },
                    );
                    continue;
                }
            };
            let loaded = match load_path_plugin(project_root, &id, &plugin.version, locked, path) {
                Ok(loaded) => loaded,
                Err(error) if !plugin.required => {
                    diagnostics.push(optional_failure(&id, "load", &error.to_string()));
                    skipped.insert(
                        id.clone(),
                        PluginExecutionRecord {
                            status: PluginExecutionStatus::Failed,
                            completed_hooks: BTreeSet::new(),
                            disposed: false,
                            generated_root: None,
                            failure: Some(error.to_string()),
                        },
                    );
                    continue;
                }
                Err(error) => {
                    return Err(ComponentHostError::Required {
                        id,
                        stage: "load".into(),
                        message: error.to_string(),
                    });
                }
            };
            match ComponentSession::instantiate(
                &id,
                &plugin.version,
                plugin.required,
                &plugin.configuration,
                project,
                project_root,
                output_root,
                build_context_id,
                limits,
                &loaded.component,
                &loaded.manifest.capabilities,
                &loaded.manifest.hooks,
            ) {
                Ok(session) => sessions.push(session),
                Err(error) if !plugin.required => {
                    diagnostics.push(optional_failure(&id, "instantiate", &error.to_string()));
                    skipped.insert(
                        id.clone(),
                        PluginExecutionRecord {
                            status: PluginExecutionStatus::Failed,
                            completed_hooks: BTreeSet::new(),
                            disposed: false,
                            generated_root: Some(format!("generated/plugins/{}", plugin_namespace(&id))),
                            failure: Some(error.to_string()),
                        },
                    );
                }
                Err(error) => {
                    for session in sessions.iter_mut().rev() {
                        let _ = session.dispose();
                    }
                    return Err(ComponentHostError::Required {
                        id,
                        stage: "instantiate".into(),
                        message: error.to_string(),
                    });
                }
            }
        }
        Ok(Self {
            sessions,
            lock: lock.clone(),
            diagnostics,
            skipped,
        })
    }

    pub fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    pub fn run_hook(&mut self, hook: Hook, project: &mut VegaProject) -> Result<Vec<Diagnostic>, ComponentHostError> {
        if hook == Hook::Publish {
            return Err(ComponentHostError::Runtime(
                "publish is not run by the normal build pipeline".into(),
            ));
        }
        let mut diagnostics = Vec::new();
        for index in 0..self.sessions.len() {
            if !self.sessions[index].active || !self.sessions[index].hooks.contains(&hook) {
                continue;
            }
            let result = self.sessions[index].run(hook, project);
            let (mut emitted, patch) = match result {
                Ok(value) => value,
                Err(error) => {
                    let id = self.sessions[index].id.clone();
                    let required = self.sessions[index].required;
                    self.sessions[index].failure = Some(error.to_string());
                    let dispose = self.sessions[index].dispose();
                    if required {
                        self.dispose_remaining();
                        return Err(ComponentHostError::Required {
                            id,
                            stage: hook_name(hook).into(),
                            message: combine_failure(error, dispose.err()),
                        });
                    }
                    diagnostics.push(optional_failure(
                        &id,
                        hook_name(hook),
                        &combine_failure(error, dispose.err()),
                    ));
                    continue;
                }
            };
            if !self.sessions[index].required {
                for item in &mut emitted {
                    if item.severity == Severity::Error {
                        item.severity = Severity::Warning;
                    }
                }
            }
            diagnostics.extend(emitted);
            if let Some(patch) = patch
                && let Err(error) = apply_project_patch(project, &patch, &self.lock)
            {
                let id = self.sessions[index].id.clone();
                let required = self.sessions[index].required;
                self.sessions[index].failure = Some(error.to_string());
                let dispose = self.sessions[index].dispose();
                if required {
                    self.dispose_remaining();
                    return Err(ComponentHostError::Required {
                        id,
                        stage: "transform".into(),
                        message: combine_failure(error, dispose.err()),
                    });
                }
                diagnostics.push(optional_failure(
                    &id,
                    "transform",
                    &combine_failure(error, dispose.err()),
                ));
            }
        }
        Ok(diagnostics)
    }

    pub fn dispose_all(&mut self) -> Result<Vec<Diagnostic>, ComponentHostError> {
        let mut diagnostics = Vec::new();
        let mut required_failure = None;
        for session in self.sessions.iter_mut().rev() {
            if !session.active {
                continue;
            }
            match session.dispose() {
                Ok(emitted) => diagnostics.extend(emitted),
                Err(error) => {
                    if session.required && required_failure.is_none() {
                        required_failure = Some(ComponentHostError::Required {
                            id: session.id.clone(),
                            stage: "dispose".into(),
                            message: error.to_string(),
                        });
                    } else {
                        diagnostics.push(optional_failure(&session.id, "dispose", &error.to_string()));
                    }
                }
            }
        }
        if let Some(error) = required_failure {
            return Err(error);
        }
        Ok(diagnostics)
    }

    pub fn execution_records(&self) -> BTreeMap<String, PluginExecutionRecord> {
        let mut records = self.skipped.clone();
        records.extend(
            self.sessions
                .iter()
                .map(|session| (session.id.clone(), session.execution_record())),
        );
        records
    }

    fn dispose_remaining(&mut self) {
        for session in self.sessions.iter_mut().rev() {
            let _ = session.dispose();
        }
    }
}

impl Drop for ComponentBuildHost {
    fn drop(&mut self) {
        self.dispose_remaining();
    }
}

fn guarded_call<T>(
    engine: &Engine,
    store: &mut Store<HostState>,
    max_millis: u64,
    operation: &str,
    call: impl FnOnce(&mut Store<HostState>) -> wasmtime::Result<T>,
) -> Result<T, ComponentHostError> {
    if max_millis == 0 {
        return Err(ComponentHostError::Runtime(format!(
            "{operation} has a zero millisecond execution budget"
        )));
    }
    store.set_fuel(MAX_PLUGIN_FUEL_PER_CALL).map_err(runtime_error)?;
    store.set_epoch_deadline(1);
    store.epoch_deadline_trap();

    let (cancel, timeout) = mpsc::sync_channel(1);
    let timer_engine = engine.clone();
    let timed_out = Arc::new(AtomicBool::new(false));
    let timer_timed_out = Arc::clone(&timed_out);
    let timer = thread::Builder::new()
        .name("deneb-plugin-deadline".into())
        .spawn(move || {
            if timeout.recv_timeout(Duration::from_millis(max_millis)).is_err() {
                timer_timed_out.store(true, Ordering::Release);
                timer_engine.increment_epoch();
            }
        })
        .map_err(ComponentHostError::Io)?;
    let started = Instant::now();
    let result = call(store);
    let _ = cancel.send(());
    timer
        .join()
        .map_err(|_| ComponentHostError::Runtime("plugin deadline thread panicked".into()))?;
    let elapsed = started.elapsed();
    match result {
        Ok(value) if elapsed <= Duration::from_millis(max_millis) => Ok(value),
        Ok(_) => Err(ComponentHostError::Runtime(format!(
            "{operation} exceeded the {max_millis}ms wall-clock limit"
        ))),
        Err(error) => {
            let message = format!("{error:#}");
            let kind = if message.contains("fuel") {
                "fuel limit exceeded"
            } else if message.contains("epoch")
                || timed_out.load(Ordering::Acquire)
                || elapsed >= Duration::from_millis(max_millis)
            {
                "wall-clock limit exceeded"
            } else if message.contains("memory")
                || message.contains("rust_oom")
                || message.contains("alloc_error")
                || message.contains("failed to grow")
            {
                "memory limit exceeded"
            } else {
                "component trapped"
            };
            Err(ComponentHostError::Runtime(format!("{operation}: {kind}: {message}")))
        }
    }
}

fn validate_metadata(
    metadata: &guest::PluginMetadata,
    id: &str,
    version: &str,
    capabilities: &BTreeSet<Capability>,
    hooks: &BTreeSet<Hook>,
) -> Result<(), ComponentHostError> {
    let metadata_capabilities = metadata
        .capabilities
        .iter()
        .copied()
        .map(capability_from_abi)
        .collect::<BTreeSet<_>>();
    let metadata_hooks = metadata
        .hooks
        .iter()
        .copied()
        .map(hook_from_abi)
        .collect::<BTreeSet<_>>();
    if metadata.id != id
        || metadata.version != version
        || metadata.abi_version != deneb_plugin_api::DENEB_PLUGIN_ABI_VERSION
        || &metadata_capabilities != capabilities
        || metadata.capabilities.len() != metadata_capabilities.len()
        || &metadata_hooks != hooks
        || metadata.hooks.len() != metadata_hooks.len()
    {
        return Err(ComponentHostError::Runtime(
            "component metadata does not exactly match its manifest and lock".into(),
        ));
    }
    Ok(())
}

fn apply_project_patch(project: &mut VegaProject, patch: &[u8], lock: &PluginLock) -> Result<(), ComponentHostError> {
    if patch.len() > MAX_PROJECT_BYTES {
        return Err(ComponentHostError::Runtime(format!(
            "project patch exceeds {MAX_PROJECT_BYTES} bytes"
        )));
    }
    let mut document = serde_json::to_value(&*project)?;
    let patch: Value = serde_json::from_slice(patch)?;
    merge_patch(&mut document, &patch);
    let bytes = serde_json::to_vec(&document)?;
    let transformed = parse_project(&bytes).map_err(|error| ComponentHostError::Runtime(error.to_string()))?;
    if transformed.plugins != project.plugins {
        return Err(ComponentHostError::Runtime(
            "project patch changed the locked plugin request set or identity".into(),
        ));
    }
    let mut build_plugin_scope = transformed.clone();
    build_plugin_scope
        .plugins
        .retain(|plugin| lock.plugins.contains_key(&plugin.id));
    let diagnostics = validate_project(&transformed)
        .into_iter()
        .chain(lock.validate_for_project(&build_plugin_scope))
        .collect::<Vec<_>>();
    if diagnostics.iter().any(|item| item.severity == Severity::Error) {
        let message = diagnostics
            .iter()
            .filter(|item| item.severity == Severity::Error)
            .take(4)
            .map(|item| format!("{} at {}: {}", item.code, item.path, item.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ComponentHostError::Runtime(format!(
            "project patch failed validation: {message}"
        )));
    }
    *project = transformed;
    Ok(())
}

/// RFC 7396 JSON Merge Patch.
fn merge_patch(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(serde_json::Map::new());
    }
    let target = target.as_object_mut().expect("target was made an object");
    for (key, value) in patch {
        if value.is_null() {
            target.remove(key);
        } else {
            merge_patch(target.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}

fn dependency_order(project: &VegaProject, lock: &PluginLock) -> Result<Vec<String>, ComponentHostError> {
    let requested = project
        .plugins
        .iter()
        .map(|plugin| plugin.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut permanent = BTreeSet::new();
    let mut temporary = BTreeSet::new();
    let mut order = Vec::new();
    for id in &requested {
        visit_dependency(id, &requested, lock, &mut temporary, &mut permanent, &mut order)?;
    }
    Ok(order)
}

fn visit_dependency(
    id: &str,
    requested: &BTreeSet<&str>,
    lock: &PluginLock,
    temporary: &mut BTreeSet<String>,
    permanent: &mut BTreeSet<String>,
    order: &mut Vec<String>,
) -> Result<(), ComponentHostError> {
    if permanent.contains(id) {
        return Ok(());
    }
    if !temporary.insert(id.into()) {
        return Err(ComponentHostError::Dependency(format!(
            "dependency cycle contains `{id}`"
        )));
    }
    let plugin = lock
        .plugins
        .get(id)
        .ok_or_else(|| ComponentHostError::Dependency(format!("`{id}` is not locked")))?;
    for dependency in plugin.dependencies.keys() {
        if !requested.contains(dependency.as_str()) {
            return Err(ComponentHostError::Dependency(format!(
                "`{id}` requires `{dependency}`, but the project does not request it"
            )));
        }
        visit_dependency(dependency, requested, lock, temporary, permanent, order)?;
    }
    temporary.remove(id);
    permanent.insert(id.into());
    order.push(id.into());
    Ok(())
}

fn validate_abi_diagnostic(value: &abi::Diagnostic) -> Result<(), String> {
    if !matches!(value.severity.as_str(), "info" | "warning" | "error") {
        return Err(format!("invalid diagnostic severity `{}`", value.severity));
    }
    HostState::validate_text(&value.code, "diagnostic code")?;
    HostState::validate_text(&value.message, "diagnostic message")?;
    if value.code.is_empty() {
        return Err("diagnostic code must not be empty".into());
    }
    if let Some(path) = &value.path {
        HostState::validate_text(path, "diagnostic path")?;
    }
    Ok(())
}

fn convert_diagnostic(plugin_id: &str, value: abi::Diagnostic) -> Result<Diagnostic, ComponentHostError> {
    validate_abi_diagnostic(&value).map_err(ComponentHostError::Runtime)?;
    let severity = match value.severity.as_str() {
        "info" => Severity::Info,
        "warning" => Severity::Warning,
        "error" => Severity::Error,
        _ => unreachable!("validated above"),
    };
    Ok(Diagnostic {
        severity,
        code: format!("plugin.{plugin_id}.{}", value.code),
        path: value.path.unwrap_or_else(|| format!("plugins.{plugin_id}")),
        message: value.message,
    })
}

fn optional_failure(id: &str, stage: &str, message: &str) -> Diagnostic {
    Diagnostic::warning(
        "deneb.plugin.optional-failure",
        format!("plugins.{id}"),
        format!("Optional plugin `{id}` failed during {stage}: {message}"),
    )
}

fn combine_failure(primary: ComponentHostError, dispose: Option<ComponentHostError>) -> String {
    match dispose {
        Some(dispose) => format!("{primary}; dispose also failed: {dispose}"),
        None => primary.to_string(),
    }
}

fn plugin_namespace(id: &str) -> String {
    hex::encode(Sha256::digest(id.as_bytes()))[..16].into()
}

fn decode_data_uri(source: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    let (metadata, payload) = source
        .strip_prefix("data:")
        .and_then(|value| value.split_once(','))
        .ok_or_else(|| "malformed data URI".to_owned())?;
    let bytes = if metadata
        .split(';')
        .any(|segment| segment.eq_ignore_ascii_case("base64"))
    {
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map_err(|error| format!("invalid base64 data URI: {error}"))?
    } else {
        percent_decode(payload)?
    };
    if bytes.len() as u64 > MAX_PLUGIN_ASSET_READ_BYTES {
        return Err(format!(
            "asset exceeds the {MAX_PLUGIN_ASSET_READ_BYTES}-byte plugin read limit"
        ));
    }
    Ok(bytes)
}

fn percent_decode(value: &str) -> Result<Vec<u8>, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let encoded = bytes
                .get(index + 1..index + 3)
                .ok_or_else(|| "truncated percent escape".to_owned())?;
            let encoded = std::str::from_utf8(encoded).map_err(|_| "invalid percent escape".to_owned())?;
            decoded.push(u8::from_str_radix(encoded, 16).map_err(|_| "invalid percent escape".to_owned())?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Ok(decoded)
}

fn canonical_utf8(path: &Utf8Path) -> Result<Utf8PathBuf, std::io::Error> {
    Utf8PathBuf::from_path_buf(fs::canonicalize(path)?).map_err(|path| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("path is not UTF-8: {}", path.display()),
        )
    })
}

fn runtime_error(error: anyhow::Error) -> ComponentHostError {
    ComponentHostError::Runtime(format!("{error:#}"))
}

fn hook_name(hook: Hook) -> &'static str {
    match hook {
        Hook::Validate => "validate",
        Hook::Transform => "transform",
        Hook::Assets => "assets",
        Hook::Emit => "emit",
        Hook::Package => "package",
        Hook::Publish => "publish",
    }
}

fn hook_to_abi(hook: Hook) -> abi::Hook {
    match hook {
        Hook::Validate => abi::Hook::Validate,
        Hook::Transform => abi::Hook::Transform,
        Hook::Assets => abi::Hook::Assets,
        Hook::Emit => abi::Hook::Emit,
        Hook::Package => abi::Hook::Package,
        Hook::Publish => abi::Hook::Publish,
    }
}

fn hook_from_abi(hook: abi::Hook) -> Hook {
    match hook {
        abi::Hook::Validate => Hook::Validate,
        abi::Hook::Transform => Hook::Transform,
        abi::Hook::Assets => Hook::Assets,
        abi::Hook::Emit => Hook::Emit,
        abi::Hook::Package => Hook::Package,
        abi::Hook::Publish => Hook::Publish,
    }
}

fn capability_to_abi(capability: &Capability) -> abi::Capability {
    match capability {
        Capability::ReadProject => abi::Capability::ReadProject,
        Capability::ReadAssets => abi::Capability::ReadAssets,
        Capability::WriteGenerated => abi::Capability::WriteGenerated,
        Capability::TransformProject => abi::Capability::TransformProject,
        Capability::EncodeAsset => abi::Capability::EncodeAsset,
        Capability::TargetProvider => abi::Capability::TargetProvider,
        Capability::Publish => abi::Capability::Publish,
        Capability::Network => abi::Capability::Network,
        Capability::Process => abi::Capability::Process,
    }
}

fn capability_from_abi(capability: abi::Capability) -> Capability {
    match capability {
        abi::Capability::ReadProject => Capability::ReadProject,
        abi::Capability::ReadAssets => Capability::ReadAssets,
        abi::Capability::WriteGenerated => Capability::WriteGenerated,
        abi::Capability::TransformProject => Capability::TransformProject,
        abi::Capability::EncodeAsset => Capability::EncodeAsset,
        abi::Capability::TargetProvider => Capability::TargetProvider,
        abi::Capability::Publish => Capability::Publish,
        abi::Capability::Network => Capability::Network,
        abi::Capability::Process => Capability::Process,
    }
}
