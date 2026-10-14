//! Orchestrator that runs all registered analyzers and merges their output.

use std::sync::{Arc, OnceLock};

use pkl_types::AnalysisContext;

use crate::analyzer::{Analyzer, run_analyzer_with_tree};
use crate::analyzers;
use crate::diagnostic::Diagnostic;
use crate::holder::DiagnosticsHolder;

/// Production diagnostics and their quick fixes for a root module.
///
/// Value equality includes ranges and fixes, allowing Salsa to backdate
/// unchanged results. Publication and code actions share this memoized result.
#[salsa::tracked(returns(clone))]
pub fn diagnostics(db: &dyn pkl_db::Db, file: pkl_db::File) -> Arc<[Diagnostic]> {
    static ENGINE: OnceLock<DiagnosticsEngine> = OnceLock::new();
    let ctx = AnalysisContext::for_root(db, file);
    ENGINE
        .get_or_init(DiagnosticsEngine::with_default_analyzers)
        .analyze(&ctx, file)
        .into()
}

/// Holds the configured analyzer pipeline. Build via [`Self::new`] and
/// [`Self::with`], or call [`Self::with_default_analyzers`] for the production
/// pipeline of all 11.
pub struct DiagnosticsEngine {
    analyzers: Vec<Box<dyn Analyzer>>,
}

impl DiagnosticsEngine {
    /// Empty engine — registers no analyzers. Useful in tests when you want
    /// to exercise just one analyzer in isolation.
    pub fn new() -> Self {
        Self {
            analyzers: Vec::new(),
        }
    }

    /// Add one analyzer to the pipeline. Returns `self` for chaining.
    pub fn with(mut self, analyzer: Box<dyn Analyzer>) -> Self {
        self.analyzers.push(analyzer);
        self
    }

    /// The production pipeline, in stable registration order: all 11 analyzers.
    pub fn with_default_analyzers() -> Self {
        Self::new()
            .with(Box::new(analyzers::SyntaxAnalyzer))
            .with(Box::new(analyzers::ModifierAnalyzer))
            .with(Box::new(analyzers::AnnotationAnalyzer))
            .with(Box::new(analyzers::MemberAnalyzer))
            .with(Box::new(analyzers::ModuleUriAnalyzer))
            .with(Box::new(analyzers::UnsupportedFeatureAnalyzer))
            .with(Box::new(analyzers::TypeCheckAnalyzer))
            .with(Box::new(analyzers::AccessExprAnalyzer))
            .with(Box::new(analyzers::ExprAnalyzer))
            .with(Box::new(analyzers::TypeNameAnalyzer))
            .with(Box::new(analyzers::TypeAnalyzer))
    }

    /// Run every registered analyzer against `file` and concatenate their
    /// diagnostics. Order: analyzers run in registration order; within an
    /// analyzer, diagnostics appear in tree-walk order.
    pub fn analyze(&self, ctx: &AnalysisContext, file: pkl_db::File) -> Vec<Diagnostic> {
        let Some(tree) = ctx.tree(file) else {
            return Vec::new();
        };
        let mut all = Vec::new();
        for analyzer in &self.analyzers {
            let mut holder = DiagnosticsHolder::new();
            run_analyzer_with_tree(
                &**analyzer,
                pkl_types::NodeRef::new(file, tree.root()),
                &mut holder,
                ctx,
                tree,
            );
            all.extend(holder.into_diagnostics());
        }
        all
    }
}

impl Default for DiagnosticsEngine {
    fn default() -> Self {
        Self::with_default_analyzers()
    }
}
