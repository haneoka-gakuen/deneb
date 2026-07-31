#![forbid(unsafe_code)]

pub mod build;
mod component_host;
pub mod diagnostics;
pub mod distribution;
pub mod external_runtimes;
pub mod format;
pub mod model;
pub mod opcodes;
pub mod plugin_lock;
pub mod plugin_resolution;
pub mod plugins;
pub mod runtime;
pub mod target;
pub mod validate;
pub mod vega_plugins;

pub use build::{BuildDistribution, BuildOptions, BuildReport, build_project};
pub use diagnostics::{Diagnostic, Severity};
pub use external_runtimes::{
    EXTERNAL_RUNTIME_INDEX_FILE, EXTERNAL_RUNTIME_SBOM_FILE, ExternalRuntimeAuthorizationRecord,
    ExternalRuntimeBundleReport, ExternalRuntimeDistribution, ExternalRuntimeError, ExternalRuntimeFileDeclaration,
    ExternalRuntimeIndex, ExternalRuntimeLicenseAcknowledgement, ExternalRuntimeLicenseAuthorization,
    ExternalRuntimeReceipt, ExternalRuntimeResolution, ExternalRuntimeResolutionStatus, ExternalRuntimeSbom,
    VegaExternalRuntimeArtifact, VegaExternalRuntimeProvisioning, VegaExternalRuntimeRequirement,
    resolve_external_runtimes,
};
pub use format::{MAX_PROJECT_BYTES, ProjectFormatError, parse_project};
pub use model::{VegaAsset, VegaBundle, VegaCommand, VegaProject, VegaScene};
pub use plugin_lock::PluginLock;
pub use plugin_resolution::{PluginResolutionEntry, PluginResolutionReport, PluginResolutionStatus, resolve_plugins};
pub use runtime::{RUNTIME_ABI_VERSION, RuntimeBundleManifest, RuntimeStatus};
pub use target::{BuildMode, BuildTarget};
pub use validate::validate_project;
pub use vega_plugins::{
    VEGA_PLUGIN_ARTIFACT_MAP_FILE, VEGA_PLUGIN_LOCK_FILE, VegaPluginArtifactKind, VegaPluginArtifactMap,
    VegaPluginArtifactMapEntry, VegaPluginInstallSource, VegaPluginLock, VegaPluginLockEntry, VegaPluginLockTarget,
    VegaPluginTargets, VegaRuntimePluginPlan, VegaRuntimePluginResolution, VegaRuntimePluginStatus,
    VegaRuntimePluginTarget, read_vega_plugin_artifact_map, read_vega_plugin_lock, resolve_vega_runtime_plugins,
};
