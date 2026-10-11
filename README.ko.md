# Deneb

[English](README.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

![Deneb](docs/brand/header.svg)

**DENEB** — **D**istribution **E**ngine for **N**ative & **E**xecutable **B**uilds

Deneb는 완성된 비주얼 노벨을 설치할 수 있는 작품으로 만들어 줍니다.
[Vega](https://github.com/haneoka-gakuen/vega) 프로젝트를 읽어 필요한 장면, 에셋, 플러그인이
모두 실제로 있는지 확인하고, 빌드를 재현할 수 있도록 버전을 고정한 뒤, 바로 배포할 수 있는
웹사이트, 오프라인 PWA, 데스크톱 앱(Electron 또는 Tauri), 모바일 프로젝트(Android 또는 iOS)를
만듭니다. 모두 같은 프로젝트, 같은 플레이어입니다.

이야기는 [Altair](https://github.com/haneoka-gakuen/altair)에서 쓰고 현지화하고, Vega로 재생하고,
Deneb로 배포하세요.

## 설치

Deneb는 Rust 명령줄 도구입니다.

```sh
git clone https://github.com/haneoka-gakuen/deneb.git
cd deneb
cargo install --locked --path crates/deneb-cli
```

설치하면 `deneb` 명령을 쓸 수 있습니다. 툴체인 버전은 `rust-toolchain.toml`에 고정되어 있습니다.

## 시작하기

```sh
deneb doctor                                  # 이 컴퓨터에서 빌드할 수 있는지 확인
deneb validate examples/minimal/project.vega.json
deneb build examples/minimal/project.vega.json --output dist --target pwa --runtime-bundle ./vega-runtime
```

`examples/`에는 최소 프로젝트, 여러 장면으로 된 프로젝트, 에셋 계약과 런타임 계약을 보여 주는
프로젝트가 있습니다.

## 명령

| 명령 | 하는 일 |
| --- | --- |
| `deneb doctor [--target T]` | 대상 플랫폼에 필요한 툴체인(Node, Android SDK, Xcode, Rust 등)을 확인합니다. |
| `deneb validate <project>` | 프로젝트 파일, 장면, 에셋을 검증합니다. |
| `deneb inspect <project>` | 프로젝트에 무엇이 들어 있고 무엇이 필요한지 보여 줍니다. |
| `deneb resolve <project>` | 플러그인을 실행하지 않고 의존성 그래프를 해석합니다. |
| `deneb plugins list <project>` | 루트 플러그인, 의존성, 각각의 해석 결과를 나열합니다. |
| `deneb plugins validate <project>` | 그래프가 불완전하거나 맞지 않으면 실패합니다. |
| `deneb build <project> --output <dir>` | 대상을 빌드합니다. |

모든 명령에 `--json`을 붙이면 기계가 읽을 수 있는 출력과 고정된 오류 코드를 얻습니다.

### 빌드 옵션

| 옵션 | 의미 | 기본값 |
| --- | --- | --- |
| `--target` | `web`, `pwa`, `electron`, `tauri`, `android`, `ios` | `web` |
| `--mode` | `release`(오프라인 완결, 런타임 고정) 또는 `preview` | `release` |
| `--runtime-bundle DIR` | 함께 묶을 Vega 런타임(`deneb-runtime.json`이 있는 디렉터리) | |
| `--plugin-lock FILE` | Deneb 플러그인 잠금 파일 | 프로젝트 옆 `deneb.lock.json` |
| `--vega-plugin-lock FILE` | Vega 런타임 플러그인 잠금 파일 | `vega.plugins.lock.json` |
| `--vega-plugin-artifact-map FILE` | 검토된 플러그인 빌드 산출물 위치 | `vega.plugins.cache.json` |
| `--vega-plugin-architecture ARCH` | 네이티브 플러그인 산출물의 아키텍처 | |
| `--allow-private-external-runtimes` | 프로젝트가 따로 제공한, 라이선스를 받은 서드파티 런타임(예: Live2D SDK) 포함 | 끔 |
| `--bundle-identifier ID` | 데스크톱·모바일 앱 식별자 | `app.vega.<project-id>` |
| `--color-mode-storage-key KEY` | 라이트/다크 설정을 저장할 키 | |
| `--force` | 기존 출력을 교체 | 끔 |
| `--archive` | 배포용 압축 파일도 생성 | 켬 |

## 대상 플랫폼

| 대상 | 출력 |
| --- | --- |
| `web` | 상대 링크를 쓰는 정적 사이트. 어떤 호스트나 하위 경로에도 둘 수 있습니다. |
| `pwa` | 웹 빌드에 매니페스트와 Service Worker를 더해 오프라인에서도 플레이할 수 있습니다. |
| `electron` | 컨텍스트 격리와 Chromium 샌드박스를 켠 Electron 앱. |
| `tauri` | Tauri 앱. 데스크톱 다운로드가 훨씬 작습니다. |
| `android`, `ios` | 같은 웹 콘텐츠를 담은 네이티브 프로젝트. Android Studio나 Xcode에서 바로 열 수 있습니다. |

모든 대상에 같은 프로젝트, 에셋, 플레이어가 들어가므로 이야기는 어디서나 똑같이 동작합니다.
자세한 내용은 [docs/targets.md](docs/targets.md)를 보세요.

## 재현 가능한 빌드

릴리스 빌드는 프로젝트가 오프라인으로 완결되고 런타임이 고정되어 있지 않으면 실행되지 않습니다.
잠금 파일에는 플러그인의 정확한 버전과 다이제스트가 기록되므로, 같은 프로젝트와 잠금 파일에서는
언제나 같은 빌드가 나옵니다.

## 플러그인

Deneb 플러그인은 타입이 정해진 인터페이스([`wit/deneb-plugin.wit`](wit/deneb-plugin.wit))를 가진
WebAssembly 컴포넌트입니다. 프로젝트가 허락한 기능만 쓸 수 있는 샌드박스에서 실행되며, Deneb는
플러그인을 실행하지 않고 의존성 그래프를 해석하고 검증합니다.
[docs/plugin-system.md](docs/plugin-system.md)를 참고하세요.

## 더 읽을거리

| 문서 | 주제 |
| --- | --- |
| [docs/format.md](docs/format.md) | 프로젝트 파일 형식 |
| [docs/targets.md](docs/targets.md) | 대상별 출력 |
| [docs/runtime-contract.md](docs/runtime-contract.md) | Vega 런타임을 묶는 방법 |
| [docs/plugin-system.md](docs/plugin-system.md) | 플러그인 작성과 해석 |
| [docs/security.md](docs/security.md) | 샌드박스와 신뢰 |

## 라이선스

MPL-2.0. 서드파티 툴체인과 런타임은 각자의 라이선스를 따릅니다.
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)를 참고하세요.
