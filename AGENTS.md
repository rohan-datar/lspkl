# Repository Guidelines

## Project Overview

lspkl is a native Rust language server for Pkl: diagnostics, hover, definitions, completion, quick fixes, and CLI-backed formatting. It speaks LSP over stdio only; stdout is protocol traffic, and tracing belongs on stderr.

## Architecture & Data Flow

### Transport and scheduling (`crates/lspkl/src/server.rs`)

- `lsp-server` owns the stdio reader/writer threads. `server::run` is an ordered main loop that owns `Workspace` by value and drains protocol messages before handling worker results, so edits, `$/cancelRequest` and shutdown are never queued behind computation or external jobs.
- Notifications apply their input writes synchronously, in arrival order, on the main loop. It never executes queries; Salsa setters cancel readers and wait for active snapshots to drop.
- One compute worker announces readiness; only then does the loop take a `Snapshot` and hand it over through a rendezvous channel, so no database clone exists unless it is executing. Priority when idle: semantic input discovery, then queued requests, then coalesced per-document diagnostics (skipped while reloads are outstanding). Diagnostics that touched missing inputs are held back until those inputs load or `INPUT_WAIT` expires, so new imports do not flash as unresolved.
- Requests queue as parameters plus the document's arrival epoch. A request whose document was edited, closed or reopened answers `ContentModified` (-32801). A computation cancelled by a background write (input load, reload) is recomputed instead, up to `MAX_RETRIES`. Cancellation answers `RequestCancelled` (-32800) immediately; shutdown cancels everything outstanding; every request gets exactly one response.
- Four I/O workers run owned `IoJob`s (formatter/CLI, HTTPS fetch, source loads, settings resolution, project restoration) without database handles. Each job carries the completion delivered if it panics, so registrations and input bookkeeping are always cleared. Accepted project metadata is persisted by a separate serialized thread; rejected or cancelled syncs never write `.lspkl/projects.json`.
- Epochs: per-document arrival epochs; analysis `generation` for diagnostics publication; `project_generation` for CLI project jobs (advanced on folder arrival and project watcher events); configuration epochs (advanced on every pull and push, only the current pull reply is applied); `io_epoch` for input loads (advanced by settings, project, folder and watched-file changes, not by editor edits).
- Settings precedence is per section: a non-null value in the current `workspace/configuration` reply wins; null sections keep the latest explicit initializationOptions or `didChangeConfiguration` push, otherwise defaults. Folderless relative modulepath/cacheDir paths bind to the process directory.

### Inputs and I/O (`crates/pkl-db`)

- Salsa inputs: canonical `File` (text/status/version), directories, settings, projects and routing facts. Open/change/close set text/status/version; client-open URIs are tracked separately because aliases share a canonical `File` but close independently. The server advertises full synchronization.
- Queries never touch the filesystem or network. `Db::file`/`Db::directory` create missing identities lazily; `RootDatabase::pending_io` reports the source, directory and routing facts open import graphs still need, `PendingIo::load` reads them on an I/O worker, and `apply_io` installs results on the main loop if their `io_epoch` is current (`PendingIo::failed` for loads that panic).
- Requests use `discover_request_io` to collect only the missing inputs they touched, including memoized import resolutions (replayed past the memo). A request waits for its own loads only, and answers with its best result after `INPUT_WAIT` or `MAX_RETRIES`.
- Watched files and completed downloads refresh source/directory inputs and call `request_routing_refresh`. HTTPS misses are fetched by I/O jobs; missing packages keep the download quick fix rather than downloading automatically.
- Exception: module-URI completion (`ModuleResolver::children`, `cached_packages`) lists local directories and the package cache directly on the compute thread, outside tracked queries.

### Semantics and features

