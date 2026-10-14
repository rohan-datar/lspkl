//! Synchronous fixture preparation; production callers use owned worker jobs.
use lspkl_db::{IoUpdate, PendingIo, RootDatabase, import_graph};
use lspkl_vfs::StandardSourceLoader;

/// Apply all reachable source/directory/routing reads, without fetching network fixtures.
pub fn preload(db: &mut RootDatabase) {
    let loader = db.source_loader_handle();
    let updates = db
        .directory_roots()
        .iter()
        .map(|root| lspkl_db::DirectoryUpdate::load(&*loader, root))
        .collect();
    db.apply_directory_updates(updates);
    settle_inputs(db);
}

/// Apply missing inputs until every known file's import graph is settled.
pub fn settle_inputs(db: &mut RootDatabase) {
    let loader = db.source_loader_handle();
    loop {
        for file in db.all_files() {
            let _ = import_graph(db, file);
        }
        let jobs = db.pending_io();
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            let update = match job {
                PendingIo::Source { uri, cache_dir } => IoUpdate::Source {
                    text: loader
                        .load(&uri, &cache_dir)
                        .ok()
                        .map(|source| source.text.clone()),
                    uri,
                },
                job => job.load(&*loader, &StandardSourceLoader::new()),
            };
            db.apply_io(update).expect("fixture input should apply");
        }
    }
}

/// Absolute in-memory fixture URI; Windows file URIs need a drive to map to paths.
#[allow(
    dead_code,
    reason = "used by semantic crate harnesses, not every lspkl-db test"
)]
pub fn file_uri(path: &str) -> fluent_uri::Uri<String> {
    let root = if cfg!(windows) {
        "file:///C:/"
    } else {
        "file:///"
    };
    fluent_uri::Uri::parse(format!("{root}{path}")).unwrap()
}
