use crate::distribution::{DistributionPolicyError, DistributionScope, enforce_distribution_policy};
use crate::model::VegaProject;
use crate::plugin_lock::{LockedPlugin, PluginLock, PluginSource};
use crate::target::BuildTarget;
use crate::validate::validate_portable_relative_path;
use camino::{Utf8Path, Utf8PathBuf};
use deneb_plugin_api::{Capability, Hook, PluginManifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use wasmparser::{Encoding, Parser, Payload};

pub const PLUGIN_BUNDLE_INDEX_FORMAT: &str = "deneb-plugin-bundle";
pub const PLUGIN_BUNDLE_INDEX_VERSION: u32 = 1;
pub const PLUGIN_SBOM_FORMAT: &str = "deneb-plugin-sbom-summary";
pub const PLUGIN_SBOM_VERSION: u32 = 1;
pub const PLUGIN_MANIFEST_FILE: &str = "deneb.plugin.json";
const MAX_PLUGIN_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_PLUGIN_COMPONENT_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) fn normal_build_grants(capabilities: &BTreeSet<Capability>) -> BTreeSet<Capability> {
    capabilities
        .iter()
        .filter(|capability| {
            !matches!(
                capability,
                Capability::Network | Capability::Process | Capability::Publish
            )
        })
        .cloned()
        .collect()
}

