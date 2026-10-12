# Repository Guidelines

## Project Overview

lspkl is a native Rust language server for Pkl: diagnostics, hover, definitions, completion, quick fixes, and CLI-backed formatting. It speaks LSP over stdio only; stdout is protocol traffic, and tracing belongs on stderr.

## Architecture & Data Flow

- `PklBackend` is a thin `tower_lsp_server` adapter around `Arc<Mutex<Workspace>>` on a Tokio current-thread runtime. Notifications call synchronous `apply_*` mutators before their first await; computational requests capture cheap `Snapshot` Arc clones, release the workspace lock, and run synchronous `handlers/` functions entirely in `spawn_blocking`. Keep transport/error conversion out of semantic handlers and crates.
- Open/change notifications replace immutable VFS buffer epochs. Request snapshots retain those buffers and settings through edit/close/reopen. Direct VFS cache-miss parsing and tree insertion share the contents-update lock; never install a tree parsed before a concurrent invalidation. Open/change/close refresh diagnostics for all open documents, including import dependents, through per-document latest-snapshot workers. Generation checks cover dependent edits with unchanged root versions; closing cancels the worker and evicts stale disk entries. The server advertises full, not incremental, synchronization.
- `Snapshot::prepare_analysis` pins the root, reachable imports, and ordinary registered base file into an `AnalysisSnapshot`. `AnalysisSnapshot::context` is the production construction point for file-agnostic `AnalysisContext`; `tree(FileId)` returns only pinned trees, with no current-file state or base fallback. Loader/project/package caches remain shared pending the database cutover.
- Source loading and project-aware import routing feed semantic analysis; `pkl-types` resolves names/types, `pkl-diagnostics` walks analyzers, and `lsp-features` computes editor results. Module-URI completion is handled in `lspkl` before general completion.
- Semantic spans/cursors are `TextRange`/`TextSize` byte offsets. Each `SyntaxTree` owns its `LineIndex`; `lsp_features::positions::PositionConverter` is the single protocol conversion boundary, reused by adapters and feature tests. Initialization prefers offered UTF-8, otherwise UTF-16. Convert diagnostics with the same pinned tree that produced them and definitions with the target tree, never the requesting tree.
- Stdlib, HTTPS, and archive definitions use editor-facing `lspkl://` URIs and the `pkl/fileContents` RPC. Reuse `pkl-vfs/src/uri.rs` conversions. Embedded SDK sources are read-only: editor epochs track versions but share immutable source/tree identity across open/change/close. SDK text is independent of its parse-cache lock; notification capture must not wait on parsing. Project initialization restores `.lspkl/projects.json`; explicit sync/download operations invoke the external CLI.

## Key Directories

All crate paths below are under `crates/`.

| Directory | Responsibility |
| --- | --- |
| `lspkl/` | Binary, LSP handlers, settings, snapshots, file identities, URI completion |
| `pkl-syntax/` | Tree-sitter parsing and indexed AST |
| `pkl-types/` | Cross-file name resolution, type inference, expression computation |
| `pkl-diagnostics/` | Analyzer engine, diagnostics, rendering |
| `lsp-features/` | Hover, definitions, completion, code actions, formatting |
| `pkl-vfs/` | Live buffers, source loading/cache, virtual-source URI mapping |
| `pkl-project/` | Project metadata, packages, modulepath/import routing, CLI wrapper |
| `pkl-stdlib/` | Build-time verified SDK sources embedded into the binary |

## Development Commands

Run from the repository root. `nix develop` supplies the development tools; `.envrc` also supports direnv.

```sh
cargo build --locked -p lspkl --bin lspkl
cargo run --locked -p lspkl -- --version
cargo run --locked -p lspkl --bin lspkl # stdio server; normally launched by a client
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo nextest run --workspace --locked --test-threads=1 --no-fail-fast \
  --failure-output immediate-final --status-level all
cargo test --workspace --doc --locked
cargo test --locked -p lspkl --test stdio -- --test-threads=1
cargo run --locked -p lspkl --example smoke -- --binary target/debug/lspkl
```

