# haneoka-deneb-core

Deterministic validation, sandboxed build-Component execution, asset processing,
runtime binding, archive generation, and platform scaffolding for
`vega-project@1`.

The crate is UI-independent and forbids unsafe Rust. It provides the build graph used by the Deneb CLI, but performs no signing or store upload and does not grant ambient filesystem or network access to plugins. See the [Deneb repository](https://github.com/haneoka-gakuen/deneb) for format and security documentation.
