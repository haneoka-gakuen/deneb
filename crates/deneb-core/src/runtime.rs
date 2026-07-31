use crate::validate::{is_sha256, validate_portable_relative_path};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const RUNTIME_BUNDLE_FORMAT: &str = "vega-runtime";
pub const RUNTIME_BUNDLE_FORMAT_VERSION: u32 = 1;
pub const RUNTIME_CONTRACT_FORMAT: &str = "deneb-runtime-contract";
pub const RUNTIME_CONTRACT_FORMAT_VERSION: u32 = 1;
pub const RUNTIME_ABI_VERSION: u32 = 1;
pub const RUNTIME_MANIFEST_FILE: &str = "deneb-runtime.json";
pub const RUNTIME_FACTORY_EXPORT: &str = "createDenebRuntime";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBundleManifest {
    pub format: String,
    pub format_version: u32,
    pub abi_version: u32,
    pub entrypoint: String,
    /// Concrete Vega engine version implemented by this runtime. Older
    /// manifests may omit it; target-pinned runtime plugins then fail closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_version: Option<String>,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
    #[serde(default)]
    pub integrity: BTreeMap<String, String>,
}

impl RuntimeBundleManifest {
    pub fn validate(&self) -> Result<(), RuntimeManifestError> {
        if self.format != RUNTIME_BUNDLE_FORMAT {
            return Err(RuntimeManifestError::Format(self.format.clone()));
        }
        if self.format_version != RUNTIME_BUNDLE_FORMAT_VERSION {
            return Err(RuntimeManifestError::FormatVersion(self.format_version));
        }
        if self.abi_version != RUNTIME_ABI_VERSION {
            return Err(RuntimeManifestError::AbiVersion {
                expected: RUNTIME_ABI_VERSION,
                actual: self.abi_version,
            });
        }
        validate_portable_relative_path(&self.entrypoint).map_err(RuntimeManifestError::Entrypoint)?;
        if !self.entrypoint.ends_with(".js") && !self.entrypoint.ends_with(".mjs") {
            return Err(RuntimeManifestError::Entrypoint(
                "Runtime entrypoint must be an ESM `.js` or `.mjs` file".into(),
            ));
        }
        if let Some(version) = &self.engine_version {
            Version::parse(version).map_err(|_| RuntimeManifestError::EngineVersion(version.clone()))?;
        }
        for (path, digest) in &self.integrity {
            validate_portable_relative_path(path).map_err(RuntimeManifestError::IntegrityPath)?;
            if !is_sha256(digest) {
                return Err(RuntimeManifestError::IntegrityDigest(path.clone()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeStatus {
    Bound,
    Unbound,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeContract {
    pub format: String,
    pub format_version: u32,
    pub abi_version: u32,
    pub status: RuntimeStatus,
    pub factory_export: String,
    pub entrypoint: Option<String>,
    pub project: String,
    pub bundle: String,
    pub build_manifest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_plugins: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_runtimes: Option<String>,
    pub capabilities: BTreeSet<String>,
    pub integrity: BTreeMap<String, String>,
}

impl RuntimeContract {
    pub fn unbound() -> Self {
        Self {
            format: RUNTIME_CONTRACT_FORMAT.into(),
            format_version: RUNTIME_CONTRACT_FORMAT_VERSION,
            abi_version: RUNTIME_ABI_VERSION,
            status: RuntimeStatus::Unbound,
            factory_export: RUNTIME_FACTORY_EXPORT.into(),
            entrypoint: None,
            project: "../project.vega.json".into(),
            bundle: "../project.vgb".into(),
            build_manifest: "../deneb-manifest.json".into(),
            runtime_plugins: None,
            external_runtimes: None,
            capabilities: BTreeSet::new(),
            integrity: BTreeMap::new(),
        }
    }

    pub fn bound(manifest: &RuntimeBundleManifest, integrity: BTreeMap<String, String>) -> Self {
        Self {
            format: RUNTIME_CONTRACT_FORMAT.into(),
            format_version: RUNTIME_CONTRACT_FORMAT_VERSION,
            abi_version: RUNTIME_ABI_VERSION,
            status: RuntimeStatus::Bound,
            factory_export: RUNTIME_FACTORY_EXPORT.into(),
            entrypoint: Some(format!("./bundle/{}", manifest.entrypoint)),
            project: "../project.vega.json".into(),
            bundle: "../project.vgb".into(),
            build_manifest: "../deneb-manifest.json".into(),
            runtime_plugins: None,
            external_runtimes: None,
            capabilities: manifest.capabilities.clone(),
            integrity,
        }
    }

    pub fn with_runtime_plugins(mut self, enabled: bool) -> Self {
        self.runtime_plugins = enabled.then(|| "../runtime-plugins/vega-runtime-plugins.json".into());
        self
    }

    pub fn with_external_runtimes(mut self, enabled: bool) -> Self {
        self.external_runtimes = enabled.then(|| "../external-runtimes/deneb-external-runtimes.json".into());
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeManifestError {
    #[error("expected runtime format `{RUNTIME_BUNDLE_FORMAT}`, got `{0}`")]
    Format(String),
    #[error("unsupported runtime bundle format version {0}")]
    FormatVersion(u32),
    #[error("runtime ABI mismatch: expected {expected}, got {actual}")]
    AbiVersion { expected: u32, actual: u32 },
    #[error("invalid runtime entrypoint: {0}")]
    Entrypoint(String),
    #[error("invalid Vega engine version `{0}`")]
    EngineVersion(String),
    #[error("invalid runtime integrity path: {0}")]
    IntegrityPath(String),
    #[error("invalid sha256 digest for runtime file `{0}`")]
    IntegrityDigest(String),
}
