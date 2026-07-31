use crate::component_host::{ComponentBuildHost, ComponentHostError};
use crate::diagnostics::{Diagnostic, Severity};
use crate::distribution::{DistributionPolicyError, DistributionScope, enforce_distribution_policy};
use crate::external_runtimes::{ExternalRuntimeBundleReport, ExternalRuntimeError, resolve_external_runtimes};
use crate::format::{
    BUILD_MANIFEST_FORMAT, BUILD_MANIFEST_VERSION, BUNDLE_FORMAT, BUNDLE_FORMAT_VERSION, MAX_PROJECT_BYTES,
    ProjectFormatError, parse_project,
};
use crate::model::{CompilerIdentity, VegaBundle, VegaProject};
use crate::plugin_lock::PluginLock;
use crate::plugin_resolution::resolve_plugins;
use crate::plugins::{PluginBundleAudit, PluginBundleError, bundle_plugins_with_execution};
use crate::runtime::{
    RUNTIME_ABI_VERSION, RUNTIME_MANIFEST_FILE, RuntimeBundleManifest, RuntimeContract, RuntimeManifestError,
    RuntimeStatus,
};
use crate::target::{BuildMode, BuildTarget};
use crate::validate::validate_project;
use crate::vega_plugins::{
    VEGA_PLUGIN_ARTIFACT_MAP_FILE, VEGA_PLUGIN_LOCK_FILE, VegaPluginArtifactMap, VegaPluginLock,
    VegaRuntimePluginError, VegaRuntimePluginTarget, read_vega_plugin_artifact_map, read_vega_plugin_lock,
    resolve_vega_runtime_plugins,
};
use base64::Engine;
use camino::{Utf8Path, Utf8PathBuf};
use deneb_plugin_api::{Hook, HostLimits};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, PathBuf};
use tempfile::Builder;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;

const VGB_MAGIC: &[u8] = b"VEGAB\x01\r\n";
const MAX_PLUGIN_LOCK_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ASSET_FILES: usize = 20_000;
const MAX_ASSET_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ASSET_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_RUNTIME_FILES: usize = 50_000;
const MAX_RUNTIME_BYTES: u64 = 1024 * 1024 * 1024;
const MULTI_SCENE_RUNTIME_CAPABILITY: &str = "vega-project-multiscene-v1";
const ELECTRON_VERSION: &str = "43.2.0";
const ELECTRON_BUILDER_VERSION: &str = "26.15.3";
const TAURI_VERSION: &str = "2.11.5";
const TAURI_BUILD_VERSION: &str = "2.6.3";
const TAURI_CLI_VERSION: &str = "2.11.4";
const DENEB_LICENSE: &[u8] = include_bytes!("../LICENSE");
const DENEB_THIRD_PARTY_NOTICES: &[u8] = include_bytes!("../THIRD_PARTY_NOTICES.md");
const DENEB_ICON_SVG: &str = include_str!("../assets/deneb-icon.svg");
const DENEB_ICON_PNG: &[u8] = include_bytes!("../assets/deneb-icon-512.png");

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub project_file: Utf8PathBuf,
    pub output: Utf8PathBuf,
    pub target: BuildTarget,
    /// Preview permits an unbound runtime and remote resources. Release is
    /// offline-closed and requires a compatible runtime.
    pub mode: BuildMode,
    pub force: bool,
    pub archive: bool,
    /// Directory containing `deneb-runtime.json` and a self-contained ESM
    /// runtime bundle. When absent, a complete fail-fast boot shell is emitted.
    pub runtime_bundle: Option<Utf8PathBuf>,
    /// Explicit lock path. When absent, `deneb.lock.json` next to the project is
    /// used if present.
    pub plugin_lock: Option<Utf8PathBuf>,
    /// Canonical Vega runtime-plugin lock. This is independent from the Deneb
    /// Component build-plugin lock.
    pub vega_plugin_lock: Option<Utf8PathBuf>,
    /// Host/audited artifact mapping for locked Vega runtime plugins. Deneb
    /// never downloads or executes entries from the canonical lock.
    pub vega_plugin_artifact_map: Option<Utf8PathBuf>,
    /// Concrete target architecture used for lock compatibility checks.
    pub vega_plugin_architecture: Option<String>,
    /// Separately licensed external runtimes are copied only when this explicit
    /// application-private distribution acknowledgement is enabled.
    pub allow_private_external_runtimes: bool,
    /// Reverse-DNS identifier for generated native hosts. When absent, Deneb
    /// derives a neutral development identifier from the project ID.
    pub bundle_identifier: Option<String>,
    /// Local-storage key used by the generated boot shell for color mode.
    /// When absent, it is namespaced below the resolved bundle identifier.
    pub color_mode_storage_key: Option<String>,
    /// Aggregate output and wall-clock limits applied independently to each
    /// build-scoped Component plugin session.
    pub plugin_limits: HostLimits,
}