Nix packaging: `nix build`, `nix flake check`. Release automation is in `justfile`, not general build/test recipes: `just release-build TARGET`, `just release-package TARGET`, and `just release-checksums`. Native packaging runs the real stdio smoke; cross-target packaging does not. `crates/lspkl/examples/measure.rs` measures initialization/first analysis; see `.github/workflows/artifacts.yml` for release measurement commands. `examples/load.rs` measures scheduled edit bursts with cross-imports, interactive requests, and stale diagnostics; reproduction and baseline numbers are in `docs/transport-evidence.md`.

## Code Conventions & Common Patterns

- Use rustfmt, conventional Rust `snake_case` functions/modules and `PascalCase` types. Shared dependencies belong in root `[workspace.dependencies]`, consumed with `workspace = true`.
- Cross-file references are `NodeRef = InFile<NodeId>` with `{ file, value }`; typed AST views use `InFile<T>`. Never interpret a bare node in another file. `FileId::BASE` belongs only in registry/base loading; the base tree is pinned and looked up like every other tree. User IDs start at 1 and remain stable after close.
- Reuse request snapshots and explicit contexts rather than reloading trees or re-contexting during semantic computation. Public `pkl-types` queries are binding-free; recursive generic substitutions stay crate-private. `Type::Unknown` and `Option` represent unavailable analysis; ordinary malformed Pkl still produces an analyzable syntax tree.
- Grammar child order and token decisions belong in `pkl_syntax::ast::nodes`, split into common helpers, declarations, expressions, object members, and types behind the existing public node module. Use its typed views and `Expr`/`TypeNode`/`ObjectMember`/`ModuleMember` categories. The arena preserves named grammar fields and keyword/operator terminals; `NodeKind` is `Copy` with allocation-free `Unmapped(u16)` grammar identities. Add accessor behavior tests and parser snapshots when depending on unfielded child order; keep malformed/recovered slots explicit rather than shifting operands.
- Synchronous parsing, loading, semantic computation, diagnostics conversion, and subprocess work belong in `spawn_blocking` at async boundaries. Never hold workspace/registry/project/settings guards across `.await`; capture the snapshot or clone the needed handles first.
- State uses `Arc`, mutexes/RwLocks, and lazily initialized shared base/diagnostic engines. Dependency injection is explicit constructors, settings, and callbacks: configured `PklCli`, `PklBackend::for_testing`, analyzer registration, and definition URI mapping—not a container.
- Domain failures use `thiserror` enums and `?`; convert to JSON-RPC errors at transport boundaries. Missing feature results commonly return `Ok(None)`; notification failures are logged. Avoid panics for unavailable semantic information.
- Diagnostic analyzers implement `Analyzer`; preserve registration order and the shared walker's descend/skip contract. Formatting is a synchronous CLI exception to otherwise computation-focused features: exit 0 means unchanged, exit 11 means a whole-document edit.

## Important Files

