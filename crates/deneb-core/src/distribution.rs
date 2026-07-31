use camino::{Utf8Path, Utf8PathBuf};
use std::fs::File;
use std::io::Read;

const CUBISM_CORE_FINGERPRINT: &[&[u8]] = &[b"live2d cubism core", b"copyright", b"live2d inc"];
const CUBISM_FRAMEWORK_FINGERPRINT: &[&[u8]] = &[b"live2d open software license", b"copyright(c) live2d inc"];
const SPINE_RUNTIME_FINGERPRINT: &[&[u8]] = &[b"spine runtimes license agreement", b"esoteric software"];
const RESTRICTED_CONTENT_FINGERPRINTS: &[(&str, &[&[u8]])] = &[
    ("cubism-core", CUBISM_CORE_FINGERPRINT),
    ("cubism-framework", CUBISM_FRAMEWORK_FINGERPRINT),
    ("spine-runtime", SPINE_RUNTIME_FINGERPRINT),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistributionScope {
    /// A selected Deneb or Vega plugin artifact and its generated output.
    PublicPluginPackage,
    /// User-authored assets copied into the application bundle.
    ProjectBundle,
    /// The ordinary Vega runtime bundle, excluding provisioned attachments.
    RuntimeBundle,
}

impl std::fmt::Display for DistributionScope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PublicPluginPackage => "public-plugin-package",
            Self::ProjectBundle => "project-bundle",
            Self::RuntimeBundle => "runtime-bundle",
        })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{scope} distribution policy rejected `{path}` because it matches restricted runtime `{marker}`")]
pub struct DistributionPolicyError {
    pub scope: DistributionScope,
    pub path: Utf8PathBuf,
    pub marker: String,
}

/// Reject known SDK, Core, Framework, and binary runtime payloads from an
/// ordinary output scope.
///
/// Model, motion, atlas, and skeleton files are project content, not runtime
/// SDKs, so their extensions and file signatures are deliberately ignored.
/// Separately licensed runtime bytes enter only through Deneb's explicit,
/// receipt-backed application-private external-runtime attachment flow.
pub fn enforce_distribution_policy(scope: DistributionScope, path: &Utf8Path) -> Result<(), DistributionPolicyError> {
    enforce_distribution_policy_bytes(scope, path, &[])?;
    if is_legal_notice(path) {
        return Ok(());
    }

    let mut source = File::open(path).map_err(|_| DistributionPolicyError {
        scope,
        path: path.to_owned(),
        marker: "unreadable-input".into(),
    })?;
    let mut scanner = RestrictedContentScanner::default();
    let mut carry = Vec::new();
    let mut buffer = [0_u8; 64 * 1024];
    let carry_bytes = RESTRICTED_CONTENT_FINGERPRINTS
        .iter()
        .flat_map(|(_, markers)| markers.iter().map(|marker| marker.len()))
        .max()
        .unwrap_or(1)
        .saturating_sub(1);
    loop {
        let count = source.read(&mut buffer).map_err(|_| DistributionPolicyError {
            scope,
            path: path.to_owned(),
            marker: "unreadable-input".into(),
        })?;
        if count == 0 {
            break;
        }
        let mut searchable = Vec::with_capacity(carry.len() + count);
        searchable.extend_from_slice(&carry);
        searchable.extend_from_slice(&buffer[..count]);
        if let Some(marker) = scanner.scan(&searchable) {
            return Err(rejection(scope, path, marker));
        }
        carry.clear();
        carry.extend(searchable.iter().rev().take(carry_bytes).rev());
    }
    Ok(())
}

fn is_legal_notice(path: &Utf8Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_ascii_lowercase();
        matches!(
            name.as_str(),
            "license"
                | "license.md"
                | "license.txt"
                | "copying"
                | "copying.md"
                | "copying.txt"
                | "notice"
                | "notice.md"
                | "notice.txt"
                | "third_party_notices.md"
                | "third_party_notices.txt"
        )
    })
}

pub fn enforce_distribution_policy_bytes(
    scope: DistributionScope,
    path: &Utf8Path,
    bytes: &[u8],
) -> Result<(), DistributionPolicyError> {
    if is_legal_notice(path) {
        return Ok(());
    }
    if let Some(marker) = restricted_path_marker(path) {
        return Err(rejection(scope, path, marker));
    }
    let mut scanner = RestrictedContentScanner::default();
    if let Some(marker) = scanner.scan(bytes) {
        return Err(rejection(scope, path, marker));
    }
    Ok(())
}

fn restricted_path_marker(path: &Utf8Path) -> Option<&'static str> {
    let path = path.as_str().replace('\\', "/").to_ascii_lowercase();
    let components = path
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    if components.iter().any(|component| {
        matches!(
            *component,
            "cubismsdk" | "cubism-sdk" | "spine-cpp" | "spine-ts" | "spine-runtimes"
        ) || component.starts_with("cubismsdkfor")
    }) {
        return Some(if components.iter().any(|component| component.starts_with("spine")) {
            "spine-runtime"
        } else {
            "cubism-sdk"
        });
    }
    if components
        .windows(2)
        .any(|parts| parts[0] == "@esotericsoftware" && parts[1].starts_with("spine-"))
    {
        return Some("spine-runtime");
    }
    let file_name = components.last().copied().unwrap_or_default();
    matches!(
        file_name,
        "live2dcubismcore.js"
            | "live2dcubismcore.min.js"
            | "live2dcubismcore.wasm"
            | "live2dcubismcore.dll"
            | "live2dcubismcore.dylib"
            | "live2dcubismcore.so"
            | "cubismcore.js"
            | "cubismcore.min.js"
            | "cubismcore.wasm"
            | "cubismcore.dll"
            | "cubismcore.dylib"
            | "cubismcore.so"
    )
    .then_some("cubism-core")
}

#[derive(Default)]
struct RestrictedContentScanner {
    matched: Vec<Vec<bool>>,
}

impl RestrictedContentScanner {
    fn scan(&mut self, bytes: &[u8]) -> Option<&'static str> {
        if self.matched.is_empty() {
            self.matched = RESTRICTED_CONTENT_FINGERPRINTS
                .iter()
                .map(|(_, markers)| vec![false; markers.len()])
                .collect();
        }
        for ((name, markers), matched) in RESTRICTED_CONTENT_FINGERPRINTS.iter().zip(&mut self.matched) {
            for (marker, seen) in markers.iter().zip(matched.iter_mut()) {
                *seen |= bytes
                    .windows(marker.len())
                    .any(|part| part.eq_ignore_ascii_case(marker));
            }
            if matched.iter().all(|seen| *seen) {
                return Some(*name);
            }
        }
        None
    }
}

fn rejection(scope: DistributionScope, path: &Utf8Path, marker: &str) -> DistributionPolicyError {
    DistributionPolicyError {
        scope,
        path: path.to_owned(),
        marker: marker.into(),
    }
}