- `pkl-db` also owns tracked `parse`/`line_index`, contextual `resolve_import` and root-keyed `import_graph` (edges, not trees). `AnalysisContext` borrows the database and carries the routing root.
- `pkl-types::module_items` lowers names, modifiers and position-independent type references into declaration arenas; initializers and method bodies stay outside this firewall, and `item_source_map` supplies locations on demand. `class_members`/`module_members` share inherited member tables; `scope_at` serves resolution, completion, hover and access diagnostics.
- `infer(db, DeclId)` memoizes declaration bodies; `decl_type`, `resolve_type_ref`, `this_type` and `expected_type` are shared by diagnostics and editor features. Cycles recover through Salsa.
- `pkl_diagnostics::diagnostics(db, File)` memoizes diagnostics and quick fixes (structural equality including ranges); publication and code actions share it, and both use the wire form in `lsp_features::diagnostics`.
- `pkl_project::ModuleResolver` (implemented by `ImportResolver`, constructed only through `pkl_db::with_module_resolver`) owns routing and completion discovery. Module-URI completion lives in `lsp-features`.
- Positions are `TextRange`/`TextSize` byte offsets; `lsp_features::positions::PositionConverter` performs the negotiated conversion (UTF-8 if offered, else UTF-16) using the originating or target tree.
- SDK sources are read-only, high-durability inputs; `Workspace` populates the `base_module` memo before the transport starts. Stdlib, HTTPS and archive definitions use `lspkl://` URIs served by `pkl/fileContents`; reuse `pkl-vfs` URI conversions. Internal identity is `File`; `url::Url` is only for filesystem path conversion.

## Key Directories

All crate paths below are under `crates/`.

| Directory | Responsibility |
| --- | --- |
| `lspkl/` | Binary, protocol handlers, Workspace/Snapshot wiring and conversion |
| `pkl-db/` | Salsa database, canonical files, settings/projects, parse/index/import queries |
| `pkl-syntax/` | Tree-sitter parsing and indexed AST |
| `pkl-types/` | Declaration/member queries, shared scopes, cross-file resolution and type computation |
| `pkl-diagnostics/` | Tracked diagnostics/fixes, analyzer engine, rendering |
| `lsp-features/` | Hover, definitions, semantic/module-URI completion, code actions, formatting |
| `pkl-vfs/` | Deterministic source loading, explicit HTTPS fetch, virtual-source URI mapping |
| `pkl-project/` | ModuleResolver, source-aware candidates, projects, packages, routing, CLI |
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
- Synchronous parsing, semantic computation and conversion run on the demand-dispatched compute thread. Filesystem/network/subprocess work runs on owned I/O jobs without database handles. Never hold project guards during Salsa writes. No database clone may remain idle across external I/O: setters cancel readers and wait for active clones to drop.
- State uses the writable database, short-lived clones and eagerly populated database-local SDK memo. Production diagnostics are a tracked query with a shared immutable analyzer engine in `pkl-diagnostics`. Dependency injection uses configured CLI, SourceLoader, ModuleResolver, database event callbacks, Workspace constructors, analyzer pipelines and definition URI mapping. ProjectManager locking is orchestration-only; semantic queries read project inputs.
- Domain failures use `thiserror` enums and `?`; convert to JSON-RPC errors at transport boundaries. Missing feature results commonly return `Ok(None)`; notification failures are logged. Avoid panics for unavailable semantic information.
- Diagnostic analyzers implement `Analyzer`; preserve registration order and the shared walker's descend/skip contract. Formatting is a synchronous CLI exception to otherwise computation-focused features: exit 0 means unchanged, exit 11 means a whole-document edit.

## Important Files

- `crates/lspkl/src/main.rs`: startup and stderr tracing; `server.rs`: main loop, request/notification routing (including `pkl/*` RPCs), worker scheduling, epochs and diagnostic publication; `perf.rs`: handler/worker/Salsa setter timings and cancellation/ordering counters; `state.rs`: writable Workspace and active Snapshot ownership.
- `crates/lspkl/src/handlers/`: `lifecycle.rs` owns initialization/capabilities, workspace notifications, and restored project metadata; `documents.rs` owns editor requests and source retrieval; `diagnostics.rs` runs the diagnostics query for publication; `commands.rs` owns explicit project/package operations. `mod.rs` declares these modules and the common error/result types; callers use module-qualified handlers without root re-exports.
- `crates/pkl-db/src/lib.rs`: inputs, file side table and tracked queries; `settings.rs`: nested `pkl` settings including `cli.path`, `modulepath`, `projects.excludedDirectories`, `formatter.grammarVersion`, and `cacheDir`. Do not substitute dotted top-level keys. `effective_cache_dir()` is the shared borrowed import/completion/download cache path. Missing relative modulepath roots retain deterministic workspace bindings.
- `crates/pkl-syntax/src/ast.rs`: byte-range arena, retained grammar fields, and per-tree line index; `ast/nodes.rs`: typed syntax views, semantic categories, and centralized positional accessors. `crates/lsp-features/src/positions.rs`: negotiated LSP position/range conversions. `crates/pkl-types/src/context.rs` and `file_id.rs`: cross-file context/reference invariants; `crates/pkl-project/src/import_resolver.rs`: import routing.
- `crates/pkl-diagnostics/src/engine.rs`: tracked production diagnostics and fixed analyzer pipeline; `analyzer.rs`: same-file tree walking. `crates/lsp-features/src/module_uri_completion.rs`: URI edits and lazy module documentation; `crates/pkl-project/src/module_resolver.rs`: resolver contract and source-side candidate discovery.
- `Cargo.toml`/`Cargo.lock`, `flake.nix`/`flake.lock`, `nix/package.nix`: dependency/build inputs. `.github/workflows/checks.yml`: quality gates.
- `README.md`: editor setup and virtual-document caveats. Clients must support `lspkl://` and `pkl/fileContents`; stock pkl-vscode's `pkl.lsp.path` expects a JAR, not this executable.

