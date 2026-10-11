//! Tests for project discovery and state persistence.
//!
//! Discovery, state persistence, and CLI orchestration use temporary fixtures.
//! Project settings retain synced state and pinned project snapshots.

use std::fs;
use std::path::{Path, PathBuf};

use pkl_project::cli::PklCli;
use pkl_project::project_manager::ProjectManager;

/// Layout:
/// ```text
/// ws/
/// ├── PklProject            (root project, depends on lib)
/// ├── app/
/// │   └── PklProject        (nested project)
/// ├── lib/
/// │   └── PklProject
/// └── .git/                 (must be skipped by discovery)
///     └── nested/PklProject
fn write_workspace(ws: &Path) {
    let mk = |rel: &str| {
        let dir = ws.join(rel);
        fs::create_dir_all(&dir).unwrap();
        dir
    };
    fs::write(mk("").join("PklProject"), "amends \"pkl:0.7.0\"\n").unwrap();
    fs::write(
        mk("app").join("PklProject"),
        "amends \"pkl:0.7.0\"\ndependencies { \"@lib\" { path = \"../lib\" } }\n",
    )
    .unwrap();
    fs::write(mk("lib").join("PklProject"), "amends \"pkl:0.7.0\"\n").unwrap();
    // Noise: must NOT be discovered.
    fs::write(
        mk(".git/nested").join("PklProject"),
        "amends \"pkl:0.7.0\"\n",
    )
    .unwrap();
}

#[test]
fn initialize_replaces_workspace_folders() {
    let first = PathBuf::from("/workspace/first");
    let second = PathBuf::from("/workspace/second");
    let replacement = PathBuf::from("/workspace/replacement");
    let mut manager = ProjectManager::default();

    manager.initialize(vec![first.clone(), second.clone()]);
    assert_eq!(manager.workspace_folders(), [first, second]);

    manager.initialize(vec![replacement.clone()]);
    assert_eq!(manager.workspace_folders(), [replacement]);
}

#[test]
fn discovery_finds_projects_and_skips_noise_directories() {
    let ws = tempfile::tempdir().unwrap();
    write_workspace(ws.path());
    fs::create_dir_all(ws.path().join(".hidden")).unwrap();
    fs::write(ws.path().join(".hidden/PklProject"), "").unwrap();
    fs::create_dir_all(ws.path().join("node_modules/pkg")).unwrap();
    fs::write(ws.path().join("node_modules/pkg/PklProject"), "").unwrap();
    fs::create_dir_all(ws.path().join("target/generated")).unwrap();
    fs::write(ws.path().join("target/generated/PklProject"), "").unwrap();

    let mut manager = ProjectManager::default();
    manager.initialize(vec![ws.path().to_path_buf()]);
    let mut discovered = manager.discover_project_files().unwrap();
    discovered.sort();

    let mut expected = vec![
        ws.path().join("PklProject"),
        ws.path().join("app/PklProject"),
        ws.path().join("lib/PklProject"),
    ];
    expected.sort();
    assert_eq!(discovered, expected);
}

#[test]
fn discovery_ignores_ordinary_files() {
    let ws = tempfile::tempdir().unwrap();
    fs::write(ws.path().join("README.md"), "workspace notes").unwrap();
    fs::create_dir_all(ws.path().join("app")).unwrap();
    fs::write(ws.path().join("app/config.pkl"), "name = \"demo\"\n").unwrap();
    fs::write(ws.path().join("app/PklProject"), "amends \"pkl:0.7.0\"\n").unwrap();

    let mut manager = ProjectManager::default();
    manager.initialize(vec![ws.path().to_path_buf()]);

    assert_eq!(
        manager.discover_project_files().unwrap(),
        [ws.path().join("app/PklProject")]
    );
}

