use crate::model::VegaProject;
use serde_json::Value;

pub const PROJECT_FORMAT: &str = "vega-project";
pub const PROJECT_FORMAT_VERSION: u32 = 1;
pub const BUNDLE_FORMAT: &str = "vega-bundle";
pub const BUNDLE_FORMAT_VERSION: u32 = 1;
pub const BUILD_MANIFEST_FORMAT: &str = "deneb-build";
pub const BUILD_MANIFEST_VERSION: u32 = 1;
pub const MAX_PROJECT_BYTES: usize = 32 * 1024 * 1024;
const MAX_JSON_NODES: usize = 1_000_000;
const MAX_JSON_DEPTH: usize = 128;

#[derive(Debug, thiserror::Error)]
pub enum ProjectFormatError {
    #[error("project is {actual} bytes; the limit is {limit} bytes")]
    TooLarge { actual: usize, limit: usize },
    #[error("project JSON exceeds the structural limit")]
    TooComplex,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Parse the canonical project envelope with bounded input and structure.
///
/// Semantic validation remains a separate pass so authoring tools can show all
/// actionable diagnostics at once.
pub fn parse_project(bytes: &[u8]) -> Result<VegaProject, ProjectFormatError> {
    if bytes.len() > MAX_PROJECT_BYTES {
        return Err(ProjectFormatError::TooLarge {
            actual: bytes.len(),
            limit: MAX_PROJECT_BYTES,
        });
    }
    let value: Value = serde_json::from_slice(bytes)?;
    let mut remaining = MAX_JSON_NODES;
    validate_complexity(&value, 0, &mut remaining)?;
    Ok(serde_json::from_value(value)?)
}

fn validate_complexity(value: &Value, depth: usize, remaining: &mut usize) -> Result<(), ProjectFormatError> {
    if depth > MAX_JSON_DEPTH || *remaining == 0 {
        return Err(ProjectFormatError::TooComplex);
    }
    *remaining -= 1;
    match value {
        Value::Array(values) => {
            for value in values {
                validate_complexity(value, depth + 1, remaining)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                validate_complexity(value, depth + 1, remaining)?;
            }
        }
        _ => {}
    }
    Ok(())
}