impl BuildOptions {
    pub fn new(project_file: impl Into<Utf8PathBuf>, output: impl Into<Utf8PathBuf>, target: BuildTarget) -> Self {
        Self {
            project_file: project_file.into(),
            output: output.into(),
            target,
            mode: BuildMode::Release,
            force: false,
            archive: true,
            runtime_bundle: None,
            plugin_lock: None,
            vega_plugin_lock: None,
            vega_plugin_artifact_map: None,
            vega_plugin_architecture: None,
            allow_private_external_runtimes: false,
            bundle_identifier: None,
            color_mode_storage_key: None,
            plugin_limits: HostLimits::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BuildDistribution {
    #[default]
    Public,
    ApplicationPrivate,
}

fn publicly_redistributable_default() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct BuildReport {
    pub build_id: String,
    pub target: BuildTarget,
    pub mode: BuildMode,
    pub playable: bool,
    pub output: Utf8PathBuf,
    pub archive: Option<Utf8PathBuf>,
    pub runtime: RuntimeStatus,
    pub plugin_lock_digest: Option<String>,
    #[serde(default)]
    pub vega_plugin_lock_digest: Option<String>,
    #[serde(default)]
    pub runtime_plugins: usize,
    #[serde(default)]
    pub distribution: BuildDistribution,
    #[serde(default = "publicly_redistributable_default")]
    pub publicly_redistributable: bool,
    #[serde(default)]
    pub external_runtimes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_runtime_index: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_runtime_sbom: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildManifest {
    format: &'static str,
    format_version: u32,
    build_id: String,
    compiler: CompilerIdentity,
    target: BuildTarget,
    mode: BuildMode,
    playable: bool,
    runtime_abi_version: u32,
    runtime: RuntimeStatus,
    plugin_lock_digest: Option<String>,
    vega_plugin_lock_digest: Option<String>,
    runtime_plugins: usize,
    distribution: BuildDistribution,
    publicly_redistributable: bool,
    external_runtimes: usize,
    external_runtime_index: Option<String>,
    external_runtime_sbom: Option<String>,
    files: BTreeMap<String, String>,
}

#[derive(Debug)]
struct RuntimeSource {
    root: Utf8PathBuf,
    manifest: RuntimeBundleManifest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ApplicationConfiguration {
    bundle_identifier: String,
    color_mode_storage_key: String,
}

#[derive(Debug, Clone, Copy)]
struct TargetScaffoldConfiguration<'a> {
    target: BuildTarget,
    mode: BuildMode,
    playable: bool,
    application_private: bool,
    application: &'a ApplicationConfiguration,
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("project or output path has no usable parent directory")]
    MissingProjectDirectory,
    #[error("unsafe output path: {0}")]
    UnsafeOutput(Utf8PathBuf),
    #[error("output already exists; pass --force to replace it: {0}")]
    OutputExists(Utf8PathBuf),
    #[error("archive already exists; pass --force to replace it: {0}")]
    ArchiveExists(Utf8PathBuf),
    #[error("project validation failed")]
    Validation(Vec<Diagnostic>),
    #[error("project requests plugins but no deneb.lock.json was provided")]
    MissingPluginLock,
    #[error("release builds require --runtime-bundle")]
    ReleaseRuntimeRequired,
    #[error("release builds cannot contain remote asset source `{0}`; copy it into the project first")]
    ReleaseRemoteAsset(String),
    #[error("runtime does not support required release capability `{0}`")]
    RuntimeCapabilityRequired(String),
    #[error("invalid application configuration: {0}")]
    ApplicationConfiguration(String),
    #[error("asset resolves outside the project directory: {0}")]
    AssetEscape(Utf8PathBuf),
    #[error("asset is not a regular file: {0}")]
    InvalidAsset(Utf8PathBuf),
    #[error("asset exceeds the {limit}-byte per-file limit: {path} ({actual} bytes)")]
    AssetTooLarge { path: Utf8PathBuf, actual: u64, limit: u64 },
    #[error("asset graph exceeds the configured limit: {0}")]
    AssetBudget(String),
    #[error(transparent)]
    DistributionPolicy(#[from] DistributionPolicyError),
    #[error("asset integrity mismatch for `{path}`: expected {expected}, got {actual}")]
    AssetIntegrity {
        path: Utf8PathBuf,
        expected: String,
        actual: String,
    },
    #[error("runtime bundle error: {0}")]
    Runtime(String),
    #[error(transparent)]
    RuntimeManifest(#[from] RuntimeManifestError),
    #[error(transparent)]
    ProjectFormat(#[from] ProjectFormatError),
    #[error(transparent)]
    PluginBundle(#[from] PluginBundleError),
    #[error(transparent)]
    ComponentHost(#[from] ComponentHostError),
    #[error(transparent)]
    VegaRuntimePlugin(#[from] VegaRuntimePluginError),
    #[error(transparent)]
    ExternalRuntime(#[from] ExternalRuntimeError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    MessagePack(#[from] rmp_serde::encode::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Walk(#[from] walkdir::Error),
}

pub fn build_project(options: &BuildOptions) -> Result<BuildReport, BuildError> {
    let project_file = canonical_utf8(&options.project_file)?;
    let project_dir = project_file.parent().ok_or(BuildError::MissingProjectDirectory)?;
    let output = absolute_utf8(&options.output)?;
    validate_output_path(project_dir, &project_file, &output)?;
    let output_parent = output.parent().ok_or(BuildError::MissingProjectDirectory)?;
    fs::create_dir_all(output_parent)?;

    let archive_path = options.archive.then(|| output.with_extension("vgpack"));
    preflight_outputs(&output, archive_path.as_deref(), options.force)?;

    let project_size = fs::metadata(&project_file)?.len();
    if project_size > MAX_PROJECT_BYTES as u64 {
        return Err(ProjectFormatError::TooLarge {
            actual: usize::try_from(project_size).unwrap_or(usize::MAX),
            limit: MAX_PROJECT_BYTES,
        }
        .into());
    }
    let source = fs::read(&project_file)?;
    let source_project = parse_project(&source)?;
    let application = resolve_application_configuration(options, &source_project)?;
    let mut diagnostics = validate_project(&source_project);
    validate_build_mode(&source_project, options.mode)?;
    let runtime = options
        .runtime_bundle
        .as_ref()
        .map(|path| load_runtime_source(path, &output))
        .transpose()?;
    if options.mode == BuildMode::Release && runtime.is_none() {
        return Err(BuildError::ReleaseRuntimeRequired);
    }

    let plugin_lock = load_plugin_lock(options, project_dir)?;
    let source_plugin_lock_digest = plugin_lock.as_ref().map(|(_, lock)| lock.digest()).transpose()?;
    let vega_plugin_lock = load_vega_plugin_lock(options, project_dir)?;
    let vega_plugin_artifact_map = load_vega_plugin_artifact_map(options, project_dir)?;
    if vega_plugin_artifact_map.is_some() && vega_plugin_lock.is_none() {
        return Err(BuildError::Runtime(
            "--vega-plugin-artifact-map requires a canonical Vega runtime-plugin lock".into(),
        ));
    }
    if options.allow_private_external_runtimes && vega_plugin_artifact_map.is_none() {
        return Err(BuildError::Runtime(
            "--allow-private-external-runtimes requires a Vega runtime-plugin artifact map".into(),
        ));
    }
    let vega_plugin_lock_digest = vega_plugin_lock.as_ref().map(|(_, lock)| lock.digest()).transpose()?;

    // A canonical Vega lock selects player/runtime plugins. It must never be
    // interpreted as a Deneb Component build-plugin lock. If both independent
    // locks are present, membership partitions the authored request list.
    let mut build_plugin_input = source_project.clone();
    let mut runtime_plugin_input = source_project.clone();
    if let Some((_, vega_lock)) = &vega_plugin_lock {
        if let Some((_, deneb_lock)) = &plugin_lock {
            let vega_ids = vega_lock
                .plugins
                .iter()
                .map(|plugin| plugin.id.as_str())
                .collect::<BTreeSet<_>>();
            let deneb_ids = deneb_lock.plugins.keys().map(String::as_str).collect::<BTreeSet<_>>();
            build_plugin_input
                .plugins
                .retain(|plugin| deneb_ids.contains(plugin.id.as_str()));
            runtime_plugin_input
                .plugins
                .retain(|plugin| vega_ids.contains(plugin.id.as_str()));
            for (index, plugin) in source_project.plugins.iter().enumerate() {
                if !vega_ids.contains(plugin.id.as_str()) && !deneb_ids.contains(plugin.id.as_str()) {
                    let diagnostic = if plugin.required {
                        Diagnostic::error(
                            "deneb.plugin.scope-unassigned",
                            format!("plugins.{index}.id"),
                            "Plugin is selected by neither the Vega runtime lock nor the Deneb build-plugin lock",
                        )
                    } else {
                        Diagnostic::warning(
                            "deneb.plugin.scope-unassigned",
                            format!("plugins.{index}.id"),
                            "Optional plugin is selected by neither lock and will be omitted",
                        )
                    };
                    diagnostics.push(diagnostic);
                }
            }
        } else {
            build_plugin_input.plugins.clear();
        }
    }
    let plugin_resolution = resolve_plugins(
        &build_plugin_input,
        plugin_lock.as_ref().map(|(_, lock)| lock),
        project_dir,
        options.target,
    );
    diagnostics.extend(plugin_resolution.diagnostics.clone());
    if diagnostics.iter().any(|item| item.severity == Severity::Error) {
        return Err(BuildError::Validation(diagnostics));
    }
    let resolved_lock = plugin_lock
        .as_ref()
        .map(|(_, lock)| plugin_resolution.resolved_lock(lock));
    let plugin_lock_digest = resolved_lock.as_ref().map(PluginLock::digest).transpose()?;
    let build_plugin_project = plugin_resolution.resolved_project(&build_plugin_input);

    let runtime_plugin_resolution = vega_plugin_lock.as_ref().map(|(_, lock)| {
        let artifacts = vega_plugin_artifact_map
            .as_ref()
            .map(|(path, map)| (map, path.parent().unwrap_or(project_dir)));
        resolve_vega_runtime_plugins(
            &runtime_plugin_input,
            Some(lock),
            artifacts,
            &VegaRuntimePluginTarget {
                build_target: options.target,
                architecture: options.vega_plugin_architecture.clone(),
                engine_version: runtime
                    .as_ref()
                    .and_then(|runtime| runtime.manifest.engine_version.clone()),
            },
        )
    });
    if let Some(resolution) = &runtime_plugin_resolution {
        diagnostics.extend(resolution.plan.diagnostics.clone());
    }
    if diagnostics.iter().any(|item| item.severity == Severity::Error) {
        return Err(BuildError::Validation(diagnostics));
    }
    let external_runtime_resolution = if let Some((_, lock)) = &vega_plugin_lock {
        let artifacts = vega_plugin_artifact_map
            .as_ref()
            .map(|(path, map)| (map, path.parent().unwrap_or(project_dir)));
        let selected = runtime_plugin_resolution
            .as_ref()
            .map(|resolution| resolution.plan.order.as_slice())
            .unwrap_or(&[]);
        Some(resolve_external_runtimes(
            lock,
            selected,
            artifacts,
            &source_project.id,
            options.allow_private_external_runtimes,
        )?)
    } else {
        None
    };
    let mut project = runtime_plugin_resolution
        .as_ref()
        .map(|resolution| resolution.resolved_project(&runtime_plugin_input))
        .unwrap_or_else(|| plugin_resolution.resolved_project(&source_project));
    let resolved_runtime_plugins = runtime_plugin_resolution.as_ref().map(|_| project.plugins.clone());

    let staging = Builder::new().prefix(".deneb-build-").tempdir_in(output_parent)?;
    let staging_path = Utf8Path::from_path(staging.path()).ok_or(BuildError::MissingProjectDirectory)?;
    let web_root = if options.target.is_web_root() {
        staging_path.to_owned()
    } else {
        let path = staging_path.join("web");
        fs::create_dir_all(&path)?;
        path
    };

    // This deterministic pre-output identity is supplied to plugins. The final
    // build ID is derived later from every emitted byte, including plugin
    // output, so there is no self-referential hash cycle.
    let build_context_id = digest_bytes(&serde_json::to_vec(&(
        digest_bytes(&source),
        &source_plugin_lock_digest,
        &vega_plugin_lock_digest,
        options.target,
        options.mode,
        &application.bundle_identifier,
        &application.color_mode_storage_key,
    ))?);
    let mut component_host = if let Some(lock) = &resolved_lock
        && !build_plugin_project.plugins.is_empty()
    {
        let mut host = ComponentBuildHost::instantiate(
            &build_plugin_project,
            lock,
            project_dir,
            &web_root,
            &build_context_id,
            &options.plugin_limits,
            options.mode == BuildMode::Release,
        )?;
        diagnostics.extend(host.take_diagnostics());
        Some(host)
    } else {
        None
    };
    if let Some(host) = &mut component_host {
        let emitted = host.run_hook(Hook::Validate, &mut project)?;
        accept_plugin_diagnostics(host, &mut diagnostics, emitted)?;
        let emitted = host.run_hook(Hook::Transform, &mut project)?;
        accept_plugin_diagnostics(host, &mut diagnostics, emitted)?;
    }
    if let Some(plugins) = &resolved_runtime_plugins {
        // Build plugins may transform content, but cannot rewrite the
        // independently locked runtime-plugin selection.
        project.plugins.clone_from(plugins);
    }
    validate_build_mode(&project, options.mode)?;
    if let Some(runtime) = &runtime {
        diagnostics.extend(runtime_compatibility_diagnostics(&project, runtime, options.mode)?);
    }
    if diagnostics.iter().any(|item| item.severity == Severity::Error) {
        if let Some(host) = &mut component_host {
            diagnostics.extend(host.dispose_all()?);
        }
        return Err(BuildError::Validation(diagnostics));
    }

    if let Some(host) = &mut component_host {
        let emitted = host.run_hook(Hook::Assets, &mut project)?;
        accept_plugin_diagnostics(host, &mut diagnostics, emitted)?;
    }
    let mut integrity = BTreeMap::new();
    let asset_rewrites = copy_assets(project_dir, &web_root, &mut project, &mut integrity)?;
    rewrite_embedded_asset_sources(&mut project, &asset_rewrites);
    let mut bundle = VegaBundle {
        format: BUNDLE_FORMAT.into(),
        format_version: BUNDLE_FORMAT_VERSION,
        compiler: compiler_identity(),
        project,
        integrity,
    };
    fs::write(
        web_root.join("project.vega.json"),
        serde_json::to_vec_pretty(&bundle.project)?,
    )?;
    let mut binary = VGB_MAGIC.to_vec();
    binary.extend(rmp_serde::to_vec_named(&bundle)?);
    fs::write(web_root.join("project.vgb"), &binary)?;
    if let Some(host) = &mut component_host {
        let emitted = host.run_hook(Hook::Emit, &mut bundle.project)?;
        accept_plugin_diagnostics(host, &mut diagnostics, emitted)?;
    }

    if let Some(lock) = &resolved_lock {
        fs::write(web_root.join("deneb.lock.json"), lock.canonical_bytes()?)?;
    }

    write_deneb_legal(&web_root)?;
    let has_external_runtime_index = external_runtime_resolution
        .as_ref()
        .is_some_and(|resolution| resolution.has_entries());
    let private_distribution = external_runtime_resolution
        .as_ref()
        .is_some_and(|resolution| resolution.has_private_attachments());
    let runtime_status = write_web_runtime(
        &web_root,
        runtime.as_ref(),
        runtime_plugin_resolution.is_some(),
        has_external_runtime_index,
        &application.color_mode_storage_key,
    )?;
    write_web_shell(&web_root, &bundle.project, options.target)?;
    write_target_scaffold(
        staging_path,
        &web_root,
        &bundle.project,
        TargetScaffoldConfiguration {
            target: options.target,
            mode: options.mode,
            playable: runtime_status == RuntimeStatus::Bound,
            application_private: private_distribution,
            application: &application,
        },
    )?;
    if let Some(host) = &mut component_host {
        let emitted = host.run_hook(Hook::Package, &mut bundle.project)?;
        accept_plugin_diagnostics(host, &mut diagnostics, emitted)?;
        diagnostics.extend(host.dispose_all()?);
    }
    let plugin_output_root = web_root.join("plugins");
    fs::create_dir_all(&plugin_output_root)?;
    fs::write(
        plugin_output_root.join("deneb-plugin-resolution.json"),
        serde_json::to_vec_pretty(&plugin_resolution)?,
    )?;
    let empty_lock = PluginLock::default();
    let bundle_lock = resolved_lock.as_ref().unwrap_or(&empty_lock);
    let execution = component_host
        .as_ref()
        .map(ComponentBuildHost::execution_records)
        .unwrap_or_default();
    let bundled_build_plugin_project = if runtime_plugin_resolution.is_some() {
        &build_plugin_project
    } else {
        &bundle.project
    };
    bundle_plugins_with_execution(
        bundled_build_plugin_project,
        bundle_lock,
        project_dir,
        &web_root,
        options.mode == BuildMode::Release,
        PluginBundleAudit {
            target: options.target,
            source_lock_digest: plugin_lock_digest.as_deref(),
            execution: &execution,
        },
    )?;
    let runtime_plugin_count = if let Some(resolution) = &runtime_plugin_resolution {
        resolution.bundle(&web_root)?.plugins.len()
    } else {
        0
    };
    let external_runtime_report = if let Some(resolution) = &external_runtime_resolution {
        resolution.bundle(&web_root)?
    } else {
        ExternalRuntimeBundleReport {
            bundled: 0,
            host: 0,
            publicly_redistributable: true,
            index: None,
            sbom: None,
        }
    };
    if options.target == BuildTarget::Pwa {
        write_pwa(&web_root, &bundle.project)?;
    }

    let files = hash_output_files(staging_path)?;
    let build_id = digest_bytes(&serde_json::to_vec(&files)?);
    let distribution = if external_runtime_report.publicly_redistributable {
        BuildDistribution::Public
    } else {
        BuildDistribution::ApplicationPrivate
    };
    let manifest = BuildManifest {
        format: BUILD_MANIFEST_FORMAT,
        format_version: BUILD_MANIFEST_VERSION,
        build_id: build_id.clone(),
        compiler: compiler_identity(),
        target: options.target,
        mode: options.mode,
        playable: runtime_status == RuntimeStatus::Bound,
        runtime_abi_version: RUNTIME_ABI_VERSION,
        runtime: runtime_status.clone(),
        plugin_lock_digest: plugin_lock_digest.clone(),
        vega_plugin_lock_digest: vega_plugin_lock_digest.clone(),
        runtime_plugins: runtime_plugin_count,
        distribution,
        publicly_redistributable: external_runtime_report.publicly_redistributable,
        external_runtimes: external_runtime_report.bundled,
        external_runtime_index: external_runtime_report.index.clone(),
        external_runtime_sbom: external_runtime_report.sbom.clone(),
        files: files.clone(),
    };
    fs::write(
        web_root.join("deneb-manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;

    let archive_staging = if options.archive {
        let directory = Builder::new().prefix(".deneb-archive-").tempdir_in(output_parent)?;
        let path = Utf8PathBuf::from_path_buf(directory.path().join("artifact.vgpack"))
            .map_err(|_| BuildError::MissingProjectDirectory)?;
        write_archive(staging_path, &path)?;
        Some((directory, path))
    } else {
        None
    };

    let staging_path_buf =
        Utf8PathBuf::from_path_buf(staging.keep()).map_err(|_| BuildError::MissingProjectDirectory)?;
    replace_path(&staging_path_buf, &output, options.force)?;

    if let (Some((_, temporary)), Some(archive)) = (&archive_staging, &archive_path) {
        replace_path(temporary, archive, options.force)?;
    }

    Ok(BuildReport {
        build_id,
        target: options.target,
        mode: options.mode,
        playable: runtime_status == RuntimeStatus::Bound,
        output,
        archive: archive_path,
        runtime: runtime_status,
        plugin_lock_digest,
        vega_plugin_lock_digest,
        runtime_plugins: runtime_plugin_count,
        distribution,
        publicly_redistributable: external_runtime_report.publicly_redistributable,
        external_runtimes: external_runtime_report.bundled,
        external_runtime_index: external_runtime_report.index,
        external_runtime_sbom: external_runtime_report.sbom,
        diagnostics,
        files,
    })
}

fn accept_plugin_diagnostics(
    host: &mut ComponentBuildHost,
    diagnostics: &mut Vec<Diagnostic>,
    emitted: Vec<Diagnostic>,
) -> Result<(), BuildError> {
    let failed = emitted.iter().any(|item| item.severity == Severity::Error);
    diagnostics.extend(emitted);
    if failed {
        diagnostics.extend(host.dispose_all()?);
        return Err(BuildError::Validation(std::mem::take(diagnostics)));
    }
    Ok(())
}

fn compiler_identity() -> CompilerIdentity {
    CompilerIdentity {
        name: "deneb".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

fn load_plugin_lock(
    options: &BuildOptions,
    project_dir: &Utf8Path,
) -> Result<Option<(Utf8PathBuf, PluginLock)>, BuildError> {
    let path = options.plugin_lock.clone().or_else(|| {
        let conventional = project_dir.join("deneb.lock.json");
        conventional.exists().then_some(conventional)
    });
    let Some(path) = path else {
        return Ok(None);
    };
    let path = canonical_utf8(&path)?;
    let metadata = fs::metadata(&path)?;
    if metadata.len() > MAX_PLUGIN_LOCK_BYTES {
        return Err(BuildError::Runtime(format!(
            "plugin lock exceeds {MAX_PLUGIN_LOCK_BYTES} bytes"
        )));
    }
    let lock = serde_json::from_slice(&fs::read(&path)?)?;
    Ok(Some((path, lock)))
}

fn load_vega_plugin_lock(
    options: &BuildOptions,
    project_dir: &Utf8Path,
) -> Result<Option<(Utf8PathBuf, VegaPluginLock)>, BuildError> {
    let path = options.vega_plugin_lock.clone().or_else(|| {
        let conventional = project_dir.join(VEGA_PLUGIN_LOCK_FILE);
        conventional.exists().then_some(conventional)
    });
    let Some(path) = path else {
        return Ok(None);
    };
    let path = canonical_utf8(&path)?;
    let lock = read_vega_plugin_lock(&path)?;
    Ok(Some((path, lock)))
}

fn load_vega_plugin_artifact_map(
    options: &BuildOptions,
    project_dir: &Utf8Path,
) -> Result<Option<(Utf8PathBuf, VegaPluginArtifactMap)>, BuildError> {
    let path = options.vega_plugin_artifact_map.clone().or_else(|| {
        let conventional = project_dir.join(VEGA_PLUGIN_ARTIFACT_MAP_FILE);
        conventional.exists().then_some(conventional)
    });
    let Some(path) = path else {
        return Ok(None);
    };
    let path = canonical_utf8(&path)?;
    let map = read_vega_plugin_artifact_map(&path)?;
    Ok(Some((path, map)))
}

fn load_runtime_source(path: &Utf8Path, output: &Utf8Path) -> Result<RuntimeSource, BuildError> {
    let root = canonical_utf8(path)?;
    if !root.is_dir() {
        return Err(BuildError::Runtime(format!(
            "runtime bundle is not a directory: {root}"
        )));
    }
    if root.starts_with(output) {
        return Err(BuildError::Runtime(
            "runtime bundle cannot be read from inside the output directory".into(),
        ));
    }
    let manifest_path = root.join(RUNTIME_MANIFEST_FILE);
    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|error| BuildError::Runtime(format!("failed to read {manifest_path}: {error}")))?;
    let manifest: RuntimeBundleManifest = serde_json::from_slice(&manifest_bytes)?;
    manifest.validate()?;
    let entrypoint = root.join(&manifest.entrypoint);
    if !entrypoint.is_file() {
        return Err(BuildError::Runtime(format!(
            "runtime entrypoint does not exist: {}",
            manifest.entrypoint
        )));
    }
    Ok(RuntimeSource { root, manifest })
}

fn runtime_compatibility_diagnostics(
    project: &VegaProject,
    runtime: &RuntimeSource,
    mode: BuildMode,
) -> Result<Vec<Diagnostic>, BuildError> {
    let mut diagnostics = Vec::new();
    let capabilities = &runtime.manifest.capabilities;
    if project.scenes.len() > 1 && !capabilities.contains(MULTI_SCENE_RUNTIME_CAPABILITY) {
        if mode == BuildMode::Release {
            return Err(BuildError::RuntimeCapabilityRequired(
                MULTI_SCENE_RUNTIME_CAPABILITY.into(),
            ));
        }
        diagnostics.push(Diagnostic::warning(
            "deneb.runtime.capability-loss.multiscene",
            "runtime.capabilities",
            format!(
                "Project has {} scenes, but the runtime does not declare `{MULTI_SCENE_RUNTIME_CAPABILITY}`",
                project.scenes.len()
            ),
        ));
    }
    if !project.assets.is_empty() && !capabilities.contains("vega-project-assets-v1") {
        diagnostics.push(Diagnostic::warning(
            "deneb.runtime.capability-loss.assets",
            "runtime.capabilities",
            "Project assets are packaged, but this runtime does not declare `vega-project-assets-v1`; asset aliases/catalog metadata may not reach the player",
        ));
    }
    if let Some(features) = project
        .engine
        .as_ref()
        .and_then(|engine| engine.get("features"))
        .and_then(serde_json::Value::as_array)
    {
        for (index, feature) in features.iter().enumerate() {
            if let Some(feature) = feature.as_str()
                && !capabilities.contains(feature)
            {
                diagnostics.push(Diagnostic::warning(
                    "deneb.runtime.capability-loss.feature",
                    format!("engine.features.{index}"),
                    format!("Runtime does not declare required engine feature `{feature}`"),
                ));
            }
        }
    }
    if !runtime_contains_legal_notice(&runtime.root)? {
        diagnostics.push(Diagnostic::warning(
            "deneb.runtime.legal-notice-missing",
            "runtime",
            "Runtime bundle contains no LICENSE, COPYING, or NOTICE file; Deneb can only preserve legal material supplied by the runtime producer",
        ));
    }
    Ok(diagnostics)
}

fn validate_build_mode(project: &VegaProject, mode: BuildMode) -> Result<(), BuildError> {
    if mode != BuildMode::Release {
        return Ok(());
    }
    for asset in project.assets.values() {
        for source in std::iter::once(&asset.source).chain(asset.variants.values()) {
            if source.starts_with("http://") || source.starts_with("https://") {
                return Err(BuildError::ReleaseRemoteAsset(source.clone()));
            }
        }
    }
    for scene in project.scenes.values() {
        for command in &scene.commands {
            for (field, value) in &command.fields {
                if let Some(source) = find_remote_resource(field, value) {
                    return Err(BuildError::ReleaseRemoteAsset(source.into()));
                }
            }
        }
    }
    Ok(())
}

fn find_remote_resource<'a>(field: &str, value: &'a serde_json::Value) -> Option<&'a str> {
    match value {
        serde_json::Value::String(source) if is_embedded_resource_field(field) && is_remote_source(source) => {
            Some(source)
        }
        serde_json::Value::Array(values) => values.iter().find_map(|value| find_remote_resource(field, value)),
        serde_json::Value::Object(fields) => fields
            .iter()
            .find_map(|(field, value)| find_remote_resource(field, value)),
        _ => None,
    }
}

fn runtime_contains_legal_notice(root: &Utf8Path) -> Result<bool, BuildError> {
    for entry in WalkDir::new(root).max_depth(2).follow_links(false).sort_by_file_name() {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with("license") || name.starts_with("copying") || name.contains("notice") {
            return Ok(true);
        }
    }
    Ok(false)
}

fn copy_assets(
    project_dir: &Utf8Path,
    web_root: &Utf8Path,
    project: &mut VegaProject,
    integrity: &mut BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, BuildError> {
    let mut rewrites = BTreeMap::new();
    let mut budget = AssetBudget::default();
    for (key, asset) in &mut project.assets {
        if !is_remote_source(&asset.source) {
            let original = asset.source.clone();
            let (relative, digest, bytes) = copy_asset_source(project_dir, web_root, &original, &mut budget)?;
            if let Some(expected) = &asset.hash
                && expected != &digest
            {
                return Err(BuildError::AssetIntegrity {
                    path: project_dir.join(&original),
                    expected: expected.clone(),
                    actual: digest,
                });
            }
            if let Some(expected) = asset.bytes
                && expected != bytes
            {
                return Err(BuildError::Runtime(format!(
                    "asset byte length mismatch for `{key}`: expected {expected}, got {bytes}"
                )));
            }
            asset.source = relative.clone();
            asset.hash = Some(digest.clone());
            asset.bytes = Some(bytes);
            integrity.insert(key.clone(), digest);
            rewrites.insert(original, relative);
        }
        for (variant, source) in &mut asset.variants {
            if is_remote_source(source) {
                continue;
            }
            let original = source.clone();
            let (relative, digest, _) = copy_asset_source(project_dir, web_root, &original, &mut budget)?;
            *source = relative;
            integrity.insert(format!("{key}#{variant}"), digest);
            rewrites.insert(original, source.clone());
        }
    }
    Ok(rewrites)
}

fn rewrite_embedded_asset_sources(project: &mut VegaProject, rewrites: &BTreeMap<String, String>) -> usize {
    let mut rewritten = 0;
    for scene in project.scenes.values_mut() {
        for command in &mut scene.commands {
            for (field, value) in &mut command.fields {
                rewrite_resource_value(field, value, rewrites, &mut rewritten);
            }
        }
    }
    rewritten
}

fn rewrite_resource_value(
    field: &str,
    value: &mut serde_json::Value,
    rewrites: &BTreeMap<String, String>,
    rewritten: &mut usize,
) {
    match value {
        serde_json::Value::String(source) if is_embedded_resource_field(field) => {
            if let Some(replacement) = rewrites.get(source) {
                source.clone_from(replacement);
                *rewritten += 1;
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                rewrite_resource_value(field, value, rewrites, rewritten);
            }
        }
        serde_json::Value::Object(fields) => {
            for (field, value) in fields {
                rewrite_resource_value(field, value, rewrites, rewritten);
            }
        }
        _ => {}
    }
}

fn is_embedded_resource_field(field: &str) -> bool {
    let field = field.to_ascii_lowercase();
    field.ends_with("url")
        || field.ends_with("urls")
        || field.ends_with("uri")
        || field.ends_with("uris")
        || field.ends_with("texture")
        || field.ends_with("textures")
        || matches!(
            field.as_str(),
            "src"
                | "source"
                | "asset"
                | "image"
                | "audio"
                | "video"
                | "font"
                | "model"
                | "poster"
                | "icon"
                | "background"
                | "foreground"
                | "sprite"
                | "character"
                | "voice"
                | "bgm"
                | "music"
                | "sound"
                | "se"
                | "movie"
                | "thumbnail"
                | "resource"
                | "href"
        )
}

fn write_deneb_legal(web_root: &Utf8Path) -> Result<(), BuildError> {
    let legal = web_root.join("licenses/deneb");
    fs::create_dir_all(&legal)?;
    fs::write(legal.join("LICENSE"), DENEB_LICENSE)?;
    fs::write(legal.join("THIRD_PARTY_NOTICES.md"), DENEB_THIRD_PARTY_NOTICES)?;
    Ok(())
}

fn copy_asset_source(
    project_dir: &Utf8Path,
    web_root: &Utf8Path,
    source: &str,
    budget: &mut AssetBudget,
) -> Result<(String, String, u64), BuildError> {
    if source.starts_with("data:") {
        return copy_data_asset(web_root, source, budget);
    }
    copy_local_asset(project_dir, web_root, source, budget)
}

fn copy_local_asset(
    project_dir: &Utf8Path,
    web_root: &Utf8Path,
    relative_source: &str,
    budget: &mut AssetBudget,
) -> Result<(String, String, u64), BuildError> {
    let requested = project_dir.join(relative_source);
    let source = canonical_utf8(&requested)?;
    if !source.starts_with(project_dir) {
        return Err(BuildError::AssetEscape(requested));
    }
    if !source.is_file() {
        return Err(BuildError::InvalidAsset(source));
    }
    let size = fs::metadata(&source)?.len();
    if size > MAX_ASSET_FILE_BYTES {
        return Err(BuildError::AssetTooLarge {
            path: source,
            actual: size,
            limit: MAX_ASSET_FILE_BYTES,
        });
    }
    budget.reserve(size)?;
    enforce_distribution_policy(DistributionScope::ProjectBundle, &source)?;
    let (hex_digest, bytes) = digest_file(&source)?;
    let digest = format!("sha256-{hex_digest}");
    let extension = portable_extension(&source);
    let hex = digest.trim_start_matches("sha256-");
    let relative = format!("assets/{}/{hex}.{extension}", &hex[..2]);
    let destination = web_root.join(&relative);
    fs::create_dir_all(destination.parent().ok_or(BuildError::MissingProjectDirectory)?)?;
    fs::copy(&source, &destination)?;
    Ok((relative, digest, bytes))
}

fn copy_data_asset(
    web_root: &Utf8Path,
    source: &str,
    budget: &mut AssetBudget,
) -> Result<(String, String, u64), BuildError> {
    let (metadata, payload) = source
        .strip_prefix("data:")
        .and_then(|value| value.split_once(','))
        .ok_or_else(|| BuildError::AssetBudget("malformed data URI".into()))?;
    let base64_encoded = metadata
        .split(';')
        .any(|segment| segment.eq_ignore_ascii_case("base64"));
    let bytes = if base64_encoded {
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map_err(|error| BuildError::AssetBudget(format!("invalid base64 data URI: {error}")))?
    } else {
        percent_decode(payload)?
    };
    let bytes_len = bytes.len() as u64;
    if bytes_len > MAX_ASSET_FILE_BYTES {
        return Err(BuildError::AssetTooLarge {
            path: Utf8PathBuf::from("<data-uri>"),
            actual: bytes_len,
            limit: MAX_ASSET_FILE_BYTES,
        });
    }
    budget.reserve(bytes_len)?;
    let hex = digest_bytes(&bytes);
    let media_type = metadata.split(';').next().unwrap_or_default();
    let extension = media_type_extension(media_type);
    let relative = format!("assets/{}/{}.{}", &hex[..2], hex, extension);
    let destination = web_root.join(&relative);
    fs::create_dir_all(destination.parent().ok_or(BuildError::MissingProjectDirectory)?)?;
    fs::write(&destination, &bytes)?;
    enforce_distribution_policy(DistributionScope::ProjectBundle, &destination)?;
    Ok((relative, format!("sha256-{hex}"), bytes_len))
}

fn percent_decode(value: &str) -> Result<Vec<u8>, BuildError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let encoded = bytes
                .get(index + 1..index + 3)
                .ok_or_else(|| BuildError::AssetBudget("truncated percent escape in data URI".into()))?;
            let encoded = std::str::from_utf8(encoded)
                .map_err(|_| BuildError::AssetBudget("invalid percent escape in data URI".into()))?;
            let value = u8::from_str_radix(encoded, 16)
                .map_err(|_| BuildError::AssetBudget("invalid percent escape in data URI".into()))?;
            decoded.push(value);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Ok(decoded)
}

fn media_type_extension(media_type: &str) -> &'static str {
    match media_type.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/wav" => "wav",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "font/woff2" => "woff2",
        "application/json" => "json",
        "text/plain" => "txt",
        _ => "bin",
    }
}

#[derive(Debug, Default)]
struct AssetBudget {
    files: usize,
    bytes: u64,
}

impl AssetBudget {
    fn reserve(&mut self, bytes: u64) -> Result<(), BuildError> {
        self.files += 1;
        if self.files > MAX_ASSET_FILES {
            return Err(BuildError::AssetBudget(format!(
                "more than {MAX_ASSET_FILES} local asset files"
            )));
        }
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > MAX_ASSET_TOTAL_BYTES {
            return Err(BuildError::AssetBudget(format!(
                "more than {MAX_ASSET_TOTAL_BYTES} local asset bytes"
            )));
        }
        Ok(())
    }
}

fn write_web_runtime(
    web_root: &Utf8Path,
    source: Option<&RuntimeSource>,
    has_runtime_plugins: bool,
    has_external_runtimes: bool,
    color_mode_storage_key: &str,
) -> Result<RuntimeStatus, BuildError> {
    let runtime_root = web_root.join("runtime");
    fs::create_dir_all(&runtime_root)?;
    fs::write(runtime_root.join("boot.js"), render_boot_js(color_mode_storage_key)?)?;
    fs::write(runtime_root.join("shell.css"), SHELL_CSS)?;
    let contract = if let Some(source) = source {
        let bundle_root = runtime_root.join("bundle");
        let integrity = copy_runtime_tree(source, &bundle_root)?;
        RuntimeContract::bound(&source.manifest, integrity)
    } else {
        RuntimeContract::unbound()
    }
    .with_runtime_plugins(has_runtime_plugins)
    .with_external_runtimes(has_external_runtimes);
    fs::write(
        runtime_root.join("contract.json"),
        serde_json::to_vec_pretty(&contract)?,
    )?;
    Ok(contract.status)
}

fn copy_runtime_tree(source: &RuntimeSource, destination: &Utf8Path) -> Result<BTreeMap<String, String>, BuildError> {
    fs::create_dir_all(destination)?;
    let mut file_count = 0_usize;
    let mut byte_count = 0_u64;
    let mut actual = BTreeMap::new();
    for entry in WalkDir::new(&source.root).follow_links(false).sort_by_file_name() {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            return Err(BuildError::Runtime(format!(
                "runtime bundles may not contain symlinks: {}",
                entry.path().display()
            )));
        }
        if !entry.file_type().is_file() {
            continue;
        }
        file_count += 1;
        if file_count > MAX_RUNTIME_FILES {
            return Err(BuildError::Runtime(format!(
                "runtime bundle exceeds {MAX_RUNTIME_FILES} files"
            )));
        }
        let source_path = Utf8Path::from_path(entry.path()).ok_or(BuildError::MissingProjectDirectory)?;
        enforce_distribution_policy(DistributionScope::RuntimeBundle, source_path)?;
        let relative = source_path
            .strip_prefix(&source.root)
            .map_err(|_| BuildError::Runtime("runtime walk escaped its root".into()))?;
        let size = fs::metadata(source_path)?.len();
        byte_count = byte_count.saturating_add(size);
        if byte_count > MAX_RUNTIME_BYTES {
            return Err(BuildError::Runtime(format!(
                "runtime bundle exceeds {MAX_RUNTIME_BYTES} bytes"
            )));
        }
        let target = destination.join(relative);
        fs::create_dir_all(target.parent().ok_or(BuildError::MissingProjectDirectory)?)?;
        let (digest, copied) = copy_file_hashed(source_path, &target)?;
        if copied != size {
            return Err(BuildError::Runtime(format!(
                "runtime file changed while it was copied: {relative}"
            )));
        }
        let digest = format!("sha256-{digest}");
        actual.insert(relative.as_str().replace('\\', "/"), digest);
    }
    for (path, expected) in &source.manifest.integrity {
        let actual_digest = actual
            .get(path)
            .ok_or_else(|| BuildError::Runtime(format!("runtime integrity entry is missing: {path}")))?;
        if actual_digest != expected {
            return Err(BuildError::Runtime(format!(
                "runtime integrity mismatch for `{path}`: expected {expected}, got {actual_digest}"
            )));
        }
    }
    if !actual.contains_key(&source.manifest.entrypoint) {
        return Err(BuildError::Runtime(
            "runtime entrypoint was not copied as a regular file".into(),
        ));
    }
    Ok(actual
        .into_iter()
        .map(|(path, digest)| (format!("bundle/{path}"), digest))
        .collect())
}

fn write_web_shell(web_root: &Utf8Path, project: &VegaProject, target: BuildTarget) -> Result<(), BuildError> {
    let title = html_escape(match &project.title {
        serde_json::Value::String(value) => value,
        _ => &project.id,
    });
    let pwa = if target == BuildTarget::Pwa {
        r#"<link rel="manifest" href="./manifest.webmanifest">"#
    } else {
        ""
    };
    let html = format!(
        r##"<!doctype html>
<html lang="en" data-deneb-target="{target}">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
  <meta name="theme-color" content="#f7f8fb">
  <meta http-equiv="Content-Security-Policy" content="default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob: https: http:; media-src 'self' data: blob: https: http:; connect-src 'self' https: http:; worker-src 'self'; object-src 'none'; base-uri 'none'">
  <title>{title}</title>
  <link rel="icon" href="./deneb-icon.svg" type="image/svg+xml">
  <link rel="apple-touch-icon" href="./deneb-icon-512.png">
  <link rel="stylesheet" href="./runtime/shell.css">
  {pwa}
</head>
<body>
  <main id="vega" aria-live="polite">
    <section id="deneb-boot" role="status">
      <span class="deneb-star" aria-hidden="true">✦</span>
      <p class="deneb-eyebrow">DENEB</p>
      <h1>{title}</h1>
      <p id="deneb-status">Loading…</p>
      <pre id="deneb-error" hidden></pre>
    </section>
  </main>
  <script type="module" src="./runtime/boot.js"></script>
</body>
</html>
"##
    );
    fs::write(web_root.join("index.html"), html)?;
    fs::write(web_root.join("deneb-icon.svg"), DENEB_ICON_SVG)?;
    fs::write(web_root.join("deneb-icon-512.png"), DENEB_ICON_PNG)?;
    Ok(())
}

fn write_target_scaffold(
    staging: &Utf8Path,
    web_root: &Utf8Path,
    project: &VegaProject,
    configuration: TargetScaffoldConfiguration<'_>,
) -> Result<(), BuildError> {
    let TargetScaffoldConfiguration {
        target,
        mode,
        playable,
        application_private,
        application,
    } = configuration;
    match target {
        BuildTarget::Web | BuildTarget::Pwa => {}
        BuildTarget::Electron => write_electron(staging, project, application)?,
        BuildTarget::Tauri | BuildTarget::Android | BuildTarget::Ios => {
            write_tauri(staging, project, target, application)?
        }
    }
    let web_root_relative = web_root.strip_prefix(staging).unwrap_or(Utf8Path::new("")).as_str();
    let (commands, requirements): (Vec<&str>, Vec<&str>) = match target {
        BuildTarget::Web => (
            vec!["serve the output directory over HTTP"],
            vec!["ES2022 browser", "Vega runtime bundle for playable output"],
        ),
        BuildTarget::Pwa => (
            vec!["serve the output directory over HTTPS"],
            vec![
                "service-worker capable browser",
                "Vega runtime bundle for playable output",
            ],
        ),
        BuildTarget::Electron => (
            vec!["npm install", "npm run package"],
            vec!["Node.js >=22", "platform signing identity for release"],
        ),
        BuildTarget::Tauri => (
            vec!["npm install", "npm run package"],
            vec!["Rust", "Node.js >=22", "Tauri platform prerequisites"],
        ),
        BuildTarget::Android => (
            vec!["npm install", "npm run android:init", "npm run android:build"],
            vec!["Android Studio", "Android SDK/NDK", "Rust Android targets"],
        ),
        BuildTarget::Ios => (
            vec!["npm install", "npm run ios:init", "npm run ios:build"],
            vec!["macOS", "Xcode", "CocoaPods", "Rust Apple targets"],
        ),
    };
    fs::write(
        staging.join("deneb-target.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "format": "deneb-target",
            "formatVersion": 1,
            "target": target,
            "mode": mode,
            "playable": playable,
            "distribution": if application_private { "application-private" } else { "public" },
            "publiclyRedistributable": !application_private,
            "provider": target.provider(),
            "bundleIdentifier": application.bundle_identifier,
            "colorModeStorageKey": application.color_mode_storage_key,
            "webRoot": if web_root_relative.is_empty() { "." } else { web_root_relative },
            "commands": commands,
            "requirements": requirements,
            "artifactKind": if matches!(target, BuildTarget::Web | BuildTarget::Pwa) {
                "static-bundle"
            } else {
                "buildable-source-project"
            },
            "compileVerified": matches!(target, BuildTarget::Web | BuildTarget::Pwa),
            "scaffoldOnly": !matches!(target, BuildTarget::Web | BuildTarget::Pwa),
        }))?,
    )?;
    Ok(())
}

fn write_pwa(web_root: &Utf8Path, project: &VegaProject) -> Result<(), BuildError> {
    let name = match &project.title {
        serde_json::Value::String(value) => value.as_str(),
        _ => project.id.as_str(),
    };
    fs::write(
        web_root.join("manifest.webmanifest"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "id": "./",
            "name": name,
            "short_name": truncate_chars(name, 24),
            "start_url": "./",
            "scope": "./",
            "display": "standalone",
            "orientation": "any",
            "background_color": "#f7f8fb",
            "theme_color": "#f7f8fb",
            "icons": [{
                "src": "./deneb-icon.svg",
                "sizes": "any",
                "type": "image/svg+xml",
                "purpose": "any maskable"
            }, {
                "src": "./deneb-icon-512.png",
                "sizes": "512x512",
                "type": "image/png",
                "purpose": "any"
            }]
        }))?,
    )?;
    let mut urls = BTreeSet::from([
        "./".to_owned(),
        "./deneb-manifest.json".to_owned(),
        "./index.html".to_owned(),
    ]);
    let mut integrity = BTreeMap::new();
    for entry in WalkDir::new(web_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = Utf8Path::from_path(entry.path()).ok_or(BuildError::MissingProjectDirectory)?;
        let relative = path
            .strip_prefix(web_root)
            .map_err(|_| BuildError::Runtime("PWA walk escaped the web root".into()))?
            .as_str()
            .replace('\\', "/");
        if !matches!(
            relative.as_str(),
            "sw.js" | "deneb-precache.json" | "deneb-manifest.json"
        ) {
            urls.insert(format!("./{relative}"));
            let (digest, _) = digest_file(path)?;
            integrity.insert(relative, digest);
        }
    }
    let precache = serde_json::to_vec_pretty(&urls)?;
    let cache_seed = serde_json::to_vec(&(&urls, &integrity))?;
    let cache = format!("deneb-{}", &digest_bytes(&cache_seed)[..16]);
    fs::write(web_root.join("deneb-precache.json"), &precache)?;
    fs::write(web_root.join("sw.js"), PWA_WORKER.replace("__DENEB_CACHE__", &cache))?;
    Ok(())
}

fn write_electron(
    staging: &Utf8Path,
    project: &VegaProject,
    application: &ApplicationConfiguration,
) -> Result<(), BuildError> {
    fs::write(
        staging.join("package.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": project.id,
            "version": "0.1.0",
            "private": true,
            "main": "platform/electron/main.cjs",
            "engines": { "node": ">=22" },
            "scripts": {
                "start": "electron .",
                "package": "electron-builder --publish never"
            },
            "devDependencies": {
                "electron": ELECTRON_VERSION,
                "electron-builder": ELECTRON_BUILDER_VERSION
            },
            "build": {
                "appId": application.bundle_identifier,
                "productName": display_title(project),
                "icon": "web/deneb-icon-512.png",
                "asar": true,
                "directories": { "output": "release" },
                "files": ["web/**/*", "platform/electron/**/*", "package.json"]
            }
        }))?,
    )?;
    let platform = staging.join("platform/electron");
    fs::create_dir_all(&platform)?;
    fs::write(platform.join("main.cjs"), ELECTRON_MAIN)?;
    fs::write(
        platform.join("README.md"),
        "# Electron host\n\nRun `npm install`, commit the generated `package-lock.json`, then use `npm run package`. Release artifacts still require the platform's signing/notarization credentials.\n",
    )?;
    Ok(())
}

fn write_tauri(
    staging: &Utf8Path,
    project: &VegaProject,
    target: BuildTarget,
    application: &ApplicationConfiguration,
) -> Result<(), BuildError> {
    let tauri = staging.join("src-tauri");
    fs::create_dir_all(tauri.join("src"))?;
    fs::create_dir_all(tauri.join("capabilities"))?;
    fs::create_dir_all(tauri.join("icons"))?;
    fs::write(tauri.join("icons/icon.png"), DENEB_ICON_PNG)?;
    let crate_name = rust_identifier(&project.id);
    fs::write(
        tauri.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{crate_name}"
version = "0.1.0"
edition = "2024"
rust-version = "1.88"
publish = false

[lib]
name = "{crate_name}_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = "={TAURI_BUILD_VERSION}"

[dependencies]
tauri = {{ version = "={TAURI_VERSION}", features = [] }}

# Keep generated hosts independent when output below another Cargo workspace.
[workspace]
resolver = "2"
"#
        ),
    )?;
    fs::write(tauri.join("build.rs"), "fn main() { tauri_build::build() }\n")?;
    fs::write(
        tauri.join("src/lib.rs"),
        format!(
            "#![forbid(unsafe_code)]\n\n#[cfg_attr(mobile, tauri::mobile_entry_point)]\npub fn run() {{\n    tauri::Builder::default()\n        .run(tauri::generate_context!())\n        .expect(\"{crate_name}: Tauri runtime failed\");\n}}\n"
        ),
    )?;
    fs::write(
        tauri.join("src/main.rs"),
        format!("#![forbid(unsafe_code)]\n\nfn main() {{ {crate_name}_lib::run(); }}\n"),
    )?;
    fs::write(
        tauri.join("capabilities/default.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "$schema": "../gen/schemas/desktop-schema.json",
            "identifier": "default",
            "description": "Minimum capability set for the Vega player window",
            "windows": ["main"],
            "permissions": ["core:default"]
        }))?,
    )?;
    fs::write(
        tauri.join("tauri.conf.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "$schema": "https://schema.tauri.app/config/2",
            "productName": display_title(project),
            "version": "0.1.0",
            "identifier": application.bundle_identifier,
            "build": { "frontendDist": "../web" },
            "app": {
                "windows": [{
                    "label": "main",
                    "title": display_title(project),
                    "url": "index.html",
                    "width": 1280,
                    "height": 720,
                    "minWidth": 640,
                    "minHeight": 360,
                    "resizable": true,
                    "fullscreen": false
                }],
                "security": {
                    "csp": "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' asset: data: blob: https: http:; media-src 'self' asset: data: blob: https: http:; connect-src 'self' ipc: http://ipc.localhost https: http:; object-src 'none'; base-uri 'none'"
                }
            },
            "bundle": {
                "active": true,
                "icon": ["icons/icon.png"]
            }
        }))?,
    )?;
    let scripts = match target {
        BuildTarget::Android => serde_json::json!({
            "tauri": "tauri",
            "android:init": "tauri android init",
            "android:dev": "tauri android dev",
            "android:build": "tauri android build"
        }),
        BuildTarget::Ios => serde_json::json!({
            "tauri": "tauri",
            "ios:init": "tauri ios init",
            "ios:dev": "tauri ios dev",
            "ios:build": "tauri ios build"
        }),
        _ => serde_json::json!({
            "tauri": "tauri",
            "dev": "tauri dev",
            "package": "tauri build"
        }),
    };
    fs::write(
        staging.join("package.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": project.id,
            "version": "0.1.0",
            "private": true,
            "engines": { "node": ">=22" },
            "scripts": scripts,
            "devDependencies": { "@tauri-apps/cli": TAURI_CLI_VERSION }
        }))?,
    )?;
    fs::write(staging.join("HOSTING.md"), target_hosting_readme(target))?;
    Ok(())
}

