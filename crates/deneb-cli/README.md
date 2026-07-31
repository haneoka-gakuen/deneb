# deneb

Command-line validation, inspection, and reproducible multi-target packaging for Vega visual novels.

```sh
deneb validate project.vega.json
deneb resolve project.vega.json --target web
deneb plugins list project.vega.json
deneb plugins validate project.vega.json --target tauri
deneb resolve project.vega.json \
  --vega-plugin-lock vega.plugins.lock.json \
  --vega-plugin-artifact-map vega.plugins.cache.json
deneb build project.vega.json --output dist/web --target web \
  --runtime-bundle path/to/vega-runtime \
  --bundle-identifier com.example.story \
  --color-mode-storage-key com.example.story.shell.color-mode
deneb build project.vega.json --output dist/private \
  --vega-plugin-lock vega.plugins.lock.json \
  --vega-plugin-artifact-map vega.plugins.cache.json \
  --allow-private-external-runtimes
```

`vega.plugins.lock.json` is the canonical Vega runtime-plugin lock.
`deneb.lock.json` is the separate WASM Component build-plugin lock. Deneb never
parses either format as the other, and never downloads or executes canonical
runtime-plugin sources during resolution or packaging.

Separately licensed runtime directories are local-only external attachments.
They are never downloaded or copied by default. Packaging requires the
explicit private-runtime flag plus an application-bound artifact-map entry,
an exhaustive per-file SHA-256 manifest, and a hashed license receipt.
Character model, motion, atlas, and skeleton files remain ordinary project
assets; this private attachment path is only for separately licensed runtime
bytes.

See the [Deneb repository](https://github.com/haneoka-gakuen/deneb) for runtime binding, Web/PWA and native target requirements, plugin security, and release policy.
