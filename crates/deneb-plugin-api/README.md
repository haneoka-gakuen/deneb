# haneoka-deneb-plugin-api

Stable manifest types (including engine/target constraints) and the versioned
WIT Component boundary for capability-isolated Deneb build plugins.

This crate defines manifests, capabilities, hooks, bounded generated output, and validation helpers. It does not execute third-party components by itself. See the [Deneb repository](https://github.com/haneoka-gakuen/deneb) for the trust model, lockfile format, and lifecycle contract.
