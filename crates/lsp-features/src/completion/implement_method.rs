//! Implement-method completion: offer abstract superclass methods that the
//! current class doesn't implement yet.

use lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat};

use pkl_syntax::ast::{NodeId, NodeKind};
use pkl_types::resolver::resolve_type_name;
use pkl_types::{AnalysisContext, NodeRef};

pub(crate) fn completions(node: NodeId, ctx: &AnalysisContext) -> Vec<CompletionItem> {
    let tree = ctx.tree;
    let body = if matches!(tree.kind(node), NodeKind::ClassBody) {
        node
    } else {
        match tree.parent(node) {
            Some(p) if matches!(tree.kind(p), NodeKind::ClassBody) => p,
            _ => return Vec::new(),
        }
    };
    let Some(def_class) = tree.parent(body) else {
        return Vec::new();
    };
    if !matches!(tree.kind(def_class), NodeKind::Class) {
        return Vec::new();
    }

    // The superclass whose abstract members we're completing.
    let Some(extends_target) = tree.class_extends_target(def_class) else {
        return Vec::new();
    };
    let segments = tree.qualified_identifier_segments(extends_target);
    let Some(super_class) =
        resolve_type_name(ctx, &segments, NodeRef::new(ctx.file_id, extends_target))
    else {
        return Vec::new();
    };

    // Methods already declared in this class body.
    let declared: std::collections::HashSet<String> = tree
        .children_matching(body, |k| {
            matches!(k, NodeKind::ClassMethod | NodeKind::ClassProperty)
        })
        .iter()
        .filter_map(|&m| tree.name(m).map(str::to_string))
        .collect();

    let super_tree = match ctx.tree_for(super_class.file_id) {
        Some(t) => t,
        None => return Vec::new(),
    };
    let super_body = super_tree
        .child(super_class.node_id, |k| matches!(k, NodeKind::ClassBody))
        .unwrap_or(super_class.node_id);

    let mut out = Vec::new();
    for method in super_tree.children_matching(super_body, |k| matches!(k, NodeKind::ClassMethod)) {
        let Some(name) = super_tree.name(method) else {
            continue;
        };
        if declared.contains(name) || !is_abstract(super_tree, method) {
            continue;
        }
        let signature = render_method_signature(super_tree, method);
        out.push(CompletionItem {
            label: format!("{signature} = …"),
            kind: Some(CompletionItemKind::METHOD),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            insert_text: Some(format!("{signature} = $0")),
            detail: Some(signature),
            ..Default::default()
        });
    }
    out
}

/// Abstract = explicitly marked `abstract`, or bodiless inside a class body.
fn is_abstract(tree: &pkl_syntax::ast::SyntaxTree, method: NodeId) -> bool {
    use pkl_syntax::ast::TokenType;
    if tree.has_modifier(method, TokenType::Abstract) {
        return true;
    }
    // Bodiless method: no expression after the method header.
    tree.children_matching(method, |k| k.is_expr_node())
        .is_empty()
}

fn render_method_signature(tree: &pkl_syntax::ast::SyntaxTree, method: NodeId) -> String {
    let header = tree.child(method, |k| matches!(k, NodeKind::MethodHeader));
    let name = tree.name(method).unwrap_or("");

    let mut sig = format!("function {name}(");
    if let Some(params) =
        header.and_then(|h| tree.child(h, |k| matches!(k, NodeKind::ParameterList)))
    {
        let rendered: Vec<String> = tree
            .children_matching(params, |k| matches!(k, NodeKind::TypedIdentifier))
            .iter()
            .map(|&tid| {
                let name = tree.name(tid).unwrap_or("");
                match param_type_text(tree, tid) {
                    Some(ty) => format!("{name}: {ty}"),
                    None => name.to_string(),
                }
            })
            .collect();
        sig.push_str(&rendered.join(", "));
    }
    sig.push(')');

    if let Some(ret) = header.and_then(|h| member_return_type_text(tree, h)) {
        sig.push_str(": ");
        sig.push_str(&ret);
    }
    sig
}

fn param_type_text(tree: &pkl_syntax::ast::SyntaxTree, typed_ident: NodeId) -> Option<String> {
    let ann = tree.child(typed_ident, |k| matches!(k, NodeKind::TypeAnnotation))?;
    let type_node = tree.child(ann, |k| k.is_type_node())?;
    Some(tree.text(type_node).to_string())
}

fn member_return_type_text(tree: &pkl_syntax::ast::SyntaxTree, header: NodeId) -> Option<String> {
    let ann = tree.child(header, |k| matches!(k, NodeKind::TypeAnnotation))?;
    let type_node = tree.child(ann, |k| k.is_type_node())?;
    Some(tree.text(type_node).to_string())
}
