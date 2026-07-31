# Deneb

[English](README.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

![Deneb](docs/brand/header.svg)

**DENEB** — **D**istribution **E**ngine for **N**ative & **E**xecutable **B**uilds

Deneb 是视觉小说的构建与交付工具。它读取 Vega 项目，检查资源和插件，并生成适合
发布的 Web、PWA、桌面与移动端工程。

## Deneb 能做什么

- 检查项目、资源与插件是否完整。
- 生成 Web 和可离线运行的 PWA。
- 生成 Electron、Tauri、Android 与 iOS 工程。
- 固定构建输入，使同一项目能够稳定复现。
- 在许可证允许时，把项目提供的外部运行时加入私有构建。

## 开始使用

```sh
cargo run --locked -p haneoka-deneb -- validate path/to/project.vega.json
cargo run --locked -p haneoka-deneb -- build path/to/project.vega.json \
  --output dist \
  --target pwa \
  --runtime-bundle ./runtime
```

使用 [Altair](https://github.com/haneoka-gakuen/altair) 创作与本地化项目，
使用 [Vega](https://github.com/haneoka-gakuen/vega) 播放项目。

## 许可证

Deneb 以 MPL-2.0 发布。第三方工具链和运行时适用各自许可证。
