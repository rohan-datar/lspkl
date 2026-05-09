//! Convert AST `PklType` syntax nodes into resolved [`Type`] values.

use pkl_analysis::ast::{NodeId, NodeKind};

use crate::NodeRef;
use crate::context::AnalysisContext;
use crate::ty::{Type, TypeBindings};

/// Convert an AST type node into a resolved [`Type`].
///
/// `node` must point at a node whose `NodeKind` is one of the type variants
/// (`DeclaredType`, `NullableType`, `UnionType`, ...). Returns
/// [`Type::Unknown`] on unresolved names — never an error — to avoid
/// cascading diagnostics.
pub fn to_type(node: NodeId, bindings: &TypeBindings, ctx: &AnalysisContext) -> Type {
    // todo!(
    //     "match on ctx.tree.kind(node):\n\
    //      - UnknownType -> Type::Unknown\n\
    //      - NothingType -> Type::Nothing\n\
    //      - DeclaredType -> resolve simpleTypeName, dispatch on what was found:\n\
    //          * PklClass -> Type::class_with_args(node_ref, recursed_args)\n\
    //          * PklTypeAlias -> Type::Alias {{ ... }}\n\
    //          * PklTypeParameter -> bindings.get(node_ref).cloned().unwrap_or(Type::Unknown)\n\
    //          * PklModule -> Type::Module {{ ... }}\n\
    //      - NullableType -> recurse on inner, wrap via .nullable(ctx)\n\
    //      - UnionType -> recurse on left+right, build Type::Union\n\
    //      - ParenthesizedType / DefaultUnionType -> recurse on inner\n\
    //      - ConstrainedType -> recurse on inner, attach constraints\n\
    //      - StringLiteralType -> Type::string_literal(escaped_text)\n\
    //      - FunctionType -> base.function_N_type with args (skip until BaseModule exposes those)\n\
    //      - ModuleType -> Type::Module pointing at enclosing module (or base.module_type)\n\
    // )
    use NodeKind::*;
    match ctx.tree.kind(node) {
        UnknownType => Type::Unknown,
        NothingType => Type::Nothing,
        ModuleType => todo!("ModuleType"),
        StringLiteralType => Type::string_literal(
            ctx.tree
                .escaped_string_literal(node)
                .expect("could not escape string literal text"),
        ),
        DeclaredType => {
            todo!("resolve type name")
        }
        ParenthesizedType | DefaultUnionType => to_type(ctx.tree.children(node)[0], bindings, ctx),
        NullableType => to_type(ctx.tree.children(node)[0], bindings, ctx).nullable(ctx),
        ConstrainedType => {
            todo!("ConstrainedType")
        }
        UnionType => {
            todo!("UnionType")
        }
        FunctionType => {
            todo!("FunctionType")
        }
        TypeArgumentList => {
            todo!("TypeArgumentList")
        }
        TypeName => {
            todo!("TypeName")
        }
        SimpleTypeName => {
            todo!("SimpleTypeName")
        }
        ModuleName => {
            todo!("ModuleName")
        }
        _ => Type::Unknown,
    }
}