fn hash_output_files(root: &Utf8Path) -> Result<BTreeMap<String, String>, BuildError> {
    let mut files = BTreeMap::new();
    for entry in WalkDir::new(root).follow_links(false).sort_by_file_name() {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            return Err(BuildError::Runtime(format!(
                "generated output unexpectedly contains a symlink: {}",
                entry.path().display()
            )));
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let path = Utf8Path::from_path(entry.path()).ok_or(BuildError::MissingProjectDirectory)?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| BuildError::Runtime("output walk escaped its root".into()))?
            .as_str()
            .replace('\\', "/");
        let (digest, _) = digest_file(path)?;
        files.insert(relative, format!("sha256-{digest}"));
    }
    Ok(files)
}

fn write_archive(root: &Utf8Path, output: &Utf8Path) -> Result<(), BuildError> {
    let file = File::create(output)?;
    let mut archive = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for entry in WalkDir::new(root).sort_by_file_name() {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = Utf8Path::from_path(entry.path()).ok_or(BuildError::MissingProjectDirectory)?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| BuildError::Runtime("archive walk escaped its root".into()))?
            .as_str()
            .replace('\\', "/");
        archive.start_file(relative, options)?;
        let mut source = File::open(path)?;
        std::io::copy(&mut source, &mut archive)?;
    }
    archive.finish()?;
    Ok(())
}

