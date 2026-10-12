//! Enumerate the members of a resolved type.

use std::collections::HashSet;

use crate::resolver::{class_member_bindings, resolve_type_name};
use crate::resolver::{definition_type_with_bindings, module_super, module_type};
use crate::ty::TypeBindings;
use crate::{AnalysisContext, NodeRef, Type};
use pkl_syntax::ast::nodes::{Class, ClassBody, ModuleMember, ObjectMember};
use pkl_syntax::ast::{NodeId, SyntaxTree};

/// What kind of declaration a member is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Property,
    Method,
    Class,
    TypeAlias,
}

/// One enumerated member of a type.
pub struct MemberInfo {
    pub name: String,
    /// Where the member is declared (may live in `pkl:base` or a superclass).
    pub node: NodeRef,
    /// The member's declared type (`Unknown` when it has no annotation).
    pub ty: Type,
    pub kind: MemberKind,
}

impl MemberInfo {
    pub fn is_property(&self) -> bool {
        self.kind == MemberKind::Property
    }

    pub fn is_fixed_or_const(&self, ctx: &AnalysisContext) -> bool {
        use pkl_syntax::ast::TokenType;
        let Some(tree) = ctx.tree(self.node.file) else {
            return false;
        };
        tree.has_modifier(self.node.value, TokenType::Fixed)
            || tree.has_modifier(self.node.value, TokenType::Const)
    }
}

/// List all members of `ty`, walking superclass chains.
pub fn members_of_type(ty: &Type, ctx: &AnalysisContext) -> Vec<MemberInfo> {
    let bindings = &TypeBindings::new();
    let mut out = Vec::new();
    let mut seen_names = HashSet::new();
    let mut seen_nodes = HashSet::new();
    collect(
        ty,
        bindings,
        ctx,
        &mut out,
        &mut seen_names,
        &mut seen_nodes,
    );
    out
}

fn collect(
    ty: &Type,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
    out: &mut Vec<MemberInfo>,
    seen_names: &mut HashSet<String>,
    seen_nodes: &mut HashSet<NodeRef>,
) {
    match ty {
        Type::Class {
            class_node,
            type_args,
            ..
        } => {
            if !seen_nodes.insert(*class_node) {
                return;
            }
            let Some(tree) = ctx.tree(class_node.file) else {
                return;
            };
            let member_bindings = class_member_bindings(*class_node, type_args, bindings, ctx);
            let Some(class) = Class::cast(tree, class_node.value) else {
                return;
            };
            let body = class.body(tree).unwrap_or(class_node.value);
            collect_body_members(
                tree,
                class_node.file,
                body,
                &member_bindings,
                ctx,
                out,
                seen_names,
            );

            // Continue up the superclass chain.
            if let Some(target) = class.extends_target(tree) {
                let segments = tree.qualified_identifier_segments(target);
                if let Some(super_node) =
                    resolve_type_name(ctx, &segments, NodeRef::new(class_node.file, target))
                {
                    collect(
                        &Type::class(super_node),
                        &member_bindings,
                        ctx,
                        out,
                        seen_names,
                        seen_nodes,
                    );
                }
            }
        }
        Type::Module { module_node, .. } => {
            if !seen_nodes.insert(*module_node) {
                return;
            }
            let Some(tree) = ctx.tree(module_node.file) else {
                return;
            };
            // Module-level properties/methods plus nested classes/aliases —
            // all accessible as unqualified names on the module.
            for member in ModuleMember::children(tree, module_node.value) {
                push_member(
                    tree,
                    module_node.file,
                    member,
                    bindings,
                    ctx,
                    out,
                    seen_names,
                );
            }
            if let Some(parent) = module_super(*module_node, ctx) {
                collect(
                    &module_type(parent, ctx),
                    bindings,
                    ctx,
                    out,
                    seen_names,
                    seen_nodes,
                );
            } else {
                collect(
                    ctx.base.module_type(),
                    bindings,
                    ctx,
                    out,
                    seen_names,
                    seen_nodes,
                );
            }
        }
        Type::Alias { .. } => collect(
            &ty.unaliased(ctx),
            bindings,
            ctx,
            out,
            seen_names,
            seen_nodes,
        ),
        Type::Union { left, right, .. } => {
            // Collect both sides independently, then keep left entries whose
            // name also exists on the right.
            let mut left_members = Vec::new();
            let mut left_names = HashSet::new();
            let mut left_nodes = HashSet::new();
            collect(
                left,
                bindings,
                ctx,
                &mut left_members,
                &mut left_names,
                &mut left_nodes,
            );

            let mut right_members = Vec::new();
            let mut right_names = HashSet::new();
            let mut right_nodes = HashSet::new();
            collect(
                right,
                bindings,
                ctx,
                &mut right_members,
                &mut right_names,
                &mut right_nodes,
            );

            for info in left_members {
                if right_members.iter().any(|r| r.name == info.name)
                    && seen_names.insert(info.name.clone())
                {
                    out.push(info);
                }
            }
        }
        _ => {}
    }
}

fn collect_body_members(
    tree: &SyntaxTree,
    file_id: crate::FileId,
    body: NodeId,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
    out: &mut Vec<MemberInfo>,
    seen_names: &mut HashSet<String>,
) {
    for member in ClassBody::cast(tree, body)
        .into_iter()
        .flat_map(|body| body.members(tree))
    {
        push_member(tree, file_id, member, bindings, ctx, out, seen_names);
    }
}

fn push_member(
    tree: &SyntaxTree,
    file_id: crate::FileId,
    member: NodeId,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
    out: &mut Vec<MemberInfo>,
    seen_names: &mut HashSet<String>,
) {
    let Some(name) = tree.name(member) else {
        return;
    };
    if !seen_names.insert(name.to_string()) {
        return; // shadowed by an override closer to the receiver
    }
    let kind = match ModuleMember::cast(tree, member) {
        Some(ModuleMember::ClassProperty(_)) => MemberKind::Property,
        Some(ModuleMember::ClassMethod(_)) => MemberKind::Method,
        Some(ModuleMember::Class(_)) => MemberKind::Class,
        Some(ModuleMember::TypeAlias(_)) => MemberKind::TypeAlias,
        None => match ObjectMember::cast(tree, member) {
            Some(ObjectMember::ObjectProperty(_)) => MemberKind::Property,
            Some(ObjectMember::ObjectMethod(_)) => MemberKind::Method,
            _ => return,
        },
    };
    let ty = definition_type_with_bindings(NodeRef::new(file_id, member), bindings, ctx);
    out.push(MemberInfo {
        name: name.to_string(),
        node: NodeRef::new(file_id, member),
        ty,
        kind,
    });
}
