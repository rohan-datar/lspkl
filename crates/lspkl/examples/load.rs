//! Stdio load scenario; the existing `measure` example remains unchanged.
#[path = "../tests/support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use serde_json::{Value, json};
use support::{LspClient, PendingRequest};
use tokio::time::{Instant, sleep_until};

#[derive(Parser)]
#[command(about = "Measure diagnostics and interactive requests under scheduled edit bursts")]
struct Args {
    #[arg(long, default_value = "target/release/lspkl")]
    binary: PathBuf,
    /// Modules in a cross-import ring, each also importing pkl:json.
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(2..=10000))]
    modules: u32,
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=10000))]
    open: u32,
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=10000))]
    bursts: u32,
    /// Changes per burst, distributed round-robin over open documents.
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=10000))]
    changes: u32,
    /// Burst start spacing; absolute deadlines do not drift with response latency.
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..))]
    interval_ms: u32,
    /// Silence required after all responses and final-version diagnostics arrive.
    #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u32).range(1..))]
    quiet_ms: u32,
    /// Hard limit for draining after the edit/request schedule has been sent.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u32).range(1..))]
    timeout_seconds: u32,
}

fn source(index: u32, modules: u32, version: i64) -> String {
    format!(
        "import \"pkl:json\"\nimport \"module{}.pkl\" as neighbor\nparser: json.Parser = new {{}}\ntext: String = \"edit {version}\"\nlinked: String = neighbor.text\n",
        (index + 1) % modules
    )
}

fn stats(values: &mut [f64]) -> Value {
    if values.is_empty() {
        return json!({"samples":0,"p50":null,"p95":null,"max":null});
    }
    values.sort_unstable_by(f64::total_cmp);
    json!({"samples": values.len(), "p50": values[(values.len() - 1) / 2],
        "p95": values[(values.len() * 95).div_ceil(100) - 1], "max": values[values.len() - 1]})
}

fn valid_response(response: &Value, hover: bool) -> bool {
    if response.get("error").is_some() {
        return false;
    }
    if hover {
        response["result"]["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains("class String"))
    } else {
        response["result"]
            .as_array()
            .or_else(|| response["result"]["items"].as_array())
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item["label"]
                        .as_str()
                        .is_some_and(|label| label.contains("json"))
                })
            })
    }
}

