//! Synchronous fixture preparation; production callers use owned worker jobs.
use pkl_db::{IoUpdate, PendingIo, RootDatabase, import_graph};
use pkl_vfs::StandardSourceLoader;

/// Apply all reachable source/directory/routing reads, without fetching network fixtures.
pub fn preload(db: &mut RootDatabase) {
    let loader = db.source_loader_handle();
    let updates = db
        .directory_roots()
        .iter()
        .map(|root| pkl_db::DirectoryUpdate::load(&*loader, root))
        .collect();
    db.apply_directory_updates(updates);
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
