use lspkl_project::project_manager::ProjectManager;
use serde_json::json;
use std::fs;

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