#[test]
fn workspace_state_round_trips_through_disk() {
    let ws = tempfile::tempdir().unwrap();
    write_workspace(ws.path());
    let state_file = ws.path().join(".lspkl/projects.json");
    fs::create_dir_all(state_file.parent().unwrap()).unwrap();
    let fixture = serde_json::json!({
        "schemaVersion": 1,
        "pklProjects": [
            {
                "projectFile": "PklProject",
                "packageUri": null,
                "declaredDependencies": {},
                "resolvedDependencies": null,
                "evaluatorSettings": null
            },
            {
                "projectFile": "app/PklProject",
                "packageUri": "package://example.com/app@1.2.3",
                "declaredDependencies": {
                    "@lib": "package://example.com/lib@1.0.0"
                },
                "resolvedDependencies": {
                    "package://example.com/lib@1": {
                        "type": "remote",
                        "uri": "projectpackage://example.com/lib@1.0.0",
                        "checksums": { "sha256": "cafebabe" }
                    }
                },
                "evaluatorSettings": { "modulePath": ["lib"] }
            },
            {
                "projectFile": "lib/PklProject",
                "packageUri": null,
                "declaredDependencies": {},
                "resolvedDependencies": null,
                "evaluatorSettings": null
            }
        ]
    });
    fs::write(&state_file, serde_json::to_vec(&fixture).unwrap()).unwrap();

    let mut manager = ProjectManager::new(PklCli {
        binary_path: Some(PathBuf::from("/bin/false")),
    });
    manager.load_state(&state_file).expect("restores projects");
    assert_eq!(manager.projects().count(), 3);
    let nested_file = ws.path().join("app/src/main.pkl");
    let nested = manager
        .get_pkl_project(&nested_file)
        .expect("nested project");
    assert_eq!(nested.project_dir().unwrap(), ws.path().join("app"));
    assert_eq!(nested.metadata.declared_dependencies.len(), 1);
    assert_eq!(
        nested
            .project_deps
            .as_ref()
            .unwrap()
            .resolved_dependencies
            .len(),
        1
    );
    assert_eq!(
        manager
            .get_pkl_project(&ws.path().join("other/main.pkl"))
            .unwrap()
            .project_dir()
            .unwrap(),
        ws.path(),
        "root project must win over a sibling"
    );
    assert!(
        manager
            .get_pkl_project(&ws.path().parent().unwrap().join("outside.pkl"))
            .is_none()
    );

    manager.persist_state(&state_file).expect("writes state");
    let mut restored = ProjectManager::default();
    restored.load_state(&state_file).expect("reads state back");
    assert_eq!(restored.projects().count(), 3);
    assert_eq!(
        restored.get_pkl_project(&nested_file),
        manager.get_pkl_project(&nested_file)
    );
}

