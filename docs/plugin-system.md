# Plugin system

Deneb has two independent plugin scopes. They are never parsed as each other:

- **Deneb build plugins** use `deneb-plugin-lock@1`, run as sandboxed WASM
  Components during the build, and are emitted under `plugins/`.
- **Vega runtime plugins** use canonical `vega-plugin-lock@1`, are copied from
  a host/audited artifact map without being downloaded or executed, and are
  emitted under `runtime-plugins/`.

The existing Deneb build-plugin ABI and lock remain unchanged. A nearby
`vega.plugins.lock.json` enables the runtime-plugin path; without it, existing
`deneb.lock.json` behavior is preserved. When both locks exist, membership
partitions roots between the scopes; a shared ID is intentionally evaluated in
both.

## Deneb build plugins

Deneb separates build-plugin identity, resolution, authority, and execution:

1. a project enables a root by requesting an ID and exact version;
2. `deneb-plugin-lock@1` pins source, checksum, ABI, capabilities, dependencies,
   and command numbers;
3. the resolver closes transitive dependencies and orders them
   lexically/topologically;
4. `deneb-plugin.wit` defines the Component Model boundary;
5. a host grants only capabilities available to the normal build;
6. one build-scoped session is disposed even after a hook fails.

## Lock file

```json
{
  "format": "deneb-plugin-lock",
  "version": 1,
  "pluginAbiVersion": 1,
  "plugins": {
    "dev.example.weather": {
      "version": "1.2.3",
      "source": {
        "kind": "registry",
        "url": "https://plugins.example"
      },
      "checksum": "sha256-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      "abiVersion": 1,
      "capabilities": ["read-project", "write-generated"],
      "dependencies": {}
    }
  },
  "commands": {
    "weather/rain": 10000
  }
}
```

Registry sources require HTTPS, Git sources require an immutable hexadecimal
revision, and path sources must remain below the project. A requested plugin
missing from the lock or with a different version fails the build when
required. Optional failures are warnings and the affected plugin subgraph is
omitted. Dependency requirements accept SemVer ranges; legacy opaque versions
remain compatible when requirement and version are identical.

Lock v1 also accepts optional `manifestChecksum`, `engineRange`, `targets`, and
`conflicts` fields. New path locks should populate them so the complete
manifest policy is pinned; their absence preserves compatibility with existing
version-1 locks. `manifestChecksum` is the SHA-256 of the exact
`deneb.plugin.json` bytes, separate from `checksum`, which pins the Component.

For an offline release, `source.kind: "path"` points to a directory below the
project containing `deneb.plugin.json` and its `.wasm` Component entrypoint.
Deneb validates Component encoding, manifest ABI, identity, capabilities, and
both manifest/lock SHA-256 values before executing and copying it. Registry and
Git entries are not fetched during a build, and registry JavaScript is never
evaluated by resolution or packaging. A required unresolved plugin stops the
build; an optional unresolved plugin is recorded in the resolution report and
does not enter the payload.

## Resolution

`deneb resolve <project> --target <target>` emits the complete deterministic
plan. `deneb plugins list` presents roots and transitive dependencies, and
`deneb plugins validate` validates the same graph without instantiating a
Component. All commands accept `--plugin-lock`; otherwise
`deneb.lock.json` beside the project is used.

For path sources, resolution reads `deneb.plugin.json` and immutable Component
bytes, but does not instantiate them. It validates:

- manifest API/ABI, project engine version against `engineRange`, and target;
- exact manifest/lock capability and dependency declarations;
- dependency SemVer ranges, missing dependencies, cycles, and conflicts;
- manifest, lock, and computed SHA-256 integrity;
- portable paths, Component encoding, and distribution policy.

The optional manifest field `targets` is a set of `web`, `pwa`, `electron`,
`tauri`, `android`, and `ios`; an empty or absent set means all targets.
`engineRange` is checked against `project.engine.version`. Existing manifests
without `targets`, old projects without plugins, and version-1 locks remain
valid.

## Vega runtime plugins

The runtime path reads the canonical Vega v1 lock produced by Vega or Altair:

```json
{
  "format": "vega-plugin-lock",
  "formatVersion": 1,
  "projectFormatVersion": 1,
  "target": {
    "runtime": "vega",
    "platform": "pwa",
    "architecture": "wasm32",
    "engineVersion": "1.4.0",
    "apiVersion": 1
  },
  "plugins": [{
    "id": "overlay",
    "version": "1.2.3",
    "source": {
      "type": "registry",
      "package": "@vega/overlay",
      "registry": "https://registry.example"
    },
    "integrity": "registry-integrity-value",
    "dependencies": {},
    "capabilities": ["render.overlay"],
    "permissions": ["project.read"]
  }]
}
```

