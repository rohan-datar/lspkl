# Repository Guidelines

## Project Overview

lspkl is a native Rust language server for Pkl: diagnostics, hover, definitions, completion, quick fixes, and CLI-backed formatting. It speaks LSP over stdio only; stdout is protocol traffic, and tracing belongs on stderr.

## Architecture & Data Flow

- `PklBackend` adapts `tower_lsp_server` around `Arc<Mutex<Workspace>>` on a Tokio current-thread runtime. Notifications synchronously set Salsa inputs; computational requests clone the database into a short-lived `Snapshot`, release the workspace lock, and compute in `spawn_blocking`. Catch `salsa::Cancelled` at the request boundary and return `ContentModified` (-32801).
- Open/change/close own canonical `pkl_db::File` text/status/version inputs. Keep live client-open URIs separately for diagnostic worker keys and publication; aliases share a canonical File but close independently. Diagnostics use latest-generation watch slots containing generation numbers, not idle database clones; generation/version checks suppress stale publishes. Drop stale snapshots and await a watch change rather than spinning. The server advertises full synchronization.
- `pkl-db` owns URI identities, configuration/project/source inputs, tracked `parse`/`line_index`, contextual `resolve_import`, and root-keyed `import_graph`. `AnalysisContext` borrows the database and carries the routing root; every semantic node remains explicitly file-qualified. Graphs retain edges, not syntax trees; nearest local projects override inherited foreign-source context.
- `pkl-types::module_items` lowers interned names, modifiers and position-independent type references into declaration arenas; property initializers and method bodies stay outside the declaration firewall. `item_source_map` supplies fresh file-qualified locations on demand. Root-keyed `class_members`/`module_members` share inherited member tables, and `scope_at` serves resolution, completion, hover navigation and access diagnostics. Expression lookup and qualified type-import lookup use distinct namespaces.
- `infer(db, DeclId)` memoizes a declaration body; private bindings-aware expression queries retain intra-body dependencies and recover cycles with Salsa rather than thread-local state. `decl_type`, `resolve_type_ref`, `this_type` and `expected_type` share semantic results across diagnostics and editor requests. Alias bodies use declaration-keyed symbolic templates before concrete substitution, so growing recursive arguments cannot evade cycle recovery.
- `SourceLoader` provides deterministic filesystem, archive, SDK and HTTPS-cache reads. Network and CLI work runs outside queries and without database clones; completed reads set inputs. Watched `.pkl`, `PklProject`, and `PklProject.deps.json` events refresh source/directory/project inputs; archive backing events also reload known members, including missing members. Only explicit watched project events invalidate derived project metadata; ordinary close/full-refresh reads retain synced routing. CLI job validity follows project/configuration changes, not unrelated editor edits.
- Semantic spans/cursors are `TextRange`/`TextSize` byte offsets. Syntax trees share source and `Arc<LineIndex>` storage with database queries; `lsp_features::positions::PositionConverter` owns negotiated LSP conversion. Initialization prefers offered UTF-8, otherwise UTF-16. Convert diagnostics and definitions using their originating/target trees.
- SDK text/URIs are read-only, high-durability inputs; editor open/change/close affects status/version only. `Workspace` eagerly populates the shared database-local `base_module` memo before exposing snapshots; the binary constructs it in `spawn_blocking` before transport startup, so editor writes cannot repeatedly cancel bootstrap. Stdlib, HTTPS, and archive definitions use `lspkl://` and `pkl/fileContents`; reuse `pkl-vfs` URI conversions. Initialization restores `.lspkl/projects.json`; explicit sync/download invokes the external CLI.

- Performance follow-up after Phase 8: snapshots captured before `spawn_blocking` can hold up input writes while queued. A cached `fileContents` request queued behind an occupied worker delayed writes by about 20 ms with zero query executions; ordinary measured queues were below 0.1 ms at p95. Re-profile scheduling separately from semantic work before choosing a fix.

## Key Directories

All crate paths below are under `crates/`.

