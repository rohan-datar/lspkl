//! Qualified access completion: members of the receiver's type.

use tower_lsp_server::lsp_types::{CompletionItem, CompletionItemKind};

use pkl_syntax::ast::{NodeId, NodeKind};
use pkl_types::{AnalysisContext, Type, TypeBindings};

use super::members::{MemberKind, members_of_type};

/// Complete members of the receiver of a qualified access expression.
pub(crate) fn completions(node: NodeId, ctx: &AnalysisContext) -> Vec<CompletionItem> {
    let tree = ctx.tree;
    let bindings = TypeBindings::new();
    let mut type_position = false;
    let receiver_ty = if let Some(access) = super::ancestor_or_self(node, ctx, |k| {
        matches!(k, NodeKind::QualifiedAccessExpr | NodeKind::SuperAccessExpr)
    }) {
        if let Some(receiver) = tree.child(access, |k| k.is_expr_node()) {
            let mut current = Some(node);
            while let Some(id) = current {
                if id == receiver {
                    return Vec::new();
                }
                if id == access {
                    break;
                }
                current = tree.parent(id);
            }
        }
        let Some(ty) = pkl_types::resolver::access_receiver_type(access, &bindings, ctx) else {
            return Vec::new();
        };
        ty
    } else if let Some(identifier) =
        super::ancestor_or_self(node, ctx, |k| matches!(k, NodeKind::QualifiedIdentifier))
    {
        let parts = tree.qualified_identifier_segments(identifier);
        if parts.len() != 2 {
            return Vec::new();
        }
        type_position = true;
        let Some(import) = tree.children(tree.root()).iter().copied().find(|&id| {
            matches!(tree.kind(id), NodeKind::Import)
                && pkl_types::resolver::import_name(tree, id).as_deref() == Some(parts[0])
        }) else {
            return Vec::new();
        };
        pkl_types::resolver::import_type(pkl_types::NodeRef::new(ctx.file_id, import), ctx)
    } else {
        return Vec::new();
    };

    members_of_type(&receiver_ty, &TypeBindings::new(), ctx)
        .into_iter()
        .filter(|member| {
            !type_position || matches!(member.kind, MemberKind::Class | MemberKind::TypeAlias)
        })
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
