#![allow(dead_code)]
use fluent_uri::Uri;
use pkl_db::{Db, File, FileStatus, RootDatabase};
use pkl_types::{AnalysisContext, BaseModule};
use pkl_vfs::{FileError, Source, SourceLoader};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::Path,
    sync::{Arc, RwLock},
};
use std::{fs, path::PathBuf};

#[derive(Default)]
struct TestSources(RwLock<BTreeMap<String, Arc<Source>>>);
impl SourceLoader for TestSources {
    fn load(&self, uri: &Uri<String>, _cache: &Path) -> Result<Arc<Source>, FileError> {
        if let Some(source) = self.0.read().unwrap().get(uri.as_str()) {
            return Ok(source.clone());
        }
        Err(FileError::InvalidSource(uri.to_string()))
    }
    fn list_files(&self, root: &Uri<String>) -> Result<Vec<Uri<String>>, FileError> {
        Ok(self
            .0
            .read()
            .unwrap()
            .values()
            .filter(|source| source.url.as_str().starts_with(root.as_str()))
            .map(|source| source.url.clone())
            .collect())
    }
}
pub struct TestBase {
    pub db: RefCell<RootDatabase>,
    base: BaseModule,
    sources: Arc<TestSources>,
}
impl std::ops::Deref for TestBase {
    type Target = BaseModule;
    fn deref(&self) -> &BaseModule {
        &self.base
    }
}
impl TestBase {
    pub fn add(&self, name: u32, text: &str) -> File {
        self.add_path(&format!("{name}.pkl"), text)
    }
    pub fn add_path(&self, name: &str, text: &str) -> File {
        let url = Uri::parse(format!("file:///{name}")).unwrap();
        let text: Arc<str> = Arc::from(text);
        self.sources.0.write().unwrap().insert(
            url.to_string(),
            Arc::new(Source {
                url: url.clone(),
                text: text.clone(),
            }),
        );
        self.db
            .borrow_mut()
            .upsert_file(&url, text, None, FileStatus::Present)
            .unwrap()
    }
}
pub fn uri(name: u32) -> Uri<String> {
    Uri::parse(format!("file:///{name}.pkl")).unwrap()
}
pub fn file(ctx: &AnalysisContext, name: u32) -> File {
    ctx.db.file(&uri(name)).unwrap()
}
pub fn load_test_base() -> TestBase {
    let sources = Arc::new(TestSources::default());
    let db = RootDatabase::new(sources.clone(), Default::default());
    let file = db
        .file(&Uri::parse("pkl:base".to_owned()).unwrap())
        .unwrap();
    let tree = pkl_db::parse(&db, file).as_ref().unwrap().clone();
    let base = BaseModule::from_tree(file, tree).unwrap();
    TestBase {
        db: RefCell::new(db),
        base,
        sources,
    }
}

pub fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("CARGO_MANIFEST_DIR must be reachable")
}

/// Root of the diagnostic input fixtures (`.pkl` files exercising each
/// analyzer).
pub fn fixtures_root() -> PathBuf {
    crate_root().join("tests/fixtures/diagnostics/inputs")
}

/// Recursively collect every `*.pkl` file under `root`, sorted for stable
/// snapshot ordering.
pub fn collect_pkl_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(root, &mut files);
    files.sort();
    files
}

fn walk(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "pkl") {
            files.push(path);
        }
    }
}
