//! Hover: resolve the node at the cursor, render its type + docs as markdown.

use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};

use lspkl_syntax::TextSize;
use lspkl_syntax::ast::nodes::{
    ClassMethod, ClassProperty, MethodHeader, ObjectMethod, ParameterList, TypeAnnotation,
    TypeParameterList, TypedIdentifier,
};
use lspkl_syntax::ast::{NodeId, NodeKind, SyntaxTree};

use lspkl_db::{Db, File};
use lspkl_types::{AnalysisContext, NodeRef, Type, resolve_reference};

/// Resolve hover information for the given cursor byte `offset`.
///
/// Returns `None` when there is nothing meaningful at the cursor — the
/// caller should translate `None` into an empty `Hover` per the LSP spec.
///
/// `offset` identifies both the source file and its UTF-8 byte offset.
///
/// Pipeline:
///   1. Locate the deepest node at the cursor (`find_at`).
///   2. Resolve what it refers to (`resolve_reference`).
///   3. Render the resolved declaration as Markdown (`render_markdown`).
///   4. Wrap as a Markdown `Hover`.
pub fn hover(db: &dyn Db, root: File, offset: lspkl_types::InFile<TextSize>) -> Option<Hover> {
    let ctx = &AnalysisContext::for_root(db, root);
    let file = offset.file;
    let tree = ctx.tree(file)?;
    let offset = offset.value;
    let node_id = tree.find_at(offset)?;
    let original = NodeRef::new(file, node_id);
    let resolved = resolve_reference(original, offset, ctx)?;
    let value = render_markdown(resolved, original, ctx)?;

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: None,
    })
}

/// Render a resolved declaration as Markdown.
///
/// Returns `None` for node kinds we don't expose in hover (anything that
/// isn't a declaration the user cares about — bare terminals, punctuation,
/// etc.). The caller turns `None` into "no hover".
///
/// `original` is the node the user actually pointed at; `resolved` is what
/// `resolve_reference` returned. They differ when the cursor was on a
/// reference (`x` in `y = x`) and `resolved` is the declaration site
/// (`x = 1`). A few rendering paths need `original` to know which sub-span
/// to highlight (e.g. type-parameter substitution).
pub fn render_markdown(
    resolved: NodeRef,
    original: NodeRef,
    ctx: &AnalysisContext,
) -> Option<String> {
    // The original cursor node is reserved for type-parameter substitution.
    let _ = original;

    // `resolved` may live in another file (e.g. pkl:base), so look up its
    // tree — never use the request source tree directly here.
    let tree = ctx.tree(resolved.file)?;
    let node_id = resolved.value;

    let signature = match tree.kind(node_id) {
        NodeKind::Module => format!("module {}", tree.name(node_id).unwrap_or("")),
        NodeKind::ImportGlob => format!(
            "{}: {}",
            lspkl_types::resolver::import_name(tree, node_id)?,
            lspkl_types::resolver::import_type(resolved, ctx).render(ctx.db)
        ),
        NodeKind::ClassProperty => {
            let name = tree.name(node_id)?;
            match property_type(tree, resolved, ctx) {
                Some(ty) => format!("{}{name}: {ty}", modifiers_prefix(tree, node_id)),
                None => format!("{}{name}", modifiers_prefix(tree, node_id)),
            }
        }
        NodeKind::Class => format!(
            "{}class {}",
            modifiers_prefix(tree, node_id),
            tree.name(node_id)?
        ),
        NodeKind::TypeAlias => format!(
            "{}typealias {}",
            modifiers_prefix(tree, node_id),
            tree.name(node_id)?
        ),
        NodeKind::ClassMethod | NodeKind::ObjectMethod => format!(
            "{}{}",
            modifiers_prefix(tree, node_id),
            method_signature(tree, node_id, resolved.file, ctx)?
        ),
        _ => return None,
    };

    // Place documentation after the fenced declaration signature.
    let mut md = format!("```pkl\n{signature}\n```");
    if let Some(docs) = tree.doc_comment_text(node_id) {
        md.push_str("\n\n---\n\n");
        md.push_str(&docs);
    }
    Some(md)
}

/// Render the type of a `ClassProperty` for hover.
///
/// Prefers the explicit annotation (`x: String`), rendered through
/// `to_type` so aliases and generics display the same way diagnostics do.
/// Falls back to inferring the initializer's type (`x = 1` → `Int`) via
/// `expr_type`. Returns `None` when neither yields anything useful.
fn property_type(tree: &SyntaxTree, property: NodeRef, ctx: &AnalysisContext) -> Option<String> {
    let declaration = ClassProperty::cast(tree, property.value)?;
    let annotated = declaration
        .type_annotation(tree)
        .and_then(|ann| TypeAnnotation::cast(tree, ann)?.ty(tree));
    if let Some(type_node) = annotated {
        let ty = NodeRef::new(property.file, type_node)
            .cast(ctx, lspkl_syntax::ast::nodes::TypeNode::cast)
            .map_or(Type::Unknown, |ty| {
                lspkl_types::to_type(ctx.db, ty, ctx.root)
            });
        return Some(ty.render(ctx.db));
    }

    // No annotation: infer from the initializer.
    let expr = declaration.value(tree)?;
    let ty = NodeRef::new(property.file, expr)
        .cast(ctx, lspkl_syntax::ast::nodes::Expr::cast)
        .map_or(Type::Unknown, |expr| {
            lspkl_types::expr_type(ctx.db, expr, ctx.root)
        });
    if ty != Type::Unknown {
        return Some(ty.render(ctx.db));
    }
    None
}

