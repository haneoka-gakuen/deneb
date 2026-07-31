# Target providers

Every target contains the same project, VGB, assets, bootloader, and runtime
contract. Target providers only decide how that web payload is hosted.
`--bundle-identifier` configures the Electron/Tauri application identity and
is recorded for every target. The neutral default is
`app.vega.<project-id>`. `--color-mode-storage-key` optionally overrides the
boot shell's namespaced color-mode preference key.

## Web and PWA

Web writes its payload at the output root. It can be embedded below any
same-origin route because all emitted references are relative.
Its `deneb-target.json` is a complete `static-bundle` with
`compileVerified: true` and `scaffoldOnly: false`.

PWA adds `manifest.webmanifest`, `deneb-precache.json`, and `sw.js`. The cache
name derives from the sorted precache bytes. Old Deneb caches are removed on
activation. Requests are network-first so a preview or deployment cannot keep
executing an older mutable project/runtime from a controlling worker; the
versioned precache is the offline fallback. Navigation falls back to
`index.html` only after the network and exact cache miss. The precache includes
the build manifest and every runtime/legal dependency, and the generated SVG
mark is used by both the page favicon and application manifest.

Serve both over HTTP during development and HTTPS in production. `file://`
does not provide the fetch/service-worker semantics used by the runtime.

## Electron

The output layout is:

```text
package.json
web/
platform/electron/main.cjs
deneb-target.json
```

The window uses context isolation, Chromium sandboxing, no Node integration,
no preload bridge, denied popups, and navigation confinement to `web/`.
Dependency versions are exact. The Deneb artifact is a buildable host project,
not a precompiled application. Run `npm install`, commit `package-lock.json`,
then `npm run package` on the target OS. Electron Builder emits the native
package formats supported by that OS. Signing and notarization remain
release-operator responsibilities.

## Tauri desktop

Tauri outputs `web/`, `src-tauri/`, `package.json`, and `HOSTING.md`. Its only
capability is `core:default`; there are no shell, filesystem, updater, or
network plugins. CSP allows the local IPC transport and HTTPS game resources.

Run `npm install`, commit both lockfiles, install the official platform
prerequisites, then `npm run package` on the target OS. The generated Rust
manifest is an independent workspace, includes an application icon, and can be
checked with `cargo check --manifest-path src-tauri/Cargo.toml`. Tauri emits the
platform's native bundle; signing and notarization still require publisher
credentials.

## Android and iOS

Mobile targets share the Tauri source and add explicit `android:*` or `ios:*`
commands. `*:init` is intentionally separate: it calls the installed official
Tauri CLI to create SDK-specific native files on a configured developer
machine. Deneb cannot infer Android keystores, Apple teams, bundle profiles, or
store metadata.

The generated `deneb-target.json` calls native output a
`buildable-source-project`, sets `compileVerified: false`, and keeps
`scaffoldOnly: true`. Automation must not report a native binary until the
target's own build command and signing checks succeed. `deneb doctor --target`
reports missing host tools, SDK environment, and Rust mobile targets without
claiming cross-platform compilation happened.