fn preflight_outputs(output: &Utf8Path, archive: Option<&Utf8Path>, force: bool) -> Result<(), BuildError> {
    if output.exists() && !force {
        return Err(BuildError::OutputExists(output.to_owned()));
    }
    if let Some(archive) = archive
        && archive.exists()
        && !force
    {
        return Err(BuildError::ArchiveExists(archive.to_owned()));
    }
    Ok(())
}

fn validate_output_path(project_dir: &Utf8Path, project_file: &Utf8Path, output: &Utf8Path) -> Result<(), BuildError> {
    if output.file_name().is_none()
        || output == project_dir
        || project_file.starts_with(output)
        || output.as_str() == "/"
    {
        return Err(BuildError::UnsafeOutput(output.to_owned()));
    }
    Ok(())
}

fn replace_path(source: &Utf8Path, destination: &Utf8Path, force: bool) -> Result<(), BuildError> {
    if !destination.exists() {
        fs::rename(source, destination)?;
        return Ok(());
    }
    if !force {
        return Err(BuildError::OutputExists(destination.to_owned()));
    }
    let parent = destination.parent().ok_or(BuildError::MissingProjectDirectory)?;
    let backup_directory = Builder::new().prefix(".deneb-backup-").tempdir_in(parent)?;
    let backup = Utf8PathBuf::from_path_buf(backup_directory.path().join("previous"))
        .map_err(|_| BuildError::MissingProjectDirectory)?;
    fs::rename(destination, &backup)?;
    if let Err(error) = fs::rename(source, destination) {
        let _ = fs::rename(&backup, destination);
        return Err(BuildError::Io(error));
    }
    Ok(())
}

