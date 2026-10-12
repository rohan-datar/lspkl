//! A handle to a single source file the LSP knows about.
use std::sync::{Arc, Mutex, RwLock};

use fluent_uri::Uri;
use pkl_syntax::ast::SyntaxTree;

/// Where the contents of a file came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// On-disk file accessible via `path`.
    Fs { url: Uri<String> },

    /// Part of the Stdlib
    Stdlib,

    /// Remote file retrieved over http
    Https,

    /// Part of a .jar
    Jar,

    /// In-memory file, not backed by disk. Used for snippet evaluation and
    /// pre-save buffers.
    Ephemeral,
}

/// Mutable per-file state. Held behind an `RwLock` so the `FileManager` can
/// hand out `Arc<VirtualFile>` and still let multiple readers see fresh
/// contents.
#[derive(Debug)]
pub struct FileState {
    /// Current source contents.
    pub contents: String,

    /// LSP document version, if the client tracks one. `None` for files that
    /// have never been opened by the editor.
    pub version: Option<i32>,

    /// Cached parse result. `None` until first parse, or after invalidation.
    /// Wrapped in `Arc` so handlers can keep using it after the file's contents
    /// are mutated by a later `didChange`.
    pub tree: Option<Arc<SyntaxTree>>,
}

#[derive(Debug)]
struct ReadOnlySource {
    contents: &'static str,
    tree: Mutex<Option<Arc<SyntaxTree>>>,
}

/// A single file the language server knows about.
#[derive(Debug)]
pub struct VirtualFile {
    pub url: Uri<String>,
    pub origin: Origin,
    state: RwLock<FileState>,
    read_only_source: Option<Arc<ReadOnlySource>>,
}

impl VirtualFile {
    /// Build a new in-memory file. Used both for `didOpen` and for ephemeral
    /// snippets that never touch disk.
    pub fn new(url: Uri<String>, origin: Origin, contents: String, version: Option<i32>) -> Self {
        let state = RwLock::new(FileState {
            contents,
            version,
            tree: None,
        });
        Self {
            url,
            origin,
            state,
            read_only_source: None,
        }
    }

    pub(crate) fn from_embedded_source(url: Uri<String>, contents: &'static str) -> Self {
        let mut file = Self::new(url, Origin::Stdlib, String::new(), None);
        file.read_only_source = Some(Arc::new(ReadOnlySource {
            contents,
            tree: Mutex::new(None),
        }));
        file
    }

    /// Capture a new editor epoch without taking the shared parse-cache lock.
    /// Immutable SDK text and its tree remain authoritative across versions.
    pub fn editor_epoch(&self, contents: String, version: i32) -> Self {
        let contents = if self.read_only_source.is_some() {
            String::new()
        } else {
            contents
        };
        let mut file = Self::new(
            self.url.clone(),
            self.origin.clone(),
            contents,
            Some(version),
        );
        file.read_only_source = self.read_only_source.clone();
        file
    }

    /// Snapshot of the authoritative source contents.
    pub fn contents(&self) -> String {
        if let Some(source) = &self.read_only_source {
            return source.contents.to_owned();
        }
        self.state
            .read()
            .expect("virtual file state lock poisoned")
            .contents
            .clone()
    }

    pub(crate) fn is_read_only(&self) -> bool {
        self.read_only_source.is_some()
    }

    /// Current LSP document version, if tracked.
    pub fn version(&self) -> Option<i32> {
        self.state
            .read()
            .expect("virtual file state lock poisoned")
            .version
    }

    /// Update the editor version, replacing and invalidating mutable source only.
    pub fn set_contents(&self, new_contents: String, new_version: Option<i32>) {
        let mut fs = self
            .state
            .write()
            .expect("virtual file state lock poisoned");

        if self.read_only_source.is_none() {
            fs.contents = new_contents;
            fs.tree = None;
        }
        fs.version = new_version;
    }

    /// Get the cached tree, if one is present.
    pub fn cached_tree(&self) -> Option<Arc<SyntaxTree>> {
        if let Some(source) = &self.read_only_source {
            return source
                .tree
                .lock()
                .expect("read-only parse lock poisoned")
                .clone();
        }
        self.state
            .read()
            .expect("virtual file state lock poisoned")
            .tree
            .clone()
    }

    /// Parse and cache under the same lock used to replace contents. An edit
    /// cannot invalidate the cache between reading source and storing its tree.
    pub fn parsed_tree(&self) -> Result<Arc<SyntaxTree>, pkl_syntax::ParseError> {
        if let Some(source) = &self.read_only_source {
            let mut tree = source.tree.lock().expect("read-only parse lock poisoned");
            if let Some(tree) = &*tree {
                return Ok(Arc::clone(tree));
            }
            let parsed = Arc::new(pkl_syntax::parse(source.contents)?);
            *tree = Some(Arc::clone(&parsed));
            return Ok(parsed);
        }
        if let Some(tree) = self.cached_tree() {
            return Ok(tree);
        }
        let mut state = self
            .state
            .write()
            .expect("virtual file state lock poisoned");
        if let Some(tree) = &state.tree {
            return Ok(Arc::clone(tree));
        }
        let tree = Arc::new(pkl_syntax::parse(&state.contents)?);
        state.tree = Some(Arc::clone(&tree));
        Ok(tree)
    }
}
