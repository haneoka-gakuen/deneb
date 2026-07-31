use crate::diagnostics::Diagnostic;
use crate::format::{PROJECT_FORMAT, PROJECT_FORMAT_VERSION};
use crate::model::VegaProject;
use crate::opcodes;
use crate::vega_plugins::{is_vega_plugin_id, validate_source};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Component, Path};

const ASSET_KINDS: &[&str] = &["image", "audio", "video", "model", "font", "data", "other"];

pub fn validate_project(project: &VegaProject) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if project.format != PROJECT_FORMAT {
        diagnostics.push(Diagnostic::error(
            "deneb.format",
            "format",
            format!("Expected format `{PROJECT_FORMAT}`"),
        ));
    }
    if project.format_version != PROJECT_FORMAT_VERSION {
        diagnostics.push(Diagnostic::error(
            "deneb.format-version",
            "formatVersion",
            format!("Unsupported Vega project version {}", project.format_version),
        ));
    }
    if !is_portable_id(&project.id) {
        diagnostics.push(Diagnostic::error(
            "deneb.project-id",
            "id",
            "Project id must be 1-64 portable ASCII characters and start with a letter or digit",
        ));
    }
    validate_localized_text(&project.title, "title", &mut diagnostics);
    if project.engine.as_ref().is_some_and(|engine| !engine.is_object()) {
        diagnostics.push(Diagnostic::error(
            "deneb.engine",
            "engine",
            "Engine requirements must be an object",
        ));
    }
    if !project.scenes.contains_key(&project.entry_scene) {
        diagnostics.push(Diagnostic::error(
            "deneb.entry-scene",
            "entryScene",
            format!("Entry scene `{}` does not exist", project.entry_scene),
        ));
    }

    for (scene_key, scene) in &project.scenes {
        if !is_portable_id(scene_key) {
            diagnostics.push(Diagnostic::error(
                "deneb.scene-key",
                format!("scenes.{scene_key}"),
                "Scene keys must be portable identifiers",
            ));
        }
        if scene.id != *scene_key {
            diagnostics.push(Diagnostic::warning(
                "deneb.scene-id",
                format!("scenes.{scene_key}.id"),
                "Scene id differs from its map key",
            ));
        }
        if let Some(title) = &scene.title {
            validate_localized_text(title, &format!("scenes.{scene_key}.title"), &mut diagnostics);
        }
        let mut keys = BTreeSet::new();
        for (index, command) in scene.commands.iter().enumerate() {
            let command_path = format!("scenes.{scene_key}.commands.{index}");
            let opcode_path = format!("{command_path}.command");
            if command.command < 0 {
                diagnostics.push(Diagnostic::error(
                    "deneb.opcode.negative",
                    opcode_path,
                    "Opcode must be non-negative",
                ));
            } else if opcodes::is_retired(command.command) {
                diagnostics.push(Diagnostic::error(
                    "deneb.opcode.retired",
                    opcode_path,
                    format!("Opcode {} is a frozen retired native slot", command.command),
                ));
            } else if command.command <= 100 && !opcodes::is_native(command.command) {
                diagnostics.push(Diagnostic::error(
                    "deneb.opcode.reserved",
                    opcode_path,
                    format!("Opcode {} is reserved and has no native meaning", command.command),
                ));
            } else if (101..opcodes::COMMAND_GROUP).contains(&command.command) {
                diagnostics.push(Diagnostic::error(
                    "deneb.opcode.future-native",
                    opcode_path,
                    "Opcodes 101-499 are reserved for future native semantics",
                ));
            } else if command.command > opcodes::COMMAND_GROUP && command.command < opcodes::FIRST_THIRD_PARTY_EXTENSION
            {
                diagnostics.push(Diagnostic::warning(
                    "deneb.opcode.official-extension",
                    opcode_path,
                    "Opcode is in Vega's official extension range",
                ));
            }
            if command.index.is_some_and(|value| value < 0) {
                diagnostics.push(Diagnostic::error(
                    "deneb.command-index",
                    format!("{command_path}.index"),
                    "Command index must be non-negative",
                ));
            }
            if let Some(key) = &command.key
                && (!is_portable_key(key) || !keys.insert(key))
            {
                let (code, message) = if is_portable_key(key) {
                    ("deneb.command-key.duplicate", format!("Duplicate command key `{key}`"))
                } else {
                    (
                        "deneb.command-key.invalid",
                        "Command key must be non-empty, printable, and at most 128 characters".to_owned(),
                    )
                };
                diagnostics.push(Diagnostic::error(code, format!("{command_path}.key"), message));
            }
        }
    }

    for (key, asset) in &project.assets {
        if !is_portable_key(key) {
            diagnostics.push(Diagnostic::error(
                "deneb.asset-key",
                format!("assets.{key}"),
                "Asset keys must be non-empty, printable, and at most 128 characters",
            ));
        }
        if !ASSET_KINDS.contains(&asset.kind.as_str()) {
            diagnostics.push(Diagnostic::error(
                "deneb.asset-type",
                format!("assets.{key}.type"),
                format!("Unsupported asset type `{}`", asset.kind),
            ));
        }
        if let Err(message) = validate_asset_source(&asset.source) {
            diagnostics.push(Diagnostic::error(
                "deneb.asset-path",
                format!("assets.{key}.source"),
                message,
            ));
        } else if asset.source.starts_with("http://") {
            diagnostics.push(Diagnostic::warning(
                "deneb.asset-http",
                format!("assets.{key}.source"),
                "HTTP assets are not integrity-protected in transit; prefer HTTPS",
            ));
        }
        for (variant, source) in &asset.variants {
            if let Err(message) = validate_asset_source(source) {
                diagnostics.push(Diagnostic::error(
                    "deneb.asset-variant-path",
                    format!("assets.{key}.variants.{variant}"),
                    message,
                ));
            }
        }
        if let Some(hash) = &asset.hash
            && !is_sha256(hash)
        {
            diagnostics.push(Diagnostic::error(
                "deneb.asset-hash",
                format!("assets.{key}.hash"),
                "Asset hash must use `sha256-` followed by 64 lowercase hex characters",
            ));
        }
    }

    let mut plugin_ids = BTreeSet::new();
    for (index, plugin) in project.plugins.iter().enumerate() {
        let path = format!("plugins.{index}");
        if !is_vega_plugin_id(&plugin.id) {
            diagnostics.push(Diagnostic::error(
                "deneb.plugin-id",
                format!("{path}.id"),
                "Plugin id does not match the canonical Vega plugin identifier syntax",
            ));
        } else if !plugin_ids.insert(&plugin.id) {
            diagnostics.push(Diagnostic::error(
                "deneb.plugin-duplicate",
                format!("{path}.id"),
                format!("Plugin `{}` is listed more than once", plugin.id),
            ));
        }
        if plugin.version.trim().is_empty() || plugin.version.chars().any(char::is_control) {
            diagnostics.push(Diagnostic::error(
                "deneb.plugin-version",
                format!("{path}.version"),
                "Plugin version must be a non-empty printable value",
            ));
        }
        if !plugin.configuration.is_null() && !plugin.configuration.is_object() {
            diagnostics.push(Diagnostic::error(
                "deneb.plugin-configuration",
                format!("{path}.configuration"),
                "Plugin configuration must be an object",
            ));
        }
        validate_unique_plugin_strings(&plugin.capabilities, &format!("{path}.capabilities"), &mut diagnostics);
        validate_unique_plugin_strings(&plugin.permissions, &format!("{path}.permissions"), &mut diagnostics);
        for (dependency, requirement) in &plugin.dependencies {
            if !is_vega_plugin_id(dependency) {
                diagnostics.push(Diagnostic::error(
                    "deneb.plugin-dependency-id",
                    format!("{path}.dependencies.{dependency}"),
                    "Dependency id does not match the canonical Vega plugin identifier syntax",
                ));
            }
            if requirement.trim().is_empty() || requirement.chars().any(char::is_control) {
                diagnostics.push(Diagnostic::error(
                    "deneb.plugin-dependency-version",
                    format!("{path}.dependencies.{dependency}"),
                    "Dependency version requirement must be non-empty and printable",
                ));
            }
        }
        if let Some(targets) = &plugin.targets {
            for (field, values) in [
                ("runtimes", &targets.runtimes),
                ("platforms", &targets.platforms),
                ("architectures", &targets.architectures),
            ] {
                validate_unique_plugin_strings(values, &format!("{path}.targets.{field}"), &mut diagnostics);
            }
            if targets
                .engine_version
                .as_ref()
                .is_some_and(|value| value.trim().is_empty() || value.chars().any(char::is_control))
            {
                diagnostics.push(Diagnostic::error(
                    "deneb.plugin-target-engine",
                    format!("{path}.targets.engineVersion"),
                    "Engine version requirement must be non-empty and printable",
                ));
            }
            for (api_index, version) in targets.api_versions.iter().enumerate() {
                if *version == 0 {
                    diagnostics.push(Diagnostic::error(
                        "deneb.plugin-target-api",
                        format!("{path}.targets.apiVersions.{api_index}"),
                        "Plugin API versions must be positive",
                    ));
                }
            }
        }
        if let Some(source) = &plugin.source {
            validate_source(source, &format!("{path}.source"), &mut diagnostics);
        }
    }
    diagnostics
}