Entries and dependency keys must be uniquely sorted. Dependencies must select
the exact version present in the lock and the graph must be acyclic. Unknown
fields, invalid sources, incompatible targets, capability/permission drift,
and project version-range mismatches fail strict validation. Canonical
`registry`, `url`, `workspace`, and `builtin` sources are declarations only:
Deneb performs no registry request, URL fetch, workspace build, package
installation, or plugin execution.

Actual deployable bytes must be supplied by the host or an audited cache using
`deneb-vega-plugin-artifact-map@1`:

```json
{
  "format": "deneb-vega-plugin-artifact-map",
  "formatVersion": 1,
  "plugins": [{
    "id": "overlay",
    "version": "1.2.3",
    "source": {
      "type": "registry",
      "package": "@vega/overlay",
      "registry": "https://registry.example"
    },
    "kind": "module",
    "path": "approved/overlay.mjs",
    "lockIntegrity": "registry-integrity-value",
    "integrity": "sha256-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    "grantedPermissions": ["project.read"],
    "mediaType": "text/javascript"
  }]
}
```

`lockIntegrity` binds the mapping to the canonical source identity.
`integrity` independently verifies the exact local runtime artifact with
SHA-256. Paths are confined below the artifact-map directory; symlink escape,
oversize input, prohibited distribution payloads, unapproved permissions, and
identity/source drift are rejected. `module` and `package` entries are copied
opaquely. `wasm-component` entries additionally receive Component encoding
validation, but are not instantiated.

Required missing or invalid artifacts fail the build. Optional failures are
warnings and their dependent subgraph is omitted. Resolution is deterministic
and dependency-first. The build emits:

- `vega.plugins.lock.json`, a canonical lock snapshot;
- `runtime-plugins/deneb-vega-plugin-plan.json`;
- `runtime-plugins/vega-runtime-plugins.json`;
- `runtime-plugins/vega-runtime-plugin-sbom.json`;
- verified artifacts below `runtime-plugins/artifacts/`.

The same web payload is used by Web, PWA, Electron, Tauri, Android, and iOS.
PWA precache includes the runtime-plugin files. Runtime ABI 1 exposes the index
as `runtimePluginIndexUrl` to both `createDenebRuntime()` and `mount()`.

Use `--vega-plugin-lock`; the default is `vega.plugins.lock.json` beside the
project. Use `--vega-plugin-artifact-map`; the default is
`vega.plugins.cache.json`. `--vega-plugin-architecture` supplies an explicit
architecture for target-pinned locks. `resolve`, `plugins list`, and
`plugins validate` accept the same flags plus `--vega-engine-version`.

## External runtime attachments

A public Vega plugin may be a code-only adapter for a separately licensed
runtime. Each lock entry declares that boundary without carrying local paths
or license evidence:

```json
{
  "id": "haneoka.cubism",
  "version": "0.1.0",
  "source": { "type": "registry", "package": "@haneoka/vega-plugin-cubism" },
  "dependencies": {},
  "externalRuntimes": [{
    "id": "vendor.character-runtime",
    "name": "Character Runtime",
    "version": "5.0.0",
    "license": "LicenseRef-Vendor-Proprietary",
    "homepage": "https://vendor.example/runtime",
    "optional": false,
    "provisioning": ["application-bundle", "host"]
  }]
}
```

`host` means the application may provide the runtime without putting it in a
Deneb artifact. `application-bundle` merely permits an audited local
attachment; it never causes a download or copy by itself.

The artifact map may materialize a requirement at top-level
`externalRuntimes`. The source directory and receipt are relative to the
artifact-map file:

```json
{
  "format": "deneb-vega-plugin-artifact-map",
  "formatVersion": 1,
  "plugins": [],
  "externalRuntimes": [{
    "id": "vendor.character-runtime",
    "version": "5.0.0",
    "distribution": "application-private",
    "applicationId": "summer-story",
    "path": "approved/character-runtime",
    "files": [{
      "path": "runtime.js",
      "integrity": "sha256-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    }],
    "license": {
      "id": "LicenseRef-Vendor-Proprietary",
      "acknowledgement": "accepted-for-application-private-build",
      "receipt": {
        "path": "receipts/character-runtime.json",
        "integrity": "sha256-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
      }
    }
  }]
}
```

