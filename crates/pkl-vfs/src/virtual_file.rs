//! A handle to a single source file the LSP knows about.
use std::sync::{Arc, RwLock};

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

/// A single file the language server knows about.
#[derive(Debug)]
pub struct VirtualFile {
    pub url: Uri<String>,
    pub origin: Origin,
    state: RwLock<FileState>,
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
        Self { url, origin, state }
    }

    /// Snapshot of current contents. Avoid calling this if you can hold the
    /// read guard yourself; cloning the whole source on every access is
    /// wasteful.
    pub fn contents(&self) -> String {
        self.state
            .read()
            .expect("virtual file state lock poisoned")
            .contents
            .clone()
    }

    /// Current LSP document version, if tracked.
    pub fn version(&self) -> Option<i32> {
        self.state
            .read()
            .expect("virtual file state lock poisoned")
            .version
    }

    /// Replace the contents wholesale. Matches `TextDocumentSyncKind::Full`.
    /// Must invalidate the cached parse tree.
    pub fn set_contents(&self, new_contents: String, new_version: Option<i32>) {
        let mut fs = self
            .state
            .write()
            .expect("virtual file state lock poisoned");

        fs.contents = new_contents;
        fs.version = new_version;
        fs.tree = None;
    }

    /// Get the cached tree, if one is present.
    pub fn cached_tree(&self) -> Option<Arc<SyntaxTree>> {
        self.state
            .read()
            .expect("virtual file state lock poisoned")
            .tree
            .clone()
    }

    /// Parse and cache under the same lock used to replace contents. An edit
    /// cannot invalidate the cache between reading source and storing its tree.
    pub(crate) fn parsed_tree(&self) -> Result<Arc<SyntaxTree>, pkl_syntax::ParseError> {
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
