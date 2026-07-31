# Contributing

Thank you for helping Deneb become a dependable visual-novel delivery tool.

## Before a pull request

Use the pinned toolchain and committed lockfile:

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

Changes to a serialized structure require a compatibility review and an update
to the matching document under `docs/`. Never reinterpret opcodes 0–100. New
official commands start after 500; third-party commands belong in the lock's
10,000+ namespace.

Target changes must preserve the framework-neutral Web payload and every
referenced local boot resource.

The upstream repositories under the parent `references/` directory are for
behavioral research. Do not copy GPL/LGPL implementation source into Deneb.
Include the source and license of any vendored material in the relevant notice.

## Commit scope

Keep format, plugin ABI, runtime ABI, and provider changes separate when
possible. Explain migration impact and external toolchain assumptions in the
pull request. Generated platform lockfiles belong to generated applications,
not to Deneb's source templates.
