//! Exercise the unpacked binary before publishing a release archive.

#[path = "../tests/support/mod.rs"]
mod support;

use std::error::Error;
use std::path::PathBuf;

use clap::Parser;
use serde_json::json;
use support::LspClient;

#[derive(Parser)]
#[command(about = "Smoke-test a release executable over real stdio")]
struct Args {
    #[arg(long)]
    binary: PathBuf,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let binary = Args::parse().binary.canonicalize()?;
    let workspace = tempfile::tempdir()?;
    let mut client = LspClient::spawn(&binary, workspace.path()).await;
    let initialized = client
        .request(
            "initialize",
            json!({
                "processId": null, "rootUri": null, "capabilities": {},
            }),
        )
        .await;
    if initialized.get("error").is_some() {
        return Err(format!("initialize failed: {initialized}").into());
    }
    client.notify("initialized", json!({})).await;
    let document = "untitled:release-smoke";
    client.notify("textDocument/didOpen", json!({"textDocument": {
        "uri": document, "languageId": "pkl", "version": 1,
        "text": "import \"pkl:json\"\nparser: json.Parser = new {}\ntext: String = \"hello\"\n",
    }})).await;
    let diagnostics = loop {
        let message = client.notification("textDocument/publishDiagnostics").await;
        if message["params"]["uri"] == document {
            break message;
        }
    };
    if diagnostics["params"]["diagnostics"]
        .as_array()
        .is_none_or(|items| !items.is_empty())
    {
        return Err(format!("valid release fixture has diagnostics: {diagnostics}").into());
    }
    let definition = client
        .request(
            "textDocument/definition",
            json!({
                "textDocument": {"uri": document}, "position": {"line": 2, "character": 7},
            }),
        )
        .await;
    let target = definition["result"]["uri"]
        .as_str()
        .ok_or("String definition is missing")?;
    let contents = client
        .request("pkl/fileContents", json!({"uri": target}))
        .await;
    let source = contents["result"]
        .as_str()
        .ok_or("stdlib source could not be opened")?;
    let start = definition["result"]["range"]["start"]["line"]
        .as_u64()
        .ok_or("definition start line is missing")? as usize;
    let end = definition["result"]["range"]["end"]["line"]
        .as_u64()
        .ok_or("definition end line is missing")? as usize;
    if end < start
        || !source
            .lines()
            .skip(start)
            .take(end - start + 1)
            .any(|line| line.contains("class String"))
    {
        return Err("definition range does not point to String in its actual source".into());
    }
    client.shutdown().await;
    println!("PASS: stdio analysis, stdlib definition/source, shutdown and exit");
    Ok(())
}
