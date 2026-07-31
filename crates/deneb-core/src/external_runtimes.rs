use crate::validate::{is_sha256, validate_portable_relative_path};
use crate::vega_plugins::{VegaPluginArtifactMap, VegaPluginLock, is_vega_plugin_id};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use walkdir::WalkDir;

pub const EXTERNAL_RUNTIME_INDEX_FILE: &str = "external-runtimes/deneb-external-runtimes.json";
pub const EXTERNAL_RUNTIME_SBOM_FILE: &str = "external-runtimes/deneb-external-runtime-sbom.json";
const EXTERNAL_RUNTIME_INDEX_FORMAT: &str = "deneb-external-runtime-bundle";
const EXTERNAL_RUNTIME_SBOM_FORMAT: &str = "deneb-external-runtime-sbom";
const MAX_EXTERNAL_RUNTIME_FILES: usize = 50_000;
const MAX_EXTERNAL_RUNTIME_ENTRIES: usize = 100_000;
const MAX_EXTERNAL_RUNTIME_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXTERNAL_RUNTIME_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_EXTERNAL_RUNTIME_RECEIPT_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum VegaExternalRuntimeProvisioning {
    ApplicationBundle,
    Host,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaExternalRuntimeRequirement {
    pub id: String,
    pub name: String,
    pub version: String,
    pub license: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default)]
    pub optional: bool,
    pub provisioning: Vec<VegaExternalRuntimeProvisioning>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalRuntimeDistribution {
    ApplicationPrivate,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalRuntimeLicenseAcknowledgement {
    AcceptedForApplicationPrivateBuild,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct ExternalRuntimeReceipt {
    pub path: String,
    pub integrity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct ExternalRuntimeLicenseAuthorization {
    pub id: String,
    pub acknowledgement: ExternalRuntimeLicenseAcknowledgement,
    pub receipt: ExternalRuntimeReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct ExternalRuntimeFileDeclaration {
    pub path: String,
    pub integrity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaExternalRuntimeArtifact {
    pub id: String,
    pub version: String,
    pub distribution: ExternalRuntimeDistribution,
    pub application_id: String,
    /// Directory relative to the artifact-map file. Deneb never downloads it.
    pub path: String,
    pub files: Vec<ExternalRuntimeFileDeclaration>,
    pub license: ExternalRuntimeLicenseAuthorization,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalRuntimeResolutionStatus {
    ApplicationBundle,
    Host,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct ExternalRuntimeIndex {
    pub format: String,
    pub format_version: u32,
    pub application_id: String,
    pub source_lock_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_map_digest: Option<String>,
    pub publicly_redistributable: bool,
    pub external_runtimes: Vec<ExternalRuntimeIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct ExternalRuntimeIndexEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub license: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub required_by: Vec<String>,
    pub optional: bool,
    pub status: ExternalRuntimeResolutionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_root: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ExternalRuntimeBundledFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<ExternalRuntimeAuthorizationRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct ExternalRuntimeBundledFile {
    pub path: String,
    pub integrity: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct ExternalRuntimeAuthorizationRecord {
    pub distribution: ExternalRuntimeDistribution,
    pub application_id: String,
    pub license: String,
    pub acknowledgement: ExternalRuntimeLicenseAcknowledgement,
    pub receipt_integrity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct ExternalRuntimeSbom {
    pub format: String,
    pub format_version: u32,
    pub application_id: String,
    pub source_lock_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_map_digest: Option<String>,
    pub publicly_redistributable: bool,
    pub components: Vec<ExternalRuntimeSbomComponent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct ExternalRuntimeSbomComponent {
    pub id: String,
    pub version: String,
    pub license: String,
    pub required_by: Vec<String>,
    pub artifact_root: String,
    pub files: Vec<ExternalRuntimeBundledFile>,
    pub authorization: ExternalRuntimeAuthorizationRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRuntimeBundleReport {
    pub bundled: usize,
    pub host: usize,
    pub publicly_redistributable: bool,
    pub index: Option<String>,
    pub sbom: Option<String>,
}

#[derive(Debug)]
pub struct ExternalRuntimeResolution {
    application_id: String,
    source_lock_digest: String,
    artifact_map_digest: Option<String>,
    entries: Vec<ResolvedExternalRuntime>,
}

#[derive(Debug)]
enum ResolvedExternalRuntime {
    Host(AggregatedRequirement),
    ApplicationBundle {
        requirement: AggregatedRequirement,
        attachment: Box<VegaExternalRuntimeArtifact>,
        root: Utf8PathBuf,
        receipt: Utf8PathBuf,
        receipt_integrity: String,
        namespace: String,
    },
}

#[derive(Debug, Clone)]
struct AggregatedRequirement {
    requirement: VegaExternalRuntimeRequirement,
    required_by: Vec<String>,
}

#[derive(Debug)]
struct VerifiedExternalRuntimeFile {
    source: Utf8PathBuf,
    path: String,
    integrity: String,
    bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalRuntimeError {
    #[error("invalid external runtime configuration: {0}")]
    Invalid(String),
    #[error("external runtime attachments require the explicit `--allow-private-external-runtimes` build option")]
    PrivateOptInRequired,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Walk(#[from] walkdir::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub fn validate_external_runtime_requirement(
    requirement: &VegaExternalRuntimeRequirement,
    path: &str,
    diagnostics: &mut Vec<crate::Diagnostic>,
) {
    let error = |field: &str, message: &str| {
        crate::Diagnostic::error(
            "deneb.vega-plugin-lock.external-runtime",
            format!("{path}.{field}"),
            message,
        )
    };
    if !is_vega_plugin_id(&requirement.id) {
        diagnostics.push(error("id", "External runtime id is not portable"));
    }
    for (field, value) in [
        ("name", requirement.name.as_str()),
        ("version", requirement.version.as_str()),
        ("license", requirement.license.as_str()),
    ] {
        if !is_non_empty_printable(value) {
            diagnostics.push(error(
                field,
                "External runtime metadata must be non-empty and printable",
            ));
        }
    }
    if requirement
        .homepage
        .as_ref()
        .is_some_and(|value| !is_absolute_https_url(value))
    {
        diagnostics.push(error(
            "homepage",
            "External runtime homepage must be an absolute HTTPS URL",
        ));
    }
    if requirement.provisioning.is_empty() {
        diagnostics.push(error(
            "provisioning",
            "External runtime must support host or application-bundle provisioning",
        ));
    }
    let mut seen = BTreeSet::new();
    for (index, provisioning) in requirement.provisioning.iter().enumerate() {
        if !seen.insert(*provisioning) {
            diagnostics.push(error(
                &format!("provisioning.{index}"),
                "External runtime provisioning values must be unique",
            ));
        }
    }
}

pub fn validate_external_runtime_artifact(
    artifact: &VegaExternalRuntimeArtifact,
    path: &str,
    diagnostics: &mut Vec<crate::Diagnostic>,
) {
    let error = |field: &str, message: &str| {
        crate::Diagnostic::error(
            "deneb.vega-plugin-artifacts.external-runtime",
            format!("{path}.{field}"),
            message,
        )
    };
    if !is_vega_plugin_id(&artifact.id) || !is_non_empty_printable(&artifact.version) {
        diagnostics.push(error("id", "External runtime artifact identity is invalid"));
    }
    if !is_non_empty_printable(&artifact.application_id) {
        diagnostics.push(error("applicationId", "Application id must be non-empty and printable"));
    }
    if validate_portable_relative_path(&artifact.path).is_err() {
        diagnostics.push(error(
            "path",
            "External runtime path must remain below the artifact-map directory",
        ));
    }
    if artifact.files.is_empty() {
        diagnostics.push(error("files", "External runtime attachment must enumerate every file"));
    }
    let mut previous = None::<&str>;
    let mut case_folded = BTreeSet::new();
    for (index, file) in artifact.files.iter().enumerate() {
        if validate_portable_relative_path(&file.path).is_err() {
            diagnostics.push(error(
                &format!("files.{index}.path"),
                "Attachment file path must be portable and relative",
            ));
        }
        if previous.is_some_and(|previous| previous >= file.path.as_str())
            || !case_folded.insert(file.path.to_ascii_lowercase())
        {
            diagnostics.push(error(
                &format!("files.{index}.path"),
                "Attachment files must be unique and sorted, including on case-insensitive filesystems",
            ));
        }
        previous = Some(&file.path);
        if !is_sha256(&file.integrity) {
            diagnostics.push(error(
                &format!("files.{index}.integrity"),
                "Every attachment file requires a lowercase hexadecimal SHA-256 digest",
            ));
        }
    }
    if artifact.license.id.trim().is_empty() {
        diagnostics.push(error("license.id", "License identifier must be non-empty"));
    }
    if validate_portable_relative_path(&artifact.license.receipt.path).is_err() {
        diagnostics.push(error(
            "license.receipt.path",
            "License receipt path must remain below the artifact-map directory",
        ));
    }
    if !is_sha256(&artifact.license.receipt.integrity) {
        diagnostics.push(error(
            "license.receipt.integrity",
            "License receipt requires a lowercase hexadecimal SHA-256 digest",
        ));
    }
}

pub fn resolve_external_runtimes(
    lock: &VegaPluginLock,
    selected_plugin_ids: &[String],
    artifact_map: Option<(&VegaPluginArtifactMap, &Utf8Path)>,
    application_id: &str,
    allow_private: bool,
) -> Result<ExternalRuntimeResolution, ExternalRuntimeError> {
    let requirements = aggregate_requirements(lock, selected_plugin_ids)?;
    let mapped = artifact_map
        .map(|(map, _)| {
            map.external_runtimes
                .iter()
                .map(|entry| (entry.id.as_str(), entry))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    if !mapped.is_empty() && !allow_private {
        return Err(ExternalRuntimeError::PrivateOptInRequired);
    }

    let mut entries = Vec::new();
    let mut consumed = BTreeSet::new();
    for (id, requirement) in requirements {
        if let Some(attachment) = mapped.get(id.as_str()) {
            if !requirement
                .requirement
                .provisioning
                .contains(&VegaExternalRuntimeProvisioning::ApplicationBundle)
            {
                return Err(ExternalRuntimeError::Invalid(format!(
                    "external runtime `{id}` does not permit application-bundle provisioning"
                )));
            }
            let (_, map_root) = artifact_map.expect("mapped attachment has an artifact map");
            let resolved = resolve_attachment(&requirement, attachment, map_root, application_id)?;
            consumed.insert(id);
            entries.push(resolved);
        } else if requirement
            .requirement
            .provisioning
            .contains(&VegaExternalRuntimeProvisioning::Host)
        {
            entries.push(ResolvedExternalRuntime::Host(requirement));
        } else if !requirement.requirement.optional {
            return Err(ExternalRuntimeError::Invalid(format!(
                "required external runtime `{id}` has no authorized application attachment and cannot be host-provisioned"
            )));
        }
    }
    let unexpected = mapped
        .keys()
        .filter(|id| !consumed.contains(**id))
        .copied()
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        return Err(ExternalRuntimeError::Invalid(format!(
            "artifact map contains external runtimes not required by the selected plugins: {}",
            unexpected.join(", ")
        )));
    }
    Ok(ExternalRuntimeResolution {
        application_id: application_id.into(),
        source_lock_digest: lock.digest()?,
        artifact_map_digest: artifact_map.map(|(map, _)| map.digest()).transpose()?,
        entries,
    })
}

impl ExternalRuntimeResolution {
    pub fn has_entries(&self) -> bool {
        !self.entries.is_empty()
    }

    pub fn has_private_attachments(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| matches!(entry, ResolvedExternalRuntime::ApplicationBundle { .. }))
    }

    pub fn bundle(&self, web_root: &Utf8Path) -> Result<ExternalRuntimeBundleReport, ExternalRuntimeError> {
        if self.entries.is_empty() {
            return Ok(ExternalRuntimeBundleReport {
                bundled: 0,
                host: 0,
                publicly_redistributable: true,
                index: None,
                sbom: None,
            });
        }
        let output_root = web_root.join("external-runtimes");
        fs::create_dir_all(&output_root)?;
        let mut index_entries = Vec::new();
        let mut components = Vec::new();
        let mut bundled = 0;
        let mut host = 0;
        for entry in &self.entries {
            match entry {
                ResolvedExternalRuntime::Host(requirement) => {
                    host += 1;
                    index_entries.push(index_entry(requirement, ExternalRuntimeResolutionStatus::Host));
                }
                ResolvedExternalRuntime::ApplicationBundle {
                    requirement,
                    attachment,
                    root,
                    receipt,
                    receipt_integrity,
                    namespace,
                } => {
                    verify_receipt(receipt, &attachment.license.receipt.integrity)?;
                    let verified = verify_complete_attachment(root, &attachment.files)?;
                    let artifact_root = format!("./artifacts/{namespace}/");
                    let destination_root = output_root.join(format!("artifacts/{namespace}"));
                    fs::create_dir_all(&destination_root)?;
                    let mut files = Vec::new();
                    for file in verified {
                        let destination = destination_root.join(&file.path);
                        fs::create_dir_all(
                            destination.parent().ok_or_else(|| {
                                ExternalRuntimeError::Invalid("attachment output has no parent".into())
                            })?,
                        )?;
                        let (actual, bytes) =
                            copy_file_hashed(&file.source, &destination, MAX_EXTERNAL_RUNTIME_FILE_BYTES)?;
                        let actual = format!("sha256-{actual}");
                        if actual != file.integrity || bytes != file.bytes {
                            return Err(ExternalRuntimeError::Invalid(format!(
                                "external runtime file changed while copying: {}",
                                file.path
                            )));
                        }
                        files.push(ExternalRuntimeBundledFile {
                            path: file.path,
                            integrity: actual,
                            bytes,
                        });
                    }
                    let authorization = ExternalRuntimeAuthorizationRecord {
                        distribution: attachment.distribution,
                        application_id: attachment.application_id.clone(),
                        license: attachment.license.id.clone(),
                        acknowledgement: attachment.license.acknowledgement,
                        receipt_integrity: receipt_integrity.clone(),
                    };
                    let mut index = index_entry(requirement, ExternalRuntimeResolutionStatus::ApplicationBundle);
                    index.artifact_root = Some(artifact_root.clone());
                    index.files.clone_from(&files);
                    index.authorization = Some(authorization.clone());
                    index_entries.push(index);
                    components.push(ExternalRuntimeSbomComponent {
                        id: requirement.requirement.id.clone(),
                        version: requirement.requirement.version.clone(),
                        license: requirement.requirement.license.clone(),
                        required_by: requirement.required_by.clone(),
                        artifact_root,
                        files,
                        authorization,
                    });
                    bundled += 1;
                }
            }
        }
        let publicly_redistributable = bundled == 0;
        let index = ExternalRuntimeIndex {
            format: EXTERNAL_RUNTIME_INDEX_FORMAT.into(),
            format_version: 1,
            application_id: self.application_id.clone(),
            source_lock_digest: self.source_lock_digest.clone(),
            artifact_map_digest: self.artifact_map_digest.clone(),
            publicly_redistributable,
            external_runtimes: index_entries,
        };
        let sbom = ExternalRuntimeSbom {
            format: EXTERNAL_RUNTIME_SBOM_FORMAT.into(),
            format_version: 1,
            application_id: self.application_id.clone(),
            source_lock_digest: self.source_lock_digest.clone(),
            artifact_map_digest: self.artifact_map_digest.clone(),
            publicly_redistributable,
            components,
        };
        fs::write(
            web_root.join(EXTERNAL_RUNTIME_INDEX_FILE),
            serde_json::to_vec_pretty(&index)?,
        )?;
        fs::write(
            web_root.join(EXTERNAL_RUNTIME_SBOM_FILE),
            serde_json::to_vec_pretty(&sbom)?,
        )?;
        Ok(ExternalRuntimeBundleReport {
            bundled,
            host,
            publicly_redistributable,
            index: Some(EXTERNAL_RUNTIME_INDEX_FILE.into()),
            sbom: Some(EXTERNAL_RUNTIME_SBOM_FILE.into()),
        })
    }
}

fn aggregate_requirements(
    lock: &VegaPluginLock,
    selected_plugin_ids: &[String],
) -> Result<BTreeMap<String, AggregatedRequirement>, ExternalRuntimeError> {
    let selected = selected_plugin_ids.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut aggregated = BTreeMap::<String, AggregatedRequirement>::new();
    for plugin in lock
        .plugins
        .iter()
        .filter(|plugin| selected.contains(plugin.id.as_str()))
    {
        for requirement in &plugin.external_runtimes {
            match aggregated.get_mut(&requirement.id) {
                Some(existing) => {
                    if !same_requirement(&existing.requirement, requirement) {
                        return Err(ExternalRuntimeError::Invalid(format!(
                            "plugins declare conflicting metadata for external runtime `{}`",
                            requirement.id
                        )));
                    }
                    existing.requirement.optional &= requirement.optional;
                    existing.required_by.push(plugin.id.clone());
                }
                None => {
                    aggregated.insert(
                        requirement.id.clone(),
                        AggregatedRequirement {
                            requirement: requirement.clone(),
                            required_by: vec![plugin.id.clone()],
                        },
                    );
                }
            }
        }
    }
    Ok(aggregated)
}

fn same_requirement(left: &VegaExternalRuntimeRequirement, right: &VegaExternalRuntimeRequirement) -> bool {
    left.id == right.id
        && left.name == right.name
        && left.version == right.version
        && left.license == right.license
        && left.homepage == right.homepage
        && left.provisioning == right.provisioning
}

fn resolve_attachment(
    requirement: &AggregatedRequirement,
    attachment: &VegaExternalRuntimeArtifact,
    map_root: &Utf8Path,
    application_id: &str,
) -> Result<ResolvedExternalRuntime, ExternalRuntimeError> {
    if attachment.id != requirement.requirement.id || attachment.version != requirement.requirement.version {
        return Err(ExternalRuntimeError::Invalid(format!(
            "external runtime attachment `{}` identity/version differs from the canonical lock",
            attachment.id
        )));
    }
    if attachment.application_id != application_id {
        return Err(ExternalRuntimeError::Invalid(format!(
            "external runtime `{}` is authorized for application `{}`, not `{application_id}`",
            attachment.id, attachment.application_id
        )));
    }
    if attachment.license.id != requirement.requirement.license {
        return Err(ExternalRuntimeError::Invalid(format!(
            "external runtime `{}` receipt acknowledges `{}`, but the lock requires `{}`",
            attachment.id, attachment.license.id, requirement.requirement.license
        )));
    }
    let map_root = canonical_directory(map_root, "artifact-map root")?;
    let root = resolve_below(&map_root, &attachment.path, true)?;
    let receipt = resolve_below(&map_root, &attachment.license.receipt.path, false)?;
    if receipt.starts_with(&root) {
        return Err(ExternalRuntimeError::Invalid(format!(
            "license receipt for `{}` must stay outside the attached runtime directory",
            attachment.id
        )));
    }
    let receipt_integrity = verify_receipt(&receipt, &attachment.license.receipt.integrity)?;
    let files = verify_complete_attachment(&root, &attachment.files)?;
    let namespace_seed = serde_json::to_vec(&(
        &attachment.id,
        &attachment.version,
        files
            .iter()
            .map(|file| (&file.path, &file.integrity, file.bytes))
            .collect::<Vec<_>>(),
    ))?;
    let namespace = hex::encode(Sha256::digest(namespace_seed))[..16].to_owned();
    Ok(ResolvedExternalRuntime::ApplicationBundle {
        requirement: requirement.clone(),
        attachment: Box::new(attachment.clone()),
        root,
        receipt,
        receipt_integrity,
        namespace,
    })
}

fn verify_complete_attachment(
    root: &Utf8Path,
    declarations: &[ExternalRuntimeFileDeclaration],
) -> Result<Vec<VerifiedExternalRuntimeFile>, ExternalRuntimeError> {
    let declared = declarations
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let mut actual = BTreeSet::new();
    let mut verified = Vec::new();
    let mut total = 0_u64;
    let mut entries = 0_usize;
    for entry in WalkDir::new(root).follow_links(false).sort_by_file_name() {
        let entry = entry?;
        entries += 1;
        if entries > MAX_EXTERNAL_RUNTIME_ENTRIES {
            return Err(ExternalRuntimeError::Invalid(format!(
                "external runtime attachment exceeds {MAX_EXTERNAL_RUNTIME_ENTRIES} filesystem entries"
            )));
        }
        if entry.file_type().is_symlink() {
            return Err(ExternalRuntimeError::Invalid(format!(
                "external runtime attachments may not contain symlinks: {}",
                entry.path().display()
            )));
        }
        if entry.file_type().is_dir() {
            continue;
        }
        if !entry.file_type().is_file() {
            return Err(ExternalRuntimeError::Invalid(format!(
                "external runtime attachments may contain only directories and regular files: {}",
                entry.path().display()
            )));
        }
        if verified.len() >= MAX_EXTERNAL_RUNTIME_FILES {
            return Err(ExternalRuntimeError::Invalid(format!(
                "external runtime attachment exceeds {MAX_EXTERNAL_RUNTIME_FILES} files"
            )));
        }
        let source = Utf8Path::from_path(entry.path())
            .ok_or_else(|| ExternalRuntimeError::Invalid("external runtime path is not UTF-8".into()))?;
        let relative = source
            .strip_prefix(root)
            .map_err(|_| ExternalRuntimeError::Invalid("external runtime walk escaped its root".into()))?
            .as_str()
            .replace('\\', "/");
        let Some(declaration) = declared.get(relative.as_str()) else {
            return Err(ExternalRuntimeError::Invalid(format!(
                "external runtime contains unlisted file `{relative}`"
            )));
        };
        let (digest, bytes) = digest_file_bounded(source, MAX_EXTERNAL_RUNTIME_FILE_BYTES)?;
        total = total.saturating_add(bytes);
        if total > MAX_EXTERNAL_RUNTIME_TOTAL_BYTES {
            return Err(ExternalRuntimeError::Invalid(format!(
                "external runtime attachment exceeds {MAX_EXTERNAL_RUNTIME_TOTAL_BYTES} bytes"
            )));
        }
        let integrity = format!("sha256-{digest}");
        if integrity != declaration.integrity {
            return Err(ExternalRuntimeError::Invalid(format!(
                "external runtime integrity mismatch for `{relative}`"
            )));
        }
        actual.insert(relative.clone());
        verified.push(VerifiedExternalRuntimeFile {
            source: source.to_owned(),
            path: relative,
            integrity,
            bytes,
        });
    }
    if actual.len() != declared.len() {
        let missing = declared
            .keys()
            .filter(|path| !actual.contains(**path))
            .copied()
            .collect::<Vec<_>>();
        return Err(ExternalRuntimeError::Invalid(format!(
            "external runtime manifest lists missing files: {}",
            missing.join(", ")
        )));
    }
    Ok(verified)
}

fn verify_receipt(path: &Utf8Path, expected: &str) -> Result<String, ExternalRuntimeError> {
    let (digest, _) = digest_file_bounded(path, MAX_EXTERNAL_RUNTIME_RECEIPT_BYTES)?;
    let actual = format!("sha256-{digest}");
    if actual != expected {
        return Err(ExternalRuntimeError::Invalid(format!(
            "license receipt integrity mismatch for `{path}`"
        )));
    }
    Ok(actual)
}

fn index_entry(
    requirement: &AggregatedRequirement,
    status: ExternalRuntimeResolutionStatus,
) -> ExternalRuntimeIndexEntry {
    ExternalRuntimeIndexEntry {
        id: requirement.requirement.id.clone(),
        name: requirement.requirement.name.clone(),
        version: requirement.requirement.version.clone(),
        license: requirement.requirement.license.clone(),
        homepage: requirement.requirement.homepage.clone(),
        required_by: requirement.required_by.clone(),
        optional: requirement.requirement.optional,
        status,
        artifact_root: None,
        files: Vec::new(),
        authorization: None,
    }
}

fn resolve_below(root: &Utf8Path, relative: &str, directory: bool) -> Result<Utf8PathBuf, ExternalRuntimeError> {
    validate_portable_relative_path(relative).map_err(ExternalRuntimeError::Invalid)?;
    let requested = root.join(relative);
    let mut cursor = root.to_owned();
    for component in relative.split('/') {
        cursor.push(component);
        if fs::symlink_metadata(&cursor)?.file_type().is_symlink() {
            return Err(ExternalRuntimeError::Invalid(format!(
                "external runtime paths may not contain symlinks: `{relative}`"
            )));
        }
    }
    let resolved = Utf8PathBuf::from_path_buf(fs::canonicalize(&requested)?)
        .map_err(|_| ExternalRuntimeError::Invalid("external runtime path is not UTF-8".into()))?;
    if !resolved.starts_with(root) || (directory && !resolved.is_dir()) || (!directory && !resolved.is_file()) {
        return Err(ExternalRuntimeError::Invalid(format!(
            "external runtime path escapes the artifact-map root or has the wrong type: `{relative}`"
        )));
    }
    Ok(resolved)
}

fn canonical_directory(path: &Utf8Path, label: &str) -> Result<Utf8PathBuf, ExternalRuntimeError> {
    let path = Utf8PathBuf::from_path_buf(fs::canonicalize(path)?)
        .map_err(|_| ExternalRuntimeError::Invalid(format!("{label} is not UTF-8")))?;
    if !path.is_dir() {
        return Err(ExternalRuntimeError::Invalid(format!("{label} is not a directory")));
    }
    Ok(path)
}

fn digest_file_bounded(path: &Utf8Path, limit: u64) -> Result<(String, u64), ExternalRuntimeError> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > limit {
            return Err(ExternalRuntimeError::Invalid(format!(
                "external runtime input `{path}` exceeds {limit} bytes"
            )));
        }
        digest.update(&buffer[..read]);
    }
    Ok((hex::encode(digest.finalize()), total))
}

fn copy_file_hashed(
    source: &Utf8Path,
    destination: &Utf8Path,
    limit: u64,
) -> Result<(String, u64), ExternalRuntimeError> {
    let source_path = source;
    let mut source = File::open(source_path)?;
    let mut destination = File::create(destination)?;
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > limit {
            return Err(ExternalRuntimeError::Invalid(format!(
                "external runtime input `{source_path}` exceeds {limit} bytes while copying"
            )));
        }
        digest.update(&buffer[..read]);
        destination.write_all(&buffer[..read])?;
    }
    destination.sync_all()?;
    Ok((hex::encode(digest.finalize()), total))
}

fn is_non_empty_printable(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

fn is_absolute_https_url(value: &str) -> bool {
    value
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .is_some_and(|authority| !authority.is_empty())
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}
