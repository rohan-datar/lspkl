//! Qualified access completion: members of the receiver's type.

use pkl_types::NodeRef;

use lsp_types::{CompletionItem, CompletionItemKind};

use pkl_syntax::ast::nodes::{Import, Module, QualifiedAccessExpr};
use pkl_syntax::ast::{NodeId, NodeKind};
use pkl_types::{AnalysisContext, Type};

use pkl_types::{MemberKind, members_of_type};

/// Complete members of the receiver of a qualified access expression.
pub(crate) fn completions(
    node: NodeId,
    tree: &pkl_syntax::ast::SyntaxTree,
    file: pkl_types::FileId,
    ctx: &AnalysisContext,
) -> Vec<CompletionItem> {
    let mut type_position = false;
    let receiver_ty = if let Some(access) = super::ancestor_or_self(node, tree, |k| {
        matches!(k, NodeKind::QualifiedAccessExpr | NodeKind::SuperAccessExpr)
    }) {
        if let Some(receiver) =
            QualifiedAccessExpr::cast(tree, access).and_then(|access| access.receiver(tree))
        {
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
        let Some(ty) = pkl_types::resolver::access_receiver_type(NodeRef::new(file, access), ctx)
        else {
            return Vec::new();
        };
        ty
    } else if let Some(identifier) =
        super::ancestor_or_self(node, tree, |k| matches!(k, NodeKind::QualifiedIdentifier))
    {
        let parts = tree.qualified_identifier_segments(identifier);
        if parts.len() != 2 {
            return Vec::new();
        }
        type_position = true;
        let Some(import) = Module::cast(tree, tree.root())
            .into_iter()
            .flat_map(|module| module.imports(tree))
            .find(|&id| {
                Import::cast(tree, id).is_some()
                    && pkl_types::resolver::import_name(tree, id).as_deref() == Some(parts[0])
            })
        else {
            return Vec::new();
        };
        pkl_types::resolver::import_type(pkl_types::NodeRef::new(file, import), ctx)
    } else {
        return Vec::new();
    };

    members_of_type(&receiver_ty, ctx)
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
