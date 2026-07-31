#![forbid(unsafe_code)]

use anyhow::{Context, Result, bail};
use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand};
use deneb_core::{
    BuildMode, BuildOptions, BuildTarget, Diagnostic, MAX_PROJECT_BYTES, PluginLock, PluginResolutionReport, Severity,
    VEGA_PLUGIN_ARTIFACT_MAP_FILE, VEGA_PLUGIN_LOCK_FILE, VegaProject, VegaRuntimePluginPlan, VegaRuntimePluginTarget,
    build_project, parse_project, read_vega_plugin_artifact_map, read_vega_plugin_lock, resolve_plugins,
    resolve_vega_runtime_plugins, validate_project,
};
use std::fs;
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, ExitCode};

#[derive(Debug, Parser)]
#[command(name = "deneb", version, about = "Vega visual-novel build and delivery toolchain")]
struct Cli {
    #[arg(long, global = true, help = "Emit machine-readable JSON")]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Doctor {
        #[arg(long, help = "Report readiness for one target")]
        target: Option<BuildTarget>,
    },
    Validate {
        project: Utf8PathBuf,
    },
    Inspect {
        project: Utf8PathBuf,
    },
    /// Resolve the project's selected plugin graph without executing plugins.
    Resolve(PluginResolutionArgs),
    /// Inspect or validate the project's selected plugin graph.
    Plugins {
        #[command(subcommand)]
        command: PluginsCommand,
    },
    Build {
        project: Utf8PathBuf,
        #[arg(short, long)]
        output: Utf8PathBuf,
        #[arg(long, default_value = "web")]
        target: BuildTarget,
        #[arg(
            long,
            default_value = "release",
            help = "Release requires an offline-closed project and bound compatible runtime"
        )]
        mode: BuildMode,
        #[arg(
            long,
            visible_alias = "runtime",
            value_name = "DIR",
            help = "Self-contained Vega runtime directory with deneb-runtime.json"
        )]
        runtime_bundle: Option<Utf8PathBuf>,
        #[arg(
            long,
            value_name = "FILE",
            help = "Plugin lock (defaults to deneb.lock.json beside the project)"
        )]
        plugin_lock: Option<Utf8PathBuf>,
        #[arg(
            long,
            value_name = "FILE",
            help = "Canonical Vega runtime-plugin lock (defaults to vega.plugins.lock.json beside the project)"
        )]
        vega_plugin_lock: Option<Utf8PathBuf>,
        #[arg(
            long,
            value_name = "FILE",
            help = "Host/audited runtime-plugin artifact map (defaults to vega.plugins.cache.json)"
        )]
        vega_plugin_artifact_map: Option<Utf8PathBuf>,
        #[arg(long, value_name = "ARCH")]
        vega_plugin_architecture: Option<String>,
        #[arg(
            long,
            help = "Permit lock-required, receipt-backed application-private external runtime attachments"
        )]
        allow_private_external_runtimes: bool,
        #[arg(
            long,
            value_name = "REVERSE_DNS",
            help = "Bundle identifier for generated native hosts"
        )]
        bundle_identifier: Option<String>,
        #[arg(
            long,
            value_name = "KEY",
            help = "Local-storage key for the generated shell color mode"
        )]
        color_mode_storage_key: Option<String>,
        #[arg(long)]
        force: bool,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        archive: bool,
    },
}

#[derive(Debug, Clone, Args)]
struct PluginResolutionArgs {
    project: Utf8PathBuf,
    #[arg(long, default_value = "web")]
    target: BuildTarget,
    #[arg(
        long,
        value_name = "FILE",
        help = "Plugin lock (defaults to deneb.lock.json beside the project)"
    )]
    plugin_lock: Option<Utf8PathBuf>,
    #[arg(
        long,
        value_name = "FILE",
        help = "Canonical Vega runtime-plugin lock (defaults to vega.plugins.lock.json beside the project)"
    )]
    vega_plugin_lock: Option<Utf8PathBuf>,
    #[arg(
        long,
        value_name = "FILE",
        help = "Host/audited runtime-plugin artifact map (defaults to vega.plugins.cache.json)"
    )]
    vega_plugin_artifact_map: Option<Utf8PathBuf>,
    #[arg(long, value_name = "ARCH")]
    vega_plugin_architecture: Option<String>,
    #[arg(long, value_name = "VERSION")]
    vega_engine_version: Option<String>,
}

