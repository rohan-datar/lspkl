//! Explicit project synchronization and package download commands.

use super::{HandlerError, Result};
use crate::state::Snapshot;
use lsp_types::ExecuteCommandParams;
use pkl_project::dto::PackageUri;
use tracing::error;

pub fn sync_projects(snapshot: &Snapshot) -> Result<()> {
    snapshot
        .project_manager
        .lock()
        .sync_projects()
        .map_err(|err| {
            error!(?err, "syncProjects failed");
            HandlerError::Domain(err.to_string())
        })?;
    Ok(())
}
pub fn download_package(snapshot: &Snapshot, uri: String) -> Result<()> {
    let uri =
        PackageUri::create(&uri).map_err(|err| HandlerError::InvalidParams(err.to_string()))?;
    snapshot
        .package_manager
        .lock()
        .download_package(&uri, snapshot.settings.effective_cache_dir())
        .map_err(|err| {
            error!(?err, "downloadPackage failed");
            HandlerError::Domain(err.to_string())
        })?;
    Ok(())
}
pub fn execute_command(
    snapshot: &Snapshot,
    mut params: ExecuteCommandParams,
) -> Result<Option<serde_json::Value>> {
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
            download_package(snapshot, uri)?;
        }
        "pkl.syncProjects" => {
            if !params.arguments.is_empty() {
                return Err(HandlerError::InvalidParams(
                    "pkl.syncProjects expects no arguments".into(),
                ));
            }
            sync_projects(snapshot)?;
        }
        _ => return Err(HandlerError::InvalidParams("unknown Pkl command".into())),
    }
    Ok(None)
}
