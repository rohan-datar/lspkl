# lspkl

A native Rust language server for the [Pkl configuration language](https://pkl-lang.org).
It provides diagnostics, hover documentation, go-to-definition, completion,
quick fixes, and Pkl CLI-backed formatting over the Language Server Protocol
(LSP).


## Installation

### Release binaries

Download the archive for your system from
[GitHub Releases](https://github.com/rohan-datar/lspkl/releases). 

| System | Archive target |
| --- | --- |
| Linux, x86-64 | `x86_64-unknown-linux-musl` |
| Linux, ARM64 | `aarch64-unknown-linux-musl` |
| macOS, Apple Silicon | `aarch64-apple-darwin` |
| macOS, Intel | `x86_64-apple-darwin` |
| Windows, x86-64 | `x86_64-pc-windows-msvc` |


### Install with Cargo

```sh
cargo install --git https://github.com/rohan-datar/lspkl.git --locked lspkl
lspkl --version
```

A fresh build downloads and verifies the pinned SDK archive from GitHub. The source can be overridden by setting `PKL_STDLIB_URL`; for a pre-fetched archive,
set `PKL_STDLIB_ARCHIVE`:

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
cargo test --workspace --locked -- --test-threads=1
```

CI runs native tests on Linux, macOS, and Windows. 


Licensed under [Apache 2.0](LICENSE). See [NOTICE.txt](NOTICE.txt) and the SDK
license/notice files for applicable attributions.