- `crates/lspkl/src/main.rs`: startup, stderr tracing, custom RPC registration; `server.rs`: thin protocol adapter and diagnostic scheduling; `perf.rs`: opt-in handler/worker timings and ordering counter (`RUST_LOG=warn,lspkl::perf=debug`); `state.rs`: Workspace/Snapshot and analysis graph preparation; `file_registry.rs`: stable identities.
- `crates/lspkl/src/handlers/`: `lifecycle.rs` owns initialization/capabilities, workspace notifications, and restored project metadata; `documents.rs` owns editor requests and source retrieval; `diagnostics.rs` shares snapshot analysis and protocol conversion; `commands.rs` owns explicit project/package operations. `mod.rs` declares these modules and the common error/result types; callers use module-qualified handlers without root re-exports.
- `crates/lspkl/src/settings.rs`: nested `pkl` settings, including `cli.path`, `modulepath`, `projects.excludedDirectories`, `formatter.grammarVersion`, and `cacheDir`. Do not substitute dotted top-level keys. `effective_cache_dir()` is the shared borrowed cache path for imports, URI completion, and package downloads; explicit overrides win, otherwise HOME/USERPROFILE `.pkl/cache`, then the VFS fallback.
- `crates/pkl-syntax/src/ast.rs`: byte-range arena, retained grammar fields, and per-tree line index; `ast/nodes.rs`: typed syntax views, semantic categories, and centralized positional accessors. `crates/lsp-features/src/positions.rs`: negotiated LSP position/range conversions. `crates/pkl-types/src/context.rs` and `file_id.rs`: cross-file context/reference invariants; `crates/pkl-project/src/import_resolver.rs`: import routing.
- `Cargo.toml`/`Cargo.lock`, `flake.nix`/`flake.lock`, `nix/package.nix`: dependency/build inputs. `.github/workflows/checks.yml`: quality gates.
- `README.md`: editor setup and virtual-document caveats. Clients must support `lspkl://` and `pkl/fileContents`; stock pkl-vscode's `pkl.lsp.path` expects a JAR, not this executable.

## Runtime/Tooling Preferences

- Rust edition 2024, Cargo, stable Rust, rustfmt, Clippy, and cargo-nextest. No explicit MSRV is declared. Node/Bun/npm are not repository build requirements; README npm commands target an external editor extension.
- External `pkl` is needed for formatting, project operations, and package downloads, not basic embedded-stdlib analysis. Configure `pkl.cli.path` or provide it on PATH.
- `crates/pkl-stdlib/build.rs` downloads/checksums pinned Pkl 0.32.1 sources and generates files in `OUT_DIR`; do not edit generated output. Offline builds can set `PKL_STDLIB_ARCHIVE` to a local ZIP. `PKL_STDLIB_URL` changes the URL, not the accepted hashes.
- SDK upgrades must synchronize build-script pins/checksums with `nix/package.nix`. Grammar dependency upgrades must keep `Cargo.lock` and Nix's Git output hash coherent. Preserve SDK LICENSE/NOTICE packaging.
- Linux release builds use `cross` and Docker; packaging requires Just >=1.32, Bash, and the utilities listed atop `justfile`. These are release-tool requirements, not server runtime dependencies.

## Testing & QA

- Rust `#[test]`/`#[tokio::test]`; reuse crate-local `tests/harness.rs` and `crates/lspkl/tests/support/mod.rs`. Semantic harnesses use embedded `pkl:base` and `FileId(1)` for snippets.
- Insta parser inputs: `crates/pkl-syntax/tests/snapshots/parser/inputs/`; diagnostic inputs: `crates/pkl-diagnostics/tests/fixtures/diagnostics/inputs/`. Review snapshot changes with `cargo insta review` rather than blindly accepting them.
- Match the changed boundary: semantic crate tests, `lspkl/tests/analysis_snapshot.rs` for graph consistency, handler tests for lifecycle, and `lspkl/tests/stdio.rs` for actual executable/JSON-RPC behavior. `lspkl/tests/positions.rs` covers encoding negotiation and Unicode request/response ranges over the executable. The smoke example checks diagnostics, stdlib definition/source ranges, and shutdown.
- Keep fixtures isolated: temporary workspaces/caches, explicit CLI executables, locally generated ZIPs and loopback TLS services. Stdio helpers intentionally clear PATH and isolate HOME/USERPROFILE; preserve Unix-only fixture guards.
- CI runs native tests serially on Linux/macOS/Windows plus separate doctests. No quantitative coverage threshold is configured. If cached stdio tests embed an old checkout path after moving the repo, rebuild the affected test artifacts before interpreting those launch failures as server regressions.