fn canonical_utf8(path: &Utf8Path) -> Result<Utf8PathBuf, BuildError> {
    Utf8PathBuf::from_path_buf(fs::canonicalize(path)?)
        .map_err(|_| BuildError::Runtime(format!("path is not valid UTF-8: {path}")))
}

fn absolute_utf8(path: &Utf8Path) -> Result<Utf8PathBuf, BuildError> {
    let path = if path.is_absolute() {
        PathBuf::from(path)
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Utf8PathBuf::from_path_buf(normalized)
        .map_err(|path| BuildError::Runtime(format!("path is not valid UTF-8: {}", path.display())))
}

fn is_remote_source(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn portable_extension(path: &Utf8Path) -> String {
    path.extension()
        .filter(|extension| {
            !extension.is_empty() && extension.len() <= 12 && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "bin".into())
}

fn digest_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn digest_file(path: &Utf8Path) -> Result<(String, u64), BuildError> {
    let mut source = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        total = total.saturating_add(count as u64);
    }
    Ok((hex::encode(hasher.finalize()), total))
}

fn copy_file_hashed(source: &Utf8Path, destination: &Utf8Path) -> Result<(String, u64), BuildError> {
    let mut source = File::open(source)?;
    let mut destination = File::create(destination)?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        destination.write_all(&buffer[..count])?;
        total = total.saturating_add(count as u64);
    }
    destination.sync_all()?;
    Ok((hex::encode(hasher.finalize()), total))
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn rust_identifier(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("vega_{}", sanitized.trim_matches('_'))
}

fn default_bundle_identifier(project_id: &str) -> String {
    let suffix = project_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("app.vega.{}", suffix.trim_matches('-'))
}

fn resolve_application_configuration(
    options: &BuildOptions,
    project: &VegaProject,
) -> Result<ApplicationConfiguration, BuildError> {
    let bundle_identifier = options
        .bundle_identifier
        .clone()
        .unwrap_or_else(|| default_bundle_identifier(&project.id));
    if !is_bundle_identifier(&bundle_identifier) {
        return Err(BuildError::ApplicationConfiguration(format!(
            "bundle identifier `{bundle_identifier}` must be a portable reverse-DNS name"
        )));
    }
    let color_mode_storage_key = options
        .color_mode_storage_key
        .clone()
        .unwrap_or_else(|| format!("{bundle_identifier}.shell.color-mode"));
    if color_mode_storage_key.trim() != color_mode_storage_key
        || color_mode_storage_key.is_empty()
        || color_mode_storage_key.len() > 256
        || color_mode_storage_key.chars().any(char::is_control)
    {
        return Err(BuildError::ApplicationConfiguration(
            "color-mode storage key must be 1–256 non-control bytes without surrounding whitespace".into(),
        ));
    }
    Ok(ApplicationConfiguration {
        bundle_identifier,
        color_mode_storage_key,
    })
}

fn is_bundle_identifier(value: &str) -> bool {
    value.len() <= 255
        && value.split('.').count() >= 2
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment.len() <= 63
                && segment.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && segment.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                && segment.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
        })
}

