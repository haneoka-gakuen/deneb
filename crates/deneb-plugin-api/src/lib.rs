#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

/// Incremented only for a breaking host/component boundary change.
pub const DENEB_PLUGIN_ABI_VERSION: u32 = 1;
/// Compatibility alias for manifests emitted before the ABI field was renamed.
pub const DENEB_PLUGIN_API_VERSION: u32 = DENEB_PLUGIN_ABI_VERSION;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    #[serde(default)]
    pub engine_range: Option<String>,
    /// Empty means that the plugin supports every Deneb packaging target.
    #[serde(default)]
    pub targets: BTreeSet<PluginTarget>,
    #[serde(default)]
    pub capabilities: BTreeSet<Capability>,
    #[serde(default)]
    pub hooks: BTreeSet<Hook>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub conflicts: BTreeSet<String>,
    pub entrypoint: String,
    pub checksum: String,
    #[serde(default)]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum PluginTarget {
    Web,
    Pwa,
    Electron,
    Tauri,
    Android,
    Ios,
}

impl PluginManifest {
    pub fn validate(&self) -> Result<(), PluginManifestError> {
        if !is_plugin_id(&self.id) {
            return Err(PluginManifestError::Id);
        }
        if self.name.trim().is_empty() || self.version.trim().is_empty() {
            return Err(PluginManifestError::Identity);
        }
        if self.api_version != DENEB_PLUGIN_ABI_VERSION {
            return Err(PluginManifestError::AbiVersion {
                expected: DENEB_PLUGIN_ABI_VERSION,
                actual: self.api_version,
            });
        }
        if self
            .engine_range
            .as_ref()
            .is_some_and(|range| range.trim().is_empty() || range.chars().any(char::is_control))
        {
            return Err(PluginManifestError::EngineRange);
        }
        validate_relative_path(&self.entrypoint)?;
        if !self.entrypoint.ends_with(".wasm") {
            return Err(PluginManifestError::Entrypoint(
                "WASM component entrypoint must end in `.wasm`".into(),
            ));
        }
        if !is_sha256(&self.checksum) {
            return Err(PluginManifestError::Checksum);
        }
        for (dependency, requirement) in &self.dependencies {
            if !is_plugin_id(dependency) || requirement.trim().is_empty() {
                return Err(PluginManifestError::Dependency(dependency.clone()));
            }
            if dependency == &self.id {
                return Err(PluginManifestError::SelfReference);
            }
        }
        for conflict in &self.conflicts {
            if !is_plugin_id(conflict) {
                return Err(PluginManifestError::Conflict(conflict.clone()));
            }
            if conflict == &self.id {
                return Err(PluginManifestError::SelfReference);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    ReadProject,
    ReadAssets,
    WriteGenerated,
    TransformProject,
    EncodeAsset,
    TargetProvider,
    Publish,
    Network,
    Process,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Hook {
    Validate,
    Transform,
    Assets,
    Emit,
    Package,
    Publish,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct HostLimits {
    pub max_generated_files: u32,
    pub max_generated_bytes: u64,
    pub max_diagnostics: u32,
    pub max_execution_millis: u64,
}

impl Default for HostLimits {
    fn default() -> Self {
        Self {
            max_generated_files: 1_024,
            max_generated_bytes: 256 * 1024 * 1024,
            max_diagnostics: 1_024,
            max_execution_millis: 30_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct HookRequest {
    #[serde(default = "current_abi_version")]
    pub abi_version: u32,
    pub build_id: String,
    pub session_id: String,
    pub hook: Hook,
    pub project: serde_json::Value,
    pub configuration: serde_json::Value,
    #[serde(default)]
    pub granted_capabilities: BTreeSet<Capability>,
    #[serde(default)]
    pub limits: HostLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct HookResponse {
    pub session_id: String,
    #[serde(default)]
    pub diagnostics: Vec<PluginDiagnostic>,
    #[serde(default)]
    pub project_patch: Option<serde_json::Value>,
    #[serde(default)]
    pub generated_files: BTreeMap<String, Vec<u8>>,
}

impl HookResponse {
    pub fn validate_limits(&self, limits: &HostLimits) -> Result<(), PluginOutputError> {
        if self.diagnostics.len() > limits.max_diagnostics as usize {
            return Err(PluginOutputError::Diagnostics);
        }
        if self.generated_files.len() > limits.max_generated_files as usize {
            return Err(PluginOutputError::Files);
        }
        let mut total = 0_u64;
        for (path, bytes) in &self.generated_files {
            validate_relative_path(path).map_err(|_| PluginOutputError::Path(path.clone()))?;
            total = total.saturating_add(bytes.len() as u64);
            if total > limits.max_generated_bytes {
                return Err(PluginOutputError::Bytes);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PluginDiagnostic {
    pub severity: String,
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PluginManifestError {
    #[error("plugin id must be a lowercase reverse-domain or scoped identifier")]
    Id,
    #[error("plugin name and version must be non-empty")]
    Identity,
    #[error("plugin ABI mismatch: expected {expected}, got {actual}")]
    AbiVersion { expected: u32, actual: u32 },
    #[error("plugin engine range must be a non-empty printable requirement")]
    EngineRange,
    #[error("invalid plugin entrypoint: {0}")]
    Entrypoint(String),
    #[error("plugin checksum must be `sha256-` followed by 64 lowercase hex characters")]
    Checksum,
    #[error("invalid dependency `{0}`")]
    Dependency(String),
    #[error("invalid conflict `{0}`")]
    Conflict(String),
    #[error("a plugin cannot depend on or conflict with itself")]
    SelfReference,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PluginOutputError {
    #[error("plugin exceeded the diagnostic limit")]
    Diagnostics,
    #[error("plugin exceeded the generated-file count limit")]
    Files,
    #[error("plugin exceeded the generated-byte limit")]
    Bytes,
    #[error("plugin attempted to write an unsafe path `{0}`")]
    Path(String),
}

fn current_abi_version() -> u32 {
    DENEB_PLUGIN_ABI_VERSION
}

fn validate_relative_path(value: &str) -> Result<(), PluginManifestError> {
    if value.is_empty()
        || value.contains(['\\', ':'])
        || value
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || value.chars().any(char::is_control)
    {
        return Err(PluginManifestError::Entrypoint(
            "path contains unsafe characters".into(),
        ));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_) | Component::CurDir
            )
        })
    {
        return Err(PluginManifestError::Entrypoint(
            "path must remain inside the plugin bundle".into(),
        ));
    }
    Ok(())
}

fn is_plugin_id(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit())
        && value.len() <= 96
        && value.contains(['.', '/'])
        && !value.contains("..")
        && !value.ends_with(['.', '/'])
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '-' | '_' | '.' | '/')
        })
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256-").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}
