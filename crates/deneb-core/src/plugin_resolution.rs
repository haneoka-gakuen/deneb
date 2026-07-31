use crate::diagnostics::{Diagnostic, Severity};
use crate::model::{VegaProject, VegaProjectPlugin};
use crate::plugin_lock::{LockedPlugin, PluginLock, PluginSource};
use crate::plugins::{PluginBundleError, load_path_plugin, normal_build_grants};
use crate::target::BuildTarget;
use camino::Utf8Path;
use deneb_plugin_api::{Capability, PluginManifest, PluginTarget};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const PLUGIN_RESOLUTION_FORMAT: &str = "deneb-plugin-resolution";
pub const PLUGIN_RESOLUTION_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PluginResolutionStatus {
    Resolved,
    Skipped,
    Failed,
}

impl fmt::Display for PluginResolutionStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Resolved => "resolved",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PluginResolutionEntry {
    pub id: String,
    pub version: String,
    pub required: bool,
    pub direct: bool,
    pub status: PluginResolutionStatus,
    pub source: Option<PluginSource>,
    pub checksum: Option<String>,
    pub manifest_checksum: Option<String>,
    pub declared_capabilities: BTreeSet<Capability>,
    pub granted_capabilities: BTreeSet<Capability>,
    pub dependencies: BTreeMap<String, String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PluginResolutionReport {
    pub format: String,
    pub format_version: u32,
    pub target: BuildTarget,
    pub engine_version: Option<String>,
    pub lock_digest: Option<String>,
    pub order: Vec<String>,
    pub plugins: BTreeMap<String, PluginResolutionEntry>,
    pub diagnostics: Vec<Diagnostic>,
}

impl PluginResolutionReport {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    /// Return the exact project view consumed by the build host. Project
    /// entries are roots; lock dependencies are added with null configuration.
    pub fn resolved_project(&self, project: &VegaProject) -> VegaProject {
        let direct = project
            .plugins
            .iter()
            .map(|plugin| (plugin.id.as_str(), plugin))
            .collect::<BTreeMap<_, _>>();
        let mut resolved = project.clone();
        resolved.plugins = self
            .order
            .iter()
            .filter_map(|id| {
                let entry = self.plugins.get(id)?;
                let mut plugin = direct
                    .get(id.as_str())
                    .map(|plugin| (*plugin).clone())
                    .unwrap_or_else(|| VegaProjectPlugin {
                        id: id.clone(),
                        version: entry.version.clone(),
                        required: entry.required,
                        configuration: Value::Null,
                        ..VegaProjectPlugin::default()
                    });
                plugin.required = entry.required;
                Some(plugin)
            })
            .collect();
        resolved
    }