fn display_title(project: &VegaProject) -> &str {
    project.title.as_str().unwrap_or(&project.id)
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn target_hosting_readme(target: BuildTarget) -> &'static str {
    match target {
        BuildTarget::Android => {
            "# Android host\n\nRun `npm install`, commit `package-lock.json`, then run `npm run android:init` once and commit the generated native project. `npm run android:build` requires a configured Android SDK/NDK and release signing for distributable artifacts.\n"
        }
        BuildTarget::Ios => {
            "# iOS host\n\nRun on macOS with Xcode and CocoaPods. Run `npm install`, commit `package-lock.json`, then run `npm run ios:init` once and commit the generated Xcode project. Distribution requires an Apple signing team and provisioning profile.\n"
        }
        _ => {
            "# Tauri host\n\nRun `npm install`, commit `package-lock.json`, then use `npm run package`. Install the official Tauri platform prerequisites first. The generated Rust application deliberately exposes only `core:default` permissions.\n"
        }
    }
}

fn render_boot_js(color_mode_storage_key: &str) -> Result<String, BuildError> {
    let literal = serde_json::to_string(color_mode_storage_key)?;
    Ok(BOOT_JS_TEMPLATE.replacen("__DENEB_COLOR_MODE_STORAGE_KEY__", &literal, 1))
}

