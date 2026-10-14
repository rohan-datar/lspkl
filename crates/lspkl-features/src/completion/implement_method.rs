//! Implement-method completion: offer abstract superclass methods that the
//! current class doesn't implement yet.

use lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat};

use lspkl_syntax::ast::NodeId;
use lspkl_syntax::ast::nodes::{
    Class, ClassBody, ClassMethod, MethodHeader, ParameterList, TypeAnnotation, TypedIdentifier,
};
use lspkl_types::{AnalysisContext, NodeRef};

pub(crate) fn completions(
    node: NodeId,
    tree: &lspkl_syntax::ast::SyntaxTree,
    file: lspkl_db::File,
    ctx: &AnalysisContext,
) -> Vec<CompletionItem> {
    let body = ClassBody::cast(tree, node).or_else(|| ClassBody::cast(tree, tree.parent(node)?));
    let Some(body) = body else {
        return Vec::new();
    };
    let Some(def_class) = tree.parent(body.id()).and_then(|id| Class::cast(tree, id)) else {
        return Vec::new();
    };

    let Some(class) = lspkl_types::class_id(ctx.db, NodeRef::new(file, def_class.id()), ctx.root)
    else {
        return Vec::new();
    };
    let table = lspkl_types::class_members(ctx.db, class);
    let mut out = Vec::new();
    for member in &table.entries[table.own_len..] {
        if member.kind != lspkl_types::MemberKind::Method {
            continue;
        }
        let Some(method) = lspkl_types::item_node(ctx.db, member.declaration) else {
            continue;
        };
        let Some(super_tree) = ctx.tree(method.file) else {
            continue;
        };
        if !is_abstract(super_tree, method.value) {
            continue;
        }
        let signature = render_method_signature(super_tree, method.value);
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
fn is_abstract(tree: &lspkl_syntax::ast::SyntaxTree, method: NodeId) -> bool {
    use lspkl_syntax::ast::TokenType;
    if tree.has_modifier(method, TokenType::Abstract) {
        return true;
    }
    // Bodiless method: no expression after the method header.
    ClassMethod::cast(tree, method)
        .and_then(|method| method.body(tree))
        .is_none()
}

fn render_method_signature(tree: &lspkl_syntax::ast::SyntaxTree, method: NodeId) -> String {
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

fn param_type_text(tree: &lspkl_syntax::ast::SyntaxTree, typed_ident: NodeId) -> Option<String> {
    let ann = TypedIdentifier::cast(tree, typed_ident)?.type_annotation(tree)?;
    let type_node = TypeAnnotation::cast(tree, ann)?.ty(tree)?;
    Some(tree.text(type_node).to_string())
}

fn member_return_type_text(tree: &lspkl_syntax::ast::SyntaxTree, header: NodeId) -> Option<String> {
    let ann = MethodHeader::cast(tree, header)?.type_annotation(tree)?;
    let type_node = TypeAnnotation::cast(tree, ann)?.ty(tree)?;
    Some(tree.text(type_node).to_string())
}