#[test]
fn load_state_rejects_unsupported_schema_without_erasing_projects() {
    let ws = tempfile::tempdir().unwrap();
    let state_file = ws.path().join("projects.json");
    fs::write(&state_file, r#"{"schemaVersion": 2, "pklProjects": []}"#).unwrap();
    let mut manager = ProjectManager::default();
    assert!(manager.load_state(&state_file).is_err());
    assert_eq!(manager.projects().count(), 0);
}

#[test]
fn sync_projects_with_no_project_files_does_not_call_cli() {
    let ws = tempfile::tempdir().unwrap();
    let mut manager = ProjectManager::new(PklCli {
        binary_path: Some(ws.path().join("missing-pkl")),
    });
    manager.initialize(vec![ws.path().to_path_buf()]);

    manager
        .sync_projects()
        .expect("empty workspace needs no CLI");
    assert_eq!(manager.projects().count(), 0);
    let state_file = ws.path().join(".lspkl/projects.json");
    let state: serde_json::Value = serde_json::from_slice(&fs::read(state_file).unwrap()).unwrap();
    assert_eq!(state["pklProjects"].as_array().unwrap().len(), 0);
}

#[cfg(unix)]
#[test]
fn sync_projects_resolves_evaluates_and_loads_metadata() {
    use std::os::unix::fs::PermissionsExt;

    let ws = tempfile::tempdir().unwrap();
    let project_file = ws.path().join("PklProject");
    fs::write(&project_file, "amends \"pkl:0.7.0\"\n").unwrap();
    // A stub `project resolve` does not create files; provide its output here.
    fs::write(
        ws.path().join("PklProject.deps.json"),
        r#"{"schemaVersion":1,"resolvedDependencies":{}}"#,
    )
    .unwrap();

    let metadata = serde_json::json!({
        "projectFileUri": url::Url::from_file_path(&project_file).unwrap().to_string(),
        "packageUri": null,
        "declaredDependencies": {},
        "evaluatorSettings": null
    });
    let calls = ws.path().join("cli-calls.txt");
    let script = ws.path().join("pkl-stub");
    let staged = ws.path().join("pkl-stub.tmp");
    let body = format!(
        "#!/bin/sh\nprintf '%s\\n' '---' \"$@\" >> {}\ncase \"$1\" in\n  project) exit 0 ;;\n  eval) printf '%s' {} ;;\n  *) exit 9 ;;\nesac\n",
        shell_quote(&calls.to_string_lossy()),
        shell_quote(&metadata.to_string()),
    );
    fs::write(&staged, body).unwrap();
    let mut permissions = fs::metadata(&staged).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&staged, permissions).unwrap();
    fs::rename(&staged, &script).unwrap();

    let mut manager = ProjectManager::new(PklCli {
        binary_path: Some(script),
    });
    manager.initialize(vec![ws.path().to_path_buf()]);
    manager.sync_projects().expect("sync succeeds");

    let calls = fs::read_to_string(calls).unwrap();
    let lines: Vec<_> = calls.lines().collect();
    let commands: Vec<&[&str]> = lines
        .split(|line| *line == "---")
        .filter(|command| !command.is_empty())
        .collect();
    assert_eq!(commands.len(), 2, "expected resolve and eval: {calls}");
    assert_eq!(
        commands[0],
        ["project", "resolve", ws.path().to_str().unwrap()]
    );
    assert_eq!(commands[1][0], "eval");
    assert!(commands[1].contains(&"-x"), "eval must request metadata");
    assert!(commands[1].contains(&"--module-output-separator"));
    assert_eq!(commands[1].last(), Some(&project_file.to_str().unwrap()));

    let project = manager
        .get_pkl_project(&ws.path().join("main.pkl"))
        .expect("project registered");
    assert_eq!(project.project_dir().unwrap(), ws.path());
    assert!(project.project_deps.is_some(), "load resolved dependencies");
    assert!(
        ws.path().join(".lspkl/projects.json").exists(),
        "persist synced state"
    );
}

#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(unix)]
#[test]
fn sync_projects_persists_each_workspace_separately() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let roots: Vec<PathBuf> = ["first", "second", "empty"]
        .iter()
        .map(|name| temp.path().join(name))
        .collect();
    for root in &roots {
        fs::create_dir_all(root).unwrap();
    }
    let project_files: Vec<PathBuf> = roots[..2]
        .iter()
        .map(|root| root.join("PklProject"))
        .collect();
    for file in &project_files {
        fs::write(file, "amends \"pkl:0.7.0\"\n").unwrap();
    }
    let objects: Vec<String> = project_files
        .iter()
        .map(|file| {
            serde_json::json!({
                "projectFileUri": url::Url::from_file_path(file).unwrap().to_string(),
                "packageUri": null,
                "declaredDependencies": {},
                "evaluatorSettings": null
            })
            .to_string()
        })
        .collect();
    let staged = temp.path().join("pkl-stub.tmp");
    let script = temp.path().join("pkl-stub");
    let body = format!(
        "#!/bin/sh\ncase \"$1\" in\n  project) exit 0 ;;\n  eval) printf '%s' {} ;;\n  *) exit 9 ;;\nesac\n",
        shell_quote(&objects.join(", ")),
    );
    fs::write(&staged, body).unwrap();
    let mut permissions = fs::metadata(&staged).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&staged, permissions).unwrap();
    fs::rename(staged, &script).unwrap();

    let mut manager = ProjectManager::new(PklCli {
        binary_path: Some(script),
    });
    manager.initialize(roots.clone());
    manager.sync_projects().expect("sync all workspace folders");
    assert_eq!(manager.projects().count(), 2);
    for (index, root) in roots.iter().enumerate() {
        let state_file = root.join(".lspkl/projects.json");
        let state: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_file).unwrap()).unwrap();
        let entries = state["pklProjects"].as_array().unwrap();
        if index < 2 {
            assert_eq!(entries.len(), 1, "state for {}", root.display());
            assert_eq!(entries[0]["projectFile"], "PklProject");
        } else {
            assert!(entries.is_empty(), "empty workspace must clear stale state");
        }
    }

    let mut restored = ProjectManager::default();
    restored.initialize(roots.clone());
    for root in &roots {
        restored
            .load_state(&root.join(".lspkl/projects.json"))
            .unwrap();
    }
    assert_eq!(
        restored.projects().count(),
        2,
        "loading one root must not erase another"
    );
}

