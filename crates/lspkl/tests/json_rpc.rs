//! End-to-end JSON-RPC smoke test.
//!
//! Wires the server's stdin/stdout to in-memory `tokio::io::duplex` streams
//! and speaks LSP protocol over them. Slower than the direct-call tests in
//! `lifecycle.rs`, but verifies the JSON-RPC framing actually works.

use std::sync::Arc;
use std::time::Duration;

use lspkl::{PklBackend, ServerState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower_lsp_server::{LspService, Server};

#[tokio::test]
async fn initialize_round_trip_via_jsonrpc() {
    // Plumb client <-> server via two in-memory pipes.
    let (mut client_tx, server_rx) = tokio::io::duplex(4096);
    let (server_tx, mut client_rx) = tokio::io::duplex(4096);

    let state = Arc::new(ServerState::new());
    let (service, socket) =
        LspService::new(move |client| PklBackend::new(Arc::clone(&state), Some(client)));

    let server_task = tokio::spawn(async move {
        Server::new(server_rx, server_tx, socket)
            .serve(service)
            .await;
    });

    // 1. initialize
    write_message(
        &mut client_tx,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
    )
    .await;

    let response = read_message(&mut client_rx).await;
    let json: serde_json::Value = serde_json::from_str(&response).expect("response must be JSON");

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);
    assert!(
        json["result"]["capabilities"].is_object(),
        "initialize response must carry a capabilities object, got: {response}"
    );

    // 2. shutdown
    write_message(
        &mut client_tx,
        r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
    )
    .await;
    let shutdown_resp = read_message(&mut client_rx).await;
    let json: serde_json::Value = serde_json::from_str(&shutdown_resp).unwrap();
    assert_eq!(json["id"], 2);

    // 3. exit notification (no id, no response expected). Tower-lsp-server's
    //    Server stops serving once `exit` arrives.
    write_message(&mut client_tx, r#"{"jsonrpc":"2.0","method":"exit"}"#).await;

    // Drop our writer so the server sees EOF if it's still polling the read
    // half for some reason.
    drop(client_tx);

    // Bound the wait so a regression in shutdown doesn't hang CI forever.
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server did not finish within 2 seconds after exit")
        .expect("server task panicked");
}

/// Write one LSP-framed message: `Content-Length: <n>\r\n\r\n<body>`.
async fn write_message<W: AsyncWriteExt + Unpin>(writer: &mut W, body: &str) {
    let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    writer.write_all(frame.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();
}

/// Read one LSP-framed message and return its JSON body as a String.
///
/// Reads byte-by-byte until `\r\n\r\n`, parses `Content-Length`, then reads
/// exactly that many bytes. Inefficient but trivially correct — fine for a
/// single smoke test.
async fn read_message<R: AsyncReadExt + Unpin>(reader: &mut R) -> String {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        reader.read_exact(&mut byte).await.expect("header read");
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let header_str = std::str::from_utf8(&header).expect("header utf8");
    let content_length: usize = header_str
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length:"))
        .map(|s| s.trim().parse().expect("Content-Length must parse"))
        .expect("Content-Length header missing");

    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).await.expect("body read");
    String::from_utf8(body).expect("body utf8")
}
