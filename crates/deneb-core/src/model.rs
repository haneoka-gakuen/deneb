use crate::vega_plugins::{VegaPluginInstallSource, VegaPluginTargets};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaProject {
    pub format: String,
    pub format_version: u32,
    pub id: String,
    pub title: Value,
    pub entry_scene: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<Value>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    #[serde(default)]
    pub assets: BTreeMap<String, VegaAsset>,
    #[serde(default)]
    pub plugins: Vec<VegaProjectPlugin>,
    pub scenes: BTreeMap<String, VegaScene>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaScene {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<Value>,
    pub commands: Vec<VegaCommand>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VegaCommand {
    pub command: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, rename = "noWait", skip_serializing_if = "Option::is_none")]
    pub no_wait: Option<bool>,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaAsset {
    #[serde(rename = "type")]
    pub kind: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(default)]
    pub variants: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaProjectPlugin {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub configuration: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<VegaPluginTargets>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<VegaPluginInstallSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct VegaBundle {
    pub format: String,
    pub format_version: u32,
    pub compiler: CompilerIdentity,
    pub project: VegaProject,
    #[serde(default)]
    pub integrity: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompilerIdentity {
    pub name: String,
    pub version: String,
}
