//! Owned project/package jobs; subprocesses never retain a Salsa handle.

use super::{HandlerError, Result};
use crate::state::Snapshot;
use lsp_types::ExecuteCommandParams;
use pkl_project::{
    cli::PklCli, dto::PackageUri, package_manager::PackageManager, project_manager::ProjectManager,
};
use std::path::PathBuf;

pub enum Command {
    Sync(ProjectManager),
    Download {
        cli: PklCli,
        uri: PackageUri,
        cache_dir: PathBuf,
    },
}

pub fn prepare_sync_projects(snapshot: &Snapshot) -> Command {
    Command::Sync(snapshot.project_manager.lock().clone())
}
pub fn prepare_download_package(snapshot: &Snapshot, uri: String) -> Result<Command> {
    let uri =
        PackageUri::create(&uri).map_err(|error| HandlerError::InvalidParams(error.to_string()))?;
    Ok(Command::Download {
        cli: PklCli {
            binary_path: snapshot.settings.cli.path.clone(),
        },
        uri,
        cache_dir: snapshot.settings.effective_cache_dir().to_owned(),
    })
}
pub fn prepare_execute_command(
    snapshot: &Snapshot,
    mut params: ExecuteCommandParams,
) -> Result<Command> {
    match params.command.as_str() {
        "pkl.downloadPackage" => {
            if params.arguments.len() != 1 {
                return Err(HandlerError::InvalidParams(
                    "pkl.downloadPackage expects one package URI string".into(),
                ));
            }
            let Some(serde_json::Value::String(uri)) = params.arguments.pop() else {
                return Err(HandlerError::InvalidParams(
                    "pkl.downloadPackage expects one package URI string".into(),
                ));
            };
            prepare_download_package(snapshot, uri)
        }
        "pkl.syncProjects" => {
            if !params.arguments.is_empty() {
                return Err(HandlerError::InvalidParams(
                    "pkl.syncProjects expects no arguments".into(),
                ));
            }
            Ok(prepare_sync_projects(snapshot))
        }
        _ => Err(HandlerError::InvalidParams("unknown Pkl command".into())),
    }
}
pub fn execute_command(command: Command) -> Result<Option<ProjectManager>> {
    match command {
        Command::Sync(mut manager) => {
            manager
                .sync_projects()
                .map_err(|error| HandlerError::Domain(error.to_string()))?;
            Ok(Some(manager))
        }
        Command::Download {
            cli,
            uri,
            cache_dir,
        } => {
            PackageManager::new(cli)
                .download_package(&uri, &cache_dir)
                .map_err(|error| HandlerError::Domain(error.to_string()))?;
            Ok(None)
        }
    }
}
