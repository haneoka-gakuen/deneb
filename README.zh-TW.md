# Deneb

[English](README.md) · [簡體中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

![Deneb](docs/brand/header.svg)

**DENEB** — **D**istribution **E**ngine for **N**ative & **E**xecutable **B**uilds

Deneb 把寫完的視覺小說變成可以安裝的作品。它讀取 [Vega](https://github.com/haneoka-gakuen/vega)
專案，確認需要的每個場景、素材和外掛都真實存在，鎖定精確版本以便復現構建，然後
輸出可以直接發布的網站、離線 PWA、桌面應用（Electron 或 Tauri）或行動裝置工程
（Android 或 iOS）——同一個專案，同一個播放器。

在 [Altair](https://github.com/haneoka-gakuen/altair) 裡編寫和本地化故事，用 Vega
播放，用 Deneb 發布。

## 安裝

Deneb 是一個 Rust 命令列工具。

```sh
git clone https://github.com/haneoka-gakuen/deneb.git
cd deneb
cargo install --locked --path crates/deneb-cli
```

安裝後即可使用 `deneb` 命令。工具鏈版本固定在 `rust-toolchain.toml` 中。

## 快速開始

```sh
deneb doctor                                  # 這臺機器能構建嗎？
deneb validate examples/minimal/project.vega.json
deneb build examples/minimal/project.vega.json --output dist --target pwa --runtime-bundle ./vega-runtime
```

`examples/` 中有最小專案、多場景專案，以及演示素材契約和執行時契約的專案。

## 命令

| 命令 | 作用 |
| --- | --- |
| `deneb doctor [--target T]` | 檢查目標平臺需要的工具鏈（Node、Android SDK、Xcode、Rust 等）。 |
| `deneb validate <project>` | 校驗專案檔案、場景和素材。 |
| `deneb inspect <project>` | 列出專案包含什麼、需要什麼。 |
| `deneb resolve <project>` | 解析外掛依賴圖，不執行任何外掛。 |
| `deneb plugins list <project>` | 列出根外掛、依賴及各自的解析結果。 |
| `deneb plugins validate <project>` | 外掛依賴圖不完整或不一致時失敗。 |
| `deneb build <project> --output <dir>` | 構建一個目標。 |

任何命令加上 `--json` 都會輸出機器可讀的結果，錯誤帶有穩定的程式碼。

### 構建選項

| 選項 | 含義 | 預設值 |
| --- | --- | --- |
| `--target` | `web`、`pwa`、`electron`、`tauri`、`android` 或 `ios` | `web` |
| `--mode` | `release`（離線完整、執行時已鎖定）或 `preview` | `release` |
| `--runtime-bundle DIR` | 要打包的 Vega 執行時（含 `deneb-runtime.json` 的目錄） | |
| `--plugin-lock FILE` | Deneb 外掛鎖檔案 | 專案旁的 `deneb.lock.json` |
| `--vega-plugin-lock FILE` | Vega 執行時外掛鎖檔案 | `vega.plugins.lock.json` |
| `--vega-plugin-artifact-map FILE` | 經過稽核的外掛構建產物位置 | `vega.plugins.cache.json` |
| `--vega-plugin-architecture ARCH` | 原生外掛產物的架構 | |
| `--allow-private-external-runtimes` | 打包專案私下提供、已獲授權的第三方執行時（例如 Live2D SDK） | 關 |
| `--bundle-identifier ID` | 桌面和行動裝置的應用標識 | `app.vega.<project-id>` |
| `--color-mode-storage-key KEY` | 儲存明暗模式偏好的儲存鍵 | |
| `--force` | 替換已存在的輸出 | 關 |
| `--archive` | 同時生成可分發的壓縮包 | 開 |

## 目標平臺

| 目標 | 輸出 |
| --- | --- |
| `web` | 使用相對連結的靜態網站，可部署到任意主機或子路徑。 |
| `pwa` | 在 Web 構建基礎上加入清單和 Service Worker，離線也能遊玩。 |
| `electron` | 啟用上下文隔離和 Chromium 沙箱的 Electron 應用。 |
| `tauri` | Tauri 應用——桌面安裝包小得多。 |
| `android`、`ios` | 承載同一份 Web 內容的原生工程，可直接用 Android Studio 或 Xcode 開啟。 |

所有目標都包含同一份專案、素材和播放器，因此故事在任何平臺上表現一致。詳見
[docs/targets.md](docs/targets.md)。

## 可復現構建

發布構建要求專案離線完整、執行時已鎖定，否則拒絕執行。鎖檔案記錄外掛的精確版本
和摘要，同樣的專案和鎖檔案總能得到同樣的構建。

## 外掛

Deneb 外掛是帶型別介面的 WebAssembly 元件（[`wit/deneb-plugin.wit`](wit/deneb-plugin.wit)）。
它們在沙箱中執行，只能使用專案授予的能力；Deneb 解析和校驗外掛依賴圖時不會執行
外掛。詳見 [docs/plugin-system.md](docs/plugin-system.md)。

## 延伸閱讀

| 文件 | 主題 |
| --- | --- |
| [docs/format.md](docs/format.md) | 專案檔案格式 |
| [docs/targets.md](docs/targets.md) | 各目標平臺的輸出 |
| [docs/runtime-contract.md](docs/runtime-contract.md) | Vega 執行時如何打包 |
| [docs/plugin-system.md](docs/plugin-system.md) | 編寫和解析外掛 |
| [docs/security.md](docs/security.md) | 沙箱與信任 |

## 許可證

MPL-2.0。第三方工具鏈和執行時保留各自的許可證，見
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