fn validate_unique_plugin_strings(values: &[String], path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let mut seen = BTreeSet::new();
    for (index, value) in values.iter().enumerate() {
        if value.trim().is_empty() || value.chars().any(char::is_control) || !seen.insert(value) {
            diagnostics.push(Diagnostic::error(
                "deneb.plugin-string-list",
                format!("{path}.{index}"),
                "Values must be non-empty, printable, and unique",
            ));
        }
    }
}

pub fn validate_asset_source(source: &str) -> Result<(), String> {
    if source.starts_with("https://") || source.starts_with("http://") || source.starts_with("data:") {
        return Ok(());
    }
    validate_portable_relative_path(source)
}

pub fn validate_portable_relative_path(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.contains('\\')
        || value.contains(':')
        || value
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || value.chars().any(char::is_control)
    {
        return Err("Path is empty or contains characters that are unsafe across platforms".into());
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
        return Err("Path must remain inside the project directory".into());
    }
    Ok(())
}

pub fn is_portable_id(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphanumeric())
        && value.len() <= 64
        && chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '-' | '_' | '.')
        })
}

pub fn is_plugin_id(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit())
        && value.len() <= 96
        && value.contains(['.', '/'])
        && !value.contains("..")
        && !value.contains("//")
        && !value.ends_with(['.', '/'])
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '-' | '_' | '.' | '/')
        })
}

pub fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256-").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

fn is_portable_key(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 128 && !value.chars().any(char::is_control)
}

fn validate_localized_text(value: &Value, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let valid = match value {
        Value::String(_) => true,
        Value::Object(entries) => {
            !entries.is_empty()
                && entries.iter().all(|(locale, text)| {
                    !locale.is_empty()
                        && locale.len() <= 35
                        && locale.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                        && text.is_string()
                })
        }
        _ => false,
    };
    if !valid {
        diagnostics.push(Diagnostic::error(
            "deneb.localized-text",
            path,
            "Localized text must be a string or a non-empty locale-to-string object",
        ));
    }
}