#[derive(Debug, Clone)]
pub(crate) struct LoadedPathPlugin {
    pub root: Utf8PathBuf,
    pub manifest: PluginManifest,
    pub component: Vec<u8>,
    pub checksum: String,
    pub manifest_checksum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PluginBundleIndex {
    pub format: String,
    pub format_version: u32,
    pub plugin_abi_version: u32,
    pub lifecycle: PluginLifecyclePolicy,
    pub plugins: BTreeMap<String, BundledPlugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PluginSbomSummary {
    pub format: String,
    pub format_version: u32,
    pub target: BuildTarget,
    pub lock_digest: Option<String>,
    pub components: Vec<PluginSbomComponent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PluginSbomComponent {
    pub id: String,
    pub version: String,
    pub source: PluginSource,
    pub checksum: String,
    pub manifest_checksum: String,
    pub declared_capabilities: BTreeSet<Capability>,
    pub granted_capabilities: BTreeSet<Capability>,
    pub dependencies: BTreeMap<String, String>,
    pub component: String,
}

pub(crate) struct PluginBundleAudit<'a> {
    pub target: BuildTarget,
    pub source_lock_digest: Option<&'a str>,
    pub execution: &'a BTreeMap<String, PluginExecutionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PluginLifecyclePolicy {
    pub session_scope: String,
    pub dispose_required: bool,
    pub default_deny: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct BundledPlugin {
    pub id: String,
    pub version: String,
    pub required: bool,
    pub status: PluginBundleStatus,
    pub checksum: String,
    pub granted_capabilities: BTreeSet<Capability>,
    pub hooks: BTreeSet<Hook>,
    pub component: Option<String>,
    pub configuration: serde_json::Value,
    pub execution: PluginExecutionRecord,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PluginBundleStatus {
    Bundled,
    Unresolved,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PluginExecutionRecord {
    pub status: PluginExecutionStatus,
    pub completed_hooks: BTreeSet<Hook>,
    pub disposed: bool,
    pub generated_root: Option<String>,
    pub failure: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PluginExecutionStatus {
    Executed,
    Failed,
    Unresolved,
    NotRun,
}

impl PluginExecutionRecord {
    pub fn not_run(id: &str) -> Self {
        Self {
            status: PluginExecutionStatus::NotRun,
            completed_hooks: BTreeSet::new(),
            disposed: false,
            generated_root: Some(format!(
                "generated/plugins/{}",
                &hex::encode(Sha256::digest(id.as_bytes()))[..16]
            )),
            failure: None,
        }
    }
}

impl PluginBundleIndex {
    pub fn empty() -> Self {
        Self {
            format: PLUGIN_BUNDLE_INDEX_FORMAT.into(),
            format_version: PLUGIN_BUNDLE_INDEX_VERSION,
            plugin_abi_version: deneb_plugin_api::DENEB_PLUGIN_ABI_VERSION,
            lifecycle: PluginLifecyclePolicy {
                session_scope: "per-build".into(),
                dispose_required: true,
                default_deny: true,
            },
            plugins: BTreeMap::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginBundleError {
    #[error("plugin `{0}` is missing from the validated lock")]
    MissingLock(String),
    #[error("release plugin `{0}` is not an offline path source")]
    UnresolvedReleaseSource(String),
    #[error("plugin path escapes the project directory: {0}")]
    PathEscape(Utf8PathBuf),
    #[error("plugin bundle is not a directory: {0}")]
    InvalidDirectory(Utf8PathBuf),
    #[error("plugin manifest exceeds {MAX_PLUGIN_MANIFEST_BYTES} bytes: {0}")]
    ManifestTooLarge(Utf8PathBuf),
    #[error("plugin component exceeds {MAX_PLUGIN_COMPONENT_BYTES} bytes: {0}")]
    ComponentTooLarge(Utf8PathBuf),
    #[error("plugin manifest is invalid: {0}")]
    Manifest(String),
    #[error("plugin manifest identity does not match project/lock for `{0}`")]
    Identity(String),
    #[error("plugin capability grant does not exactly match its manifest for `{0}`")]
    CapabilityMismatch(String),
    #[error("plugin component checksum mismatch for `{id}`: expected {expected}, got {actual}")]
    Checksum {
        id: String,
        expected: String,
        actual: String,
    },
    #[error("plugin manifest checksum mismatch for `{id}`: expected {expected}, got {actual}")]
    ManifestChecksum {
        id: String,
        expected: String,
        actual: String,
    },
    #[error("plugin manifest metadata differs from the lock for `{0}`")]
    ManifestMetadata(String),
    #[error("plugin entrypoint escapes its bundle: {0}")]
    EntrypointEscape(Utf8PathBuf),
    #[error("plugin entrypoint is not a WebAssembly Component: {0}")]
    NotAComponent(Utf8PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    DistributionPolicy(#[from] DistributionPolicyError),
}

pub fn bundle_plugins(
    project: &VegaProject,
    lock: &PluginLock,
    project_dir: &Utf8Path,
    web_root: &Utf8Path,
    release: bool,
) -> Result<PluginBundleIndex, PluginBundleError> {
    let digest = lock.digest().ok();
    bundle_plugins_with_execution(
        project,
        lock,
        project_dir,
        web_root,
        release,
        PluginBundleAudit {
            target: BuildTarget::Web,
            source_lock_digest: digest.as_deref(),
            execution: &BTreeMap::new(),
        },
    )
}

pub(crate) fn bundle_plugins_with_execution(
    project: &VegaProject,
    lock: &PluginLock,
    project_dir: &Utf8Path,
    web_root: &Utf8Path,
    release: bool,
    audit: PluginBundleAudit<'_>,
) -> Result<PluginBundleIndex, PluginBundleError> {
    let mut index = PluginBundleIndex::empty();
    let output_root = web_root.join("plugins");
    fs::create_dir_all(&output_root)?;

    for requested in &project.plugins {
        let locked = lock
            .plugins
            .get(&requested.id)
            .ok_or_else(|| PluginBundleError::MissingLock(requested.id.clone()))?;
        let record = audit
            .execution
            .get(&requested.id)
            .cloned()
            .unwrap_or_else(|| PluginExecutionRecord::not_run(&requested.id));
        let bundled = match &locked.source {
            PluginSource::Path { path } => match bundle_path_plugin(
                project_dir,
                &output_root,
                &requested.id,
                &requested.version,
                requested.required,
                &requested.configuration,
                locked,
                path,
                record.clone(),
            ) {
                Ok(bundled) => bundled,
                Err(error) if !requested.required => BundledPlugin {
                    id: requested.id.clone(),
                    version: requested.version.clone(),
                    required: false,
                    status: PluginBundleStatus::Failed,
                    checksum: locked.checksum.clone(),
                    granted_capabilities: normal_build_grants(&locked.capabilities),
                    hooks: BTreeSet::new(),
                    component: None,
                    configuration: requested.configuration.clone(),
                    execution: PluginExecutionRecord {
                        status: PluginExecutionStatus::Failed,
                        failure: Some(error.to_string()),
                        ..record
                    },
                },
                Err(error) => return Err(error),
            },
            PluginSource::Registry { .. } | PluginSource::Git { .. } if release && requested.required => {
                return Err(PluginBundleError::UnresolvedReleaseSource(requested.id.clone()));
            }
            PluginSource::Registry { .. } | PluginSource::Git { .. } => BundledPlugin {
                id: requested.id.clone(),
                version: requested.version.clone(),
                required: requested.required,
                status: PluginBundleStatus::Unresolved,
                checksum: locked.checksum.clone(),
                granted_capabilities: normal_build_grants(&locked.capabilities),
                hooks: BTreeSet::new(),
                component: None,
                configuration: requested.configuration.clone(),
                execution: record,
            },
        };
        index.plugins.insert(requested.id.clone(), bundled);
    }
    fs::write(
        output_root.join("deneb-plugins.json"),
        serde_json::to_vec_pretty(&index)?,
    )?;
    let mut components = Vec::new();
    for plugin in index
        .plugins
        .values()
        .filter(|plugin| plugin.status == PluginBundleStatus::Bundled)
    {
        let Some(locked) = lock.plugins.get(&plugin.id) else {
            continue;
        };
        let Some(component) = plugin.component.clone() else {
            continue;
        };
        let manifest_path = web_root
            .join(&component)
            .parent()
            .ok_or_else(|| PluginBundleError::Manifest("bundled Component has no parent".into()))?
            .join(PLUGIN_MANIFEST_FILE);
        let manifest_checksum = format!("sha256-{}", hex::encode(Sha256::digest(fs::read(manifest_path)?)));
        components.push(PluginSbomComponent {
            id: plugin.id.clone(),
            version: plugin.version.clone(),
            source: locked.source.clone(),
            checksum: plugin.checksum.clone(),
            manifest_checksum,
            declared_capabilities: locked.capabilities.clone(),
            granted_capabilities: plugin.granted_capabilities.clone(),
            dependencies: locked.dependencies.clone(),
            component,
        });
    }
    let sbom = PluginSbomSummary {
        format: PLUGIN_SBOM_FORMAT.into(),
        format_version: PLUGIN_SBOM_VERSION,
        target: audit.target,
        lock_digest: audit.source_lock_digest.map(str::to_owned),
        components,
    };
    fs::write(
        output_root.join("deneb-plugin-sbom.json"),
        serde_json::to_vec_pretty(&sbom)?,
    )?;
    Ok(index)
}

#[allow(clippy::too_many_arguments)]
fn bundle_path_plugin(
    project_dir: &Utf8Path,
    output_root: &Utf8Path,
    id: &str,
    version: &str,
    required: bool,
    configuration: &serde_json::Value,
    locked: &LockedPlugin,
    relative_path: &str,
    execution: PluginExecutionRecord,
) -> Result<BundledPlugin, PluginBundleError> {
    let loaded = load_path_plugin(project_dir, id, version, locked, relative_path)?;
    let LoadedPathPlugin {
        root,
        manifest,
        component,
        checksum,
        manifest_checksum: _,
    } = loaded;

    let directory = format!("{:x}", Sha256::digest(id.as_bytes()));
    let output = output_root.join(&directory[..16]);
    fs::create_dir_all(&output)?;
    let component_relative = format!("plugins/{}/component.wasm", &directory[..16]);
    fs::write(output.join("component.wasm"), component)?;
    fs::write(output.join(PLUGIN_MANIFEST_FILE), serde_json::to_vec_pretty(&manifest)?)?;
    copy_legal_files(&root, &output)?;

    Ok(BundledPlugin {
        id: id.into(),
        version: version.into(),
        required,
        status: PluginBundleStatus::Bundled,
        checksum,
        granted_capabilities: normal_build_grants(&locked.capabilities),
        hooks: manifest.hooks,
        component: Some(component_relative),
        configuration: configuration.clone(),
        execution,
    })
}

pub(crate) fn load_path_plugin(
    project_dir: &Utf8Path,
    id: &str,
    version: &str,
    locked: &LockedPlugin,
    relative_path: &str,
) -> Result<LoadedPathPlugin, PluginBundleError> {
    validate_portable_relative_path(relative_path)
        .map_err(|_| PluginBundleError::PathEscape(project_dir.join(relative_path)))?;
    let requested_root = project_dir.join(relative_path);
    let root = canonical_utf8(&requested_root)?;
    if !root.starts_with(project_dir) {
        return Err(PluginBundleError::PathEscape(requested_root));
    }
    if !root.is_dir() {
        return Err(PluginBundleError::InvalidDirectory(root));
    }
    let manifest_path = root.join(PLUGIN_MANIFEST_FILE);
    if fs::metadata(&manifest_path)?.len() > MAX_PLUGIN_MANIFEST_BYTES {
        return Err(PluginBundleError::ManifestTooLarge(manifest_path));
    }
    enforce_distribution_policy(DistributionScope::PublicPluginPackage, &manifest_path)?;
    let manifest_bytes = fs::read(&manifest_path)?;
    let manifest_checksum = format!("sha256-{}", hex::encode(Sha256::digest(&manifest_bytes)));
    if let Some(expected) = &locked.manifest_checksum
        && expected != &manifest_checksum
    {
        return Err(PluginBundleError::ManifestChecksum {
            id: id.into(),
            expected: expected.clone(),
            actual: manifest_checksum,
        });
    }
    let manifest: PluginManifest = serde_json::from_slice(&manifest_bytes)?;
    manifest
        .validate()
        .map_err(|error| PluginBundleError::Manifest(error.to_string()))?;
    if manifest.id != id || manifest.version != version {
        return Err(PluginBundleError::Identity(id.into()));
    }
    if manifest.capabilities != locked.capabilities {
        return Err(PluginBundleError::CapabilityMismatch(id.into()));
    }
    if manifest.dependencies != locked.dependencies {
        return Err(PluginBundleError::Manifest(format!(
            "plugin `{id}` dependency requirements differ from the validated lock"
        )));
    }
    if locked
        .engine_range
        .as_ref()
        .is_some_and(|range| manifest.engine_range.as_ref() != Some(range))
        || locked
            .targets
            .as_ref()
            .is_some_and(|targets| &manifest.targets != targets)
        || locked
            .conflicts
            .as_ref()
            .is_some_and(|conflicts| &manifest.conflicts != conflicts)
    {
        return Err(PluginBundleError::ManifestMetadata(id.into()));
    }

    let component_path = canonical_utf8(&root.join(&manifest.entrypoint))?;
    if !component_path.starts_with(&root) {
        return Err(PluginBundleError::EntrypointEscape(component_path));
    }
    let metadata = fs::metadata(&component_path)?;
    if !metadata.is_file() {
        return Err(PluginBundleError::EntrypointEscape(component_path));
    }
    if metadata.len() > MAX_PLUGIN_COMPONENT_BYTES {
        return Err(PluginBundleError::ComponentTooLarge(component_path));
    }
    enforce_distribution_policy(DistributionScope::PublicPluginPackage, &component_path)?;
    let component = fs::read(&component_path)?;
    validate_component(&component).map_err(|_| PluginBundleError::NotAComponent(component_path.clone()))?;
    let actual = format!("sha256-{}", hex::encode(Sha256::digest(&component)));
    if manifest.checksum != actual {
        return Err(PluginBundleError::Checksum {
            id: id.into(),
            expected: manifest.checksum,
            actual,
        });
    }
    if locked.checksum != actual {
        return Err(PluginBundleError::Checksum {
            id: id.into(),
            expected: locked.checksum.clone(),
            actual,
        });
    }

    Ok(LoadedPathPlugin {
        root,
        manifest,
        component,
        checksum: actual,
        manifest_checksum,
    })
}

pub(crate) fn validate_component(bytes: &[u8]) -> Result<(), String> {
    let mut payloads = Parser::new(0).parse_all(bytes);
    match payloads.next().transpose().map_err(|error| error.to_string())? {
        Some(Payload::Version {
            encoding: Encoding::Component,
            ..
        }) => {}
        _ => return Err("expected a component".into()),
    }
    for payload in payloads {
        payload.map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn copy_legal_files(root: &Utf8Path, output: &Utf8Path) -> Result<(), PluginBundleError> {
    let mut names = fs::read_dir(root)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_file() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let lowercase = name.to_ascii_lowercase();
            (lowercase.starts_with("license") || lowercase.starts_with("copying") || lowercase.contains("notice"))
                .then_some(name)
        })
        .collect::<Vec<_>>();
    names.sort();
    for name in names {
        let source = root.join(&name);
        let destination = output.join(&name);
        enforce_distribution_policy(DistributionScope::PublicPluginPackage, &source)?;
        fs::copy(source, destination)?;
    }
    Ok(())
}

fn canonical_utf8(path: &Utf8Path) -> Result<Utf8PathBuf, PluginBundleError> {
    Utf8PathBuf::from_path_buf(fs::canonicalize(path)?)
        .map_err(|path| PluginBundleError::PathEscape(Utf8PathBuf::from(path.to_string_lossy().into_owned())))
}