| Directory | Responsibility |
| --- | --- |
| `lspkl/` | Binary, LSP handlers, Workspace/Snapshot, URI completion |
| `pkl-db/` | Salsa database, canonical files, settings/projects, parse/index/import queries |
| `pkl-syntax/` | Tree-sitter parsing and indexed AST |
| `pkl-types/` | Declaration/member queries, shared scopes, cross-file resolution and type computation |
| `pkl-diagnostics/` | Analyzer engine, diagnostics, rendering |
| `lsp-features/` | Hover, definitions, completion, code actions, formatting |
| `pkl-vfs/` | Deterministic source loading, explicit HTTPS fetch, virtual-source URI mapping |
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
- Cross-file references are `NodeRef = InFile<NodeId>` with `{ file, value }`; typed AST views use `InFile<T>`. `file` is a canonical Salsa `pkl_db::File`, stable across close/reopen. Never interpret a bare node in another file or mix database identities. Embedded base uses its ordinary canonical File.
- `Name`, `DeclId`, `ClassId`, `ModuleId` and `TypeParameterId` belong to one database and its snapshots. Interned slots are retained for that database lifetime; never move IDs between databases. `DeclId.root` carries import-routing context, while SDK identities use canonical SDK roots. Declaration arena IDs remain stable across value/body edits, not declaration reordering. Keep syntax locations in `item_source_map` and revision-local body inference maps, never in `Type`, `ModuleItems` or member tables.
- Reuse request snapshots and database-backed semantic queries rather than reloading trees or reconstructing SDK metadata. Type operations take `&dyn pkl_db::Db`; generic substitutions use stable type-parameter IDs and borrowed interned keys. `Type::Unknown` and `Option` represent unavailable analysis; ordinary malformed Pkl still produces an analyzable syntax tree.
- `Type` derives structural `Eq`/`Hash`. Construct unions with `Type::union`: alternatives are flattened, structurally ordered, de-duplicated, and stripped of `Nothing`; immutable `UnionMembers` protects this invariant. `Unknown` absorbs deterministically. Normalization uses constraint-sensitive, branch-recursive absorption and retains string-literal completion choices and distinct generic arguments.
- Refinements use `Type::Constrained`, not per-variant fields. `constraints()` borrows the outer refinement; `has_constraints(db)` includes aliases, union branches, and generic arguments for cast/type-test conclusions. Module types store only declaration identity; display names come from lowered module declarations.
- Alias conversion and normalization share an explicit expansion path. Resolve finite caller-supplied arguments before entering the callee alias body, retain generic substitutions/refinements through chains, and recover genuine body cycles as unavailable analysis. Nested nullability and common union-member types must preserve every surviving alternative.
- Grammar child order and token decisions belong in `pkl_syntax::ast::nodes`, split into common helpers, declarations, expressions, object members, and types behind the existing public node module. Use its typed views and `Expr`/`TypeNode`/`ObjectMember`/`ModuleMember` categories. The arena preserves named grammar fields and keyword/operator terminals; `NodeKind` is `Copy` with allocation-free `Unmapped(u16)` grammar identities. Add accessor behavior tests and parser snapshots when depending on unfielded child order; keep malformed/recovered slots explicit rather than shifting operands.
- Synchronous parsing, loading, semantic computation, conversion and subprocesses belong in `spawn_blocking`. Never hold workspace/project guards across `.await` or project guards during Salsa writes. No database clone may remain idle across external I/O: setters cancel readers and wait for all clones to drop.
- State uses the writable database, short-lived clones, the eagerly populated shared SDK memo and a lazy shared diagnostic engine. Dependency injection uses configured CLI, SourceLoader, database event callbacks, backend constructors, analyzer registration and definition URI mapping. ProjectManager locking is orchestration-only; semantic queries read project inputs.
- Domain failures use `thiserror` enums and `?`; convert to JSON-RPC errors at transport boundaries. Missing feature results commonly return `Ok(None)`; notification failures are logged. Avoid panics for unavailable semantic information.
- Diagnostic analyzers implement `Analyzer`; preserve registration order and the shared walker's descend/skip contract. Formatting is a synchronous CLI exception to otherwise computation-focused features: exit 0 means unchanged, exit 11 means a whole-document edit.

## Important Files

- `crates/lspkl/src/main.rs`: startup, stderr tracing and custom RPCs; `server.rs`: protocol adapter, owned job dispatch and diagnostic scheduling; `perf.rs`: handler/worker/Salsa setter timings and cancellation/ordering counters; `state.rs`: writable Workspace and active Snapshot ownership.
- `crates/lspkl/src/handlers/`: `lifecycle.rs` owns initialization/capabilities, workspace notifications, and restored project metadata; `documents.rs` owns editor requests and source retrieval; `diagnostics.rs` shares snapshot analysis and protocol conversion; `commands.rs` owns explicit project/package operations. `mod.rs` declares these modules and the common error/result types; callers use module-qualified handlers without root re-exports.
- `crates/pkl-db/src/lib.rs`: inputs, file side table and tracked queries; `settings.rs`: nested `pkl` settings including `cli.path`, `modulepath`, `projects.excludedDirectories`, `formatter.grammarVersion`, and `cacheDir`. Do not substitute dotted top-level keys. `effective_cache_dir()` is the shared borrowed import/completion/download cache path. Missing relative modulepath roots retain deterministic workspace bindings.
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
