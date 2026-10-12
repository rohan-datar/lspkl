//! Orchestrator that runs all registered analyzers and merges their output.

use pkl_types::AnalysisContext;

use crate::analyzer::{Analyzer, run_analyzer};
use crate::analyzers;
use crate::diagnostic::Diagnostic;
use crate::holder::DiagnosticsHolder;

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

    pub fn register(&mut self, analyzer: Box<dyn Analyzer>) {
        self.analyzers.push(analyzer);
    }

    /// The production pipeline: all 11 analyzers. Most are stubbed to no-ops
    /// (recurse without emitting) until their bodies are ported. As each
    /// analyzer is implemented, the snapshot tests automatically pick up the
    /// new diagnostics.
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
    pub fn analyze(&self, ctx: &AnalysisContext, file: pkl_types::FileId) -> Vec<Diagnostic> {
        let Some(tree) = ctx.tree(file) else {
            return Vec::new();
        };
        let mut all = Vec::new();
        for analyzer in &self.analyzers {
            let mut holder = DiagnosticsHolder::new();
            run_analyzer(
                &**analyzer,
                pkl_types::NodeRef::new(file, tree.root()),
                &mut holder,
                ctx,
            );
            all.extend(holder.into_diagnostics());
        }
        all
    }

    pub fn analyzers(&self) -> &[Box<dyn Analyzer>] {
        &self.analyzers
    }
}

impl Default for DiagnosticsEngine {
    fn default() -> Self {
        Self::with_default_analyzers()
    }
}
