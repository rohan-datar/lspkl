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
    match ty.unconstrained() {
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
        Type::Union { members } => {
            let mut common: Option<Vec<MemberInfo>> = None;
            for member in members.iter() {
                let mut branch = Vec::new();
                let mut names = HashSet::new();
                let mut nodes = HashSet::new();
                collect(member, bindings, ctx, &mut branch, &mut names, &mut nodes);
                if let Some(common) = &mut common {
                    branch.sort_unstable_by(|a, b| a.name.cmp(&b.name));
                    common.retain_mut(|info| {
                        let Ok(index) =
                            branch.binary_search_by(|candidate| candidate.name.cmp(&info.name))
                        else {
                            return false;
                        };
                        if info.ty != branch[index].ty {
                            let left = std::mem::replace(&mut info.ty, Type::Nothing);
                            let right = std::mem::replace(&mut branch[index].ty, Type::Nothing);
                            info.ty = Type::union([left, right], ctx);
                        }
                        true
                    });
                } else {
                    common = Some(branch);
                }
                if common.as_ref().is_some_and(Vec::is_empty) {
                    break;
                }
            }
            for info in common.into_iter().flatten() {
                if seen_names.insert(info.name.clone()) {
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
