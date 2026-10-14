# lspkl

A native Rust language server for the [Pkl configuration language](https://pkl-lang.org), providing diagnostics, hover documentation, definitions, completion, quick fixes, and Pkl CLI-backed formatting over stdio LSP.

Part of [lspkl](https://github.com/rohan-datar/lspkl). See the
[project README](https://github.com/rohan-datar/lspkl#readme) for editor setup,
configuration, and source-build instructions.

## Installation

Install a published release from crates.io:

```sh
cargo install lspkl --locked
```

Building requires a Rust toolchain supporting edition 2024 and a C compiler for Tree-sitter and its bundled Pkl grammar. The Pkl CLI is needed for formatting and project/package operations, not basic embedded-standard-library analysis.

## Build-time SDK

Building this crate requires HTTPS access to GitHub to download the pinned Pkl 0.32.1 SDK ZIP archive, unless `PKL_STDLIB_ARCHIVE` names a pre-fetched supported archive. `PKL_STDLIB_URL` may override the download URL, but does not change the accepted SHA-256 checksums. The standard-library sources are embedded at build time; no SDK fetch is needed at runtime.

## License

Licensed under [Apache 2.0](LICENSE). See [NOTICE.txt](NOTICE.txt) for attributions.
