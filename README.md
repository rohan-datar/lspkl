# lspkl

A native Rust language server for the [Pkl configuration language](https://pkl-lang.org).
It provides diagnostics, hover documentation, go-to-definition, completion,
quick fixes, and Pkl CLI-backed formatting over the Language Server Protocol
(LSP).


## Installation

### Install with Cargo

Install a published release from crates.io:

```sh
cargo install lspkl --locked
lspkl --version
```

Building from Cargo requires a Rust toolchain supporting edition 2024 and a C
compiler for Tree-sitter and the bundled Pkl grammar. A fresh build also needs
HTTPS access to GitHub to fetch the pinned Pkl 0.32.1 SDK archive; its SHA-256
checksum is verified before the standard-library sources are embedded.
To build without this fetch, set `PKL_STDLIB_ARCHIVE` to a pre-fetched supported
archive (see below).

### Release binaries

Check [GitHub Releases](https://github.com/rohan-datar/lspkl/releases) for
published archives for your system.

| System | Archive target |
| --- | --- |
| Linux, x86-64 | `x86_64-unknown-linux-musl` |
| Linux, ARM64 | `aarch64-unknown-linux-musl` |
| macOS, Apple Silicon | `aarch64-apple-darwin` |
| macOS, Intel | `x86_64-apple-darwin` |
| Windows, x86-64 | `x86_64-pc-windows-msvc` |


### Install from source

```sh
cargo install --git https://github.com/rohan-datar/lspkl.git --locked lspkl
lspkl --version
```

Source builds have the same compiler and SDK requirements as registry installs.
The download URL can be overridden with `PKL_STDLIB_URL`; this changes the
source location, not the accepted checksums. For a pre-fetched archive, set
`PKL_STDLIB_ARCHIVE`:

```sh
PKL_STDLIB_URL=https://example.com/pkl-source-0.32.1.zip \
  cargo build --release --locked -p lspkl

PKL_STDLIB_ARCHIVE=/absolute/path/pkl-source-0.32.1.zip \
  cargo build --release --locked -p lspkl
```

### Build or install with Nix

Install the package into your Nix profile:

```sh
nix profile install github:rohan-datar/lspkl#lspkl
lspkl --version
```

Packages support x86-64 and ARM64 on Linux and macOS. Nix fetches the SDK as a
fixed-output dependency; the sandboxed Cargo build uses the verified local
archive rather than fetching it itself. 

## VS Code with pkl-vscode

Install the official [Pkl extension](https://github.com/apple/pkl-vscode) from
the VS Code Marketplace.

**The current upstream extension does not expose a native server-command
setting.** Its `pkl.lsp.path` setting expects a JAR, not the `lspkl` executable.
Use its existing socket override with a TCP-to-stdio bridge instead. `lspkl`
itself has no TCP-listen option.

### Connect the installed extension

On Linux/macOS, install `socat`.
Start this bridge before opening/reloading the VS Code workspace:

```sh
socat TCP-LISTEN:5007,bind=127.0.0.1,reuseaddr EXEC:/absolute/path/to/lspkl
```

Use an executable path without spaces for this command. The listener is bound
to loopback; do not expose the unauthenticated LSP connection to a network.
The bridge serves one connection and exits when it closes. Restart it before
reloading/reconnecting the editor.

Add to workspace `.vscode/settings.json`:

```json
{
  "pkl.lsp.socket.host": "127.0.0.1",
  "pkl.lsp.socket.port": 5007,
  "pkl.cli.path": "/absolute/path/to/pkl"
}
```

The socket override bypasses the extension's default server launcher. 

### Enable navigation to virtual sources

`lspkl` returns `lspkl://` URIs for stdlib, HTTPS, and archive definitions.
The stock extension's virtual-document provider does not register this scheme:
filesystem features can work through the socket, but those definition targets
will not open without a client change.

For full virtual-source navigation, build a local copy of pkl-vscode:

```sh
git clone https://github.com/apple/pkl-vscode.git
cd pkl-vscode
```

In `src/ts/extension.ts`:

1. Add `{ scheme: "lspkl", language: "pkl" }` to the language client's
   `documentSelector`.
2. Inside `registerSubscriptions`, register the existing content provider for
   the new scheme as well:

   ```ts
   context.subscriptions.push(
     vscode.workspace.registerTextDocumentContentProvider(
       "lspkl",
       new PklTextDocumentContentProvider(languageClientRef),
     ),
   );
   ```

The existing provider requests `pkl/fileContents`, which lspkl implements.
Leave the extension's other scheme registrations intact.

The local extension build requires Node/npm, the Pkl CLI, and the compiler or
container tools required by tree-sitter's WebAssembly build. Check the upstream
repository for its current build environment. For the socket-only build, these
scripts compile/package the extension without downloading its default server:

```sh
npm ci
npm run build:pkl
npm run build:code
npm run build:tree-sitter
npm run package-only
code --install-extension .dist/vscode/pkl-vscode-*.vsix --force
```

Keep the socket settings above, restart the bridge, and reload VS Code.
Disable extension auto-updates for the local build until the scheme support is
available upstream. These are source-build instructions, not a claim that the
Marketplace extension already supports native or virtual-source integration.

## Neovim with pkl-neovim

Use Neovim 0.11 or newer and the official
[pkl-neovim](https://github.com/apple/pkl-neovim) plugin. 

With lazy.nvim, add:

```lua
{
  "apple/pkl-neovim",
  ft = "pkl",
  dependencies = {
    "nvim-treesitter/nvim-treesitter",
    "L3MON4D3/LuaSnip",
  },
  init = function()
    vim.g.pkl_neovim = {
      start_command = { "lspkl" }, -- or an absolute executable path
      pkl_cli_path = "/absolute/path/to/pkl",
    }

    -- Adapt upstream's flat settings to lspkl's nested pkl section.
    vim.api.nvim_create_autocmd("LspAttach", {
      callback = function(event)
        local client = vim.lsp.get_client_by_id(event.data.client_id)
        if client and client.name == "pkl" and not client.settings.pkl then
          local cfg = vim.g.pkl_neovim
          client.settings.pkl = {
            cli = { path = cfg.pkl_cli_path },
            modulepath = cfg.pkl_modulepath or {},
            projects = {
              excludedDirectories = cfg.pkl_projects_excluded_directories or {},
            },
            formatter = { grammarVersion = cfg.pkl_formatter_grammar_version },
          }
          client:notify("workspace/didChangeConfiguration", {
            settings = client.settings,
          })
        end
      end,
    })

    -- Reuse the plugin's source RPC loader for lspkl's virtual URIs.
    vim.api.nvim_create_autocmd("BufReadCmd", {
      pattern = "lspkl://*",
      callback = function(event)
        require("pkl-neovim").open_lspfile(event.match)
      end,
    })
  end,
  config = function()
    require("luasnip.loaders.from_snipmate").lazy_load()
  end,
}
```

Restart Neovim, install the Pkl parser with `:TSInstall pkl` if needed, and
open a `.pkl` file in a Git workspace or a directory containing `PklProject`.
The plugin loads its filetype hooks and starts the server. Avoid also enabling
a second Pkl server through nvim-lspconfig, Mason, or another configuration.

The `LspAttach` adapter is needed because current pkl-neovim sends dotted
setting keys, while lspkl reads a nested `pkl` configuration section. The
`BufReadCmd` adapter enables read-only virtual buffers through the plugin's
existing `pkl/fileContents` loader; stdlib definitions should then open normally.

Verify with `:checkhealth vim.lsp` and `:lua vim.print(vim.lsp.get_clients({ name = "pkl" }))`.


## Development

```sh
nix develop
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo nextest run --workspace --locked --test-threads=1 --no-fail-fast \
  --failure-output immediate-final --status-level all
cargo test --workspace --doc --locked
```

### Releasing

The current package version is `0.1.0`. Before a release, update all nine crate
versions and the internal dependency version requirements in the workspace
`Cargo.toml` together, and update `Cargo.lock`.

Configure the repository's crates.io publishing token locally with GitHub CLI:

```sh
gh secret set CARGO_REPOSITORY_TOKEN
```

Enter the token at the prompt; never commit it. The release workflow requires
this secret, while ordinary quality CI does not. Quality CI runs
`cargo publish --workspace --locked --dry-run`.

Push a `vVERSION` tag matching the package versions (for example, `v0.1.0`) to
trigger the gated release pipeline. Merging a PR labeled `release` into `main`
is the alternative release trigger. After the quality gates, the pipeline
publishes all nine packages to crates.io, then publishes the GitHub release
archives and checksums.

Crates.io uploads are irreversible, and publishing multiple packages is not
atomic. If a release fails, identify which package versions have already been
uploaded before retrying; do not assume that nothing was published.



## License and parser provenance

Licensed under [Apache 2.0](LICENSE). See [NOTICE.txt](NOTICE.txt) and the SDK
[license](crates/pkl-stdlib/LICENSE) and [notice](crates/pkl-stdlib/NOTICE) for
applicable attributions.

`pkl-syntax` bundles Apple's
[`tree-sitter-pkl` grammar at tag `v0.21.0`](https://github.com/apple/tree-sitter-pkl/tree/v0.21.0),
including the generated C parser, scanner, and required headers. The grammar is
compiled during the build; it is not fetched from Git and does not require a
separate grammar crate or Tree-sitter CLI. Its upstream license and notice are
retained in `crates/pkl-syntax/vendor/tree-sitter-pkl/`.
