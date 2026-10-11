# Deneb

[English](README.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

![Deneb](docs/brand/header.svg)

**DENEB** — **D**istribution **E**ngine for **N**ative & **E**xecutable **B**uilds

Deneb turns a finished visual novel into something people can install. It
reads a [Vega](https://github.com/haneoka-gakuen/vega) project, checks that
every scene, asset and plugin it needs is really there, pins exact versions so
the build can be reproduced, and writes a ready-to-ship web site, offline
PWA, desktop app (Electron or Tauri) or mobile project (Android or iOS) — all
from the same project, with the same player inside.

Write and localize the story in [Altair](https://github.com/haneoka-gakuen/altair),
play it with Vega, ship it with Deneb.

## Install

Deneb is a Rust command-line tool.

```sh
git clone https://github.com/haneoka-gakuen/deneb.git
cd deneb
cargo install --locked --path crates/deneb-cli
```

This installs the `deneb` command. The toolchain version is pinned in
`rust-toolchain.toml`.

## Quick start

```sh
deneb doctor                                  # is this machine ready to build?
deneb validate examples/minimal/project.vega.json
deneb build examples/minimal/project.vega.json --output dist --target pwa --runtime-bundle ./vega-runtime
```

The `examples/` folder has a minimal project, a multi-scene project, and
projects that show the asset and runtime contracts.

## Commands

| Command | What it does |
| --- | --- |
| `deneb doctor [--target T]` | Checks the toolchains a target needs (Node, Android SDK, Xcode, Rust…). |
| `deneb validate <project>` | Validates the project file, scenes and assets. |
| `deneb inspect <project>` | Prints what the project contains and what it needs. |
| `deneb resolve <project>` | Resolves the plugin graph without running any plugin. |
| `deneb plugins list <project>` | Lists root plugins, their dependencies and how each was resolved. |
| `deneb plugins validate <project>` | Fails if the plugin graph is incomplete or inconsistent. |
| `deneb build <project> --output <dir>` | Builds a target. |

Add `--json` to any command for machine-readable output and errors with
stable codes.

### Build options

| Option | Meaning | Default |
| --- | --- | --- |
| `--target` | `web`, `pwa`, `electron`, `tauri`, `android` or `ios` | `web` |
| `--mode` | `release` (offline-complete, pinned runtime) or `preview` | `release` |
| `--runtime-bundle DIR` | The Vega runtime to package (a directory with `deneb-runtime.json`) | |
| `--plugin-lock FILE` | Deneb plugin lock | `deneb.lock.json` next to the project |
| `--vega-plugin-lock FILE` | Vega runtime-plugin lock | `vega.plugins.lock.json` |
| `--vega-plugin-artifact-map FILE` | Where audited plugin builds are | `vega.plugins.cache.json` |
| `--vega-plugin-architecture ARCH` | Architecture for native plugin artifacts | |
| `--allow-private-external-runtimes` | Include licensed third-party runtimes (for example a Live2D SDK) that the project supplies privately | off |
| `--bundle-identifier ID` | Application id for desktop and mobile | `app.vega.<project-id>` |
| `--color-mode-storage-key KEY` | Storage key for the light/dark preference | |
| `--force` | Replace an existing output | off |
| `--archive` | Also write a distributable archive | on |

## Targets

| Target | Output |
| --- | --- |
| `web` | A static site with relative links, ready for any host or sub-path. |
| `pwa` | The web build plus a manifest and a service worker that keeps the game playable offline. |
| `electron` | An Electron app with context isolation and Chromium sandboxing. |
| `tauri` | A Tauri app — a much smaller desktop download. |
| `android`, `ios` | Native projects that host the same web payload, ready to open in Android Studio or Xcode. |

Every target carries the same project, assets and player, so a story behaves
identically everywhere. Details are in [docs/targets.md](docs/targets.md).

## Reproducible builds

A release build refuses to run unless the project is complete offline and the
runtime is pinned. Locks record exact plugin versions and digests, so the same
project and locks always produce the same build.

## Plugins

Deneb plugins are WebAssembly components with a typed interface
([`wit/deneb-plugin.wit`](wit/deneb-plugin.wit)). They run in a sandbox with
only the capabilities the project grants, and Deneb resolves and validates the
plugin graph without executing it. See
[docs/plugin-system.md](docs/plugin-system.md).

## Further reading

| Document | Topic |
| --- | --- |
| [docs/format.md](docs/format.md) | The project file format |
| [docs/targets.md](docs/targets.md) | What each target produces |
| [docs/runtime-contract.md](docs/runtime-contract.md) | How a Vega runtime is packaged |
| [docs/plugin-system.md](docs/plugin-system.md) | Writing and resolving plugins |
| [docs/security.md](docs/security.md) | Sandboxing and trust |

## License

MPL-2.0. Third-party toolchains and runtimes keep their own licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
