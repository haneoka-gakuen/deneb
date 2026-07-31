# Vega project and Deneb bundle formats

## `vega-project@1`

The canonical source is UTF-8 JSON:

```json
{
  "format": "vega-project",
  "formatVersion": 1,
  "id": "summer-story",
  "title": { "en": "Summer Story", "zh-CN": "夏日物语" },
  "entryScene": "main",
  "assets": {},
  "plugins": [],
  "scenes": {
    "main": {
      "id": "main",
      "commands": [{ "command": 0, "index": 0, "key": "opening" }]
    }
  }
}
```

Top-level, scene, asset, and plugin envelopes reject unknown fields so typos do
not silently change a release. Commands deliberately retain a flattened field
map: the original ADV fields and future command-specific data must survive a
lossless converter round trip. Explicit `metadata` maps are the extension point
for application-specific data.

Each canonical `plugins` entry enables one project-level runtime-plugin root.
It may declare `required`, object-valued `configuration`, capabilities,
permission grants, dependency version ranges, target constraints, and a
registry/URL/workspace/builtin source constraint. Exact selected versions and
the transitive graph come from `vega-plugin-lock@1`. Required failures stop the
build; optional failed subgraphs are omitted and recorded in
`runtime-plugins/deneb-vega-plugin-plan.json`.

For backward compatibility, a project built with only `deneb.lock.json`
retains the established Deneb Component build-plugin meaning. When a canonical
`vega.plugins.lock.json` is present, runtime resolution is independent. If both
locks are intentionally supplied, lock membership partitions the authored
roots: Vega-lock members enter the player payload, Deneb-lock members enter the
build host, and an ID present in both is evaluated in both scopes. Required
roots selected by neither lock fail. One lock is never accepted in place of the
other.

Titles are strings or non-empty BCP-47-like locale maps. IDs and local paths
use a cross-platform ASCII subset. Local assets cannot be absolute, contain
`.`/`..`, a backslash, a drive prefix, empty segments, or control characters.
`http`, `https`, and `data` sources remain external; Deneb warns for plain
HTTP. Local assets are verified against an authored hash/byte count when those
fields are present.

During a build, Deneb rewrites exact authored local paths in recognized nested
resource fields (`url`, `src`, `texture`, `model`, and related forms) to the
content-addressed output. It does not rewrite prose or infer the meaning of an
asset alias. Alias consumption is a declared runtime capability.

## Opcode policy

Deneb validates allocation without changing Vega's established semantics:

- `0–100`: frozen native namespace; existing native meanings are preserved;
- `8` and `22`: retired frozen slots and therefore rejected;
- `101–499`: reserved for future native commands;
- `500`: native command group marker;
- `501–9999`: Haneoka official extension range;
- `10000+`: third-party commands allocated by `deneb-plugin-lock@1`.

Allocation sorts scoped command names before assigning the first available
number and never reassigns an existing entry. Discovery order therefore cannot
change a published project.

## `vega-bundle@1`

`project.vgb` is:

1. the eight-byte `VEGAB\x01\r\n` magic/version header;
2. named MessagePack containing `format`, `formatVersion`, compiler identity,
   the rewritten project, and asset integrity map.

`project.vega.json` is emitted alongside it for inspection and browser
adapters. Both are authoritative build outputs of the same model.

## `deneb-build@1`

`deneb-manifest.json` records the build ID, target, compiler, runtime state,
runtime ABI, the independent Deneb and Vega plugin-lock digests, runtime-plugin
count, distribution scope, external-runtime count/index/SBOM, and a sorted
path-to-SHA-256 map. A result containing a licensed external-runtime attachment
is marked `distribution: "application-private"` and
`publiclyRedistributable: false`.
`deneb-target.json` records the resolved `bundleIdentifier` and
`colorModeStorageKey`. CLI values are authoritative; otherwise Deneb derives
the neutral `app.vega.<project-id>` identifier and a namespaced color-mode key.
`plugins/deneb-plugins.json` lists copied Components,
`plugins/deneb-plugin-resolution.json` preserves the complete selection result,
and `plugins/deneb-plugin-sbom.json` summarizes build-plugin provenance.
`vega.plugins.lock.json` and the `runtime-plugins/` plan, index, SBOM, and
verified artifacts describe the separate runtime-plugin graph. The manifest
does not hash itself; the `.vgpack` archive includes it.

`external-runtimes/deneb-external-runtimes.json` records host requirements and
verified application attachments. Its companion
`deneb-external-runtime-sbom.json` contains only the bundled components, exact
file hashes, license identifier, application binding, acknowledgement, and
receipt digest. Both documents bind themselves to the canonical
`sourceLockDigest` and, when supplied, `artifactMapDigest`. License receipts
themselves are never emitted.

Format versions are independent. A runtime ABI bump does not silently reinterpret
the project or plugin ABI, and a new target does not require an opcode change.
