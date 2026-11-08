//! Type checking — checks expression types against declared types,
//! property defaults, function return types, parameter types, etc. The
//! workhorse analyzer.

use lspkl_types::NodeRef;

use lspkl_syntax::ast::nodes;

use lspkl_types::{AnalysisContext, Type};

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;

pub struct TypeCheckAnalyzer;

impl Analyzer for TypeCheckAnalyzer {
    fn analyze_node(
        &self,
        node: NodeRef,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        let file = node.file;
        let Some(source_tree) = ctx.tree(file) else {
            return false;
        };
        let node = node.value;
        if !nodes::Expr::can_cast(source_tree.kind(node)) {
            return true;
        }

        let expected = NodeRef::new(file, node)
            .cast(ctx, lspkl_syntax::ast::nodes::Expr::cast)
            .map_or(Type::Unknown, |expr| {
                lspkl_types::expected_type(ctx.db, expr.map(|expr| expr.id()), ctx.root)
            });
        if matches!(expected, Type::Unknown) {
            return true;
        }

        let actual = NodeRef::new(file, node)
            .cast(ctx, lspkl_syntax::ast::nodes::Expr::cast)
            .map_or(Type::Unknown, |expr| {
                lspkl_types::expr_type(ctx.db, expr, ctx.root)
            });
        // Static checks are deliberately separate from subtype proofs. A
        // broad Int value need not prove an Int constraint to remain valid.
        let has_constraints = expected.has_constraints(ctx.db);
        let value = has_constraints
            .then(|| static_value(NodeRef::new(file, node), &actual, ctx))
            .flatten();
        if let Some(constraint) = value
            .as_ref()
            .and_then(|value| expected.constraint_violation(&actual, value, ctx.db))
        {
            holder.add_error(
                source_tree.span(node),
                format!("Type constraint `{constraint}` violated."),
            );
            return true;
        }
        if actual.is_subtype_of(&expected, ctx.db)
            || has_constraints
                && actual
                    .without_constraints(ctx.db)
                    .is_subtype_of(&expected.without_constraints(ctx.db), ctx.db)
        {
            return true;
        }

        let span = source_tree.span(node);
        let render_actual = actual.render(ctx.db);
        let render_expected = expected.render(ctx.db);

        let class_value = is_exact_class_value(NodeRef::new(file, node), ctx);
        if class_value || !actual.has_common_subtype_with(&expected, ctx.db) {
            holder.add_error(
                span,
                format!("Type mismatch. Expected: {render_expected}. Actual: {render_actual}"),
            );
        } else if actual.is_nullable(ctx.db)
            && actual.non_null(ctx.db).is_subtype_of(&expected, ctx.db)
        {
            holder.add_warning(
                span,
                format!(
                    "Nullability mismatch. Expected: {render_expected}. Actual: {render_actual}"
                ),
            );
        } else {
            holder.add_warning(
                span,
                format!("Type mismatch. Expected: {render_expected}. Actual: {render_actual}"),
            );
        }

        true
    }
}

/// A direct class reference is an exact runtime class, unlike a Class<T> variable.
pub(super) fn is_exact_class_value(node: NodeRef, ctx: &AnalysisContext) -> bool {
    let Some(tree) = ctx.tree(node.file) else {
        return false;
    };
    matches!(
        tree.kind(node.value),
        lspkl_syntax::ast::NodeKind::UnqualifiedAccessExpr
            | lspkl_syntax::ast::NodeKind::QualifiedAccessExpr
    ) && lspkl_types::resolver::resolve_reference(node, tree.span(node.value).start(), ctx)
        .and_then(|definition| lspkl_types::decl_for_node(ctx.db, definition, ctx.root))
        .is_some_and(|decl| matches!(decl.item, lspkl_types::ItemId::Class(_)))
}

/// Read only values whose runtime value is known without executing Pkl. In
/// particular, never follow overridable properties or execute user functions.
fn static_value(
    node: NodeRef,
    actual: &Type,
    ctx: &AnalysisContext,
) -> Option<lspkl_types::constraint::StaticValue> {
    use lspkl_types::constraint::{StaticValue, parse_literal};
    let tree = ctx.tree(node.file)?;
    // The inner expression receives the same expected type, so only it should
    // report the violation of a transparent parenthesized expression.
    if nodes::ParenthesizedExpr::cast(tree, node.value).is_some() {
        return None;
    }
    if let Type::StringLiteral { value } = actual.unaliased(ctx.db).unconstrained() {
        return Some(StaticValue::String(value.clone()));
    }
    if let Some(value) = parse_literal(tree.text(node.value)) {
        return Some(value);
    }
    let mut literal_node = node.value;
    let mut negative = false;
    loop {
        if let Some(inner) = nodes::ParenthesizedExpr::cast(tree, literal_node)
            .and_then(|parenthesized| parenthesized.inner(tree))
        {
            literal_node = inner;
        } else if tree.kind(literal_node) == &lspkl_syntax::ast::NodeKind::UnaryMinusExpr {
            literal_node = nodes::UnaryExpr::cast(tree, literal_node)?.operand(tree)?;
            negative = !negative;
        } else {
            break;
        }
    }
    if let Some(value) = parse_literal(tree.text(literal_node)) {
        return if negative {
            match value {
                StaticValue::Int(value) => value.checked_neg().map(StaticValue::Int),
                StaticValue::Float(value) => Some(StaticValue::Float(-value)),
                StaticValue::Duration(value) => Some(StaticValue::Duration(-value)),
                StaticValue::DataSize(value) => Some(StaticValue::DataSize(-value)),
                _ => None,
            }
        } else {
            Some(value)
        };
    }
    let access = nodes::UnqualifiedAccessExpr::cast(tree, node.value)?;
    let name = tree.name(node.value)?;
    if !matches!(name, "List" | "Set" | "Map") {
        return None;
    }
    let definition =
        lspkl_types::resolver::resolve_reference(node, tree.span(node.value).start(), ctx)?;
    if definition.file != ctx.base.file_id() {
        return None;
    }
    let arguments = nodes::ArgumentList::cast(tree, access.arguments(tree)?)?;
    let arguments: Vec<_> = arguments.arguments(tree).collect();
    if name == "List" {
        return Some(StaticValue::CollectionLength(arguments.len()));
    }
    if name == "Map" && !arguments.len().is_multiple_of(2) {
        return None;
    }
    // Set and Map constructors deduplicate elements/keys. Only count keys of
    // one literal kind; heterogeneous numeric equality needs runtime rules.
    #[derive(PartialEq, Eq, Hash)]
    enum LiteralKey {
        Int(i64),
        String(String),
        Boolean(bool),
    }
    let mut keys = std::collections::HashSet::new();
    let mut kind = None;
    for argument in arguments.iter().step_by(if name == "Map" { 2 } else { 1 }) {
        let key = match parse_literal(tree.text(*argument))? {
            StaticValue::Int(value) => LiteralKey::Int(value),
            StaticValue::String(value) => LiteralKey::String(value),
            StaticValue::Boolean(value) => LiteralKey::Boolean(value),
            _ => return None,
        };
        let key_kind = std::mem::discriminant(&key);
        if kind.is_some_and(|kind| kind != key_kind) {
            return None;
        }
        kind = Some(key_kind);
        keys.insert(key);
    }
    Some(StaticValue::CollectionLength(keys.len()))
}
