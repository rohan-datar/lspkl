//! Implement-method completion: offer abstract superclass methods that the
//! current class doesn't implement yet.

use lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat};

use pkl_syntax::ast::NodeId;
use pkl_syntax::ast::nodes::{
    Class, ClassBody, ClassMethod, MethodHeader, ParameterList, TypeAnnotation, TypedIdentifier,
};
use pkl_types::resolver::resolve_type_name;
use pkl_types::{AnalysisContext, NodeRef};

pub(crate) fn completions(node: NodeId, ctx: &AnalysisContext) -> Vec<CompletionItem> {
    let tree = ctx.tree;
    let body = ClassBody::cast(tree, node).or_else(|| ClassBody::cast(tree, tree.parent(node)?));
    let Some(body) = body else {
        return Vec::new();
    };
    let Some(def_class) = tree.parent(body.id()).and_then(|id| Class::cast(tree, id)) else {
        return Vec::new();
    };

    // The superclass whose abstract members we're completing.
    let Some(extends_target) = def_class.extends_target(tree) else {
        return Vec::new();
    };
    let segments = tree.qualified_identifier_segments(extends_target);
    let Some(super_class) =
        resolve_type_name(ctx, &segments, NodeRef::new(ctx.file_id, extends_target))
    else {
        return Vec::new();
    };

    // Methods already declared in this class body.
    let declared: std::collections::HashSet<String> = body
        .members(tree)
        .filter_map(|m| tree.name(m).map(str::to_string))
        .collect();

    let super_tree = match ctx.tree_for(super_class.file_id) {
        Some(t) => t,
        None => return Vec::new(),
    };
    let super_body = Class::cast(super_tree, super_class.node_id)
        .and_then(|class| class.body(super_tree))
        .unwrap_or(super_class.node_id);

    let mut out = Vec::new();
    for method in ClassBody::cast(super_tree, super_body)
        .into_iter()
        .flat_map(|body| body.members(super_tree))
        .filter(|id| ClassMethod::cast(super_tree, *id).is_some())
    {
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
    ClassMethod::cast(tree, method)
        .and_then(|method| method.body(tree))
        .is_none()
}

fn render_method_signature(tree: &pkl_syntax::ast::SyntaxTree, method: NodeId) -> String {
    let header = ClassMethod::cast(tree, method)
        .and_then(|method| method.header(tree))
        .and_then(|header| MethodHeader::cast(tree, header));
    let name = tree.name(method).unwrap_or("");

    let mut sig = format!("function {name}(");
    if let Some(params) = header.and_then(|h| h.parameters(tree)) {
        let rendered: Vec<String> = ParameterList::cast(tree, params)
            .into_iter()
            .flat_map(|params| params.parameters(tree))
            .filter(|tid| TypedIdentifier::cast(tree, *tid).is_some())
            .map(|tid| {
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

    if let Some(ret) = header.and_then(|h| member_return_type_text(tree, h.id())) {
        sig.push_str(": ");
        sig.push_str(&ret);
    }
    sig
}

fn param_type_text(tree: &pkl_syntax::ast::SyntaxTree, typed_ident: NodeId) -> Option<String> {
    let ann = TypedIdentifier::cast(tree, typed_ident)?.type_annotation(tree)?;
    let type_node = TypeAnnotation::cast(tree, ann)?.ty(tree)?;
    Some(tree.text(type_node).to_string())
}

fn member_return_type_text(tree: &pkl_syntax::ast::SyntaxTree, header: NodeId) -> Option<String> {
    let ann = MethodHeader::cast(tree, header)?.type_annotation(tree)?;
    let type_node = TypeAnnotation::cast(tree, ann)?.ty(tree)?;
    Some(tree.text(type_node).to_string())
}
