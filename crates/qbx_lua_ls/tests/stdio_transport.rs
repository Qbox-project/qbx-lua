use std::path::PathBuf;
use std::time::Duration;

use crossbeam_channel::bounded;
use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::Url;
use serde_json::json;

const FILES: usize = 300;

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        if let (Ok(root), Ok(temp)) = (self.0.canonicalize(), std::env::temp_dir().canonicalize()) {
            if root.parent() == Some(temp.as_path()) {
                let _ = std::fs::remove_dir_all(root);
            }
        }
    }
}

fn fixture() -> Fixture {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let fixture = Fixture(std::env::temp_dir().join(format!("qbx-stdio-{}-{nanos}", std::process::id())));
    let client = fixture.0.join("shop/client");
    std::fs::create_dir_all(&client).unwrap();
    std::fs::write(fixture.0.join("shop/fxmanifest.lua"), "fx_version 'cerulean'\nclient_script 'client/*.lua'\n")
        .unwrap();
    for i in 0..FILES {
        std::fs::write(client.join(format!("file{i}.lua")), "print(undefinedThing)\n").unwrap();
    }
    fixture
}

fn request(id: i32, method: &str, params: serde_json::Value) -> Message {
    Message::Request(Request { id: RequestId::from(id), method: method.into(), params })
}

/// Channels without capacity, as `lsp_server`'s stdio transport uses, always look empty to the server.
#[test]
fn requests_are_answered_while_closed_files_are_still_being_checked() {
    let fixture = fixture();
    let (to_server, server_inbox) = bounded::<Message>(0);
    let (to_client, client_inbox) = bounded::<Message>(0);
    let server_side = Connection { sender: to_client, receiver: server_inbox };
    let server = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || qbx_lua_ls::server::run_connection(server_side).expect("server failed"))
        .unwrap();
    let (received, inbox) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        for message in client_inbox {
            if received.send(message).is_err() {
                break;
            }
        }
    });

    let root = Url::from_file_path(&fixture.0).unwrap();
    let workspace = json!([{ "uri": root, "name": "fixture" }]);
    let initialize = json!({ "processId": null, "rootUri": root, "capabilities": {}, "workspaceFolders": workspace });
    to_server.send(request(1, "initialize", initialize)).unwrap();
    inbox.recv_timeout(Duration::from_secs(30)).unwrap();
    to_server.send(Message::Notification(Notification { method: "initialized".into(), params: json!({}) })).unwrap();

    let mut checked_before_answer = None;
    let mut checked = 0;
    while checked < FILES || checked_before_answer.is_none() {
        match inbox.recv_timeout(Duration::from_secs(60)).expect("the server stopped checking closed files") {
            Message::Response(response) if response.id == RequestId::from(2) => checked_before_answer = Some(checked),
            Message::Notification(n) if n.method == "textDocument/publishDiagnostics" => {
                checked += usize::from(n.params["diagnostics"].as_array().is_some_and(|d| !d.is_empty()));
                if checked == 1 {
                    to_server.send(request(2, "qbx/status", json!(null))).unwrap();
                }
            }
            _ => {}
        }
    }
    let checked_before_answer = checked_before_answer.expect("the status request was answered");
    assert!(checked_before_answer < FILES, "the request waited for all {FILES} closed files to be checked");

    to_server.send(request(3, "shutdown", json!(null))).unwrap();
    to_server.send(Message::Notification(Notification { method: "exit".into(), params: json!(null) })).unwrap();
    server.join().unwrap();
}