const BOOT_JS_TEMPLATE: &str = r##"const COLOR_MODE_KEY = __DENEB_COLOR_MODE_STORAGE_KEY__;
const systemColorQuery = globalThis.matchMedia?.("(prefers-color-scheme: dark)");

function readColorMode() {
  try {
    const value = localStorage.getItem(COLOR_MODE_KEY);
    return value === "dark" || value === "system" || value === "light" ? value : "light";
  } catch {
    return "light";
  }
}

function applyColorMode() {
  const mode = readColorMode();
  document.documentElement.dataset.denebColorMode = mode;
  const dark = mode === "dark" || (mode === "system" && systemColorQuery?.matches);
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", dark ? "#090b11" : "#f7f8fb");
}

const colorModeChanged = () => applyColorMode();
const colorModeStored = (event) => {
  if (event.key === COLOR_MODE_KEY) applyColorMode();
};
applyColorMode();
systemColorQuery?.addEventListener?.("change", colorModeChanged);
addEventListener("storage", colorModeStored);

const root = document.querySelector("#vega");
const status = document.querySelector("#deneb-status");
const errorView = document.querySelector("#deneb-error");
const controller = new AbortController();
let runtime;
let mounted;
let disposed = false;

function fail(error) {
  const message = error instanceof Error ? error.message : String(error);
  const bootView = document.querySelector("#deneb-boot");
  if (!bootView) {
    root.replaceChildren();
    const view = document.createElement("section");
    view.id = "deneb-boot";
    view.setAttribute("role", "alert");
    view.append(status, errorView);
    root.append(view);
  }
  document.documentElement.dataset.denebState = "error";
  status.textContent = "Runtime boot failed";
  errorView.hidden = false;
  errorView.textContent = message;
  console.error("[deneb]", error);
}

async function dispose() {
  if (disposed) return;
  disposed = true;
  controller.abort();
  systemColorQuery?.removeEventListener?.("change", colorModeChanged);
  removeEventListener("storage", colorModeStored);
  try { await mounted?.dispose?.(); } catch (error) { console.error("[deneb] mount dispose", error); }
  try { await runtime?.dispose?.(); } catch (error) { console.error("[deneb] runtime dispose", error); }
}

