# Deneb

[English](README.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

![Deneb](docs/brand/header.svg)

**DENEB** — **D**istribution **E**ngine for **N**ative & **E**xecutable **B**uilds

Deneb 把写完的视觉小说变成可以安装的作品。它读取 [Vega](https://github.com/haneoka-gakuen/vega)
项目，确认需要的每个场景、素材和插件都真实存在，锁定精确版本以便复现构建，然后
输出可以直接发布的网站、离线 PWA、桌面应用（Electron 或 Tauri）或移动端工程
（Android 或 iOS）——同一个项目，同一个播放器。

在 [Altair](https://github.com/haneoka-gakuen/altair) 里编写和本地化故事，用 Vega
播放，用 Deneb 发布。

## 安装

Deneb 是一个 Rust 命令行工具。

```sh
git clone https://github.com/haneoka-gakuen/deneb.git
cd deneb
cargo install --locked --path crates/deneb-cli
```

安装后即可使用 `deneb` 命令。工具链版本固定在 `rust-toolchain.toml` 中。

## 快速开始

```sh
deneb doctor                                  # 这台机器能构建吗？
deneb validate examples/minimal/project.vega.json
deneb build examples/minimal/project.vega.json --output dist --target pwa --runtime-bundle ./vega-runtime
```

`examples/` 中有最小项目、多场景项目，以及演示素材契约和运行时契约的项目。

## 命令

| 命令 | 作用 |
| --- | --- |
| `deneb doctor [--target T]` | 检查目标平台需要的工具链（Node、Android SDK、Xcode、Rust 等）。 |
| `deneb validate <project>` | 校验项目文件、场景和素材。 |
| `deneb inspect <project>` | 列出项目包含什么、需要什么。 |
| `deneb resolve <project>` | 解析插件依赖图，不运行任何插件。 |
| `deneb plugins list <project>` | 列出根插件、依赖及各自的解析结果。 |
| `deneb plugins validate <project>` | 插件依赖图不完整或不一致时失败。 |
| `deneb build <project> --output <dir>` | 构建一个目标。 |

任何命令加上 `--json` 都会输出机器可读的结果，错误带有稳定的代码。

### 构建选项

| 选项 | 含义 | 默认值 |
| --- | --- | --- |
| `--target` | `web`、`pwa`、`electron`、`tauri`、`android` 或 `ios` | `web` |
| `--mode` | `release`（离线完整、运行时已锁定）或 `preview` | `release` |
| `--runtime-bundle DIR` | 要打包的 Vega 运行时（含 `deneb-runtime.json` 的目录） | |
| `--plugin-lock FILE` | Deneb 插件锁文件 | 项目旁的 `deneb.lock.json` |
| `--vega-plugin-lock FILE` | Vega 运行时插件锁文件 | `vega.plugins.lock.json` |
| `--vega-plugin-artifact-map FILE` | 经过审核的插件构建产物位置 | `vega.plugins.cache.json` |
| `--vega-plugin-architecture ARCH` | 原生插件产物的架构 | |
| `--allow-private-external-runtimes` | 打包项目私下提供、已获授权的第三方运行时（例如 Live2D SDK） | 关 |
| `--bundle-identifier ID` | 桌面和移动端的应用标识 | `app.vega.<project-id>` |
| `--color-mode-storage-key KEY` | 保存明暗模式偏好的存储键 | |
| `--force` | 替换已存在的输出 | 关 |
| `--archive` | 同时生成可分发的压缩包 | 开 |

## 目标平台

| 目标 | 输出 |
| --- | --- |
| `web` | 使用相对链接的静态网站，可部署到任意主机或子路径。 |
| `pwa` | 在 Web 构建基础上加入清单和 Service Worker，离线也能游玩。 |
| `electron` | 启用上下文隔离和 Chromium 沙箱的 Electron 应用。 |
| `tauri` | Tauri 应用——桌面安装包小得多。 |
| `android`、`ios` | 承载同一份 Web 内容的原生工程，可直接用 Android Studio 或 Xcode 打开。 |

所有目标都包含同一份项目、素材和播放器，因此故事在任何平台上表现一致。详见
[docs/targets.md](docs/targets.md)。

## 可复现构建

发布构建要求项目离线完整、运行时已锁定，否则拒绝执行。锁文件记录插件的精确版本
和摘要，同样的项目和锁文件总能得到同样的构建。

## 插件

Deneb 插件是带类型接口的 WebAssembly 组件（[`wit/deneb-plugin.wit`](wit/deneb-plugin.wit)）。
它们在沙箱中运行，只能使用项目授予的能力；Deneb 解析和校验插件依赖图时不会执行
插件。详见 [docs/plugin-system.md](docs/plugin-system.md)。

## 延伸阅读

| 文档 | 主题 |
| --- | --- |
| [docs/format.md](docs/format.md) | 项目文件格式 |
| [docs/targets.md](docs/targets.md) | 各目标平台的输出 |
| [docs/runtime-contract.md](docs/runtime-contract.md) | Vega 运行时如何打包 |
| [docs/plugin-system.md](docs/plugin-system.md) | 编写和解析插件 |
| [docs/security.md](docs/security.md) | 沙箱与信任 |

## 许可证

MPL-2.0。第三方工具链和运行时保留各自的许可证，见
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
