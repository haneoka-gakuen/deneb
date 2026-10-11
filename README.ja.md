# Deneb

[English](README.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

![Deneb](docs/brand/header.svg)

**DENEB** — **D**istribution **E**ngine for **N**ative & **E**xecutable **B**uilds

Deneb は、完成したビジュアルノベルをインストールできる作品にします。
[Vega](https://github.com/haneoka-gakuen/vega) のプロジェクトを読み込み、必要なシーン・素材・
プラグインがすべて揃っているかを確かめ、ビルドを再現できるようにバージョンを固定したうえで、
そのまま公開できる Web サイト、オフライン対応の PWA、デスクトップアプリ（Electron / Tauri）、
モバイル向けプロジェクト（Android / iOS）を出力します。どれも同じプロジェクト、同じプレイヤーです。

物語は [Altair](https://github.com/haneoka-gakuen/altair) で書いてローカライズし、Vega で
再生し、Deneb で届けます。

## インストール

Deneb は Rust 製のコマンドラインツールです。

```sh
git clone https://github.com/haneoka-gakuen/deneb.git
cd deneb
cargo install --locked --path crates/deneb-cli
```

`deneb` コマンドが使えるようになります。ツールチェーンのバージョンは
`rust-toolchain.toml` で固定されています。

## はじめに

```sh
deneb doctor                                  # このマシンでビルドできるか確認
deneb validate examples/minimal/project.vega.json
deneb build examples/minimal/project.vega.json --output dist --target pwa --runtime-bundle ./vega-runtime
```

`examples/` には最小構成のプロジェクト、複数シーンのプロジェクト、素材契約と
ランタイム契約を示すプロジェクトがあります。

## コマンド

| コマンド | 内容 |
| --- | --- |
| `deneb doctor [--target T]` | ターゲットに必要なツールチェーン（Node、Android SDK、Xcode、Rust など）を確認します。 |
| `deneb validate <project>` | プロジェクトファイル、シーン、素材を検証します。 |
| `deneb inspect <project>` | プロジェクトの内容と必要なものを表示します。 |
| `deneb resolve <project>` | プラグインの依存関係を解決します（プラグインは実行しません）。 |
| `deneb plugins list <project>` | ルートプラグイン、依存先、それぞれの解決結果を一覧表示します。 |
| `deneb plugins validate <project>` | 依存関係が不完全・不整合なら失敗します。 |
| `deneb build <project> --output <dir>` | ターゲットをビルドします。 |

どのコマンドも `--json` で機械可読な出力になり、エラーには固定のコードが付きます。

### ビルドオプション

| オプション | 意味 | 既定値 |
| --- | --- | --- |
| `--target` | `web`、`pwa`、`electron`、`tauri`、`android`、`ios` | `web` |
| `--mode` | `release`（オフラインで完結し、ランタイム固定）または `preview` | `release` |
| `--runtime-bundle DIR` | 同梱する Vega ランタイム（`deneb-runtime.json` を含むディレクトリ） | |
| `--plugin-lock FILE` | Deneb のプラグインロック | プロジェクト横の `deneb.lock.json` |
| `--vega-plugin-lock FILE` | Vega ランタイムプラグインのロック | `vega.plugins.lock.json` |
| `--vega-plugin-artifact-map FILE` | 審査済みプラグインのビルド成果物の場所 | `vega.plugins.cache.json` |
| `--vega-plugin-architecture ARCH` | ネイティブプラグイン成果物のアーキテクチャ | |
| `--allow-private-external-runtimes` | プロジェクトが個別に用意した、ライセンス済みのサードパーティランタイム（Live2D SDK など）を同梱 | 無効 |
| `--bundle-identifier ID` | デスクトップ・モバイルのアプリ識別子 | `app.vega.<project-id>` |
| `--color-mode-storage-key KEY` | ライト／ダーク設定を保存するキー | |
| `--force` | 既存の出力を置き換える | 無効 |
| `--archive` | 配布用アーカイブも作成する | 有効 |

## ターゲット

| ターゲット | 出力 |
| --- | --- |
| `web` | 相対リンクの静的サイト。任意のホストやサブパスに置けます。 |
| `pwa` | Web ビルドにマニフェストと Service Worker を加え、オフラインでも遊べます。 |
| `electron` | コンテキスト分離と Chromium サンドボックスを有効にした Electron アプリ。 |
| `tauri` | Tauri アプリ。デスクトップ版のダウンロードがずっと小さくなります。 |
| `android`、`ios` | 同じ Web コンテンツを載せたネイティブプロジェクト。Android Studio や Xcode でそのまま開けます。 |

どのターゲットにも同じプロジェクト・素材・プレイヤーが入るので、物語はどこでも同じように
動きます。詳しくは [docs/targets.md](docs/targets.md) を参照してください。

## 再現可能なビルド

リリースビルドは、プロジェクトがオフラインで完結し、ランタイムが固定されていない限り実行
されません。ロックファイルにはプラグインの正確なバージョンとダイジェストが記録され、同じ
プロジェクトとロックからは常に同じビルドが得られます。

## プラグイン

Deneb のプラグインは型付きインターフェース（[`wit/deneb-plugin.wit`](wit/deneb-plugin.wit)）を
持つ WebAssembly コンポーネントです。プロジェクトが許可した機能だけを使えるサンドボックスで
動き、依存関係の解決と検証はプラグインを実行せずに行われます。詳しくは
[docs/plugin-system.md](docs/plugin-system.md) を参照してください。

## 関連ドキュメント

| ドキュメント | 内容 |
| --- | --- |
| [docs/format.md](docs/format.md) | プロジェクトファイルの形式 |
| [docs/targets.md](docs/targets.md) | 各ターゲットの出力 |
| [docs/runtime-contract.md](docs/runtime-contract.md) | Vega ランタイムのパッケージ方法 |
| [docs/plugin-system.md](docs/plugin-system.md) | プラグインの作成と解決 |
| [docs/security.md](docs/security.md) | サンドボックスと信頼 |

## ライセンス

MPL-2.0。サードパーティのツールチェーンとランタイムはそれぞれのライセンスに従います。
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) を参照してください。
