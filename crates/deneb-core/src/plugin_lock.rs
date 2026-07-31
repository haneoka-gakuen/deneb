use crate::diagnostics::Diagnostic;
use crate::model::VegaProject;
use crate::opcodes::FIRST_THIRD_PARTY_EXTENSION;
use crate::validate::{is_plugin_id, is_sha256, validate_portable_relative_path};
use deneb_plugin_api::{Capability, DENEB_PLUGIN_ABI_VERSION, PluginTarget};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const PLUGIN_LOCK_FORMAT: &str = "deneb-plugin-lock";
pub const PLUGIN_LOCK_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PluginLock {
    #[serde(default = "lock_format")]
    pub format: String,
    pub version: u32,
    #[serde(default = "plugin_abi_version")]
    pub plugin_abi_version: u32,
    #[serde(default)]
    pub plugins: BTreeMap<String, LockedPlugin>,
    #[serde(default)]
    pub commands: BTreeMap<String, i64>,
}

impl Default for PluginLock {
    fn default() -> Self {
        Self {
            format: PLUGIN_LOCK_FORMAT.into(),
            version: PLUGIN_LOCK_VERSION,
            plugin_abi_version: DENEB_PLUGIN_ABI_VERSION,
            plugins: BTreeMap::new(),
            commands: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct LockedPlugin {
    pub version: String,
    pub source: PluginSource,
    pub checksum: String,
    pub abi_version: u32,
    /// Optional in lock v1 for compatibility; new locks should pin it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_checksum: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_range: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<BTreeSet<PluginTarget>>,
    #[serde(default)]
    pub capabilities: BTreeSet<Capability>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflicts: Option<BTreeSet<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PluginSource {
    Registry { url: String },
    Git { repository: String, revision: String },
    Path { path: String },
}

impl PluginLock {
    /// Allocate command opcodes in lexical order without reassigning existing
    /// entries. This makes regeneration independent of discovery order.
    pub fn allocate<'a>(&mut self, commands: impl IntoIterator<Item = &'a str>) {
        self.format = PLUGIN_LOCK_FORMAT.into();
        self.version = PLUGIN_LOCK_VERSION;
        self.plugin_abi_version = DENEB_PLUGIN_ABI_VERSION;
        let requested = commands.into_iter().map(str::to_owned).collect::<BTreeSet<_>>();
        let mut used = self.commands.values().copied().collect::<BTreeSet<_>>();
        for command in requested {
            if self.commands.contains_key(&command) {
                continue;
            }
            let mut opcode = FIRST_THIRD_PARTY_EXTENSION;
            while used.contains(&opcode) {
                opcode += 1;
            }
            self.commands.insert(command, opcode);
            used.insert(opcode);
        }
    }

    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        if self.format != PLUGIN_LOCK_FORMAT {
            diagnostics.push(Diagnostic::error(
                "deneb.plugin-lock.format",
                "format",
                format!("Expected `{PLUGIN_LOCK_FORMAT}`"),
            ));
        }
        if self.version != PLUGIN_LOCK_VERSION {
            diagnostics.push(Diagnostic::error(
                "deneb.plugin-lock.version",
                "version",
                format!("Unsupported plugin lock version {}", self.version),
            ));
        }
        if self.plugin_abi_version != DENEB_PLUGIN_ABI_VERSION {
            diagnostics.push(Diagnostic::error(
                "deneb.plugin-lock.abi",
                "pluginAbiVersion",
                format!(
                    "Plugin lock ABI {} does not match host ABI {}",
                    self.plugin_abi_version, DENEB_PLUGIN_ABI_VERSION
                ),
            ));
        }

        for (id, plugin) in &self.plugins {
            let path = format!("plugins.{id}");
            if !is_plugin_id(id) {
                diagnostics.push(Diagnostic::error("deneb.plugin-lock.id", &path, "Invalid plugin id"));
            }
            if plugin.version.trim().is_empty() {
                diagnostics.push(Diagnostic::error(
                    "deneb.plugin-lock.plugin-version",
                    format!("{path}.version"),
                    "Locked plugin version must be non-empty",
                ));
            }
            if plugin.abi_version != DENEB_PLUGIN_ABI_VERSION {
                diagnostics.push(Diagnostic::error(
                    "deneb.plugin-lock.plugin-abi",
                    format!("{path}.abiVersion"),
                    "Locked plugin ABI does not match the Deneb host",
                ));
            }
            if !is_sha256(&plugin.checksum) {
                diagnostics.push(Diagnostic::error(
                    "deneb.plugin-lock.checksum",
                    format!("{path}.checksum"),
                    "Locked plugin checksum must be a lowercase sha256 digest",
                ));
            }
            if plugin
                .manifest_checksum
                .as_ref()
                .is_some_and(|checksum| !is_sha256(checksum))
            {
                diagnostics.push(Diagnostic::error(
                    "deneb.plugin-lock.manifest-checksum",
                    format!("{path}.manifestChecksum"),
                    "Locked manifest checksum must be a lowercase sha256 digest",
                ));
            }
            if plugin
                .engine_range
                .as_ref()
                .is_some_and(|range| range.trim().is_empty() || range.chars().any(char::is_control))
            {
                diagnostics.push(Diagnostic::error(
                    "deneb.plugin-lock.engine-range",
                    format!("{path}.engineRange"),
                    "Locked engine range must be a non-empty printable requirement",
                ));
            }
            validate_source(&plugin.source, &path, &mut diagnostics);
            for (dependency, requirement) in &plugin.dependencies {
                if dependency == id || !self.plugins.contains_key(dependency) {
                    diagnostics.push(Diagnostic::error(
                        "deneb.plugin-lock.dependency",
                        format!("{path}.dependencies.{dependency}"),
                        "Dependency must reference another locked plugin",
                    ));
                }
                if requirement.trim().is_empty() || requirement.chars().any(char::is_control) {
                    diagnostics.push(Diagnostic::error(
                        "deneb.plugin-lock.dependency-version",
                        format!("{path}.dependencies.{dependency}"),
                        "Dependency version requirement must be non-empty and printable",
                    ));
                }
            }
            for conflict in plugin.conflicts.iter().flatten() {
                if conflict == id || !is_plugin_id(conflict) {
                    diagnostics.push(Diagnostic::error(
                        "deneb.plugin-lock.conflict",
                        format!("{path}.conflicts.{conflict}"),
                        "Conflict must reference a valid plugin other than itself",
                    ));
                }
            }
        }

        let mut used = BTreeMap::<i64, &str>::new();
        for (command, opcode) in &self.commands {
            let path = format!("commands.{command}");
            if !is_command_name(command) {
                diagnostics.push(Diagnostic::error(
                    "deneb.plugin-lock.command",
                    &path,
                    "Command must be a scoped `namespace/name` identifier",
                ));
            }
            if *opcode < FIRST_THIRD_PARTY_EXTENSION {
                diagnostics.push(Diagnostic::error(
                    "deneb.plugin-lock.opcode-range",
                    &path,
                    format!("Third-party commands must use opcodes at or above {FIRST_THIRD_PARTY_EXTENSION}"),
                ));
            }
            if let Some(previous) = used.insert(*opcode, command) {
                diagnostics.push(Diagnostic::error(
                    "deneb.plugin-lock.opcode-duplicate",
                    &path,
                    format!("Opcode {opcode} is also allocated to `{previous}`"),
                ));
            }
        }
        diagnostics
    }

    pub fn validate_for_project(&self, project: &VegaProject) -> Vec<Diagnostic> {
        let mut diagnostics = self.validate();
        let requested = project
            .plugins
            .iter()
            .map(|plugin| (plugin.id.as_str(), plugin))
            .collect::<BTreeMap<_, _>>();
        for (id, requested) in &requested {
            match self.plugins.get(*id) {
                Some(locked) if locked.version != requested.version => {
                    diagnostics.push(Diagnostic::error(
                        "deneb.plugin-lock.version-mismatch",
                        format!("plugins.{id}.version"),
                        format!(
                            "Project requests {}, lock contains {}",
                            requested.version, locked.version
                        ),
                    ));
                }
                None => diagnostics.push(Diagnostic::error(
                    "deneb.plugin-lock.missing-plugin",
                    format!("plugins.{id}"),
                    "Project plugin is not pinned in the lock file",
                )),
                _ => {}
            }
        }
        for id in self.plugins.keys() {
            if !requested.contains_key(id.as_str()) {
                diagnostics.push(Diagnostic::warning(
                    "deneb.plugin-lock.unused-plugin",
                    format!("plugins.{id}"),
                    "Locked plugin is not requested by the project",
                ));
            }
        }
        diagnostics
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec_pretty(self)
    }

    pub fn digest(&self) -> Result<String, serde_json::Error> {
        Ok(format!(
            "sha256-{}",
            hex::encode(Sha256::digest(self.canonical_bytes()?))
        ))
    }
}

fn validate_source(source: &PluginSource, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let valid = match source {
        PluginSource::Registry { url } => url.starts_with("https://") && !url.chars().any(char::is_control),
        PluginSource::Git { repository, revision } => {
            repository.starts_with("https://")
                && revision.len() >= 7
                && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        }
        PluginSource::Path { path } => validate_portable_relative_path(path).is_ok(),
    };
    if !valid {
        diagnostics.push(Diagnostic::error(
            "deneb.plugin-lock.source",
            format!("{path}.source"),
            "Plugin source must be a safe path, HTTPS registry, or Git revision pinned by hash",
        ));
    }
}

fn is_command_name(value: &str) -> bool {
    let Some((namespace, name)) = value.split_once('/') else {
        return false;
    };
    !namespace.is_empty()
        && !name.is_empty()
        && !name.contains('/')
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
}

fn lock_format() -> String {
    PLUGIN_LOCK_FORMAT.into()
}

fn plugin_abi_version() -> u32 {
    DENEB_PLUGIN_ABI_VERSION
}
