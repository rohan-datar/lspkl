use lspkl_project::cli::PklCli;
use lspkl_project::project_manager::ProjectManager;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn missing_project_entry_is_stale_even_when_state_file_is_newer() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("PklProject"), "").unwrap();
    let state = root.join(".lspkl/projects.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(
        &state,
        json!({"schemaVersion":1,"pklProjects":[]}).to_string(),
    )
    .unwrap();
    let mut manager = ProjectManager::default();
    manager.initialize(vec![root]);
    manager.load_state(&state).unwrap();
    assert!(manager.needs_sync().unwrap());
}

#[test]
fn workspace_without_projects_is_not_stale() {
    let temp = tempfile::tempdir().unwrap();
    let mut manager = ProjectManager::default();
    manager.initialize(vec![temp.path().to_owned()]);
    assert!(!manager.needs_sync().unwrap());
}

/// A manual sync must not look stale to the next automatic check just because
/// the state file is persisted later on its own thread. The sync's own rewrite
/// of PklProject.deps.json must not count as a change either.
#[cfg(unix)]
#[test]
fn just_synced_projects_are_fresh_before_persistence_until_inputs_change() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let project = root.join("PklProject");
    fs::write(&project, "amends \"pkl:Project\"\n").unwrap();
    let metadata = json!({"projectFileUri": url::Url::from_file_path(&project).unwrap().as_str(),
        "packageUri": null, "declaredDependencies": {}, "evaluatorSettings": null});
    let deps = root.join("PklProject.deps.json");
    let cli = root.join("pkl");
    // `project resolve` rewrites the deps file, like the real CLI does.
    fs::write(&cli, format!(
        "#!/bin/sh\ncase \"$1\" in\nproject) printf '%s' '{{\"schemaVersion\":1,\"resolvedDependencies\":{{}}}}' > '{}'; exit 0;;\neval) printf '%s' '{}';;\n*) exit 9;;\nesac\n",
        deps.display(), metadata)).unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    let mut manager = ProjectManager::new(PklCli {
        binary_path: Some(cli),
    });
    manager.initialize(vec![root.clone()]);
    manager.sync_projects().unwrap();
    assert!(!root.join(".lspkl/projects.json").exists());
    assert!(!manager.needs_sync().unwrap(), "fresh right after sync");
    // Ensure a distinct modification time on coarse-resolution filesystems.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(&project, "amends \"pkl:Project\"\n// edited\n").unwrap();
    assert!(
        manager.needs_sync().unwrap(),
        "an edited PklProject is stale"
    );
}