Deneb rejects this map unless the build also passes
`--allow-private-external-runtimes`. It then requires:

- an exact selected lock requirement with matching ID, version, license, and
  `application-bundle` provisioning;
- `applicationId` equal to the project ID and the literal
  `application-private` distribution;
- a non-symlink local directory below the artifact-map root;
- a sorted, exhaustive file list with an exact lowercase SHA-256 for every
  regular file; extra, missing, changed, oversized, and case-colliding paths
  fail closed;
- a separately hashed license receipt and the exact acknowledgement literal.

The receipt is verified but never copied. Bundled bytes are rehashed while
copying. Deneb emits `external-runtimes/deneb-external-runtimes.json` and
`external-runtimes/deneb-external-runtime-sbom.json`; both record the license
identifier, receipt digest, application binding, per-file hashes, and the
canonical lock/artifact-map digests. They also record
`publiclyRedistributable: false`. `deneb-manifest.json` and
`deneb-target.json` mark the entire result `application-private`. Public
release jobs must reject that marker.

Without the CLI opt-in, external bytes are never copied. A requirement that
also permits `host` remains host-provisioned; an optional application-only
requirement is omitted; a required application-only requirement without an
authorized attachment fails.

Character model, motion, atlas, and skeleton files do not need this attachment
flow merely because of their format. They may be ordinary project assets and
remain publicly packageable when the publisher has the necessary rights. The
attachment boundary is for SDK/Core/Framework or other separately licensed
runtime bytes; public plugin artifacts cannot carry those bytes.

Target compatibility maps as follows:

- Web → `vega` / `web`;
- PWA → `vega` / `pwa`;
- Electron → `vega` / `electron`;
- Tauri → `vega` / `tauri`;
- Android → `vega` / `android`;
- iOS → `vega` / `ios`.

Plugin API version is 1. A lock-pinned engine version is checked against
`engineVersion` in `deneb-runtime.json`; an omitted concrete engine version
cannot satisfy a pinned value.

## Capabilities

Capabilities are default-deny and deliberately narrow. The executable build
host exposes only bounded project/asset reads, generated-file writes,
diagnostics, and transform responses:

- read project/assets;
- transform the project;
- write generated files;
- encode an asset;
- provide a target;
- publish;
- access network or processes.

The Wasmtime linker contains no WASI, socket, clock, environment, filesystem,
or process imports. `network`, `process`, and `publish` are therefore never
granted by the normal build host, even when listed in a manifest. Generated
output is confined to
`generated/plugins/<plugin-id-hash>/`, accepts only portable relative paths,
and is bounded across the session by file count, byte count, diagnostics,
linear memory, fuel, and wall time. Themes can be implemented as ordinary
`read-project + write-generated` plugins without process or network authority.

## Lifecycle and hooks

ABI 1 first verifies `metadata()` against the manifest and lock, then creates
one build-scoped session with the negotiated ABI, granted capabilities, and
limits. Deneb runs `validate → transform → assets → emit → package` in
deterministic dependency order and disposes every created session, including
after a failure. `publish` is reserved by the ABI and is not run by the build
pipeline.

Transform responses use RFC 7396 JSON Merge Patch. Deneb reparses and
revalidates the result and rejects any change to the locked plugin request set
or identity. Required-plugin failures stop the build; optional-plugin failures
become diagnostics and are recorded as failed or unresolved.

The emitted `plugins/deneb-plugins.json` contains only enabled, resolved
Components. It records the exact copied component path and checksum, granted
capabilities, hooks, configuration, execution status, completed hooks,
generated namespace, and disposal result.
`plugins/deneb-plugin-resolution.json` retains resolved/skipped/failed entries
and diagnostics, while `plugins/deneb-plugin-sbom.json` records target, resolved
lock digest, source, checksum, declared/granted capabilities, dependencies, and
artifact path. These files live in the shared web payload for
Web/PWA/Electron/Tauri/Android/iOS. Component bytes remain for reproducibility
and inspection; Vega does not execute Deneb build plugins. Generated and
dispose-time outputs participate in the final manifest hash and `.vgpack`.
The resolution report's `lockDigest` identifies the authored input lock; the
SBOM and build manifest identify the filtered lock emitted with the bundle.

Vega runtime plugins remain a separate trust boundary from Deneb Components.
Deneb verifies and packages approved bytes, while Vega owns runtime loading,
capability enforcement, lifecycle, and disposal through the emitted index.