    pub fn resolved_lock(&self, lock: &PluginLock) -> PluginLock {
        let selected = self.order.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let mut resolved = lock.clone();
        resolved.plugins.retain(|id, _| selected.contains(id.as_str()));
        resolved
    }
}

#[derive(Debug, Clone)]
struct EffectiveRequest {
    version: String,
    required: bool,
    direct: bool,
}

#[derive(Debug, Clone)]
struct Candidate {
    request: EffectiveRequest,
    locked: LockedPlugin,
    manifest: Option<PluginManifest>,
    active: bool,
}

/// Resolve only declarations and immutable component bytes. This function
/// never instantiates a Component and never evaluates registry JavaScript.
pub fn resolve_plugins(
    project: &VegaProject,
    lock: Option<&PluginLock>,
    project_dir: &Utf8Path,
    target: BuildTarget,
) -> PluginResolutionReport {
    let engine_version = project_engine_version(project).map(str::to_owned);
    let mut report = PluginResolutionReport {
        format: PLUGIN_RESOLUTION_FORMAT.into(),
        format_version: PLUGIN_RESOLUTION_VERSION,
        target,
        engine_version,
        lock_digest: lock.and_then(|lock| lock.digest().ok()),
        order: Vec::new(),
        plugins: BTreeMap::new(),
        diagnostics: Vec::new(),
    };
    if project.plugins.is_empty() {
        return report;
    }

    let Some(lock) = lock else {
        for plugin in &project.plugins {
            report.plugins.insert(
                plugin.id.clone(),
                PluginResolutionEntry {
                    id: plugin.id.clone(),
                    version: plugin.version.clone(),
                    required: plugin.required,
                    direct: true,
                    status: if plugin.required {
                        PluginResolutionStatus::Failed
                    } else {
                        PluginResolutionStatus::Skipped
                    },
                    source: None,
                    checksum: None,
                    manifest_checksum: None,
                    declared_capabilities: BTreeSet::new(),
                    granted_capabilities: BTreeSet::new(),
                    dependencies: BTreeMap::new(),
                    reason: Some("plugin is not pinned in a lock file".into()),
                },
            );
            push_resolution_failure(
                &mut report.diagnostics,
                plugin.required,
                "deneb.plugin.resolve.missing-lock",
                &plugin.id,
                "Plugin is not pinned because no deneb.lock.json was supplied",
            );
        }
        return report;
    };

    let mut effective = project
        .plugins
        .iter()
        .map(|plugin| {
            (
                plugin.id.clone(),
                EffectiveRequest {
                    version: plugin.version.clone(),
                    required: plugin.required,
                    direct: true,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut pending = effective.keys().cloned().collect::<BTreeSet<_>>();
    while let Some(id) = pending.pop_first() {
        let request = effective[&id].clone();
        let Some(locked) = lock.plugins.get(&id) else {
            continue;
        };
        if request.direct && request.version != locked.version {
            continue;
        }
        for dependency in locked.dependencies.keys() {
            let Some(dependency_lock) = lock.plugins.get(dependency) else {
                continue;
            };
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
                        EffectiveRequest {
                            version: dependency_lock.version.clone(),
                            required: request.required,
                            direct: false,
                        },
                    );
                    pending.insert(dependency.clone());
                }
            }
        }
    }

    let mut candidates = BTreeMap::new();
    for (id, request) in &effective {
        let locked = lock.plugins.get(id);
        report.plugins.insert(
            id.clone(),
            PluginResolutionEntry {
                id: id.clone(),
                version: request.version.clone(),
                required: request.required,
                direct: request.direct,
                status: if request.required {
                    PluginResolutionStatus::Failed
                } else {
                    PluginResolutionStatus::Skipped
                },
                source: locked.map(|plugin| plugin.source.clone()),
                checksum: locked.map(|plugin| plugin.checksum.clone()),
                manifest_checksum: locked.and_then(|plugin| plugin.manifest_checksum.clone()),
                declared_capabilities: locked.map(|plugin| plugin.capabilities.clone()).unwrap_or_default(),
                granted_capabilities: locked
                    .map(|plugin| normal_build_grants(&plugin.capabilities))
                    .unwrap_or_default(),
                dependencies: locked.map(|plugin| plugin.dependencies.clone()).unwrap_or_default(),
                reason: None,
            },
        );
        match locked {
            Some(locked) => {
                candidates.insert(
                    id.clone(),
                    Candidate {
                        request: request.clone(),
                        locked: locked.clone(),
                        manifest: None,
                        active: true,
                    },
                );
            }
            None => mark_failure(
                &mut report,
                &mut candidates,
                id,
                "deneb.plugin.resolve.missing-plugin",
                "Plugin or transitive dependency is absent from deneb.lock.json",
            ),
        }
    }

    for id in effective.keys() {
        let request = &effective[id];
        if request.direct
            && let Some(locked) = lock.plugins.get(id)
            && request.version != locked.version
        {
            mark_failure(
                &mut report,
                &mut candidates,
                id,
                "deneb.plugin.resolve.version",
                &format!(
                    "Project requests version {}, but the lock pins {}",
                    request.version, locked.version
                ),
            );
        }
    }

    for diagnostic in lock.validate() {
        if let Some(id) = plugin_id_for_path(&diagnostic.path, effective.keys()) {
            if diagnostic.severity == Severity::Error {
                mark_failure(&mut report, &mut candidates, &id, &diagnostic.code, &diagnostic.message);
            } else {
                report.diagnostics.push(diagnostic);
            }
        } else if !diagnostic.path.starts_with("plugins.") {
            report.diagnostics.push(diagnostic);
        }
    }

    let candidate_ids = candidates.keys().cloned().collect::<Vec<_>>();
    for id in candidate_ids {
        if !candidates[&id].active {
            continue;
        }
        let locked = candidates[&id].locked.clone();
        let path = match &locked.source {
            PluginSource::Path { path } => path.clone(),
            PluginSource::Registry { .. } | PluginSource::Git { .. } => {
                mark_failure(
                    &mut report,
                    &mut candidates,
                    &id,
                    "deneb.plugin.resolve.unmaterialized",
                    "Pinned registry/Git plugin is not materialized as an offline path Component",
                );
                continue;
            }
        };
        let loaded = match load_path_plugin(project_dir, &id, &candidates[&id].request.version, &locked, &path) {
            Ok(loaded) => loaded,
            Err(error) => {
                let code = resolution_error_code(&error);
                mark_failure(&mut report, &mut candidates, &id, code, &error.to_string());
                continue;
            }
        };
        if let Some(entry) = report.plugins.get_mut(&id) {
            entry.manifest_checksum = Some(loaded.manifest_checksum.clone());
        }
        let manifest = loaded.manifest;
        if !manifest.targets.is_empty() && !manifest.targets.contains(&plugin_target(target)) {
            mark_failure(
                &mut report,
                &mut candidates,
                &id,
                "deneb.plugin.resolve.target",
                &format!("Plugin does not support the `{target}` target"),
            );
            continue;
        }
        if let Some(requirement) = &manifest.engine_range {
            match report.engine_version.clone() {
                Some(version) => match requirement_matches(requirement, &version) {
                    Ok(true) => {}
                    Ok(false) => {
                        mark_failure(
                            &mut report,
                            &mut candidates,
                            &id,
                            "deneb.plugin.resolve.engine",
                            &format!("Plugin requires engine `{requirement}`, project selects `{version}`"),
                        );
                        continue;
                    }
                    Err(message) => {
                        mark_failure(
                            &mut report,
                            &mut candidates,
                            &id,
                            "deneb.plugin.resolve.engine-range",
                            &message,
                        );
                        continue;
                    }
                },
                None => {
                    mark_failure(
                        &mut report,
                        &mut candidates,
                        &id,
                        "deneb.plugin.resolve.engine-version",
                        "Plugin declares engineRange, but project.engine.version is absent",
                    );
                    continue;
                }
            }
        }
        let mut dependency_failure = None;
        for (dependency, requirement) in &manifest.dependencies {
            let Some(dependency_lock) = lock.plugins.get(dependency) else {
                dependency_failure = Some(format!("Dependency `{dependency}` is absent from the lock"));
                break;
            };
            match requirement_matches(requirement, &dependency_lock.version) {
                Ok(true) => {}
                Ok(false) => {
                    dependency_failure = Some(format!(
                        "Dependency `{dependency}` requires `{requirement}`, lock pins `{}`",
                        dependency_lock.version
                    ));
                    break;
                }
                Err(message) => {
                    dependency_failure = Some(format!("Dependency `{dependency}`: {message}"));
                    break;
                }
            }
        }
        if let Some(message) = dependency_failure {
            mark_failure(
                &mut report,
                &mut candidates,
                &id,
                "deneb.plugin.resolve.dependency-version",
                &message,
            );
            continue;
        }
        let denied = manifest
            .capabilities
            .difference(&normal_build_grants(&manifest.capabilities))
            .cloned()
            .collect::<BTreeSet<_>>();
        if !denied.is_empty() {
            report.diagnostics.push(Diagnostic::warning(
                "deneb.plugin.resolve.permission-denied",
                format!("plugins.{id}.capabilities"),
                format!(
                    "Normal builds keep ambient capabilities denied: {}",
                    denied
                        .iter()
                        .map(|capability| format!("{capability:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        candidates.get_mut(&id).expect("candidate exists").manifest = Some(manifest);
    }

    resolve_conflicts(&mut report, &mut candidates);
    propagate_dependency_failures(&mut report, &mut candidates);

    let (mut order, cyclic) = topological_order(&candidates);
    if !cyclic.is_empty() {
        let cycle = cyclic.join(", ");
        for id in cyclic {
            mark_failure(
                &mut report,
                &mut candidates,
                &id,
                "deneb.plugin.resolve.cycle",
                &format!("Dependency graph contains a cycle involving: {cycle}"),
            );
        }
        propagate_dependency_failures(&mut report, &mut candidates);
        (order, _) = topological_order(&candidates);
    }

    for id in &order {
        if let Some(entry) = report.plugins.get_mut(id) {
            entry.status = PluginResolutionStatus::Resolved;
            entry.reason = None;
        }
    }
    report.order = order;
    report
}

fn resolve_conflicts(report: &mut PluginResolutionReport, candidates: &mut BTreeMap<String, Candidate>) {
    let ids = candidates
        .iter()
        .filter(|(_, candidate)| candidate.active && candidate.manifest.is_some())
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for (offset, left) in ids.iter().enumerate() {
        for right in ids.iter().skip(offset + 1) {
            if !candidates.get(left).is_some_and(|candidate| candidate.active)
                || !candidates.get(right).is_some_and(|candidate| candidate.active)
            {
                continue;
            }
            let conflicts = candidates[left]
                .manifest
                .as_ref()
                .is_some_and(|manifest| manifest.conflicts.contains(right))
                || candidates[right]
                    .manifest
                    .as_ref()
                    .is_some_and(|manifest| manifest.conflicts.contains(left));
            if !conflicts {
                continue;
            }
            let left_required = candidates[left].request.required;
            let right_required = candidates[right].request.required;
            let message = format!("Enabled plugins `{left}` and `{right}` conflict");
            match (left_required, right_required) {
                (true, true) => {
                    mark_failure(report, candidates, left, "deneb.plugin.resolve.conflict", &message);
                    mark_failure(report, candidates, right, "deneb.plugin.resolve.conflict", &message);
                }
                (true, false) => mark_failure(report, candidates, right, "deneb.plugin.resolve.conflict", &message),
                (false, true) => mark_failure(report, candidates, left, "deneb.plugin.resolve.conflict", &message),
                (false, false) => {
                    let loser = if left < right { right } else { left };
                    mark_failure(report, candidates, loser, "deneb.plugin.resolve.conflict", &message);
                }
            }
        }
    }
}

fn propagate_dependency_failures(report: &mut PluginResolutionReport, candidates: &mut BTreeMap<String, Candidate>) {
    loop {
        let failures = candidates
            .iter()
            .filter(|(_, candidate)| candidate.active)
            .filter_map(|(id, candidate)| {
                candidate
                    .locked
                    .dependencies
                    .keys()
                    .find(|dependency| !candidates.get(*dependency).is_some_and(|candidate| candidate.active))
                    .map(|dependency| (id.clone(), dependency.clone()))
            })
            .collect::<Vec<_>>();
        if failures.is_empty() {
            break;
        }
        for (id, dependency) in failures {
            mark_failure(
                report,
                candidates,
                &id,
                "deneb.plugin.resolve.dependency",
                &format!("Dependency `{dependency}` did not resolve"),
            );
        }
    }
}

fn topological_order(candidates: &BTreeMap<String, Candidate>) -> (Vec<String>, Vec<String>) {
    let active = candidates
        .iter()
        .filter(|(_, candidate)| candidate.active)
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let mut indegree = active
        .iter()
        .map(|id| {
            let count = candidates[id]
                .locked
                .dependencies
                .keys()
                .filter(|dependency| active.contains(*dependency))
                .count();
            (id.clone(), count)
        })
        .collect::<BTreeMap<_, _>>();
    let mut dependents = BTreeMap::<String, BTreeSet<String>>::new();
    for id in &active {
        for dependency in candidates[id].locked.dependencies.keys() {
            if active.contains(dependency) {
                dependents.entry(dependency.clone()).or_default().insert(id.clone());
            }
        }
    }
    let mut ready = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(active.len());
    while let Some(id) = ready.pop_first() {
        order.push(id.clone());
        if let Some(values) = dependents.get(&id) {
            for dependent in values {
                let degree = indegree.get_mut(dependent).expect("dependent has indegree");
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(dependent.clone());
                }
            }
        }
    }
    let emitted = order.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let cyclic = active.into_iter().filter(|id| !emitted.contains(id.as_str())).collect();
    (order, cyclic)
}

fn mark_failure(
    report: &mut PluginResolutionReport,
    candidates: &mut BTreeMap<String, Candidate>,
    id: &str,
    code: &str,
    message: &str,
) {
    let Some(entry) = report.plugins.get_mut(id) else {
        return;
    };
    if let Some(candidate) = candidates.get_mut(id) {
        candidate.active = false;
    }
    entry.status = if entry.required {
        PluginResolutionStatus::Failed
    } else {
        PluginResolutionStatus::Skipped
    };
    entry.reason.get_or_insert_with(|| message.to_owned());
    push_resolution_failure(&mut report.diagnostics, entry.required, code, id, message);
}

fn push_resolution_failure(diagnostics: &mut Vec<Diagnostic>, required: bool, code: &str, id: &str, message: &str) {
    let diagnostic = if required {
        Diagnostic::error(code, format!("plugins.{id}"), message)
    } else {
        Diagnostic::warning(code, format!("plugins.{id}"), message)
    };
    diagnostics.push(diagnostic);
    if !required && code != "deneb.plugin.optional-failure" {
        diagnostics.push(Diagnostic::warning(
            "deneb.plugin.optional-failure",
            format!("plugins.{id}"),
            format!("Optional plugin `{id}` was skipped ({code}): {message}"),
        ));
    }
}

fn plugin_id_for_path<'a>(path: &str, ids: impl Iterator<Item = &'a String>) -> Option<String> {
    ids.filter(|id| {
        let prefix = format!("plugins.{id}");
        path == prefix || path.starts_with(&format!("{prefix}."))
    })
    .max_by_key(|id| id.len())
    .cloned()
}

fn project_engine_version(project: &VegaProject) -> Option<&str> {
    project
        .engine
        .as_ref()
        .and_then(|engine| engine.get("version"))
        .and_then(Value::as_str)
}

fn requirement_matches(requirement: &str, version: &str) -> Result<bool, String> {
    if requirement.trim().is_empty() {
        return Err("version requirement must not be empty".into());
    }
    match (VersionReq::parse(requirement), Version::parse(version)) {
        (Ok(requirement), Ok(version)) => Ok(requirement.matches(&version)),
        _ if requirement == version => Ok(true),
        (Err(error), _) => Err(format!(
            "`{requirement}` is not a valid semantic-version requirement: {error}"
        )),
        (_, Err(error)) => Err(format!("`{version}` is not a valid semantic version: {error}")),
    }
}

fn plugin_target(target: BuildTarget) -> PluginTarget {
    match target {
        BuildTarget::Web => PluginTarget::Web,
        BuildTarget::Pwa => PluginTarget::Pwa,
        BuildTarget::Electron => PluginTarget::Electron,
        BuildTarget::Tauri => PluginTarget::Tauri,
        BuildTarget::Android => PluginTarget::Android,
        BuildTarget::Ios => PluginTarget::Ios,
    }
}

fn resolution_error_code(error: &PluginBundleError) -> &'static str {
    match error {
        PluginBundleError::Checksum { .. } | PluginBundleError::ManifestChecksum { .. } => {
            "deneb.plugin.resolve.integrity"
        }
        PluginBundleError::CapabilityMismatch(_) => "deneb.plugin.resolve.permission",
        PluginBundleError::Identity(_) => "deneb.plugin.resolve.identity",
        PluginBundleError::Manifest(_) | PluginBundleError::ManifestMetadata(_) => "deneb.plugin.resolve.manifest",
        PluginBundleError::NotAComponent(_) => "deneb.plugin.resolve.component",
        PluginBundleError::PathEscape(_) | PluginBundleError::EntrypointEscape(_) => "deneb.plugin.resolve.path",
        _ => "deneb.plugin.resolve.source",
    }
}
