# pkl-db

The incremental Salsa analysis database used by the lspkl Pkl language server.

Part of [lspkl](https://github.com/rohan-datar/lspkl). See the
[project README](https://github.com/rohan-datar/lspkl#readme) for editor setup,
configuration, and source-build instructions.

## Build-time SDK

Building this crate requires HTTPS access to GitHub to download the pinned Pkl 0.32.1 SDK ZIP archive, unless `PKL_STDLIB_ARCHIVE` names a pre-fetched supported archive. `PKL_STDLIB_URL` may override the download URL, but does not change the accepted SHA-256 checksums. The standard-library sources are embedded at build time; no SDK fetch is needed at runtime.

## License

Licensed under [Apache 2.0](LICENSE). See [NOTICE.txt](NOTICE.txt) for attributions.
