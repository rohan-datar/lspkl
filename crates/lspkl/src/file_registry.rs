//! Server-owned URI identities for semantic files, including the embedded SDK.

use std::collections::HashMap;
use std::sync::Arc;

use fluent_uri::Uri;
use pkl_types::FileId;

/// Stable identities for the server lifetime; close never erases an identity.
#[derive(Debug)]
pub struct FileRegistry {
    by_uri: HashMap<Arc<Uri<String>>, FileId>,
    /// Indexed directly by FileId, including the pre-registered base module.
    uris: Vec<Arc<Uri<String>>>,
}

impl Default for FileRegistry {
    fn default() -> Self {
        let base = Arc::new(Uri::parse("pkl:base".to_owned()).expect("static base URI"));
        Self {
            by_uri: HashMap::from([(Arc::clone(&base), FileId::BASE)]),
            uris: vec![base],
        }
    }
}

impl FileRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reuse the URI's identity or allocate the next permanent one.
    pub fn register(&mut self, uri: &Uri<String>) -> FileId {
        if let Some(&id) = self.by_uri.get(uri) {
            return id;
        }
        let id = FileId(u32::try_from(self.uris.len()).expect("too many files open"));
        let uri = Arc::new(uri.clone());
        self.uris.push(Arc::clone(&uri));
        self.by_uri.insert(uri, id);
        id
    }

    /// Resolve any registered file, without disk or live-buffer access.
    pub fn uri_for(&self, file: FileId) -> Option<&Uri<String>> {
        self.uris.get(file.0 as usize).map(Arc::as_ref)
    }
}
