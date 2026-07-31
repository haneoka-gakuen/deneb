use crate::diagnostics::{Diagnostic, Severity};
use crate::distribution::{DistributionScope, enforce_distribution_policy};
use crate::external_runtimes::{
    VegaExternalRuntimeArtifact, VegaExternalRuntimeRequirement, validate_external_runtime_artifact,
    validate_external_runtime_requirement,
};
use crate::model::{VegaProject, VegaProjectPlugin};
use crate::plugins::validate_component;
use crate::target::BuildTarget;
use crate::validate::validate_portable_relative_path;
use base64::Engine as _;
use camino::{Utf8Path, Utf8PathBuf};
use node_semver::{Range as NodeVersionReq, Version as NodeVersion};
use semver::Version;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::Read;

pub const VEGA_PLUGIN_LOCK_FORMAT: &str = "vega-plugin-lock";
pub const VEGA_PLUGIN_LOCK_VERSION: u32 = 1;
pub const VEGA_PLUGIN_PROJECT_FORMAT_VERSION: u32 = 1;
pub const VEGA_PLUGIN_LOCK_FILE: &str = "vega.plugins.lock.json";
pub const VEGA_PLUGIN_ARTIFACT_MAP_FORMAT: &str = "deneb-vega-plugin-artifact-map";
pub const VEGA_PLUGIN_ARTIFACT_MAP_VERSION: u32 = 1;
pub const VEGA_PLUGIN_ARTIFACT_MAP_FILE: &str = "vega.plugins.cache.json";
pub const VEGA_RUNTIME_PLUGIN_PLAN_FORMAT: &str = "deneb-vega-runtime-plugin-plan";
pub const VEGA_RUNTIME_PLUGIN_INDEX_FORMAT: &str = "vega-runtime-plugin-bundle";
pub const VEGA_RUNTIME_PLUGIN_SBOM_FORMAT: &str = "deneb-vega-runtime-plugin-sbom";
const MAX_LOCK_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_COMPONENT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaPluginLock {
    pub format: String,
    pub format_version: u32,
    pub project_format_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<VegaPluginLockTarget>,
    pub plugins: Vec<VegaPluginLockEntry>,
}

impl VegaPluginLock {
    pub fn empty(target: VegaPluginLockTarget) -> Self {
        Self {
            format: VEGA_PLUGIN_LOCK_FORMAT.into(),
            format_version: VEGA_PLUGIN_LOCK_VERSION,
            project_format_version: VEGA_PLUGIN_PROJECT_FORMAT_VERSION,
            target: Some(target),
            plugins: Vec::new(),
        }
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec_pretty(self)
    }

    pub fn digest(&self) -> Result<String, serde_json::Error> {
        Ok(sha256_integrity(&self.canonical_bytes()?))
    }

    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        if self.format != VEGA_PLUGIN_LOCK_FORMAT {
            diagnostics.push(Diagnostic::error(
                "deneb.vega-plugin-lock.format",
                "format",
                format!("Expected `{VEGA_PLUGIN_LOCK_FORMAT}`"),
            ));
        }
        if self.format_version != VEGA_PLUGIN_LOCK_VERSION {
            diagnostics.push(Diagnostic::error(
                "deneb.vega-plugin-lock.version",
                "formatVersion",
                format!("Unsupported Vega plugin lock version {}", self.format_version),
            ));
        }
        if self.project_format_version != VEGA_PLUGIN_PROJECT_FORMAT_VERSION {
            diagnostics.push(Diagnostic::error(
                "deneb.vega-plugin-lock.project-version",
                "projectFormatVersion",
                format!(
                    "Unsupported Vega project format version {}",
                    self.project_format_version
                ),
            ));
        }
        if let Some(target) = &self.target {
            validate_lock_target(target, &mut diagnostics);
        }

        let mut previous = None::<&str>;
        let mut versions = BTreeMap::new();
        for (index, plugin) in self.plugins.iter().enumerate() {
            let path = format!("plugins.{index}");
            if previous.is_some_and(|previous| previous >= plugin.id.as_str()) {
                diagnostics.push(Diagnostic::error(
                    "deneb.vega-plugin-lock.order",
                    format!("{path}.id"),
                    "Plugin entries must be unique and strictly sorted by id",
                ));
            }
            previous = Some(&plugin.id);
            if !is_vega_plugin_id(&plugin.id) {
                diagnostics.push(Diagnostic::error(
                    "deneb.vega-plugin-lock.id",
                    format!("{path}.id"),
                    "Invalid canonical Vega plugin id",
                ));
            }
            if !is_non_empty_printable(&plugin.version) {
                diagnostics.push(Diagnostic::error(
                    "deneb.vega-plugin-lock.plugin-version",
                    format!("{path}.version"),
                    "Selected plugin version must be non-empty and printable",
                ));
            }
            validate_source(&plugin.source, &format!("{path}.source"), &mut diagnostics);
            if let Some(integrity) = &plugin.integrity
                && !is_non_empty_printable(integrity)
            {
                diagnostics.push(Diagnostic::error(
                    "deneb.vega-plugin-lock.integrity",
                    format!("{path}.integrity"),
                    "Canonical lock integrity must be non-empty and printable",
                ));
            }
            validate_sorted_strings(&plugin.capabilities, &format!("{path}.capabilities"), &mut diagnostics);
            validate_sorted_strings(&plugin.permissions, &format!("{path}.permissions"), &mut diagnostics);
            let mut previous_runtime = None::<&str>;
            for (runtime_index, runtime) in plugin.external_runtimes.iter().enumerate() {
                let runtime_path = format!("{path}.externalRuntimes.{runtime_index}");
                if previous_runtime.is_some_and(|previous| previous >= runtime.id.as_str()) {
                    diagnostics.push(Diagnostic::error(
                        "deneb.vega-plugin-lock.external-runtime-order",
                        format!("{runtime_path}.id"),
                        "External runtime requirements must be unique and sorted by id",
                    ));
                }
                previous_runtime = Some(&runtime.id);
                validate_external_runtime_requirement(runtime, &runtime_path, &mut diagnostics);
            }
            versions.insert(plugin.id.as_str(), plugin.version.as_str());
        }

