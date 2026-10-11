//! Measure the release binary over the same real stdio transport as integration tests.

#[path = "../tests/support/mod.rs"]
mod support;

use std::error::Error;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use clap::Parser;
use serde_json::{Value, json};
use support::LspClient;

#[derive(Parser)]
#[command(about = "Measure lspkl binary size, initialization, and first analysis")]
struct Args {
    /// Binary to measure; relative paths are resolved before changing the workspace.
    #[arg(long, default_value = "target/release/lspkl")]
    binary: PathBuf,
    /// Independent process launches, with warm OS filesystem caches after the first run.
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=1000))]
    iterations: u16,
}

fn statistics(values: &mut [f64]) -> Value {
    values.sort_unstable_by(f64::total_cmp);
    let middle = values.len() / 2;
    let median = if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    };
    json!({
        "min": values[0],
        "median": median,
        "p95": values[(values.len() * 95).div_ceil(100) - 1],
        "max": values[values.len() - 1],
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let binary = args.binary.canonicalize()?;
    let binary_size = binary.metadata()?.len();
    let version = Command::new(&binary).arg("--version").output()?;
    if !version.status.success() {
        return Err("binary --version failed".into());
    }
    let version = std::str::from_utf8(&version.stdout)?.trim();
    let workspace = tempfile::tempdir()?;
    let root_uri = url::Url::from_directory_path(workspace.path())
        .map_err(|()| "workspace path is not absolute")?;
    let document_uri = pkl_vfs::path_to_uri(&workspace.path().join("main.pkl"))?;
    let source = "import \"pkl:json\"\nparser: json.Parser = new {}\ntext: String = \"hello\"\n";
    let mut initialize_ms = Vec::with_capacity(usize::from(args.iterations));
    let mut analysis_ms = Vec::with_capacity(usize::from(args.iterations));

    for _ in 0..args.iterations {
        let params = json!({
            "processId": null,
            "rootUri": root_uri.as_str(),
            "capabilities": {},
            "initializationOptions": {"pkl": {"cacheDir": workspace.path().join("cache")}},
        });
        let started = Instant::now();
        let mut client = LspClient::spawn(&binary, workspace.path()).await;
        let response = client.request("initialize", params).await;
        initialize_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        if response.get("error").is_some() || !response["result"]["capabilities"].is_object() {
            return Err(format!("initialization failed: {response}").into());
        }
        client.notify("initialized", json!({})).await;
        let params = json!({"textDocument": {
            "uri": document_uri.as_str(), "languageId": "pkl", "version": 1, "text": source,
        }});
        let started = Instant::now();
        client.notify("textDocument/didOpen", params).await;
        let diagnostics = loop {
            let notification = client.notification("textDocument/publishDiagnostics").await;
            if notification["params"]["uri"] == document_uri.as_str() {
                break notification;
            }
        };
        analysis_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        if diagnostics["params"]["diagnostics"]
            .as_array()
            .is_none_or(|diags| !diags.is_empty())
        {
            return Err(format!("benchmark source has diagnostics: {diagnostics}").into());
        }
        let hover = client
            .request(
                "textDocument/hover",
                json!({
                    "textDocument": {"uri": document_uri.as_str()},
                    "position": {"line": 2, "character": 7},
                }),
            )
            .await;
        if !hover["result"]["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains("class String"))
        {
            return Err(format!("base type did not resolve: {hover}").into());
        }
        client.shutdown().await;
    }
    let measurements = json!({
        "schemaVersion": 1,
        "binary": binary,
        "binaryVersion": version,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "binarySizeBytes": binary_size,
        "iterations": args.iterations,
        "initializeMs": statistics(&mut initialize_ms),
        "firstAnalysisMs": statistics(&mut analysis_ms),
    });
    println!("{}", serde_json::to_string_pretty(&measurements)?);
    Ok(())
}
