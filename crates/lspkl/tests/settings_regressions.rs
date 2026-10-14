mod support;

use lspkl::Workspace;
use serde_json::json;

#[test]
fn folder_arrival_invalidates_project_jobs_when_settings_are_equal() {
    let old = tempfile::tempdir().unwrap();
    let new = tempfile::tempdir().unwrap();
    let mut workspace = Workspace::new();
    workspace.apply_initialize(
        &serde_json::from_value(json!({
            "capabilities": {},
            "workspaceFolders": [{"uri": uri(old.path()), "name": "old"}]
        }))
        .unwrap(),
    );
    let settings = workspace.settings().clone();
    let job_epoch = workspace.snapshot().project_generation;
    workspace.apply_workspace_folders(
        serde_json::from_value(json!({"event": {
            "removed": [{"uri": uri(old.path()), "name": "old"}],
            "added": [{"uri": uri(new.path()), "name": "new"}]
        }}))
        .unwrap(),
    );
    assert_eq!(workspace.settings(), &settings);
    assert!(
        workspace.snapshot().project_generation > job_epoch,
        "a sync captured before folder arrival must be stale"
    );
    assert_eq!(
        workspace.project_manager().workspace_folders(),
        &[new.path().to_owned()]
    );
}

fn uri(path: &std::path::Path) -> String {
    pkl_vfs::path_to_uri(path).unwrap().into_string()
}

#[tokio::test]
async fn no_workspace_relative_modulepath_resolves_definition_and_completion() {
    let temp = tempfile::tempdir().unwrap();
    let root = support::canonical_root(temp.path());
    std::fs::create_dir(temp.path().join("lib")).unwrap();
    std::fs::write(temp.path().join("lib/library.pkl"), "value = 42\n").unwrap();
    let mut client = support::LspClient::spawn(
        std::path::Path::new(env!("CARGO_BIN_EXE_lspkl")),
        temp.path(),
    )
    .await;
    client
        .request(
            "initialize",
            json!({"capabilities": {},
        "initializationOptions": {"modulepath": ["lib"]}}),
        )
        .await;
    client.notify("initialized", json!({})).await;
    assert_modulepath(&mut client, &root).await;
    client.shutdown().await;
}

async fn assert_modulepath(client: &mut support::LspClient, root: &std::path::Path) {
    let main = uri(&root.join("main.pkl"));
    client
        .notify(
            "textDocument/didOpen",
            json!({"textDocument": {
        "uri": main, "languageId": "pkl", "version": 1,
        "text": "import \"modulepath:/library.pkl\" as library\nresult = library.value\n"}}),
        )
        .await;
    loop {
        let diagnostics = client.notification("textDocument/publishDiagnostics").await;
        if diagnostics["params"]["uri"] == main {
            assert_eq!(diagnostics["params"]["diagnostics"], json!([]));
            break;
        }
    }
    let definition = client
        .request(
            "textDocument/definition",
            json!({
        "textDocument": {"uri": main}, "position": {"line": 1, "character": 18}}),
        )
        .await;
    assert_eq!(
        definition["result"]["uri"],
        uri(&root.join("lib/library.pkl")),
        "{definition}"
    );
    let completion = client
        .request(
            "textDocument/completion",
            json!({
        "textDocument": {"uri": main}, "position": {"line": 0, "character": 20}}),
        )
        .await;
    assert!(
        completion.to_string().contains("library.pkl"),
        "{completion}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn null_pull_preserves_initialization_settings() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("lib")).unwrap();
    std::fs::write(temp.path().join("lib/library.pkl"), "value = 42\n").unwrap();
    let cli = temp.path().join("formatter");
    std::fs::write(&cli, "#!/bin/sh\nif [ \"$1\" = download-package ]; then printf '%s' \"$4\" > \"${0%/*}/cache-argument\"; exit 0; fi\n[ \"$1\" = format ] && [ \"$2\" = --grammar-version ] && [ \"$3\" = 2 ] || exit 1\n/bin/cat >/dev/null\nprintf 'formatted = true\\n'\nexit 11\n").unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut client = support::LspClient::spawn_manual_configuration(
        std::path::Path::new(env!("CARGO_BIN_EXE_lspkl")),
        temp.path(),
    )
    .await;
    client
        .request(
            "initialize",
            json!({"capabilities": {"workspace": {"configuration": true}},
        "workspaceFolders": [{"uri": uri(temp.path()), "name": "fixture"}],
        "initializationOptions": {"modulepath": ["lib"], "cacheDir": "cache",
            "cli": {"path": cli}, "formatter": {"grammarVersion": "2"}}}),
        )
        .await;
    client.notify("initialized", json!({})).await;
    let pull = client.notification("workspace/configuration").await;
    client
        .respond(pull["id"].clone(), json!([null, null, null, null, null]))
        .await;
    assert_modulepath(&mut client, temp.path()).await;
    let formatting = client
        .request(
            "textDocument/formatting",
            json!({
        "textDocument": {"uri": uri(&temp.path().join("main.pkl"))},
        "options": {"tabSize": 2, "insertSpaces": true}}),
        )
        .await;
    assert_eq!(
        formatting["result"][0]["newText"], "formatted = true\n",
        "{formatting}"
    );
    let download = client
        .request(
            "workspace/executeCommand",
            json!({
        "command": "pkl.downloadPackage", "arguments": ["package://example.com/library@1.0.0"]}),
        )
        .await;
    assert!(download.get("error").is_none(), "{download}");
    assert_eq!(
        std::fs::read_to_string(temp.path().join("cache-argument")).unwrap(),
        temp.path().join("cache").to_str().unwrap()
    );
    client.shutdown().await;
}

#[test]
fn pulls_use_latest_explicit_source_and_do_not_retain_older_pull_values() {
    use pkl_db::settings::WorkspaceSettings;
    let mut workspace = Workspace::new();
    workspace.apply_explicit_settings(
        WorkspaceSettings::parse(json!({
            "cli": {"path": "initial-cli"}, "modulepath": ["initial-lib"],
            "cacheDir": "initial-cache", "formatter": {"grammarVersion": "1"},
            "projects": {"excludedDirectories": ["initial-excluded"]}
        }))
        .unwrap(),
    );
    workspace
        .apply_pulled_settings(&[
            json!("pull-cli"),
            json!("2"),
            json!(["pull-lib"]),
            json!(["pull-excluded"]),
            json!("pull-cache"),
        ])
        .unwrap();
    assert_eq!(
        workspace.settings().cli_path(),
        std::path::Path::new("pull-cli")
    );
    workspace
        .apply_pulled_settings(&[
            json!(null),
            json!(null),
            json!(null),
            json!(null),
            json!(null),
        ])
        .unwrap();
    assert_eq!(
        workspace.settings().cli_path(),
        std::path::Path::new("initial-cli")
    );
    lspkl::handlers::lifecycle::did_change_configuration(
        &mut workspace,
        serde_json::from_value(json!({
            "settings": {"pkl": {"cli": {"path": "pushed-cli"}, "modulepath": ["pushed-lib"],
            "cacheDir": "pushed-cache", "formatter": {"grammarVersion": "2"},
            "projects": {"excludedDirectories": ["pushed-excluded"]}}}
        }))
        .unwrap(),
    )
    .unwrap();
    let pushed = workspace.settings().clone();
    workspace
        .apply_pulled_settings(&[
            json!(null),
            json!(null),
            json!(null),
            json!(null),
            json!(null),
        ])
        .unwrap();
    assert_eq!(workspace.settings(), &pushed);
}
