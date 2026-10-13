//! Enumerate members through the same declaration tables used by resolution.
use crate::{
    AnalysisContext, MemberTable, Modifiers, ModuleId, NodeRef, Type, class_id, class_members,
    item_node, module_members,
};
use crate::{
    resolver::class_member_bindings, resolver::definition_type_with_bindings, ty::TypeBindings,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Property,
    Method,
    Class,
    TypeAlias,
}

pub struct MemberInfo {
    pub name: String,
    pub node: NodeRef,
    pub ty: Type,
    pub kind: MemberKind,
    modifiers: Modifiers,
}
impl MemberInfo {
    pub fn is_property(&self) -> bool {
        self.kind == MemberKind::Property
    }
    pub fn is_fixed_or_const(&self) -> bool {
        self.modifiers
            .intersects(Modifiers::FIXED | Modifiers::CONST)
    }
}

pub fn members_of_type(ty: &Type, ctx: &AnalysisContext) -> Vec<MemberInfo> {
    collect(ty, &TypeBindings::new(), ctx)
}
fn table_members(
    table: &MemberTable,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Vec<MemberInfo> {
    table
        .entries
        .iter()
        .filter_map(|entry| {
            let node = item_node(ctx.db, entry.declaration)?;
            Some(MemberInfo {
                name: entry.name.text(ctx.db).clone(),
                node,
                ty: definition_type_with_bindings(node, bindings, ctx),
                kind: entry.kind,
                modifiers: entry.modifiers,
            })
        })
        .collect()
}
fn collect(ty: &Type, bindings: &TypeBindings, ctx: &AnalysisContext) -> Vec<MemberInfo> {
    match ty.unconstrained() {
        Type::Class {
            class_node,
            type_args,
        } => {
            let Some(class) = class_id(ctx.db, *class_node, ctx.root) else {
                return Vec::new();
            };
            let table = class_members(ctx.db, class);
            if type_args.is_empty() {
                table_members(table, bindings, ctx)
            } else {
                let bindings = class_member_bindings(*class_node, type_args, bindings, ctx);
                table_members(table, &bindings, ctx)
            }
        }
        Type::Module { module_node } => table_members(
            module_members(ctx.db, ModuleId::new(ctx.db, module_node.file, ctx.root)),
            bindings,
            ctx,
        ),
        Type::Alias { .. } => collect(&ty.unaliased(ctx), bindings, ctx),
        Type::Union { members } => {
            let mut alternatives = members.iter();
            let Some(first) = alternatives.next() else {
                return Vec::new();
            };
            let mut common = collect(first, bindings, ctx);
            for alternative in alternatives {
                let mut branch = collect(alternative, bindings, ctx);
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
                if common.is_empty() {
                    break;
                }
            }
            common
        }
        _ => Vec::new(),
    }
}