#[derive(Debug, Subcommand)]
enum PluginsCommand {
    /// List roots, transitive dependencies, and their resolution status.
    List(PluginResolutionArgs),
    /// Validate the selected plugin graph without executing Components.
    Validate(PluginResolutionArgs),
}

fn main() -> ExitCode {
    let json_requested = std::env::args_os().any(|argument| argument == "--json");
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if json_requested {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "code": "deneb.cli.arguments",
                        "message": error.to_string(),
                    })
                );
            } else {
                let _ = error.print();
            }
            return ExitCode::from(2);
        }
    };
    let json = cli.json;
    match dispatch(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if json {
                let causes = error.chain().skip(1).map(ToString::to_string).collect::<Vec<_>>();
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "code": "deneb.command.failed",
                        "message": error.to_string(),
                        "causes": causes,
                    })
                );
            } else {
                eprintln!("deneb: {error:#}");
            }
            ExitCode::FAILURE
        }
    }
}

fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Doctor { target } => doctor(cli.json, target),
        Command::Validate { project } => validate(&project, cli.json),
        Command::Inspect { project } => inspect(&project, cli.json),
        Command::Resolve(arguments) => resolve(arguments, cli.json),
        Command::Plugins { command } => match command {
            PluginsCommand::List(arguments) => plugins_list(arguments, cli.json),
            PluginsCommand::Validate(arguments) => plugins_validate(arguments, cli.json),
        },
        Command::Build {
            project,
            output,
            target,
            mode,
            runtime_bundle,
            plugin_lock,
            vega_plugin_lock,
            vega_plugin_artifact_map,
            vega_plugin_architecture,
            allow_private_external_runtimes,
            bundle_identifier,
            color_mode_storage_key,
            force,
            archive,
        } => build(
            BuildOptions {
                project_file: project,
                output,
                target,
                mode,
                force,
                archive,
                runtime_bundle,
                plugin_lock,
                vega_plugin_lock,
                vega_plugin_artifact_map,
                vega_plugin_architecture,
                allow_private_external_runtimes,
                bundle_identifier,
                color_mode_storage_key,
                plugin_limits: Default::default(),
            },
            cli.json,
        ),
    }
}

enum PluginResolution {
    Deneb(PluginResolutionReport),
    Vega(VegaRuntimePluginPlan),
}

impl PluginResolution {
    fn diagnostics(&self) -> &[Diagnostic] {
        match self {
            Self::Deneb(resolution) => &resolution.diagnostics,
            Self::Vega(resolution) => &resolution.diagnostics,
        }
    }

    fn order(&self) -> &[String] {
        match self {
            Self::Deneb(resolution) => &resolution.order,
            Self::Vega(resolution) => &resolution.order,
        }
    }