## Runtime/Tooling Preferences

- Rust edition 2024, Cargo, stable Rust, rustfmt, Clippy, and cargo-nextest. No explicit MSRV is declared. Node/Bun/npm are not repository build requirements; README npm commands target an external editor extension.
- External `pkl` is needed for formatting, project operations, and package downloads, not basic embedded-stdlib analysis. Configure `pkl.cli.path` or provide it on PATH.
- `crates/pkl-stdlib/build.rs` downloads/checksums pinned Pkl 0.32.1 sources and generates files in `OUT_DIR`; do not edit generated output. Offline builds can set `PKL_STDLIB_ARCHIVE` to a local ZIP. `PKL_STDLIB_URL` changes the URL, not the accepted hashes.
- SDK upgrades must synchronize build-script pins/checksums with `nix/package.nix`. Grammar dependency upgrades must keep `Cargo.lock` and Nix's Git output hash coherent. Preserve SDK LICENSE/NOTICE packaging.
- Linux release builds use `cross` and Docker; packaging requires Just >=1.32, Bash, and the utilities listed atop `justfile`. These are release-tool requirements, not server runtime dependencies.

## Testing & QA

- Rust `#[test]`/`#[tokio::test]`; reuse crate-local `tests/harness.rs` and `crates/lspkl/tests/support/`: `mod.rs` (stdio peer that answers configuration pulls with null like an editor without settings; `preload` installs inputs synchronously for direct `Workspace` tests) and `memory.rs` (in-process `Connection::memory` peer). Semantic harnesses use canonical database File inputs and the embedded `pkl:base` memo.
- Insta parser inputs: `crates/pkl-syntax/tests/snapshots/parser/inputs/`; diagnostic inputs: `crates/pkl-diagnostics/tests/fixtures/diagnostics/inputs/`. Review snapshot changes with `cargo insta review` rather than blindly accepting them.
- Match the changed boundary: semantic crate tests, `lspkl/tests/analysis_snapshot.rs` for graph consistency, handler tests for lifecycle, `lspkl/tests/scheduling.rs` for main-loop guarantees (supersession, worker panics, shutdown answers) with injected `SourceLoader`s and Salsa event callbacks, and `lspkl/tests/stdio.rs` for actual executable/JSON-RPC behavior. `lspkl/tests/positions.rs` covers encoding negotiation and Unicode request/response ranges over the executable. The smoke example checks diagnostics, stdlib definition/source ranges, and shutdown.
- `pkl-diagnostics/tests/incremental.rs` covers cached diagnostics/fixes, fresh ranges after source edits, structural backdating, and imported semantic invalidation. Module URI feature tests consume Db/File/ModuleResolver directly; project tests cover candidate discovery and routing precedence. Source-boundary tests exercise URI escaping and archive/file identity.
- Keep fixtures isolated: temporary workspaces/caches, explicit CLI executables, locally generated ZIPs and loopback TLS services. Stdio helpers intentionally clear PATH and isolate HOME/USERPROFILE; preserve Unix-only fixture guards.
- CI runs native tests serially on Linux/macOS/Windows plus separate doctests. No quantitative coverage threshold is configured. If cached stdio tests embed an old checkout path after moving the repo, rebuild the affected test artifacts before interpreting those launch failures as server regressions.