async function boot() {
  if (document.documentElement.dataset.denebTarget === "pwa" && "serviceWorker" in navigator) {
    navigator.serviceWorker.register("./sw.js", { scope: "./" }).catch((error) => {
      console.warn("[deneb] service worker registration failed", error);
    });
  }
  const response = await fetch("./runtime/contract.json", { cache: "no-store", signal: controller.signal });
  if (!response.ok) throw new Error(`Runtime contract request failed (${response.status})`);
  const contract = await response.json();
  if (contract.format !== "deneb-runtime-contract" || contract.formatVersion !== 1) {
    throw new Error("Unsupported Deneb runtime contract");
  }
  if (contract.abiVersion !== 1) {
    throw new Error(`Vega runtime ABI mismatch: Deneb requires 1, contract declares ${contract.abiVersion}`);
  }
  if (contract.status !== "bound" || !contract.entrypoint) {
    throw new Error("No Vega runtime is bound. Rebuild with `deneb build --runtime-bundle <bundle-directory>`; see docs/runtime-contract.md.");
  }
  const module = await import(new URL(contract.entrypoint, import.meta.url));
  const factory = module[contract.factoryExport];
  if (typeof factory !== "function") {
    throw new Error(`Runtime entrypoint must export ${contract.factoryExport}()`);
  }
  runtime = await factory({
    abiVersion: contract.abiVersion,
    capabilities: Object.freeze([...contract.capabilities]),
    runtimePluginIndexUrl: contract.runtimePlugins ? new URL(contract.runtimePlugins, import.meta.url) : null,
    externalRuntimeIndexUrl: contract.externalRuntimes ? new URL(contract.externalRuntimes, import.meta.url) : null,
    signal: controller.signal,
  });
  if (!runtime || runtime.abiVersion !== contract.abiVersion || typeof runtime.mount !== "function" || typeof runtime.dispose !== "function") {
    throw new Error("Runtime factory returned an object that does not implement ABI 1 lifecycle");
  }
  mounted = await runtime.mount({
    root,
    projectUrl: new URL(contract.project, import.meta.url),
    bundleUrl: new URL(contract.bundle, import.meta.url),
    manifestUrl: new URL(contract.buildManifest, import.meta.url),
    runtimePluginIndexUrl: contract.runtimePlugins ? new URL(contract.runtimePlugins, import.meta.url) : null,
    externalRuntimeIndexUrl: contract.externalRuntimes ? new URL(contract.externalRuntimes, import.meta.url) : null,
    signal: controller.signal,
  });
  if (!mounted || typeof mounted.dispose !== "function") {
    throw new Error("Runtime mount() must return a disposable player handle");
  }
  if (mounted.playback && typeof mounted.playback.then === "function") {
    void mounted.playback.catch((error) => {
      if (disposed) return;
      fail(error);
      void dispose().catch((disposeError) => console.error("[deneb] playback dispose", disposeError));
    });
  }
  document.documentElement.dataset.denebState = "ready";
  document.querySelector("#deneb-boot")?.remove();
}

addEventListener("pagehide", () => void dispose(), { once: true });
boot().catch(fail);
"##;

const SHELL_CSS: &str = r#"*{box-sizing:border-box}html{--deneb-page:#f7f8fb;--deneb-page-accent:#eef0f5;--deneb-text:#17191f;--deneb-muted:#686d78;--deneb-line:#e1e4ea;--deneb-card:rgb(255 255 255 / 88%);--deneb-shadow:0 24px 70px rgb(18 24 40 / 10%);--deneb-star:#5f5bd7;--deneb-error:#a01f36;--deneb-error-bg:#fff1f3}html[data-deneb-color-mode=dark]{--deneb-page:#090b11;--deneb-page-accent:#111522;--deneb-text:#f4f5f7;--deneb-muted:#a7acb7;--deneb-line:#252a36;--deneb-card:rgb(16 19 27 / 90%);--deneb-shadow:0 24px 80px rgb(0 0 0 / 38%);--deneb-star:#a6a4ff;--deneb-error:#ffbac6;--deneb-error-bg:#32151e}@media(prefers-color-scheme:dark){html[data-deneb-color-mode=system]{--deneb-page:#090b11;--deneb-page-accent:#111522;--deneb-text:#f4f5f7;--deneb-muted:#a7acb7;--deneb-line:#252a36;--deneb-card:rgb(16 19 27 / 90%);--deneb-shadow:0 24px 80px rgb(0 0 0 / 38%);--deneb-star:#a6a4ff;--deneb-error:#ffbac6;--deneb-error-bg:#32151e}}html,body,#vega{width:100%;height:100%;margin:0}body{overflow:hidden;background:var(--deneb-page);color:var(--deneb-text);font:16px/1.5 ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}#vega{position:relative;isolation:isolate;background:radial-gradient(circle at 22% 18%,var(--deneb-page-accent),transparent 38%),linear-gradient(145deg,var(--deneb-page),var(--deneb-page-accent))}#deneb-boot{position:absolute;left:50%;top:50%;width:min(30rem,calc(100% - 2rem));transform:translate(-50%,-50%);padding:2rem;border:1px solid var(--deneb-line);border-radius:1rem;background:var(--deneb-card);box-shadow:var(--deneb-shadow);backdrop-filter:blur(18px);color-scheme:light}html[data-deneb-color-mode=dark] #deneb-boot{color-scheme:dark}@media(prefers-color-scheme:dark){html[data-deneb-color-mode=system] #deneb-boot{color-scheme:dark}}.deneb-star{display:block;color:var(--deneb-star);font-size:1.75rem}.deneb-eyebrow{margin:.45rem 0;color:var(--deneb-muted);font-size:.7rem;font-weight:700;letter-spacing:.18em}h1{margin:.2rem 0 .75rem;font:600 clamp(1.7rem,5vw,2.6rem)/1.1 ui-sans-serif,system-ui,sans-serif;letter-spacing:-.035em}#deneb-status{margin:0;color:var(--deneb-muted)}#deneb-error{overflow:auto;margin:1rem 0 0;padding:1rem;border:1px solid color-mix(in srgb,var(--deneb-error) 24%,transparent);border-radius:.65rem;white-space:pre-wrap;color:var(--deneb-error);background:var(--deneb-error-bg);font:13px/1.55 ui-monospace,SFMono-Regular,Consolas,monospace}html[data-deneb-state=ready] #vega{background:#000}@media(prefers-reduced-motion:no-preference){.deneb-star{animation:deneb-pulse 2.8s ease-in-out infinite}@keyframes deneb-pulse{50%{opacity:.55;transform:scale(.94)}}}"#;

const PWA_WORKER: &str = r#"const CACHE = "__DENEB_CACHE__";
self.addEventListener("install", (event) => {
  event.waitUntil(fetch("./deneb-precache.json").then((response) => {
    if (!response.ok) throw new Error(`precache manifest ${response.status}`);
    return response.json();
  }).then((urls) => caches.open(CACHE).then((cache) => cache.addAll(urls))).then(() => self.skipWaiting()));
});
self.addEventListener("activate", (event) => {
  event.waitUntil(caches.keys().then((keys) => Promise.all(keys.filter((key) => key.startsWith("deneb-") && key !== CACHE).map((key) => caches.delete(key)))).then(() => self.clients.claim()));
});
self.addEventListener("message", (event) => {
  if (event.data === "SKIP_WAITING") self.skipWaiting();
});

async function networkFirst(request) {
  try {
    const response = await fetch(request);
    if (response.ok) {
      const cache = await caches.open(CACHE);
      await cache.put(request, response.clone());
    }
    return response;
  } catch {
    const cached = await caches.match(request);
    if (cached) return cached;
    if (request.mode === "navigate") {
      const shell = await caches.match("./index.html");
      if (shell) return shell;
    }
    return new Response("Deneb is offline and this resource is not cached.", {
      status: 503,
      headers: { "Content-Type": "text/plain; charset=utf-8" },
    });
  }
}

self.addEventListener("fetch", (event) => {
  if (event.request.method !== "GET" || new URL(event.request.url).origin !== self.location.origin) return;
  event.respondWith(networkFirst(event.request));
});
"#;

const ELECTRON_MAIN: &str = r##"const { app, BrowserWindow, net, protocol } = require("electron");
const path = require("node:path");
const { pathToFileURL } = require("node:url");

protocol.registerSchemesAsPrivileged([{
  scheme: "deneb",
  privileges: {
    standard: true,
    secure: true,
    supportFetchAPI: true,
    corsEnabled: true,
  },
}]);

const webRoot = path.resolve(__dirname, "..", "..", "web");
const applicationUrl = "deneb://app/index.html";

function errorResponse(status, message) {
  return new Response(message, {
    status,
    headers: { "Content-Type": "text/plain; charset=utf-8" },
  });
}

function resolveRequestPath(requestUrl) {
  const url = new URL(requestUrl);
  if (url.protocol !== "deneb:" || url.hostname !== "app") return null;
  let pathname;
  try {
    pathname = decodeURIComponent(url.pathname);
  } catch {
    return null;
  }
  const relative = pathname === "/" ? "index.html" : pathname.replace(/^\/+/, "");
  const candidate = path.resolve(webRoot, relative);
  const allowedRoot = webRoot + path.sep;
  if (candidate !== webRoot && !candidate.startsWith(allowedRoot)) return null;
  return candidate;
}

async function handleDenebRequest(request) {
  const requestedPath = resolveRequestPath(request.url);
  if (!requestedPath) return errorResponse(404, "Not found");
  try {
    return await net.fetch(pathToFileURL(requestedPath).href);
  } catch {
    return errorResponse(404, "Not found");
  }
}

function createWindow() {
  const window = new BrowserWindow({
    width: 1280,
    height: 720,
    minWidth: 640,
    minHeight: 360,
    backgroundColor: "#f7f8fb",
    autoHideMenuBar: true,
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webSecurity: true,
    },
  });
  window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
  window.webContents.on("will-navigate", (event, url) => {
    if (!resolveRequestPath(url)) event.preventDefault();
  });
  void window.loadURL(applicationUrl);
}

app.whenReady().then(async () => {
  await protocol.handle("deneb", handleDenebRequest);
  createWindow();
  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});
app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});
"##;
