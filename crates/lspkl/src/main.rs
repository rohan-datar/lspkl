//! lspkl binary entry point; stdout is reserved for protocol traffic.
use clap::Parser;
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Debug, Parser)]
#[command(name = "lspkl", version, about = "Pkl language server")]
struct Cli {
    /// Verbose logging (info → debug).
    #[arg(short, long)]
    verbose: bool,
}
fn main() {
    let cli = Cli::parse();
    let default_level = if cli.verbose { "debug" } else { "info" };
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level)),
        )
        .with_writer(std::io::stderr)
        .with_target(true)
        .init();
    let workspace = lspkl::Workspace::new();
    let (connection, threads) = lsp_server::Connection::stdio();
    let success = match lspkl::server::run(connection, workspace) {
        Ok(success) => success,
        Err(error) => {
            tracing::error!(%error, "transport failed");
            false
        }
    };
    if let Err(error) = threads.join() {
        tracing::error!(%error, "transport I/O failed");
        std::process::exit(1);
    }
    if !success {
        std::process::exit(1);
    }
}