    fn has_errors(&self) -> bool {
        self.diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

fn plugin_resolution(arguments: &PluginResolutionArgs) -> Result<PluginResolution> {
    let project_path =
        fs::canonicalize(&arguments.project).with_context(|| format!("failed to resolve {}", arguments.project))?;
    let project_path = Utf8PathBuf::from_path_buf(project_path)
        .map_err(|path| anyhow::anyhow!("project path is not UTF-8: {}", path.to_string_lossy()))?;
    let project = read_project(&project_path)?;
    let project_dir = project_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("project path has no parent"))?;
    if arguments.plugin_lock.is_some() && arguments.vega_plugin_lock.is_some() {
        bail!("resolve accepts one plugin-lock scope at a time; use build to consume both scopes");
    }
    let vega_lock_path = arguments.vega_plugin_lock.clone().or_else(|| {
        if arguments.plugin_lock.is_some() {
            return None;
        }
        let conventional = project_dir.join(VEGA_PLUGIN_LOCK_FILE);
        conventional.exists().then_some(conventional)
    });
    if let Some(vega_lock_path) = vega_lock_path {
        let lock = read_vega_plugin_lock(&vega_lock_path)
            .with_context(|| format!("failed to read canonical Vega plugin lock {vega_lock_path}"))?;
        let mut runtime_project = project.clone();
        if project_dir.join("deneb.lock.json").exists() {
            let runtime_ids = lock
                .plugins
                .iter()
                .map(|plugin| plugin.id.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            runtime_project
                .plugins
                .retain(|plugin| runtime_ids.contains(plugin.id.as_str()));
        }
        let artifact_path = arguments.vega_plugin_artifact_map.clone().or_else(|| {
            let conventional = project_dir.join(VEGA_PLUGIN_ARTIFACT_MAP_FILE);
            conventional.exists().then_some(conventional)
        });
        let artifact_map = artifact_path
            .as_ref()
            .map(|path| {
                read_vega_plugin_artifact_map(path)
                    .with_context(|| format!("failed to read Vega plugin artifact map {path}"))
            })
            .transpose()?;
        let artifact_root = artifact_path
            .as_deref()
            .and_then(camino::Utf8Path::parent)
            .unwrap_or(project_dir);
        let mut resolution = resolve_vega_runtime_plugins(
            &runtime_project,
            Some(&lock),
            artifact_map.as_ref().map(|map| (map, artifact_root)),
            &VegaRuntimePluginTarget {
                build_target: arguments.target,
                architecture: arguments.vega_plugin_architecture.clone(),
                engine_version: arguments.vega_engine_version.clone(),
            },
        )
        .plan;
        let mut project_diagnostics = validate_project(&project);
        project_diagnostics.append(&mut resolution.diagnostics);
        resolution.diagnostics = project_diagnostics;
        return Ok(PluginResolution::Vega(resolution));
    }
    if arguments.vega_plugin_artifact_map.is_some() {
        bail!("--vega-plugin-artifact-map requires --vega-plugin-lock or a nearby vega.plugins.lock.json");
    }
    let lock_path = arguments.plugin_lock.clone().or_else(|| {
        let conventional = project_dir.join("deneb.lock.json");
        conventional.exists().then_some(conventional)
    });
    let lock = lock_path
        .map(|path| -> Result<PluginLock> {
            const MAX_PLUGIN_LOCK_BYTES: u64 = 4 * 1024 * 1024;
            let size = fs::metadata(&path)
                .with_context(|| format!("failed to inspect {path}"))?
                .len();
            if size > MAX_PLUGIN_LOCK_BYTES {
                bail!("{path} exceeds the {MAX_PLUGIN_LOCK_BYTES}-byte plugin-lock limit");
            }
            serde_json::from_slice(&fs::read(&path).with_context(|| format!("failed to read {path}"))?)
                .with_context(|| format!("failed to parse {path}"))
        })
        .transpose()?;
    let mut resolution = resolve_plugins(&project, lock.as_ref(), project_dir, arguments.target);
    let mut project_diagnostics = validate_project(&project);
    project_diagnostics.append(&mut resolution.diagnostics);
    resolution.diagnostics = project_diagnostics;
    Ok(PluginResolution::Deneb(resolution))
}

fn resolve(arguments: PluginResolutionArgs, json: bool) -> Result<()> {
    let resolution = plugin_resolution(&arguments)?;
    if json {
        match &resolution {
            PluginResolution::Deneb(resolution) => println!("{}", serde_json::to_string_pretty(resolution)?),
            PluginResolution::Vega(resolution) => println!("{}", serde_json::to_string_pretty(resolution)?),
        }
    } else {
        if resolution.order().is_empty() {
            println!("{}: no resolved plugins for {}", arguments.project, arguments.target);
        } else {
            println!("{}", resolution.order().join(" -> "));
        }
        print_diagnostics(&resolution);
    }
    fail_on_resolution_errors(&resolution)
}

fn plugins_list(arguments: PluginResolutionArgs, json: bool) -> Result<()> {
    let resolution = plugin_resolution(&arguments)?;
    match &resolution {
        PluginResolution::Deneb(report) if json => {
            println!("{}", serde_json::to_string_pretty(&report.plugins)?);
        }
        PluginResolution::Vega(report) if json => {
            println!("{}", serde_json::to_string_pretty(&report.plugins)?);
        }
        PluginResolution::Deneb(report) if report.plugins.is_empty() => {
            println!("{}: no plugins", arguments.project);
        }
        PluginResolution::Vega(report) if report.plugins.is_empty() => {
            println!("{}: no runtime plugins", arguments.project);
        }
        PluginResolution::Deneb(report) => {
            for plugin in report.plugins.values() {
                println!(
                    "{} {} {}{}",
                    plugin.id,
                    plugin.version,
                    plugin.status,
                    if plugin.direct { "" } else { " (dependency)" }
                );
            }
            print_diagnostics(&resolution);
        }
        PluginResolution::Vega(report) => {
            for plugin in report.plugins.values() {
                println!(
                    "{} {} {}{}",
                    plugin.id,
                    plugin.version,
                    plugin.status,
                    if plugin.direct { "" } else { " (dependency)" }
                );
            }
            print_diagnostics(&resolution);
        }
    }
    fail_on_resolution_errors(&resolution)
}

fn plugins_validate(arguments: PluginResolutionArgs, json: bool) -> Result<()> {
    let resolution = plugin_resolution(&arguments)?;
    if json {
        println!("{}", serde_json::to_string_pretty(resolution.diagnostics())?);
    } else if resolution.diagnostics().is_empty() {
        println!("{}: valid plugin graph for {}", arguments.project, arguments.target);
    } else {
        print_diagnostics(&resolution);
    }
    fail_on_resolution_errors(&resolution)
}

fn print_diagnostics(resolution: &PluginResolution) {
    for item in resolution.diagnostics() {
        eprintln!("{:?} {} {}: {}", item.severity, item.code, item.path, item.message);
    }
}

fn fail_on_resolution_errors(resolution: &PluginResolution) -> Result<()> {
    if resolution.has_errors() {
        bail!("plugin resolution failed");
    }
    Ok(())
}

fn read_project(path: &Utf8PathBuf) -> Result<VegaProject> {
    let size = fs::metadata(path)
        .with_context(|| format!("failed to inspect {path}"))?
        .len();
    if size > MAX_PROJECT_BYTES as u64 {
        bail!("{path} exceeds the {MAX_PROJECT_BYTES}-byte project limit");
    }
    let bytes = fs::read(path).with_context(|| format!("failed to read {path}"))?;
    parse_project(&bytes).with_context(|| format!("failed to parse {path}"))
}

fn doctor(json: bool, selected_target: Option<BuildTarget>) -> Result<()> {
    let tools = serde_json::json!({
        "cargo": command_probe("cargo", &["--version"]),
        "rustc": command_probe("rustc", &["--version"]),
        "rustup": command_probe("rustup", &["--version"]),
        "node": command_probe("node", &["--version"]),
        "npm": command_probe("npm", &["--version"]),
        "androidDebugBridge": command_probe("adb", &["--version"]),
        "androidSdk": android_sdk_probe(),
        "java": command_probe("java", &["-version"]),
        "xcodebuild": command_probe("xcodebuild", &["-version"]),
        "xcrun": command_probe("xcrun", &["--version"]),
        "rustTargets": installed_rust_targets(),
    });
    let targets = BuildTarget::ALL
        .into_iter()
        .filter(|target| selected_target.is_none_or(|selected| selected == *target))
        .map(|target| (target.to_string(), target_readiness(target, &tools)))
        .collect::<serde_json::Map<_, _>>();
    let ready = targets
        .values()
        .all(|target| target["ready"].as_bool().unwrap_or(false));
    let report = serde_json::json!({
        "deneb": env!("CARGO_PKG_VERSION"),
        "runtimeAbi": 1,
        "pluginAbi": 1,
        "tools": tools,
        "targets": targets,
        "status": if ready { "ready" } else { "missing-requirements" }
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Deneb {}", env!("CARGO_PKG_VERSION"));
        for (target, value) in &targets {
            let status = if value["ready"].as_bool() == Some(true) {
                "ready"
            } else {
                "missing requirements"
            };
            println!("{target}: {status}");
            if let Some(missing) = value["missing"].as_array()
                && !missing.is_empty()
            {
                println!(
                    "  missing: {}",
                    missing
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }
    Ok(())
}

fn command_probe(command: &str, arguments: &[&str]) -> serde_json::Value {
    match ProcessCommand::new(command).args(arguments).output() {
        Ok(output) => {
            let text = if output.stdout.is_empty() {
                String::from_utf8_lossy(&output.stderr)
            } else {
                String::from_utf8_lossy(&output.stdout)
            };
            serde_json::json!({
                "available": output.status.success(),
                "version": text.lines().next().unwrap_or_default().trim(),
            })
        }
        Err(error) => serde_json::json!({
            "available": false,
            "error": error.to_string(),
        }),
    }
}

fn installed_rust_targets() -> serde_json::Value {
    match ProcessCommand::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
    {
        Ok(output) if output.status.success() => serde_json::Value::Array(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| serde_json::Value::String(line.trim().into()))
                .collect(),
        ),
        _ => serde_json::Value::Array(Vec::new()),
    }
}

fn android_sdk_probe() -> serde_json::Value {
    let mut candidates = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if cfg!(target_os = "macos") {
            candidates.push(home.join("Library/Android/sdk"));
        } else {
            candidates.push(home.join("Android/Sdk"));
        }
    }
    if let Some(local_data) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local_data).join("Android/Sdk"));
    }
    candidates.dedup();

    let Some(path) = candidates.into_iter().find(|path| path.is_dir()) else {
        return serde_json::json!({ "available": false, "ndkAvailable": false });
    };
    let ndk_available = fs::read_dir(path.join("ndk"))
        .ok()
        .is_some_and(|entries| entries.filter_map(Result::ok).any(|entry| entry.path().is_dir()));
    serde_json::json!({
        "available": true,
        "ndkAvailable": ndk_available,
        "path": path.to_string_lossy(),
    })
}

fn target_readiness(target: BuildTarget, tools: &serde_json::Value) -> serde_json::Value {
    let mut missing = Vec::new();
    let available = |name: &str| tools[name]["available"].as_bool() == Some(true);
    let rust_target = |name: &str| {
        tools["rustTargets"]
            .as_array()
            .is_some_and(|targets| targets.iter().any(|target| target.as_str() == Some(name)))
    };
    match target {
        BuildTarget::Web | BuildTarget::Pwa => {}
        BuildTarget::Electron => {
            if !available("node") {
                missing.push("node");
            }
            if !available("npm") {
                missing.push("npm");
            }
        }
        BuildTarget::Tauri => {
            for requirement in ["cargo", "rustc", "node", "npm"] {
                if !available(requirement) {
                    missing.push(requirement);
                }
            }
        }
        BuildTarget::Android => {
            for requirement in ["cargo", "rustc", "rustup", "node", "npm", "java"] {
                if !available(requirement) {
                    missing.push(requirement);
                }
            }
            if !available("androidSdk") {
                missing.push("Android SDK");
            } else if tools["androidSdk"]["ndkAvailable"].as_bool() != Some(true) {
                missing.push("Android NDK");
            }
            if !rust_target("aarch64-linux-android") {
                missing.push("Rust target aarch64-linux-android");
            }
        }
        BuildTarget::Ios => {
            if !cfg!(target_os = "macos") {
                missing.push("macOS");
            }
            for requirement in ["cargo", "rustc", "rustup", "node", "npm", "xcodebuild", "xcrun"] {
                if !available(requirement) {
                    missing.push(requirement);
                }
            }
            if !rust_target("aarch64-apple-ios") {
                missing.push("Rust target aarch64-apple-ios");
            }
        }
    }
    serde_json::json!({
        "ready": missing.is_empty(),
        "missing": missing,
        "host": std::env::consts::OS,
        "produces": if matches!(target, BuildTarget::Web | BuildTarget::Pwa) {
            "static-bundle"
        } else {
            "buildable-source-project"
        }
    })
}

fn validate(path: &Utf8PathBuf, json: bool) -> Result<()> {
    let diagnostics = validate_project(&read_project(path)?);
    if json {
        println!("{}", serde_json::to_string_pretty(&diagnostics)?);
    } else if diagnostics.is_empty() {
        println!("{path}: valid Vega project");
    } else {
        for item in &diagnostics {
            println!("{:?} {} {}: {}", item.severity, item.code, item.path, item.message);
        }
    }
    if diagnostics.iter().any(|item| item.severity == Severity::Error) {
        bail!("validation failed");
    }
    Ok(())
}

fn inspect(path: &Utf8PathBuf, json: bool) -> Result<()> {
    let project = read_project(path)?;
    let command_count = project.scenes.values().map(|scene| scene.commands.len()).sum::<usize>();
    let report = serde_json::json!({
        "id": project.id,
        "formatVersion": project.format_version,
        "entryScene": project.entry_scene,
        "scenes": project.scenes.len(),
        "commands": command_count,
        "assets": project.assets.len(),
        "plugins": project.plugins.len(),
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} — {} scenes, {} commands, {} assets",
            report["id"], report["scenes"], report["commands"], report["assets"]
        );
    }
    Ok(())
}

fn build(options: BuildOptions, json: bool) -> Result<()> {
    let report = build_project(&options)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("built {} ({})", report.output, report.build_id);
        for item in &report.diagnostics {
            eprintln!("{:?} {} {}: {}", item.severity, item.code, item.path, item.message);
        }
        if let Some(archive) = report.archive {
            println!("archive {archive}");
        }
    }
    Ok(())
}