        for (index, plugin) in self.plugins.iter().enumerate() {
            for (dependency, version) in plugin.dependencies.iter() {
                let path = format!("plugins.{index}.dependencies.{dependency}");
                if !is_vega_plugin_id(dependency) || dependency == &plugin.id {
                    diagnostics.push(Diagnostic::error(
                        "deneb.vega-plugin-lock.dependency",
                        &path,
                        "Dependency must be a valid plugin other than itself",
                    ));
                }
                match versions.get(dependency.as_str()) {
                    Some(selected) if *selected == version => {}
                    Some(selected) => diagnostics.push(Diagnostic::error(
                        "deneb.vega-plugin-lock.dependency-version",
                        &path,
                        format!("Dependency selects `{selected}`, not `{version}`"),
                    )),
                    None => diagnostics.push(Diagnostic::error(
                        "deneb.vega-plugin-lock.dependency-missing",
                        &path,
                        "Dependency is absent from the lock",
                    )),
                }
            }
        }
        if let Err(cycle) = lock_topological_order(self) {
            diagnostics.push(Diagnostic::error("deneb.vega-plugin-lock.cycle", "plugins", cycle));
        }
        diagnostics
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaPluginLockTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_version: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaPluginLockEntry {
    pub id: String,
    pub version: String,
    pub source: VegaPluginInstallSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
    pub dependencies: SortedDependencies,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_runtimes: Vec<VegaExternalRuntimeRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum VegaPluginInstallSource {
    Registry {
        package: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        registry: Option<String>,
    },
    Url {
        url: String,
        integrity: String,
    },
    Workspace {
        path: String,
    },
    Builtin {
        key: String,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaPluginTargets {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtimes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub architectures: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub api_versions: Vec<u32>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct SortedDependencies(BTreeMap<String, String>);

impl SortedDependencies {
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.0.iter()
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }

    pub fn get(&self, id: &str) -> Option<&String> {
        self.0.get(id)
    }
}

impl From<BTreeMap<String, String>> for SortedDependencies {
    fn from(value: BTreeMap<String, String>) -> Self {
        Self(value)
    }
}

impl<'de> Deserialize<'de> for SortedDependencies {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DependenciesVisitor;

        impl<'de> Visitor<'de> for DependenciesVisitor {
            type Value = SortedDependencies;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a dependency object with keys sorted by plugin id")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut dependencies = BTreeMap::new();
                let mut previous = None::<String>;
                while let Some((id, version)) = map.next_entry::<String, String>()? {
                    if previous.as_ref().is_some_and(|previous| previous >= &id) {
                        return Err(serde::de::Error::custom(
                            "dependency keys must be unique and sorted by plugin id",
                        ));
                    }
                    previous = Some(id.clone());
                    dependencies.insert(id, version);
                }
                Ok(SortedDependencies(dependencies))
            }
        }

        deserializer.deserialize_map(DependenciesVisitor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaPluginArtifactMap {
    pub format: String,
    pub format_version: u32,
    pub plugins: Vec<VegaPluginArtifactMapEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_runtimes: Vec<VegaExternalRuntimeArtifact>,
}

impl VegaPluginArtifactMap {
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        if self.format != VEGA_PLUGIN_ARTIFACT_MAP_FORMAT {
            diagnostics.push(Diagnostic::error(
                "deneb.vega-plugin-artifacts.format",
                "format",
                format!("Expected `{VEGA_PLUGIN_ARTIFACT_MAP_FORMAT}`"),
            ));
        }
        if self.format_version != VEGA_PLUGIN_ARTIFACT_MAP_VERSION {
            diagnostics.push(Diagnostic::error(
                "deneb.vega-plugin-artifacts.version",
                "formatVersion",
                format!("Unsupported artifact-map version {}", self.format_version),
            ));
        }
        let mut previous = None::<&str>;
        for (index, entry) in self.plugins.iter().enumerate() {
            let path = format!("plugins.{index}");
            if previous.is_some_and(|previous| previous >= entry.id.as_str()) {
                diagnostics.push(Diagnostic::error(
                    "deneb.vega-plugin-artifacts.order",
                    format!("{path}.id"),
                    "Artifact entries must be unique and strictly sorted by id",
                ));
            }
            previous = Some(&entry.id);
            if !is_vega_plugin_id(&entry.id) || !is_non_empty_printable(&entry.version) {
                diagnostics.push(Diagnostic::error(
                    "deneb.vega-plugin-artifacts.identity",
                    &path,
                    "Artifact plugin id/version is invalid",
                ));
            }
            validate_source(&entry.source, &format!("{path}.source"), &mut diagnostics);
            if parse_sha256_integrity(&entry.integrity).is_err() {
                diagnostics.push(Diagnostic::error(
                    "deneb.vega-plugin-artifacts.integrity",
                    format!("{path}.integrity"),
                    "Artifact integrity must be SHA-256 in hex or SRI base64 form",
                ));
            }
            if entry
                .lock_integrity
                .as_ref()
                .is_some_and(|value| !is_non_empty_printable(value))
            {
                diagnostics.push(Diagnostic::error(
                    "deneb.vega-plugin-artifacts.lock-integrity",
                    format!("{path}.lockIntegrity"),
                    "Mapped lock integrity must be non-empty and printable",
                ));
            }
            if validate_portable_relative_path(&entry.path).is_err() {
                diagnostics.push(Diagnostic::error(
                    "deneb.vega-plugin-artifacts.path",
                    format!("{path}.path"),
                    "Artifact path must remain below the artifact-map directory",
                ));
            }
            validate_sorted_strings(
                &entry.granted_permissions,
                &format!("{path}.grantedPermissions"),
                &mut diagnostics,
            );
            if entry
                .media_type
                .as_ref()
                .is_some_and(|value| !is_non_empty_printable(value))
            {
                diagnostics.push(Diagnostic::error(
                    "deneb.vega-plugin-artifacts.media-type",
                    format!("{path}.mediaType"),
                    "Media type must be non-empty and printable",
                ));
            }
        }
        let mut previous_runtime = None::<&str>;
        for (index, entry) in self.external_runtimes.iter().enumerate() {
            let path = format!("externalRuntimes.{index}");
            if previous_runtime.is_some_and(|previous| previous >= entry.id.as_str()) {
                diagnostics.push(Diagnostic::error(
                    "deneb.vega-plugin-artifacts.external-runtime-order",
                    format!("{path}.id"),
                    "External runtime artifacts must be unique and sorted by id",
                ));
            }
            previous_runtime = Some(&entry.id);
            validate_external_runtime_artifact(entry, &path, &mut diagnostics);
        }
        diagnostics
    }

    pub fn digest(&self) -> Result<String, serde_json::Error> {
        Ok(sha256_integrity(&serde_json::to_vec_pretty(self)?))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaPluginArtifactMapEntry {
    pub id: String,
    pub version: String,
    pub source: VegaPluginInstallSource,
    pub kind: VegaPluginArtifactKind,
    pub path: String,
    /// Exact integrity identity from the canonical lock. This remains separate
    /// from the SHA-256 digest of the host-supplied runtime artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_integrity: Option<String>,
    pub integrity: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub granted_permissions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum VegaPluginArtifactKind {
    Module,
    WasmComponent,
    Package,
}

#[derive(Debug, Clone)]
pub struct VegaRuntimePluginTarget {
    pub build_target: BuildTarget,
    pub architecture: Option<String>,
    pub engine_version: Option<String>,
}

impl VegaRuntimePluginTarget {
    pub fn lock_target(&self) -> VegaPluginLockTarget {
        let platform = match self.build_target {
            BuildTarget::Web => "web",
            BuildTarget::Pwa => "pwa",
            BuildTarget::Electron => "electron",
            BuildTarget::Tauri => "tauri",
            BuildTarget::Android => "android",
            BuildTarget::Ios => "ios",
        };
        VegaPluginLockTarget {
            runtime: Some("vega".into()),
            platform: Some(platform.into()),
            architecture: self.architecture.clone(),
            engine_version: self.engine_version.clone(),
            api_version: Some(1),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum VegaRuntimePluginStatus {
    Resolved,
    Skipped,
    Failed,
}

impl fmt::Display for VegaRuntimePluginStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Resolved => "resolved",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaRuntimePluginPlan {
    pub format: String,
    pub format_version: u32,
    pub target: VegaPluginLockTarget,
    pub source_lock_digest: Option<String>,
    pub artifact_map_digest: Option<String>,
    pub order: Vec<String>,
    pub plugins: BTreeMap<String, VegaRuntimePluginPlanEntry>,
    pub diagnostics: Vec<Diagnostic>,
}

impl VegaRuntimePluginPlan {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaRuntimePluginPlanEntry {
    pub id: String,
    pub version: String,
    pub required: bool,
    pub direct: bool,
    pub status: VegaRuntimePluginStatus,
    pub source: VegaPluginInstallSource,
    pub declared_integrity: Option<String>,
    pub verified_integrity: Option<String>,
    pub dependencies: BTreeMap<String, String>,
    pub capabilities: Vec<String>,
    pub permissions: Vec<String>,
    pub configuration: Value,
    pub artifact_kind: Option<VegaPluginArtifactKind>,
    pub artifact: Option<String>,
    pub media_type: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaRuntimePluginIndex {
    pub format: String,
    pub format_version: u32,
    pub target: VegaPluginLockTarget,
    pub lock: String,
    pub plugins: Vec<VegaRuntimePluginIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaRuntimePluginIndexEntry {
    pub id: String,
    pub version: String,
    pub source: VegaPluginInstallSource,
    pub source_integrity: Option<String>,
    pub integrity: String,
    pub dependencies: BTreeMap<String, String>,
    pub capabilities: Vec<String>,
    pub permissions: Vec<String>,
    pub configuration: Value,
    pub artifact_kind: VegaPluginArtifactKind,
    pub artifact: String,
    pub media_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaRuntimePluginSbom {
    pub format: String,
    pub format_version: u32,
    pub lock_digest: String,
    pub components: Vec<VegaRuntimePluginSbomComponent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaRuntimePluginSbomComponent {
    pub id: String,
    pub version: String,
    pub source: VegaPluginInstallSource,
    pub source_integrity: Option<String>,
    pub integrity: String,
    pub dependencies: BTreeMap<String, String>,
    pub capabilities: Vec<String>,
    pub permissions: Vec<String>,
    pub artifact_kind: VegaPluginArtifactKind,
    pub artifact: String,
}

#[derive(Debug, Clone)]
struct EffectiveRuntimeRequest {
    required: bool,
    direct: bool,
    configuration: Value,
}

#[derive(Debug, Clone)]
struct ResolvedRuntimeArtifact {
    source: Utf8PathBuf,
    relative_output: String,
    kind: VegaPluginArtifactKind,
    integrity: String,
    media_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VegaRuntimePluginResolution {
    pub plan: VegaRuntimePluginPlan,
    pub lock_snapshot: VegaPluginLock,
    artifacts: BTreeMap<String, ResolvedRuntimeArtifact>,
}

impl VegaRuntimePluginResolution {
    /// Returns the player-facing project with exactly the runtime plugins that
    /// survived strict lock, target, permission, and artifact verification.
    pub fn resolved_project(&self, project: &VegaProject) -> VegaProject {
        let direct = project
            .plugins
            .iter()
            .map(|plugin| (plugin.id.as_str(), plugin))
            .collect::<BTreeMap<_, _>>();
        let mut resolved = project.clone();
        resolved.plugins = self
            .plan
            .order
            .iter()
            .filter_map(|id| {
                let entry = self.plan.plugins.get(id)?;
                if let Some(plugin) = direct.get(id.as_str()) {
                    let mut plugin = (*plugin).clone();
                    plugin.version.clone_from(&entry.version);
                    plugin.required = entry.required;
                    return Some(plugin);
                }
                Some(VegaProjectPlugin {
                    id: id.clone(),
                    version: entry.version.clone(),
                    required: entry.required,
                    configuration: Value::Null,
                    capabilities: entry.capabilities.clone(),
                    permissions: entry.permissions.clone(),
                    dependencies: entry.dependencies.clone(),
                    targets: None,
                    source: Some(entry.source.clone()),
                })
            })
            .collect();
        resolved
    }

    pub fn bundle(&self, web_root: &Utf8Path) -> Result<VegaRuntimePluginIndex, VegaRuntimePluginError> {
        let root = web_root.join("runtime-plugins");
        fs::create_dir_all(&root)?;
        fs::write(
            web_root.join(VEGA_PLUGIN_LOCK_FILE),
            self.lock_snapshot.canonical_bytes()?,
        )?;
        fs::write(
            root.join("deneb-vega-plugin-plan.json"),
            serde_json::to_vec_pretty(&self.plan)?,
        )?;

        let mut plugins = Vec::new();
        for id in &self.plan.order {
            let entry = &self.plan.plugins[id];
            let artifact = self
                .artifacts
                .get(id)
                .ok_or_else(|| VegaRuntimePluginError::Invalid(format!("resolved artifact `{id}` is absent")))?;
            verify_runtime_artifact(&artifact.source, artifact.kind, &artifact.integrity)?;
            let destination = web_root.join(&artifact.relative_output);
            fs::create_dir_all(
                destination
                    .parent()
                    .ok_or_else(|| VegaRuntimePluginError::Invalid("artifact output has no parent".into()))?,
            )?;
            fs::copy(&artifact.source, &destination)?;
            verify_runtime_artifact(&destination, artifact.kind, &artifact.integrity)?;
            let index_artifact = artifact
                .relative_output
                .strip_prefix("runtime-plugins/")
                .map(|path| format!("./{path}"))
                .ok_or_else(|| {
                    VegaRuntimePluginError::Invalid("runtime artifact output is outside runtime-plugins".into())
                })?;
            plugins.push(VegaRuntimePluginIndexEntry {
                id: id.clone(),
                version: entry.version.clone(),
                source: entry.source.clone(),
                source_integrity: entry.declared_integrity.clone(),
                integrity: artifact.integrity.clone(),
                dependencies: entry.dependencies.clone(),
                capabilities: entry.capabilities.clone(),
                permissions: entry.permissions.clone(),
                configuration: entry.configuration.clone(),
                artifact_kind: artifact.kind,
                artifact: index_artifact,
                media_type: artifact.media_type.clone(),
            });
        }
        let index = VegaRuntimePluginIndex {
            format: VEGA_RUNTIME_PLUGIN_INDEX_FORMAT.into(),
            format_version: 1,
            target: self.plan.target.clone(),
            lock: format!("../{VEGA_PLUGIN_LOCK_FILE}"),
            plugins,
        };
        fs::write(
            root.join("vega-runtime-plugins.json"),
            serde_json::to_vec_pretty(&index)?,
        )?;
        let sbom = VegaRuntimePluginSbom {
            format: VEGA_RUNTIME_PLUGIN_SBOM_FORMAT.into(),
            format_version: 1,
            lock_digest: self.lock_snapshot.digest()?,
            components: index
                .plugins
                .iter()
                .map(|plugin| VegaRuntimePluginSbomComponent {
                    id: plugin.id.clone(),
                    version: plugin.version.clone(),
                    source: plugin.source.clone(),
                    source_integrity: plugin.source_integrity.clone(),
                    integrity: plugin.integrity.clone(),
                    dependencies: plugin.dependencies.clone(),
                    capabilities: plugin.capabilities.clone(),
                    permissions: plugin.permissions.clone(),
                    artifact_kind: plugin.artifact_kind,
                    artifact: plugin.artifact.clone(),
                })
                .collect(),
        };
        fs::write(
            root.join("vega-runtime-plugin-sbom.json"),
            serde_json::to_vec_pretty(&sbom)?,
        )?;
        Ok(index)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VegaRuntimePluginError {
    #[error("invalid Vega runtime plugin input: {0}")]
    Invalid(String),
    #[error("Vega runtime plugin JSON exceeds {MAX_LOCK_BYTES} bytes: {0}")]
    TooLarge(Utf8PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub fn read_vega_plugin_lock(path: &Utf8Path) -> Result<VegaPluginLock, VegaRuntimePluginError> {
    read_bounded_json(path)
}

pub fn read_vega_plugin_artifact_map(path: &Utf8Path) -> Result<VegaPluginArtifactMap, VegaRuntimePluginError> {
    read_bounded_json(path)
}

pub fn resolve_vega_runtime_plugins(
    project: &VegaProject,
    lock: Option<&VegaPluginLock>,
    artifacts: Option<(&VegaPluginArtifactMap, &Utf8Path)>,
    target: &VegaRuntimePluginTarget,
) -> VegaRuntimePluginResolution {
    let current_target = target.lock_target();
    let lock_snapshot = lock
        .cloned()
        .unwrap_or_else(|| VegaPluginLock::empty(current_target.clone()));
    let mut plan = VegaRuntimePluginPlan {
        format: VEGA_RUNTIME_PLUGIN_PLAN_FORMAT.into(),
        format_version: 1,
        target: current_target.clone(),
        source_lock_digest: lock.and_then(|lock| lock.digest().ok()),
        artifact_map_digest: artifacts.and_then(|(map, _)| map.digest().ok()),
        order: Vec::new(),
        plugins: BTreeMap::new(),
        diagnostics: Vec::new(),
    };
    let mut resolved_artifacts = BTreeMap::new();
    validate_runtime_target(&current_target, &mut plan.diagnostics);
    if let Some(lock) = lock {
        plan.diagnostics.extend(lock.validate());
        validate_current_target(lock.target.as_ref(), &current_target, &mut plan.diagnostics);
    }
    if let Some((artifact_map, _)) = artifacts {
        plan.diagnostics.extend(artifact_map.validate());
    }
    if project.plugins.is_empty() {
        if let Some(lock) = lock {
            for selected in &lock.plugins {
                plan.diagnostics.push(Diagnostic::warning(
                    "deneb.vega-runtime-plugin.unused-lock-entry",
                    format!("plugins.{}", selected.id),
                    "Locked runtime plugin is not reachable from an enabled project plugin",
                ));
            }
        }
        return VegaRuntimePluginResolution {
            plan,
            lock_snapshot,
            artifacts: resolved_artifacts,
        };
    }
    let Some(lock) = lock else {
        for plugin in &project.plugins {
            let source = plugin
                .source
                .clone()
                .unwrap_or_else(|| VegaPluginInstallSource::Builtin { key: plugin.id.clone() });
            plan.plugins
                .insert(plugin.id.clone(), plan_entry_from_request(plugin, source, None));
            runtime_failure(
                &mut plan,
                &plugin.id,
                "deneb.vega-runtime-plugin.missing-lock",
                "Plugin is not selected by a canonical vega-plugin-lock",
            );
        }
        return VegaRuntimePluginResolution {
            plan,
            lock_snapshot,
            artifacts: resolved_artifacts,
        };
    };

    let locked = lock
        .plugins
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let direct = project
        .plugins
        .iter()
        .map(|plugin| (plugin.id.as_str(), plugin))
        .collect::<BTreeMap<_, _>>();
    let mut effective = BTreeMap::<String, EffectiveRuntimeRequest>::new();
    let mut pending = BTreeSet::new();
    for plugin in &project.plugins {
        if !locked.contains_key(plugin.id.as_str()) {
            let source = plugin
                .source
                .clone()
                .unwrap_or_else(|| VegaPluginInstallSource::Builtin { key: plugin.id.clone() });
            plan.plugins
                .insert(plugin.id.clone(), plan_entry_from_request(plugin, source, None));
            runtime_failure(
                &mut plan,
                &plugin.id,
                "deneb.vega-runtime-plugin.missing-selection",
                "Project plugin is absent from vega-plugin-lock",
            );
            continue;
        }
        effective.insert(
            plugin.id.clone(),
            EffectiveRuntimeRequest {
                required: plugin.required,
                direct: true,
                configuration: plugin.configuration.clone(),
            },
        );
        pending.insert(plugin.id.clone());
    }
    while let Some(id) = pending.pop_first() {
        let request = effective[&id].clone();
        let Some(selected) = locked.get(id.as_str()) else {
            continue;
        };
        for dependency in selected.dependencies.keys() {
            if !locked.contains_key(dependency.as_str()) {
                continue;
            }
            match effective.get_mut(dependency) {
                Some(existing) => {
                    if request.required && !existing.required {
                        existing.required = true;
                        pending.insert(dependency.clone());
                    }
                }
                None => {
                    effective.insert(
                        dependency.clone(),
                        EffectiveRuntimeRequest {
                            required: request.required,
                            direct: false,
                            configuration: Value::Null,
                        },
                    );
                    pending.insert(dependency.clone());
                }
            }
        }
    }

    for (id, request) in &effective {
        let selected = locked[id.as_str()];
        plan.plugins.insert(
            id.clone(),
            VegaRuntimePluginPlanEntry {
                id: id.clone(),
                version: selected.version.clone(),
                required: request.required,
                direct: request.direct,
                status: if request.required {
                    VegaRuntimePluginStatus::Failed
                } else {
                    VegaRuntimePluginStatus::Skipped
                },
                source: selected.source.clone(),
                declared_integrity: selected_integrity(selected),
                verified_integrity: None,
                dependencies: selected.dependencies.0.clone(),
                capabilities: selected.capabilities.clone(),
                permissions: selected.permissions.clone(),
                configuration: request.configuration.clone(),
                artifact_kind: None,
                artifact: None,
                media_type: None,
                reason: None,
            },
        );
    }

    for (id, request) in &effective {
        if !request.direct {
            continue;
        }
        let plugin = direct[id.as_str()];
        let selected = locked[id.as_str()];
        if !version_matches(&plugin.version, &selected.version) {
            runtime_failure(
                &mut plan,
                id,
                "deneb.vega-runtime-plugin.version",
                &format!(
                    "Project requirement `{}` does not select locked version `{}`",
                    plugin.version, selected.version
                ),
            );
        }
        if let Some(source) = &plugin.source
            && source != &selected.source
        {
            runtime_failure(
                &mut plan,
                id,
                "deneb.vega-runtime-plugin.source",
                "Project source constraint differs from the locked source",
            );
        }
        for (dependency, requirement) in &plugin.dependencies {
            match locked.get(dependency.as_str()) {
                Some(dependency_lock) if version_matches(requirement, &dependency_lock.version) => {}
                Some(dependency_lock) => runtime_failure(
                    &mut plan,
                    id,
                    "deneb.vega-runtime-plugin.dependency-version",
                    &format!(
                        "Dependency `{dependency}` requirement `{requirement}` does not select `{}`",
                        dependency_lock.version
                    ),
                ),
                None => runtime_failure(
                    &mut plan,
                    id,
                    "deneb.vega-runtime-plugin.dependency-missing",
                    &format!("Project dependency `{dependency}` is absent from the lock"),
                ),
            }
        }
        for capability in &plugin.capabilities {
            if !selected.capabilities.contains(capability) {
                runtime_failure(
                    &mut plan,
                    id,
                    "deneb.vega-runtime-plugin.capability",
                    &format!("Locked plugin does not expose required capability `{capability}`"),
                );
            }
        }
        if let Some(targets) = &plugin.targets
            && !targets_compatible(targets, &current_target)
        {
            runtime_failure(
                &mut plan,
                id,
                "deneb.vega-runtime-plugin.target",
                "Project plugin target constraints reject this build target",
            );
        }
    }

    let artifact_entries = artifacts
        .map(|(map, _)| {
            map.plugins
                .iter()
                .map(|entry| (entry.id.as_str(), entry))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let ids = effective.keys().cloned().collect::<Vec<_>>();
    for id in ids {
        if plan.plugins[&id].reason.is_some() {
            continue;
        }
        let selected = locked[id.as_str()];
        let Some(mapped) = artifact_entries.get(id.as_str()) else {
            runtime_failure(
                &mut plan,
                &id,
                "deneb.vega-runtime-plugin.artifact-missing",
                "No host/audited artifact mapping exists for the selected plugin",
            );
            continue;
        };
        if mapped.version != selected.version || mapped.source != selected.source {
            runtime_failure(
                &mut plan,
                &id,
                "deneb.vega-runtime-plugin.artifact-identity",
                "Artifact mapping identity/source differs from the canonical lock",
            );
            continue;
        }
        let declared = selected_integrity(selected);
        if !optional_integrity_equal(declared.as_deref(), mapped.lock_integrity.as_deref()) {
            runtime_failure(
                &mut plan,
                &id,
                "deneb.vega-runtime-plugin.artifact-lock-integrity",
                "Artifact mapping is not bound to the selected canonical lock integrity",
            );
            continue;
        }
        for permission in &selected.permissions {
            if !mapped.granted_permissions.contains(permission) {
                runtime_failure(
                    &mut plan,
                    &id,
                    "deneb.vega-runtime-plugin.artifact-permission",
                    &format!("Artifact mapping has not approved permission `{permission}`"),
                );
            }
        }
        if plan.plugins[&id].reason.is_some() {
            continue;
        }
        let Some((_, artifact_root)) = artifacts else {
            unreachable!("artifact entry requires an artifact root");
        };
        let source = match resolve_artifact_path(artifact_root, &mapped.path) {
            Ok(source) => source,
            Err(message) => {
                runtime_failure(&mut plan, &id, "deneb.vega-runtime-plugin.artifact-path", &message);
                continue;
            }
        };
        let verified = match verify_runtime_artifact(&source, mapped.kind, &mapped.integrity) {
            Ok(verified) => verified,
            Err(error) => {
                runtime_failure(
                    &mut plan,
                    &id,
                    "deneb.vega-runtime-plugin.integrity",
                    &error.to_string(),
                );
                continue;
            }
        };
        let artifact_identity = format!("{id}\0{}\0{verified}", selected.version);
        let namespace = &hex::encode(Sha256::digest(artifact_identity.as_bytes()))[..16];
        let filename = match mapped.kind {
            VegaPluginArtifactKind::Module => "module.mjs",
            VegaPluginArtifactKind::WasmComponent => "component.wasm",
            VegaPluginArtifactKind::Package => "package.bin",
        };
        let relative_output = format!("runtime-plugins/artifacts/{namespace}/{filename}");
        if let Some(entry) = plan.plugins.get_mut(&id) {
            entry.status = VegaRuntimePluginStatus::Resolved;
            entry.verified_integrity = Some(verified.clone());
            entry.artifact_kind = Some(mapped.kind);
            entry.artifact = Some(relative_output.clone());
            entry.media_type = mapped.media_type.clone();
        }
        resolved_artifacts.insert(
            id,
            ResolvedRuntimeArtifact {
                source,
                relative_output,
                kind: mapped.kind,
                integrity: verified,
                media_type: mapped.media_type.clone(),
            },
        );
    }

    propagate_runtime_failures(&mut plan, &effective);
    let active = plan
        .plugins
        .iter()
        .filter(|(_, entry)| entry.status == VegaRuntimePluginStatus::Resolved)
        .map(|(id, _)| id.as_str())
        .collect::<BTreeSet<_>>();
    plan.order = lock_topological_order_filtered(lock, &active).unwrap_or_default();
    resolved_artifacts.retain(|id, _| active.contains(id.as_str()));
    for selected in &lock.plugins {
        if !effective.contains_key(&selected.id) {
            plan.diagnostics.push(Diagnostic::warning(
                "deneb.vega-runtime-plugin.unused-lock-entry",
                format!("plugins.{}", selected.id),
                "Locked runtime plugin is not reachable from an enabled project plugin",
            ));
        }
    }

    VegaRuntimePluginResolution {
        plan,
        lock_snapshot,
        artifacts: resolved_artifacts,
    }
}

fn plan_entry_from_request(
    plugin: &VegaProjectPlugin,
    source: VegaPluginInstallSource,
    reason: Option<String>,
) -> VegaRuntimePluginPlanEntry {
    VegaRuntimePluginPlanEntry {
        id: plugin.id.clone(),
        version: plugin.version.clone(),
        required: plugin.required,
        direct: true,
        status: if plugin.required {
            VegaRuntimePluginStatus::Failed
        } else {
            VegaRuntimePluginStatus::Skipped
        },
        source,
        declared_integrity: None,
        verified_integrity: None,
        dependencies: plugin.dependencies.clone(),
        capabilities: plugin.capabilities.clone(),
        permissions: plugin.permissions.clone(),
        configuration: plugin.configuration.clone(),
        artifact_kind: None,
        artifact: None,
        media_type: None,
        reason,
    }
}

fn propagate_runtime_failures(plan: &mut VegaRuntimePluginPlan, effective: &BTreeMap<String, EffectiveRuntimeRequest>) {
    loop {
        let failures = plan
            .plugins
            .iter()
            .filter(|(id, entry)| effective.contains_key(*id) && entry.status == VegaRuntimePluginStatus::Resolved)
            .filter_map(|(id, entry)| {
                entry
                    .dependencies
                    .keys()
                    .find(|dependency| {
                        plan.plugins
                            .get(*dependency)
                            .is_none_or(|dependency| dependency.status != VegaRuntimePluginStatus::Resolved)
                    })
                    .map(|dependency| (id.clone(), dependency.clone()))
            })
            .collect::<Vec<_>>();
        if failures.is_empty() {
            break;
        }
        for (id, dependency) in failures {
            runtime_failure(
                plan,
                &id,
                "deneb.vega-runtime-plugin.dependency",
                &format!("Dependency `{dependency}` did not resolve"),
            );
        }
    }
}

fn runtime_failure(plan: &mut VegaRuntimePluginPlan, id: &str, code: &str, message: &str) {
    let Some(entry) = plan.plugins.get_mut(id) else {
        return;
    };
    entry.status = if entry.required {
        VegaRuntimePluginStatus::Failed
    } else {
        VegaRuntimePluginStatus::Skipped
    };
    entry.reason.get_or_insert_with(|| message.into());
    let diagnostic = if entry.required {
        Diagnostic::error(code, format!("plugins.{id}"), message)
    } else {
        Diagnostic::warning(code, format!("plugins.{id}"), message)
    };
    plan.diagnostics.push(diagnostic);
}

fn validate_lock_target(target: &VegaPluginLockTarget, diagnostics: &mut Vec<Diagnostic>) {
    for (field, value) in [
        ("runtime", target.runtime.as_ref()),
        ("platform", target.platform.as_ref()),
        ("architecture", target.architecture.as_ref()),
        ("engineVersion", target.engine_version.as_ref()),
    ] {
        if value.is_some_and(|value| !is_non_empty_printable(value)) {
            diagnostics.push(Diagnostic::error(
                "deneb.vega-plugin-lock.target",
                format!("target.{field}"),
                "Target value must be non-empty and printable",
            ));
        }
    }
    if target.api_version.is_some_and(|version| version == 0) {
        diagnostics.push(Diagnostic::error(
            "deneb.vega-plugin-lock.target-api",
            "target.apiVersion",
            "Plugin API version must be positive",
        ));
    }
}

fn validate_runtime_target(target: &VegaPluginLockTarget, diagnostics: &mut Vec<Diagnostic>) {
    for (field, value) in [
        ("runtime", target.runtime.as_ref()),
        ("platform", target.platform.as_ref()),
        ("architecture", target.architecture.as_ref()),
    ] {
        if value.is_some_and(|value| !is_non_empty_printable(value)) {
            diagnostics.push(Diagnostic::error(
                "deneb.vega-runtime-plugin.target",
                format!("target.{field}"),
                "Current build target value must be non-empty and printable",
            ));
        }
    }
    if let Some(version) = &target.engine_version
        && Version::parse(version).is_err()
    {
        diagnostics.push(Diagnostic::error(
            "deneb.vega-runtime-plugin.engine-version",
            "target.engineVersion",
            "Current Vega engine version must be concrete SemVer",
        ));
    }
    if target.api_version != Some(1) {
        diagnostics.push(Diagnostic::error(
            "deneb.vega-runtime-plugin.api-version",
            "target.apiVersion",
            "Deneb runtime-plugin host supports canonical Vega plugin API 1",
        ));
    }
}

fn validate_current_target(
    locked: Option<&VegaPluginLockTarget>,
    current: &VegaPluginLockTarget,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(locked) = locked else {
        return;
    };
    for (field, locked, current) in [
        ("runtime", locked.runtime.as_ref(), current.runtime.as_ref()),
        ("platform", locked.platform.as_ref(), current.platform.as_ref()),
        (
            "architecture",
            locked.architecture.as_ref(),
            current.architecture.as_ref(),
        ),
        (
            "engineVersion",
            locked.engine_version.as_ref(),
            current.engine_version.as_ref(),
        ),
    ] {
        match (locked, current) {
            (Some(locked), Some(current)) if locked != current => diagnostics.push(Diagnostic::error(
                "deneb.vega-plugin-lock.target-incompatible",
                format!("target.{field}"),
                format!("Lock targets `{locked}`, current build targets `{current}`"),
            )),
            (Some(_), None) => diagnostics.push(Diagnostic::error(
                "deneb.vega-plugin-lock.target-unverified",
                format!("target.{field}"),
                format!("Lock fixes {field}, but the current build did not supply it"),
            )),
            _ => {}
        }
    }
    if let Some(api) = locked.api_version
        && current.api_version != Some(api)
    {
        diagnostics.push(Diagnostic::error(
            "deneb.vega-plugin-lock.api-incompatible",
            "target.apiVersion",
            format!("Lock targets plugin API {api}, host supports API 1"),
        ));
    }
}

fn targets_compatible(targets: &VegaPluginTargets, current: &VegaPluginLockTarget) -> bool {
    allow_list_matches(&targets.runtimes, current.runtime.as_deref())
        && allow_list_matches(&targets.platforms, current.platform.as_deref())
        && allow_list_matches(&targets.architectures, current.architecture.as_deref())
        && (targets.api_versions.is_empty()
            || current
                .api_version
                .is_some_and(|api| targets.api_versions.contains(&api)))
        && match (&targets.engine_version, &current.engine_version) {
            (Some(requirement), Some(version)) => version_matches(requirement, version),
            (Some(_), None) => false,
            _ => true,
        }
}

fn allow_list_matches(values: &[String], selected: Option<&str>) -> bool {
    values.is_empty() || selected.is_some_and(|selected| values.iter().any(|value| value == selected))
}

pub(crate) fn validate_source(source: &VegaPluginInstallSource, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let valid = match source {
        VegaPluginInstallSource::Registry { package, registry } => {
            is_non_empty_printable(package)
                && registry
                    .as_ref()
                    .is_none_or(|registry| is_absolute_non_file_url(registry))
        }
        VegaPluginInstallSource::Url { url, integrity } => {
            is_absolute_non_file_url(url) && is_non_empty_printable(integrity)
        }
        VegaPluginInstallSource::Workspace { path } => is_non_empty_printable(path),
        VegaPluginInstallSource::Builtin { key } => is_non_empty_printable(key),
    };
    if !valid {
        diagnostics.push(Diagnostic::error(
            "deneb.vega-plugin-lock.source",
            path,
            "Invalid canonical registry/url/workspace/builtin source",
        ));
    }
}

fn validate_sorted_strings(values: &[String], path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let mut previous = None::<&str>;
    for (index, value) in values.iter().enumerate() {
        if !is_non_empty_printable(value) || previous.is_some_and(|previous| previous >= value.as_str()) {
            diagnostics.push(Diagnostic::error(
                "deneb.vega-plugin-lock.sorted-values",
                format!("{path}.{index}"),
                "Values must be non-empty, unique, and sorted",
            ));
        }
        previous = Some(value);
    }
}

fn lock_topological_order(lock: &VegaPluginLock) -> Result<Vec<String>, String> {
    let all = lock
        .plugins
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<BTreeSet<_>>();
    lock_topological_order_filtered(lock, &all)
}

fn lock_topological_order_filtered(lock: &VegaPluginLock, selected: &BTreeSet<&str>) -> Result<Vec<String>, String> {
    let entries = lock
        .plugins
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut indegree = selected
        .iter()
        .map(|id| {
            let degree = entries
                .get(id)
                .map(|entry| {
                    entry
                        .dependencies
                        .keys()
                        .filter(|dependency| selected.contains(dependency.as_str()))
                        .count()
                })
                .unwrap_or(0);
            ((*id).to_owned(), degree)
        })
        .collect::<BTreeMap<_, _>>();
    let mut dependents = BTreeMap::<String, BTreeSet<String>>::new();
    for id in selected {
        if let Some(entry) = entries.get(id) {
            for dependency in entry.dependencies.keys() {
                if selected.contains(dependency.as_str()) {
                    dependents.entry(dependency.clone()).or_default().insert((*id).into());
                }
            }
        }
    }
    let mut ready = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let mut order = Vec::new();
    while let Some(id) = ready.pop_first() {
        order.push(id.clone());
        for dependent in dependents.get(&id).into_iter().flatten() {
            let degree = indegree.get_mut(dependent).expect("dependent exists");
            *degree -= 1;
            if *degree == 0 {
                ready.insert(dependent.clone());
            }
        }
    }
    if order.len() != selected.len() {
        let emitted = order.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let cycle = selected
            .iter()
            .filter(|id| !emitted.contains(**id))
            .copied()
            .collect::<Vec<_>>()
            .join(" -> ");
        return Err(format!("Dependency cycle contains: {cycle}"));
    }
    Ok(order)
}

fn resolve_artifact_path(root: &Utf8Path, relative: &str) -> Result<Utf8PathBuf, String> {
    validate_portable_relative_path(relative)?;
    let root = canonical_utf8(root).map_err(|error| error.to_string())?;
    let requested = root.join(relative);
    let source = canonical_utf8(&requested).map_err(|error| error.to_string())?;
    if !source.starts_with(&root) || !source.is_file() {
        return Err("Artifact escapes its map directory or is not a regular file".into());
    }
    Ok(source)
}

fn verify_runtime_artifact(
    path: &Utf8Path,
    kind: VegaPluginArtifactKind,
    expected: &str,
) -> Result<String, VegaRuntimePluginError> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(VegaRuntimePluginError::Invalid(format!(
            "Artifact `{path}` is not a regular file or exceeds {MAX_ARTIFACT_BYTES} bytes"
        )));
    }
    enforce_distribution_policy(DistributionScope::PublicPluginPackage, path)
        .map_err(|error| VegaRuntimePluginError::Invalid(error.to_string()))?;
    let (actual, bytes) = sha256_file(path)?;
    let expected = parse_sha256_integrity(expected).map_err(VegaRuntimePluginError::Invalid)?;
    if actual != expected {
        return Err(VegaRuntimePluginError::Invalid(format!(
            "Artifact integrity mismatch for `{path}`"
        )));
    }
    if kind == VegaPluginArtifactKind::WasmComponent {
        if bytes > MAX_COMPONENT_BYTES {
            return Err(VegaRuntimePluginError::Invalid(format!(
                "WASM Component exceeds {MAX_COMPONENT_BYTES} bytes"
            )));
        }
        let component = fs::read(path)?;
        validate_component(&component).map_err(VegaRuntimePluginError::Invalid)?;
    }
    Ok(format!("sha256-{}", hex::encode(actual)))
}

fn selected_integrity(entry: &VegaPluginLockEntry) -> Option<String> {
    entry.integrity.clone().or_else(|| match &entry.source {
        VegaPluginInstallSource::Url { integrity, .. } => Some(integrity.clone()),
        _ => None,
    })
}

fn version_matches(requirement: &str, version: &str) -> bool {
    match (NodeVersionReq::parse(requirement), NodeVersion::parse(version)) {
        (Ok(requirement), Ok(version)) => requirement.satisfies(&version),
        _ => requirement == version,
    }
}

fn integrity_equal(left: &str, right: &str) -> bool {
    match (parse_sha256_integrity(left), parse_sha256_integrity(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn optional_integrity_equal(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            if parse_sha256_integrity(left).is_ok() && parse_sha256_integrity(right).is_ok() {
                integrity_equal(left, right)
            } else {
                left == right
            }
        }
        (None, None) => true,
        _ => false,
    }
}

fn parse_sha256_integrity(value: &str) -> Result<[u8; 32], String> {
    let payload = value
        .strip_prefix("sha256-")
        .ok_or_else(|| "integrity must begin with `sha256-`".to_owned())?;
    if payload.len() == 64
        && payload
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        let decoded = hex::decode(payload).map_err(|error| error.to_string())?;
        return decoded
            .try_into()
            .map_err(|_| "SHA-256 hex digest has the wrong length".into());
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|error| error.to_string())?;
    decoded
        .try_into()
        .map_err(|_| "SHA-256 SRI digest has the wrong length".into())
}

fn sha256_integrity(bytes: &[u8]) -> String {
    format!("sha256-{}", hex::encode(Sha256::digest(bytes)))
}

fn sha256_file(path: &Utf8Path) -> Result<([u8; 32], u64), std::io::Error> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        digest.update(&buffer[..read]);
    }
    Ok((digest.finalize().into(), total))
}

fn read_bounded_json<T>(path: &Utf8Path) -> Result<T, VegaRuntimePluginError>
where
    T: for<'de> Deserialize<'de>,
{
    if fs::metadata(path)?.len() > MAX_LOCK_BYTES {
        return Err(VegaRuntimePluginError::TooLarge(path.to_owned()));
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn canonical_utf8(path: &Utf8Path) -> Result<Utf8PathBuf, std::io::Error> {
    Utf8PathBuf::from_path_buf(fs::canonicalize(path)?)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "path is not UTF-8"))
}

pub(crate) fn is_vega_plugin_id(value: &str) -> bool {
    !value.is_empty()
        && value.split('/').all(|segment| {
            let mut characters = segment.chars();
            matches!(characters.next(), Some(first) if first.is_ascii_alphanumeric())
                && characters.all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'))
        })
}

fn is_non_empty_printable(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

fn is_absolute_non_file_url(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();
    let scheme_valid = matches!(characters.next(), Some(first) if first.is_ascii_alphabetic())
        && characters.all(|character| character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.'))
        && !scheme.eq_ignore_ascii_case("file");
    let clean = !rest.is_empty()
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace());
    let network_authority = if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
        rest.strip_prefix("//")
            .and_then(|rest| rest.split(['/', '?', '#']).next())
            .is_some_and(|authority| !authority.is_empty())
    } else {
        true
    };
    scheme_valid && clean && network_authority
}