#[derive(Default)]
struct Document {
    sent: Vec<(Instant, i64)>,
    publishes: u64,
    stale: u64,
    unversioned: u64,
    final_seen: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    if args.open > args.modules
        || u64::from(args.bursts) * u64::from(args.changes) < u64::from(args.open)
    {
        return Err("require open <= modules and bursts * changes >= open".into());
    }
    let binary = args.binary.canonicalize()?;
    let workspace = tempfile::tempdir()?;
    let mut uris = Vec::new();
    for index in 0..args.modules {
        let path = workspace.path().join(format!("module{index}.pkl"));
        std::fs::write(&path, source(index, args.modules, 1))?;
        uris.push(lspkl_vfs::path_to_uri(&path)?.to_string());
    }
    let mut client =
        LspClient::spawn_with_stderr_limit(&binary, workspace.path(), usize::MAX).await;
    let initialized = client.request("initialize", json!({
        "processId":null, "rootUri":url::Url::from_directory_path(workspace.path()).unwrap().as_str(),
        "capabilities":{}, "initializationOptions":{"pkl":{"cacheDir":workspace.path().join("cache")}}
    })).await;
    if initialized.get("error").is_some() || !initialized["result"]["capabilities"].is_object() {
        return Err(format!("initialization failed: {initialized}").into());
    }
    client.notify("initialized", json!({})).await;
    let sender = client.sender();
    let mut documents = BTreeMap::<String, Document>::new();
    for index in 0..args.open {
        let uri = &uris[index as usize];
        let sent = sender
            .notify(
                "textDocument/didOpen",
                json!({"textDocument":{
                    "uri":uri,"languageId":"pkl","version":1,"text":source(index,args.modules,1)
                }}),
            )
            .await;
        documents
            .entry(uri.clone())
            .or_default()
            .sent
            .push((sent, 1));
    }
    // Only writes and absolute-deadline sleeps occur here. Responses are captured
    // by the background reader even though consumption happens after scheduling.
    let started = Instant::now();
    let mut versions = vec![1_i64; args.open as usize];
    let mut pending = BTreeMap::<u64, (PendingRequest, bool)>::new();
    let mut schedule_lag = Vec::new();
    let mut last_change = started;
    for burst in 0..args.bursts {
        let deadline =
            started + Duration::from_millis(u64::from(burst) * u64::from(args.interval_ms));
        sleep_until(deadline).await;
        schedule_lag.push(Instant::now().duration_since(deadline).as_secs_f64() * 1000.0);
        for change in 0..args.changes {
            let index = ((u64::from(burst) * u64::from(args.changes) + u64::from(change))
                % u64::from(args.open)) as usize;
            versions[index] += 1;
            let uri = &uris[index];
            last_change = sender.notify("textDocument/didChange", json!({
                "textDocument":{"uri":uri,"version":versions[index]},
                "contentChanges":[{"text":source(index as u32,args.modules,versions[index])}]
            })).await;
            documents
                .get_mut(uri)
                .unwrap()
                .sent
                .push((last_change, versions[index]));
            for (method, hover, line, character) in [
                ("textDocument/hover", true, 3, 7),
                ("textDocument/completion", false, 0, 14),
            ] {
                let request = sender
                    .start_request(
                        method,
                        json!({"textDocument":{"uri":uri},
                    "position":{"line":line,"character":character}}),
                    )
                    .await;
                pending.insert(request.id, (request, hover));
            }
        }
    }
    // A request is not a notification-completion barrier in LSP. Require final
    // diagnostics for every open document too, then drain through a quiet period.
    let barrier = sender
        .start_request(
            "textDocument/hover",
            json!({
                "textDocument":{"uri":uris[0]},"position":{"line":3,"character":7}
            }),
        )
        .await;
    let mut barrier_seen = false;
    let quiet = Duration::from_millis(u64::from(args.quiet_ms));
    let stop = Instant::now() + Duration::from_secs(u64::from(args.timeout_seconds));
    let mut quiet_deadline = Instant::now() + quiet;
    let mut final_publish = None::<Instant>;
    let mut hover_ms = Vec::new();
    let mut completion_ms = Vec::new();
    let mut hover_cancelled = 0_u64;
    let mut completion_cancelled = 0_u64;
    loop {
        // Ready receives (including queued messages) can beat an expired Tokio
        // timeout. Enforce the absolute limit independently of queue readiness.
        if Instant::now() >= stop {
            return Err("scenario diagnostics/response drain exceeded --timeout-seconds".into());
        }
        let complete =
            barrier_seen && pending.is_empty() && documents.values().all(|doc| doc.final_seen);
        let deadline = if complete {
            quiet_deadline.min(stop)
        } else {
            stop
        };
        let Some(observed) = client.observe_until(deadline).await else {
            if complete && quiet_deadline <= stop {
                break;
            }
            return Err(format!("scenario timed out: {} requests pending, barrier={barrier_seen}, final diagnostics={}/{}",
                pending.len(), documents.values().filter(|doc| doc.final_seen).count(), args.open).into());
        };
        quiet_deadline = quiet_deadline.max(observed.received_at + quiet);
        let message = observed.message;
        if message["method"] == "textDocument/publishDiagnostics" {
            let params = &message["params"];
            if let Some(doc) = params["uri"]
                .as_str()
                .and_then(|uri| documents.get_mut(uri))
            {
                doc.publishes += 1;
                if observed.received_at >= last_change {
                    final_publish = Some(
                        final_publish
                            .map_or(observed.received_at, |old| old.max(observed.received_at)),
                    );
                }
                if let Some(version) = params["version"].as_i64() {
                    let latest_sent = doc
                        .sent
                        .iter()
                        .rev()
                        .find(|(sent, _)| *sent <= observed.received_at)
                        .map(|(_, version)| *version);
                    if latest_sent.is_some_and(|sent| version < sent) {
                        doc.stale += 1;
                    }
                    if version == doc.sent.last().unwrap().1 {
                        if params["diagnostics"]
                            .as_array()
                            .is_none_or(|items| !items.is_empty())
                        {
                            return Err(format!("final source has diagnostics: {message}").into());
                        }
                        doc.final_seen = true;
                    }
                } else {
                    doc.unversioned += 1;
                }
            }
        } else if let Some(id) = message["id"].as_u64() {
            if id == barrier.id {
                if !valid_response(&message, true) {
                    return Err(format!("invalid barrier hover: {message}").into());
                }
                barrier_seen = true;
            } else if let Some((request, hover)) = pending.remove(&id) {
                if message["error"]["code"] == -32801 {
                    if hover {
                        hover_cancelled += 1;
                    } else {
                        completion_cancelled += 1;
                    }
                } else if !valid_response(&message, hover) {
                    return Err(
                        format!("request did not resolve meaningful result: {message}").into(),
                    );
                }
                let ms = observed
                    .received_at
                    .saturating_duration_since(request.sent_at)
                    .as_secs_f64()
                    * 1000.0;
                if hover {
                    hover_ms.push(ms);
                } else {
                    completion_ms.push(ms);
                }
            }
        }
    }
    let per_document: Vec<_> = documents
        .iter()
        .map(|(uri, doc)| {
            json!({
                "document":uri.rsplit('/').next().unwrap(), "publishes":doc.publishes,
                "stalePublishes":doc.stale,"unversionedPublishes":doc.unversioned,
                "finalSentVersion":doc.sent.last().unwrap().1
            })
        })
        .collect();
    let trace = client.stderr();
    client.shutdown().await;
    let mut cancellation_stages = BTreeMap::<String, u64>::new();
    let mut setter_ms = Vec::new();
    let mut ordering_violations: Option<u64> = None;
    for line in trace.lines() {
        if line.contains("request_cancelled")
            && let Some(stage) = line
                .split("stage=\"")
                .nth(1)
                .and_then(|s| s.split('"').next())
        {
            *cancellation_stages.entry(stage.to_owned()).or_default() += 1;
        }
        if line.contains("compute_end")
            && line.contains("stage=\"salsa_write\"")
            && let Some(us) = line
                .split("compute_us=")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<f64>().ok())
        {
            setter_ms.push(us / 1000.0);
        }
        if let Some(count) = line
            .split("ordering_violations=")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<u64>().ok())
        {
            ordering_violations = Some(ordering_violations.unwrap_or(0).max(count));
        }
    }
    let output = json!({
        "schemaVersion":1,"binary":binary,"modules":args.modules,"open":args.open,
        "bursts":args.bursts,"changesPerBurst":args.changes,"intervalMs":args.interval_ms,"quietMs":args.quiet_ms,
        "hoverMs":stats(&mut hover_ms),"completionMs":stats(&mut completion_ms),
        "cancellations":{"hover":hover_cancelled,"completion":completion_cancelled},
        "serverCancellations":cancellation_stages,
        "runtimeSetterMs":stats(&mut setter_ms),
        "orderingViolations":ordering_violations,
        "burstScheduleLagMs":stats(&mut schedule_lag),
        "lastChangeToFinalDiagnosticsMs":final_publish.ok_or("no publish after last change")?.saturating_duration_since(last_change).as_secs_f64()*1000.0,
        "publishes":per_document,
        "metricDefinitions":{
            "latency":"request frame flush to response frame receipt, including ContentModified; nearest-rank p50/p95; barrier excluded",
            "stale":"publish version < latest document version whose frame was flushed by publish receipt",
            "finalDiagnostics":"last change frame flush to last observed publish for an open document, including stale publishes",
            "publishWindow":"all didOpen/didChange publishes through final-version diagnostics, request barrier and quiet period",
            "runtimeSetterMs":"synchronous input mutation duration on the runtime, including Salsa reader cancellation/wait; upper bound on reader wait alone; requires RUST_LOG=lspkl::perf=debug",
            "serverCancellations":"server trace counts by compute stage, including dropped diagnostics; requires perf tracing"
        },
        "limitations":["quiet period is empirical, not a transport guarantee of no future work",
            "write backpressure and client scheduling can delay bursts; schedule lag is reported",
            "timestamps are client-side frame observations, not server compute times",
            "unversioned diagnostics cannot be classified as stale; final version is required",
            "perf trace capture is unbounded in this measurement harness"]
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
