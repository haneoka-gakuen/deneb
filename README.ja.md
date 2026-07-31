# Deneb

[English](README.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

![Deneb](docs/brand/header.svg)

**DENEB** — **D**istribution **E**ngine for **N**ative & **E**xecutable **B**uilds

Deneb はビジュアルノベルのビルド・配布ツールです。Vega プロジェクトの素材と
プラグインを確認し、Web、PWA、デスクトップ、モバイル向けの配布工程を生成します。

## できること

- プロジェクト、素材、プラグインの完全性を確認
- Web ビルドとオフライン対応 PWA を生成
- Electron、Tauri、Android、iOS プロジェクトを生成
- ビルド入力を固定して同じ成果物を再現
- 許諾された外部ランタイムを非公開ビルドへ追加

## はじめる

```sh
cargo run --locked -p haneoka-deneb -- validate path/to/project.vega.json
cargo run --locked -p haneoka-deneb -- build path/to/project.vega.json \
  --output dist \
  --target pwa \
  --runtime-bundle ./runtime
```

制作と翻訳には [Altair](https://github.com/haneoka-gakuen/altair)、
再生には [Vega](https://github.com/haneoka-gakuen/vega) を使用します。

Deneb は MPL-2.0 で公開されています。
