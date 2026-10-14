# lspkl-syntax

Pkl parsing and typed syntax trees used by the lspkl language server.

Part of [lspkl](https://github.com/rohan-datar/lspkl). See the
[project README](https://github.com/rohan-datar/lspkl#readme) for editor setup,
configuration, and source-build instructions.

## Bundled parser

This crate bundles Apple's [tree-sitter-pkl grammar at `v0.21.0`](https://github.com/apple/tree-sitter-pkl/tree/v0.21.0), revision `5893fb5dc832ea51a9bff3f84462528c766ecd7d`. The generated C parser and scanner are compiled during the build, requiring a C compiler, but no Git grammar fetch, separate grammar crate, or Tree-sitter CLI. Upstream license and notice files are retained in `vendor/tree-sitter-pkl/`.

## License

Licensed under [Apache 2.0](LICENSE). See [NOTICE.txt](NOTICE.txt) for attributions.
