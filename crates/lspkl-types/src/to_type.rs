//! Source adapter: lower a body-local annotation, then use metadata resolution.
use crate::{AnalysisContext, NodeRef, Type, items::*, ty::TypeBindings};
use lspkl_db::{Db, File};

pub(crate) fn owner_for_node(db: &dyn Db, node: NodeRef, root: File) -> Option<DeclId> {
    let tree = lspkl_db::parse(db, node.file).as_ref().ok()?;
    let mut current = Some(node.value);
    while let Some(id) = current {
        if let Some(decl) = decl_for_node(db, node.with_value(id), root) {
            return Some(decl);
        }
        current = tree.parent(id);
    }
    None
}
#[salsa::interned(unsafe(no_lifetime), revisions = usize::MAX)]
struct AnnotationRequest {
    #[returns(copy)]
    node: NodeRef,
    #[returns(copy)]
    root: File,
    #[returns(ref)]
    bindings: TypeBindings,
}
fn cycle_unknown(_: &dyn Db, _: salsa::Id, _: AnnotationRequest) -> Type {
    Type::Unknown
}
#[salsa::tracked(returns(clone), cycle_result = cycle_unknown)]
fn annotation_type(db: &dyn Db, request: AnnotationRequest) -> Type {
    let node = request.node(db);
    let Ok(tree) = lspkl_db::parse(db, node.file) else {
        return Type::Unknown;
    };
    let ty = lower_type(db, tree, node.value);
    let owner = owner_for_node(db, node, request.root(db));
    let bindings = request.bindings(db);
    crate::type_resolution::resolve_in_file(db, node.file, request.root(db), owner, &ty, bindings)
}
pub(crate) fn to_type_with_bindings(
    node: NodeRef,
    bindings: &TypeBindings,
    ctx: &AnalysisContext,
) -> Type {
    annotation_type(
        ctx.db,
        AnnotationRequest::new(ctx.db, node, ctx.root, bindings),
    )
}
