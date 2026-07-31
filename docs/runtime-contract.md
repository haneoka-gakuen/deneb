# Web runtime contract

Deneb packages a **directory**, not one guessed JavaScript filename. This lets
Vega ship ESM chunks, workers, WASM, fonts, and styles without Deneb knowing the
bundler.

## Input bundle

Pass `--runtime-bundle <dir>`. The directory must contain
`deneb-runtime.json`:

```json
{
  "format": "vega-runtime",
  "formatVersion": 1,
  "abiVersion": 1,
  "entrypoint": "runtime.js",
  "engineVersion": "1.4.0",
  "capabilities": ["vega.project.v1", "vega.lifecycle.v1"],
  "integrity": {
    "runtime.js": "sha256-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  }
}
```

`engineVersion` is optional for backward compatibility, but is required to
verify a runtime-plugin lock that pins an engine version. It must be concrete
SemVer. `integrity` is optional but every supplied entry is verified. The entrypoint
must be a relative `.js`/`.mjs` file. Symlinks, traversal, and non-files are
rejected. Browser imports must be self-contained or relative to files in this
directory; bare package specifiers are not installed by Deneb.

Capabilities are auditable compatibility claims. In addition to the ABI
lifecycle, Deneb recognizes:

- `vega-project-multiscene-v1`: the runtime compiles the complete scene graph
  and begins at `entryScene`; release builds with multiple scenes require it;
- `vega-project-assets-v1`: the runtime carries the project asset catalogue and
  resolves command aliases against rewritten sources;
- any strings requested by `project.engine.features`.

Missing optional surfaces produce `deneb.runtime.capability-loss.*` warnings.
Missing multi-scene support fails a release build when that surface is used.
Packaging bytes is not treated as proof that a runtime consumes them.

Deneb Component plugins are build-time tools. Deneb executes them before the
runtime contract is written; they are never handed to the Vega browser runtime.
Canonical Vega runtime plugins are a separate payload. When present, the
contract contains `runtimePlugins` and the bootloader supplies its resolved URL
as `runtimePluginIndexUrl`.

Lock-declared external runtimes use another payload. When host requirements or
authorized application-private attachments are present, the contract contains
`externalRuntimes`; the bootloader supplies its resolved URL as
`externalRuntimeIndexUrl` to both factory and mount. A runtime decides how to
connect those already verified files to its code-only adapter.

## JavaScript ABI 1

The entrypoint exports `createDenebRuntime`:

```js
export async function createDenebRuntime({
  abiVersion,
  capabilities,
  runtimePluginIndexUrl,
  externalRuntimeIndexUrl,
  signal,
}) {
  return {
    abiVersion: 1,
    async mount({
      root,
      projectUrl,
      bundleUrl,
      manifestUrl,
      runtimePluginIndexUrl,
      externalRuntimeIndexUrl,
      signal,
    }) {
      // Fetch/parse the project, create one Vega player, and mount under root.
      return {
        playback, // Promise observed by Deneb without blocking interactive play
        async dispose() {
          // Release the player, listeners, audio, GPU objects, and workers.
        },
      };
    },
    async dispose() {
      // Release engine-wide plugin hosts and immutable caches.
    },
  };
}
```

The bootloader rejects an ABI mismatch or missing lifecycle method before
mounting. It observes an optional `playback` promise and turns asynchronous
playback failures into a visible fatal state. On `pagehide` it aborts
outstanding work, disposes the mounted player, then disposes the runtime. A
runtime should also make its disposal idempotent.

The URLs are `URL` objects rooted at the emitted web payload:

- `projectUrl`: inspectable `project.vega.json`;
- `bundleUrl`: binary `project.vgb`;
- `manifestUrl`: integrity and target metadata.
- `runtimePluginIndexUrl`: nullable URL for the verified runtime-plugin index.
- `externalRuntimeIndexUrl`: nullable URL for host requirements and verified
  application-private runtime attachments.

The runtime index contains locked identities, sources, declared and verified
integrity, dependency order, capabilities, approved permissions,
configuration, and deterministic artifact URLs. Vega, not Deneb, owns
loading and disposing these runtime modules.

The root is Deneb's full-viewport `<main id="vega">`. Runtime UI should replace
its children but must not assume Vue, React, or another host framework. That
keeps the same bundle usable in plain Web, a framework route, Electron, and
Tauri.

## Resources and legal closure

Local entries in `project.assets` are content-addressed. Exact occurrences of
their original paths in native embedded resource fields such as `url`, `src`,
`texture`, `model`, and nested variants are rewritten to the emitted path.
Ordinary text fields are never rewritten. Asset aliases still require
`vega-project-assets-v1`; Deneb does not guess an engine-specific
alias-to-command projection.

Data URIs are decoded into the same content-addressed store. Release mode
rejects HTTP(S) asset sources so its output is offline-closed; preview mode can
retain them. Local asset files are capped at 512 MiB each, 20,000 files, and
4 GiB total.

The complete runtime directory, including license and notice files, is copied
under `runtime/bundle/` and included in `.vgpack`. A runtime with no
LICENSE/COPYING/NOTICE file produces `deneb.runtime.legal-notice-missing`.
Deneb's own MPL license and dependency notice are always emitted under
`licenses/deneb/`.

## Unbound builds

Without `--runtime-bundle`, Deneb emits `index.html`, `runtime/boot.js`,
`runtime/shell.css`, and an `unbound` contract. The bootloader reads that real
contract and renders a diagnostic. It never imports a missing fallback module.
This mode is suitable for inspecting artifacts, not playing the novel.

`examples/runtime-contract` is an executable ABI fixture used by CI. It renders
project metadata to prove the complete boot path, but it is intentionally not a
substitute for Vega.
