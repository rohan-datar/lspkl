//! Qualified access completion: members of the receiver's type.

use tower_lsp_server::lsp_types::{CompletionItem, CompletionItemKind};

use pkl_syntax::ast::{NodeId, NodeKind};
use pkl_types::{AnalysisContext, Type, TypeBindings};

use super::members::{MemberKind, members_of_type};

/// Complete members of the receiver of a qualified access expression.
pub(crate) fn completions(node: NodeId, ctx: &AnalysisContext) -> Vec<CompletionItem> {
    let tree = ctx.tree;
    // `find_at` usually lands on the accessor's identifier terminal, so climb
    // to the enclosing access expression.
    let Some(access) =
        super::ancestor_or_self(node, ctx, |k| matches!(k, NodeKind::QualifiedAccessExpr))
    else {
        return Vec::new();
    };
    let Some(receiver) = tree.child(access, |k| k.is_expr_node()) else {
        return Vec::new();
    };

    let receiver_ty =
        pkl_types::compute_expr::compute_expr_type(receiver, &TypeBindings::new(), ctx);

    members_of_type(&receiver_ty, &TypeBindings::new(), ctx)
        .into_iter()
        .map(|member| {
            let kind = match member.kind {
                MemberKind::Property => CompletionItemKind::PROPERTY,
                MemberKind::Method => CompletionItemKind::METHOD,
                MemberKind::Class => CompletionItemKind::CLASS,
                MemberKind::TypeAlias => CompletionItemKind::STRUCT,
            };
            let detail = if member.ty != Type::Unknown {
                member.ty.render(ctx)
            } else {
                Default::default()
            };
            super::simple_item(&member.name, &member.name, &detail, Some(kind))
        })
        .collect()
}
