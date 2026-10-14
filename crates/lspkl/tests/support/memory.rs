//! In-process JSON-RPC peer over `lsp_server::Connection::memory`.
#![allow(dead_code)]

use lsp_server::{Connection, Message, Notification, Request, Response};
use lspkl::Workspace;
use serde_json::{Value, json};
use std::time::Duration;

pub const DEADLINE: Duration = Duration::from_secs(5);

pub struct Peer {
    pub connection: Connection,
    server: std::thread::JoinHandle<bool>,
    next: i32,
}
impl Peer {
    pub fn start(workspace: Workspace, params: Value) -> Self {
        let (connection, server_connection) = Connection::memory();
        let server =
            std::thread::spawn(move || lspkl::server::run(server_connection, workspace).unwrap());
        let mut peer = Self {
            connection,
            server,
            next: 1,
        };
        let response = peer.request("initialize", params);
        assert!(response.get("error").is_none(), "{response}");
        peer.notify("initialized", json!({}));
        peer
    }
    pub fn notify(&self, method: &str, params: Value) {
        self.connection
            .sender
            .send(Notification::new(method.into(), params).into())
            .unwrap();
    }
    pub fn start_request(&mut self, method: &str, params: Value) -> i32 {
        let id = self.next;
        self.next += 1;
        self.connection
            .sender
            .send(Request::new(id.into(), method.into(), params).into())
            .unwrap();
        id
    }
    /// Next server request/notification/response; server-to-client requests are
    /// answered like a client without settings (`null` configuration sections).
    pub fn message(&self) -> Message {
        loop {
            let message = self
                .connection
                .receiver
                .recv_timeout(DEADLINE)
                .expect("server response deadline");
            if let Message::Request(request) = message {
                let result = if request.method == "workspace/configuration" {
                    json!([null, null, null, null, null])
                } else {
                    Value::Null
                };
                self.connection
                    .sender
                    .send(Response::new_ok(request.id, result).into())
                    .unwrap();
            } else {
                return message;
            }
        }
    }
    pub fn response(&self, id: i32) -> Value {
        loop {
            if let Message::Response(response) = self.message()
                && response.id == id.into()
            {
                return serde_json::to_value(response).unwrap();
            }
        }
    }
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.start_request(method, params);
        self.response(id)
    }
    pub fn open(&self, uri: &str, version: i32, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"pkl","version":version,"text":text}}),
        );
    }
    pub fn stop(mut self) {
        assert!(self.request("shutdown", Value::Null).get("error").is_none());
        self.notify("exit", Value::Null);
        assert!(self.server.join().unwrap());
    }
    /// Shut down and exit, returning every message the server sent afterwards.
    pub fn stop_collecting(self) -> Vec<Message> {
        let shutdown = Request::new(i32::MAX.into(), "shutdown".into(), Value::Null);
        self.connection.sender.send(shutdown.into()).unwrap();
        self.notify("exit", Value::Null);
        assert!(self.server.join().unwrap(), "exit after shutdown succeeds");
        self.connection.receiver.try_iter().collect()
    }
}
