//! lspkl binary entry point.
use std::sync::Arc;

use clap::Parser;
use lspkl::{PklBackend, ServerState};
use tower_lsp_server::{LspService, Server};
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Debug, Parser)]
#[command(name = "lspkl", version, about = "Pkl language server")]
struct Cli {
    /// Verbose logging (info → debug).
    #[arg(short, long)]
    verbose: bool,
}

fn init_tracing(verbose: bool) {
    let default_level = if verbose { "debug" } else { "info" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(true)
        .init();
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let state = Arc::new(ServerState::new());
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    // Custom `pkl/*` RPC methods
    let (service, socket) = LspService::build(|client| PklBackend::new(state, Some(client)))
        .custom_method("pkl/syncProjects", PklBackend::sync_projects)
        .custom_method("pkl/downloadPackage", PklBackend::download_package)
        .finish();
    Server::new(stdin, stdout, socket).serve(service).await;
}
