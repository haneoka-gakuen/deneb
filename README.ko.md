# Deneb

[English](README.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

![Deneb](docs/brand/header.svg)

**DENEB** — **D**istribution **E**ngine for **N**ative & **E**xecutable **B**uilds

Deneb은 비주얼 노벨 빌드 및 배포 도구입니다. Vega 프로젝트의 리소스와 플러그인을
확인하고 웹, PWA, 데스크톱과 모바일용 배포 프로젝트를 만듭니다.

## 할 수 있는 일

- 프로젝트, 리소스와 플러그인의 완전성 확인
- 웹 빌드와 오프라인 PWA 생성
- Electron, Tauri, Android 및 iOS 프로젝트 생성
- 같은 결과를 재현할 수 있도록 빌드 입력 고정
- 라이선스가 허용하는 외부 런타임을 비공개 빌드에 포함

## 시작하기

```sh
cargo run --locked -p haneoka-deneb -- validate path/to/project.vega.json
cargo run --locked -p haneoka-deneb -- build path/to/project.vega.json \
  --output dist \
  --target pwa \
  --runtime-bundle ./runtime
```

제작과 현지화에는 [Altair](https://github.com/haneoka-gakuen/altair),
재생에는 [Vega](https://github.com/haneoka-gakuen/vega)를 사용합니다.

Deneb은 MPL-2.0으로 배포됩니다.