/// Render a method's full signature: `function name<T>(params): Ret`.
fn method_signature(
    tree: &SyntaxTree,
    node_id: NodeId,
    file_id: File,
    ctx: &AnalysisContext,
) -> Option<String> {
    let header = ClassMethod::cast(tree, node_id)
        .and_then(|method| method.header(tree))
        .or_else(|| ObjectMethod::cast(tree, node_id)?.header(tree))?;
    let header = MethodHeader::cast(tree, header)?;
    let name = tree.name(node_id)?;

    let type_params = header
        .type_parameters(tree)
        .map(|list| type_parameters_text(tree, list))
        .unwrap_or_default();
    let params = header
        .parameters(tree)
        .map(|list| parameters_text(tree, list))
        .unwrap_or_else(|| "()".to_string());
    let ret = return_type_text(tree, node_id, header.id(), file_id, ctx);

    Some(format!("function {name}{type_params}{params}: {ret}"))
}

/// Render `<A, B>` from a `TypeParameterList`.
///
/// Render parameter names without variance markers.
fn type_parameters_text(tree: &SyntaxTree, list: NodeId) -> String {
    let parts: Vec<&str> = TypeParameterList::cast(tree, list)
        .into_iter()
        .flat_map(|list| list.parameters(tree))
        .filter_map(|tp| tree.name(tp))
        .collect();
    format!("<{}>", parts.join(", "))
}

/// Render typed parameters; blank identifiers render as `_`.
fn parameters_text(tree: &SyntaxTree, list: NodeId) -> String {
    let params: Vec<String> = ParameterList::cast(tree, list)
        .into_iter()
        .flat_map(|list| list.parameters(tree))
        .map(|p| match tree.kind(p) {
            NodeKind::BlankIdentifier => "_".to_string(),
            _ => {
                let name = tree.name(p).unwrap_or("_");
                match TypedIdentifier::cast(tree, p).and_then(|p| p.type_annotation(tree)) {
                    // Preserve the annotation's source spelling rather than
                    // normalizing aliases and formatting through semantic types.
                    Some(ann) => format!(
                        "{name}: {}",
                        TypeAnnotation::cast(tree, ann)
                            .and_then(|ann| ann.ty(tree))
                            .map(|ty| tree.text(ty))
                            .unwrap_or(""),
                    ),
                    None => name.to_string(),
                }
            }
        })
        .collect();
    format!("({})", params.join(", "))
}

/// Render a method's return type. Prefers the declared annotation;
/// without one, infers from the body expression via `expr_type`
/// (as upstream does); gives up with `unknown`.
fn return_type_text(
    tree: &SyntaxTree,
    method_id: NodeId,
    header: NodeId,
    file_id: File,
    ctx: &AnalysisContext,
) -> String {
    if let Some(ann) = MethodHeader::cast(tree, header).and_then(|h| h.type_annotation(tree)) {
        return annotation_text(tree, ann, file_id, ctx);
    }
    let body = ClassMethod::cast(tree, method_id)
        .and_then(|method| method.body(tree))
        .or_else(|| ObjectMethod::cast(tree, method_id)?.body(tree));
    if let Some(expr) = body {
        let ty = NodeRef::new(file_id, expr)
            .cast(ctx, lspkl_syntax::ast::nodes::Expr::cast)
            .map_or(Type::Unknown, |expr| {
                lspkl_types::expr_type(ctx.db, expr, ctx.root)
            });
        if ty != Type::Unknown {
            return ty.render(ctx.db);
        }
    }
    "unknown".to_string()
}

/// Render a type annotation's inner type semantically (`to_type` +
/// `render`), falling back to the raw source text when the type can't be
/// resolved.
fn annotation_text(tree: &SyntaxTree, ann: NodeId, file_id: File, ctx: &AnalysisContext) -> String {
    match TypeAnnotation::cast(tree, ann).and_then(|ann| ann.ty(tree)) {
        Some(ty_node) => {
            let ty = NodeRef::new(file_id, ty_node)
                .cast(ctx, lspkl_syntax::ast::nodes::TypeNode::cast)
                .map_or(Type::Unknown, |ty| {
                    lspkl_types::to_type(ctx.db, ty, ctx.root)
                });
            if ty != Type::Unknown {
                ty.render(ctx.db)
            } else {
                tree.text(ty_node).to_string()
            }
        }
        None => String::new(),
    }
}

/// Modifier keywords (`hidden`, `fixed`, ...) as a trailing-space prefix.
fn modifiers_prefix(tree: &SyntaxTree, node_id: NodeId) -> String {
    // For methods the modifiers live on the MethodHeader, matching
    // `SyntaxTree::modifiers`.
    let holder = ClassMethod::cast(tree, node_id)
        .and_then(|method| method.header(tree))
        .or_else(|| ObjectMethod::cast(tree, node_id)?.header(tree))
        .unwrap_or(node_id);
    let mods: Vec<&str> = tree
        .children_matching(
            holder,
            |k| matches!(k, NodeKind::Terminal(tt) if tt.is_modifier()),
        )
        .iter()
        .map(|&m| tree.text(m))
        .collect();
    if mods.is_empty() {
        String::new()
    } else {
        format!("{} ", mods.join(" "))
    }
}