#[test]
fn nested_workspace_owns_its_own_project_state() {
    let temp = tempfile::tempdir().unwrap();
    let outer = temp.path().to_path_buf();
    let inner = outer.join("nested");
    fs::create_dir_all(&inner).unwrap();
    let state_file = outer.join(".lspkl/projects.json");
    let fixture = serde_json::json!({
        "schemaVersion": 1,
        "pklProjects": [
            { "projectFile": "PklProject", "packageUri": null,
              "declaredDependencies": {}, "resolvedDependencies": null, "evaluatorSettings": null }
        ]
    });
    fs::create_dir_all(state_file.parent().unwrap()).unwrap();
    fs::write(&state_file, serde_json::to_vec(&fixture).unwrap()).unwrap();
    let inner_file = inner.join(".lspkl/projects.json");
    fs::create_dir_all(inner_file.parent().unwrap()).unwrap();
    fs::write(&inner_file, serde_json::to_vec(&fixture).unwrap()).unwrap();

    let mut manager = ProjectManager::default();
    manager.initialize(vec![outer.clone(), inner.clone()]);
    manager.load_state(&state_file).unwrap();
    manager.load_state(&inner_file).unwrap();
    manager.persist_state(&state_file).unwrap();
    manager.persist_state(&inner_file).unwrap();
    for file in [&state_file, &inner_file] {
        let state: serde_json::Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
        assert_eq!(state["pklProjects"].as_array().unwrap().len(), 1);
        assert_eq!(state["pklProjects"][0]["projectFile"], "PklProject");
    }
}

#[test]
fn configure_preserves_shared_projects_and_applies_exclusions() {
    let ws = tempfile::tempdir().unwrap();
    write_workspace(ws.path());
    let state = ws.path().join(".lspkl/projects.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(&state, r#"{"schemaVersion":1,"pklProjects":[{"projectFile":"PklProject","declaredDependencies":{}}]}"#).unwrap();
    let mut manager = ProjectManager::default();
    manager.initialize(vec![ws.path().to_path_buf()]);
    manager.load_state(&state).unwrap();
    let pinned = manager
        .get_pkl_project_shared(&ws.path().join("config.pkl"))
        .unwrap();
    manager.configure(
        PklCli {
            binary_path: Some(ws.path().join("different-pkl")),
        },
        vec!["app".into()],
    );
    let after = manager
        .get_pkl_project_shared(&ws.path().join("config.pkl"))
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&pinned, &after));
    assert_eq!(manager.projects().count(), 1);
    assert_eq!(
        manager.discover_project_files().unwrap(),
        [
            ws.path().join("PklProject"),
            ws.path().join("lib/PklProject")
        ]
    );
    manager.initialize(Vec::new());
    assert_eq!(
        pinned.metadata.project_file_uri,
        url::Url::from_file_path(ws.path().join("PklProject"))
            .unwrap()
            .as_str()
    );
}
