# Deneb

[English](README.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

![Deneb](docs/brand/header.svg)

**DENEB** — **D**istribution **E**ngine for **N**ative & **E**xecutable **B**uilds

Deneb 是視覺小說的建置與交付工具。它讀取 Vega 專案，檢查資源與外掛，並產生適合
發布的 Web、PWA、桌面與行動端工程。


## Deneb 能做什麼

- 檢查專案、資源與外掛是否完整。
- 產生 Web 與可離線執行的 PWA。
- 產生 Electron、Tauri、Android 與 iOS 工程。
- 固定建置輸入，讓同一專案能夠穩定重現。
- 在授權允許時，把專案提供的外部執行環境加入私有建置。

## 開始使用

```sh
cargo run --locked -p haneoka-deneb -- validate path/to/project.vega.json
cargo run --locked -p haneoka-deneb -- build path/to/project.vega.json \
  --output dist \
  --target pwa \
  --runtime-bundle ./runtime
```

使用 [Altair](https://github.com/haneoka-gakuen/altair) 創作與本地化專案，
使用 [Vega](https://github.com/haneoka-gakuen/vega) 播放專案。

Deneb 以 MPL-2.0 發布。
