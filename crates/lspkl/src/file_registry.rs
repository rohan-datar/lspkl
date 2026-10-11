//! Server-owned URI identities for semantic `FileId`s.
//!
//! The VFS owns live file handles; this registry owns only URI-to-ID mappings.
//! Keys use the same URI equality as `FileManager`, not filesystem canonicalization.

use std::collections::HashMap;
use std::sync::Arc;

use fluent_uri::Uri;
use pkl_types::FileId;

/// Stable identities for the lifetime of one server state.
///
/// `FileId::BASE` is reserved for the separately managed standard library.
/// User IDs start at 1 and are never reused, including after document close.
#[derive(Debug, Default)]
pub struct FileRegistry {
    // One owned URI allocation shared by both lookup directions.
    by_uri: HashMap<Arc<Uri<String>>, FileId>,
    // Index `id - 1` contains that user file's URI.
    uris: Vec<Arc<Uri<String>>>,
}

impl FileRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the existing ID for this URI, or allocate its permanent identity.
    /// Registering an existing URI must not allocate another URI or change its ID.
    /// Panics if the u32 user-ID space is exhausted; never wraps into BASE.
    pub fn register(&mut self, uri: &Uri<String>) -> FileId {
        if let Some(&id) = self.by_uri.get(uri) {
            id
        } else {
            let current_id = self.uris.len() as u32;
            if current_id == u32::MAX {
                panic!("too many files open");
            }

            let id: u32 = current_id + 1;
            let uri = Arc::new(uri.clone());
            let id = FileId(id);
            self.uris.push(uri.clone());
            self.by_uri.insert(uri, id);
            id
        }
    }

    /// Look up a user file's URI without consulting disk or the live VFS.
    /// BASE and unknown IDs return None. Closing a document does not erase its URI.
    pub fn uri_for(&self, file_id: FileId) -> Option<&Uri<String>> {
        if file_id == FileId::BASE {
            return None;
        }
        let FileId(id) = file_id;
        self.uris.get((id - 1) as usize).map(|v| &**v)
    }
}
