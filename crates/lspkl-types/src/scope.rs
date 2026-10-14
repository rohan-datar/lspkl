//! One lexical/member scope chain for resolution, completion, and diagnostics.
mod receiver;
use receiver::ReceiverEntries;

use crate::{
    AnalysisContext, DeclId, MemberKind, MemberTable, Name, NodeRef, Type, class_id, class_members,
    import_declarations, item_node, module_members,
};
use crate::{compute_expr::compute_expr_type, resolver, ty::TypeBindings};
use lspkl_db::{Db, File, FileStatus};
use lspkl_syntax::ast::{NodeId, NodeKind, SyntaxTree, nodes};
use std::{collections::HashSet, ops::Range, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    Property,
    Method,
    Class,
    TypeAlias,
    Import,
    Local,
}
impl From<MemberKind> for ScopeKind {
    fn from(kind: MemberKind) -> Self {
        match kind {
            MemberKind::Property => Self::Property,
            MemberKind::Method => Self::Method,
            MemberKind::Class => Self::Class,
            MemberKind::TypeAlias => Self::TypeAlias,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Declaration(DeclId),
    Import(DeclId),
    Local(NodeRef),
    Receiver {
        declaration: DeclId,
        owner: NodeRef,
    },
    Let(NodeRef),
    For {
        binding: NodeRef,
        generator: NodeRef,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    name: Name,
    kind: ScopeKind,
    target: Target,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Layer {
    Locals(Vec<Entry>),
    Members {
        table: Arc<MemberTable>,
        range: Range<usize>,
    },
    Imports(File),
    Receiver {
        owner: NodeRef,
        inherited: bool,
    },
}

impl Layer {
    fn entries<'a>(&'a self, ctx: &'a AnalysisContext<'a>) -> impl Iterator<Item = Entry> + 'a {
        let db = ctx.db;
        let locals = match self {
            Self::Locals(entries) => entries.as_slice(),
            _ => &[],
        };
        let members = match self {
            Self::Members { table, range } => &table.entries[range.clone()],
            _ => &[],
        };
        let imports = match self {
            Self::Imports(file) => Some(*file),
            _ => None,
        };
        locals
            .iter()
            .copied()
            .chain(members.iter().map(|member| Entry {
                name: member.name,
                kind: ScopeKind::from(member.kind),
                target: Target::Declaration(member.declaration),
            }))
            .chain(imports.into_iter().flat_map(move |file| {
                import_declarations(db, file, ctx.root).map(|(name, declaration)| Entry {
                    name,
                    kind: ScopeKind::Import,
                    target: Target::Import(declaration),
                })
            }))
            .chain(
                match self {
                    Self::Receiver { owner, inherited } => {
                        Some(ReceiverEntries::new(*owner, *inherited, ctx))
                    }
                    _ => None,
                }
                .into_iter()
                .flatten(),
            )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct ScopeData {
    locals: Vec<Layer>,
    own: Vec<Layer>,
    inherited: Vec<Layer>,
}
impl ScopeData {
    fn layers(&self) -> impl Iterator<Item = &Layer> {
        self.locals.iter().chain(&self.own).chain(&self.inherited)
    }
}

/// The chain retains cached tables, not copies of every SDK declaration.
pub struct Scope<'db> {
    ctx: &'db AnalysisContext<'db>,
    data: &'db ScopeData,
}

#[derive(Clone, Copy, Debug)]
pub struct ScopeEntry {
    pub name: Name,
    pub kind: ScopeKind,
    target: Target,
}

impl ScopeEntry {
    /// Source mapping is deferred until a consumer asks for a declaration location.
    pub fn node(&self, ctx: &AnalysisContext) -> Option<NodeRef> {
        match self.target {
            Target::Declaration(declaration) | Target::Receiver { declaration, .. } => {
                item_node(ctx.db, declaration)
            }
            Target::Import(declaration) => {
                let source = item_node(ctx.db, declaration)?;
                Some(
                    ctx.import(source)
                        .and_then(|resolution| resolution.single())
                        .unwrap_or(source),
                )
            }
            Target::Local(node) | Target::Let(node) => Some(node),
            Target::For { binding, .. } => Some(binding),
        }
    }
    pub fn ty(&self, ctx: &AnalysisContext) -> Type {
        self.ty_with_bindings(&TypeBindings::new(), ctx)
    }
    pub(crate) fn ty_with_bindings(&self, bindings: &TypeBindings, ctx: &AnalysisContext) -> Type {
        match self.target {
            Target::Import(declaration) => crate::type_resolution::import_type(ctx.db, declaration),
            Target::Declaration(declaration) => {
                resolver::declaration_type_with_bindings(ctx.db, declaration, bindings)
            }
            Target::Receiver { owner, .. } => {
                let receiver = crate::compute_expr::compute_this_type(owner, bindings, ctx);
                resolver::find_member_on_type(&receiver, self.name.text(ctx.db), bindings, ctx)
                    .map_or(Type::Unknown, |resolved| resolved.ty)
            }
            Target::Let(node) => {
                let Some(tree) = ctx.tree(node.file) else {
                    return Type::Unknown;
                };
                if resolver::member_type_annotation(tree, node.value).is_some() {
                    return resolver::definition_type_with_bindings(node, bindings, ctx);
                }
                tree.parent(node.value)
                    .and_then(|parent| nodes::LetExpr::cast(tree, parent))
                    .and_then(|expression| expression.value(tree))
                    .map_or(Type::Unknown, |value| {
                        compute_expr_type(node.with_value(value), bindings, ctx)
                    })
            }
            Target::For { binding, generator } => {
                let Some(tree) = ctx.tree(binding.file) else {
                    return Type::Unknown;
                };
                if resolver::member_type_annotation(tree, binding.value).is_some() {
                    return resolver::definition_type_with_bindings(binding, bindings, ctx);
                }
                let Some(generator_view) = nodes::ForGenerator::cast(tree, generator.value) else {
                    return Type::Unknown;
                };
                let Some(iterable) = generator_view.iterable(tree) else {
                    return Type::Unknown;
                };
                let container = compute_expr_type(generator.with_value(iterable), bindings, ctx);
                let mut parameters = generator_view.bindings(tree);
                let key = parameters.next();
                if parameters.next().is_some() && key == Some(binding.value) {
                    resolver::container_key_type(&container, ctx).unwrap_or(Type::Unknown)
                } else {
                    resolver::container_value_type(&container, ctx).unwrap_or(Type::Unknown)
                }
            }
            _ => self.node(ctx).map_or(Type::Unknown, |node| {
                resolver::definition_type_with_bindings(node, bindings, ctx)
            }),
        }
    }
}

impl Scope<'_> {
    pub fn lookup(&self, name: &str) -> Option<ScopeEntry> {
        self.data
            .layers()
            .flat_map(|layer| layer.entries(self.ctx))
            .find(|entry| entry.name.text(self.ctx.db) == name)
            .map(|entry| ScopeEntry {
                name: entry.name,
                kind: entry.kind,
                target: entry.target,
            })
    }
    pub fn entries(&self) -> impl Iterator<Item = ScopeEntry> + '_ {
        let mut seen = HashSet::new();
        self.data
            .layers()
            .flat_map(|layer| layer.entries(self.ctx))
            .filter(move |entry| seen.insert(entry.name))
            .map(|entry| ScopeEntry {
                name: entry.name,
                kind: entry.kind,
                target: entry.target,
            })
    }
    /// Qualified type names use the import namespace, not expression shadowing.
    pub fn lookup_import(&self, name: &str) -> Option<ScopeEntry> {
        self.data
            .own
            .iter()
            .filter(|layer| matches!(layer, Layer::Imports(_)))
            .flat_map(|layer| layer.entries(self.ctx))
            .find(|entry| entry.name.text(self.ctx.db) == name)
            .map(|entry| ScopeEntry {
                name: entry.name,
                kind: entry.kind,
                target: entry.target,
            })
    }
}

pub fn scope_at<'db>(node: NodeRef, ctx: &'db AnalysisContext<'db>) -> Scope<'db> {
    Scope {
        ctx,
        data: scope_data(ctx.db, ctx.root, node).as_ref(),
    }
}

fn descendant(tree: &SyntaxTree, node: NodeId, ancestor: NodeId) -> bool {
    let mut current = Some(node);
    while let Some(id) = current {
        if id == ancestor {
            return true;
        }
        current = tree.parent(id);
    }
    false
}

fn local(
    db: &dyn Db,
    tree: &SyntaxTree,
    node: NodeRef,
    kind: ScopeKind,
    target: Target,
) -> Option<Entry> {
    let name = tree.name(node.value)?;
    Some(Entry {
        name: Name::new(db, name),
        kind,
        target,
    })
}

fn parameters(db: &dyn Db, tree: &SyntaxTree, file: File, list: Option<NodeId>) -> Vec<Entry> {
    list.and_then(|list| nodes::ParameterList::cast(tree, list))
        .into_iter()
        .flat_map(|list| list.parameters(tree))
        .filter_map(|node| {
            local(
                db,
                tree,
                NodeRef::new(file, node),
                ScopeKind::Local,
                Target::Local(NodeRef::new(file, node)),
            )
        })
        .collect()
}

fn members(table: &Arc<MemberTable>, layers: &mut Vec<Layer>, inherited: &mut Vec<Layer>) {
    if table.own_len > 0 {
        layers.push(Layer::Members {
            table: table.clone(),
            range: 0..table.own_len,
        });
    }
    if table.own_len < table.entries.len() {
        inherited.push(Layer::Members {
            table: table.clone(),
            range: table.own_len..table.entries.len(),
        });
    }
}

#[salsa::tracked(returns(ref))]
fn scope_data(db: &dyn Db, root: File, node: NodeRef) -> Arc<ScopeData> {
    let mut result = ScopeData::default();
    if node.file.status(db) == FileStatus::Missing {
        return Arc::new(result);
    }
    let Ok(tree) = lspkl_db::parse(db, node.file) else {
        return Arc::new(result);
    };
    let mut current = Some(node.value);
    while let Some(id) = current {
        let owner = node.with_value(id);
        match tree.kind(id) {
            NodeKind::ConstrainedType => {
                result.own.push(Layer::Receiver {
                    owner,
                    inherited: false,
                });
                result.inherited.push(Layer::Receiver {
                    owner,
                    inherited: true,
                });
            }
            NodeKind::LetExpr => {
                if let Some(expression) = nodes::LetExpr::cast(tree, id)
                    && expression
                        .body(tree)
                        .is_some_and(|body| descendant(tree, node.value, body))
                    && let Some(binding) = expression.binding(tree)
                    && let Some(entry) = local(
                        db,
                        tree,
                        node.with_value(binding),
                        ScopeKind::Local,
                        Target::Let(node.with_value(binding)),
                    )
                {
                    result.locals.push(Layer::Locals(vec![entry]));
                }
            }
            NodeKind::ForGenerator => {
                if let Some(generator) = nodes::ForGenerator::cast(tree, id)
                    && generator
                        .body(tree)
                        .is_some_and(|body| descendant(tree, node.value, body))
                {
                    let entries = generator
                        .bindings(tree)
                        .filter_map(|binding| {
                            local(
                                db,
                                tree,
                                node.with_value(binding),
                                ScopeKind::Local,
                                Target::For {
                                    binding: node.with_value(binding),
                                    generator: owner,
                                },
                            )
                        })
                        .collect();
                    result.locals.push(Layer::Locals(entries));
                }
            }
            NodeKind::FunctionLiteralExpr => {
                let list = nodes::FunctionLiteralExpr::cast(tree, id)
                    .and_then(|function| function.parameters(tree));
                result
                    .locals
                    .push(Layer::Locals(parameters(db, tree, node.file, list)));
            }
            NodeKind::ClassMethod | NodeKind::ObjectMethod => {
                let list = nodes::ClassMethod::cast(tree, id)
                    .and_then(|method| method.header(tree))
                    .or_else(|| {
                        nodes::ObjectMethod::cast(tree, id).and_then(|method| method.header(tree))
                    })
                    .and_then(|header| nodes::MethodHeader::cast(tree, header))
                    .and_then(|header| header.parameters(tree));
                result
                    .locals
                    .push(Layer::Locals(parameters(db, tree, node.file, list)));
            }
            NodeKind::ObjectBody => {
                if let Some(body) = nodes::ObjectBody::cast(tree, id) {
                    let mut entries: Vec<_> = body
                        .parameters(tree)
                        .filter_map(|parameter| {
                            local(
                                db,
                                tree,
                                node.with_value(parameter),
                                ScopeKind::Local,
                                Target::Local(node.with_value(parameter)),
                            )
                        })
                        .collect();
                    entries.extend(body.members(tree).filter_map(|member| {
                        let kind = match nodes::ObjectMember::cast(tree, member)? {
                            nodes::ObjectMember::ObjectProperty(_) => ScopeKind::Property,
                            nodes::ObjectMember::ObjectMethod(_) => ScopeKind::Method,
                            _ => return None,
                        };
                        local(
                            db,
                            tree,
                            node.with_value(member),
                            kind,
                            Target::Local(node.with_value(member)),
                        )
                    }));
                    result.locals.push(Layer::Locals(entries));
                }
            }
            NodeKind::ClassBody => {
                if let Some(class) = tree
                    .parent(id)
                    .and_then(|class| class_id(db, node.with_value(class), root))
                {
                    members(
                        class_members(db, class),
                        &mut result.own,
                        &mut result.inherited,
                    );
                }
            }
            NodeKind::Module => {
                members(
                    module_members(db, crate::module_id(db, node.file, root)),
                    &mut result.own,
                    &mut result.inherited,
                );
                result.own.push(Layer::Imports(node.file));
            }
            _ => {}
        }
        current = tree.parent(id);
    }
    // The base module is a lexical namespace, distinct from Module instance members.
    if let Some(base) = crate::member_queries::base_module(db, root) {
        let table = module_members(db, base);
        result.inherited.push(Layer::Members {
            table: table.clone(),
            range: 0..table.own_len,
        });
    }
    Arc::new(result)
}
