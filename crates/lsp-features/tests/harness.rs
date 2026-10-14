#![allow(dead_code)]
#[path = "../../pkl-db/tests/support/mod.rs"]
pub mod input_support;
use fluent_uri::Uri;
use pkl_db::{Db, File, FileStatus, RootDatabase};
use pkl_syntax::ast::SyntaxTree;
use pkl_types::{AnalysisContext, BaseModule};
use pkl_vfs::{FileError, Source, SourceLoader};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::Path,
    sync::{Arc, RwLock},
};

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
        let url = uri(name);
        let text: Arc<str> = Arc::from(text);
        self.sources.0.write().unwrap().insert(
            url.to_string(),
            Arc::new(Source {
                url: url.clone(),
                text: text.clone(),
            }),
        );
        let mut db = self.db.borrow_mut();
        let file = db
            .upsert_file(&url, text, None, FileStatus::Present)
            .unwrap();
        input_support::preload(&mut db);
        file
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
    let base = BaseModule::from_tree(&db, file, tree).unwrap();
    TestBase {
        db: RefCell::new(db),
        base,
        sources,
    }
}

pub fn with_snippet<R>(
    base: &TestBase,
    source: &str,
    f: impl FnOnce(&SyntaxTree, &AnalysisContext) -> R,
) -> R {
    let file = base.add(1, source);
    let db = base.db.borrow();
    let ctx = AnalysisContext::for_root(&*db, file);
    f(ctx.tree(file).unwrap(), &ctx)
}
pub fn offset(tree: &SyntaxTree, line: u32, column: u32) -> pkl_syntax::TextSize {
    tree.line_index()
        .offset(pkl_syntax::LineCol { line, col: column })
        .unwrap()
}

pub fn range(tree: &SyntaxTree, range: pkl_syntax::TextRange) -> lsp_types::Range {
    lsp_features::positions::PositionConverter::new(tree, Default::default()).range(range)
}
