//! Validates class/object members: unresolved properties, assignments to
//! const members, abstract methods in concrete owners, inherited abstract
//! method obligations, and override type-parameter counts.
//!
//! Inheritance checks reuse the semantic member tables (including imported and
//! SDK ancestors) and never walk another file's syntax with local node IDs.

use lspkl_syntax::ast::{NodeId, NodeKind, SyntaxTree, TokenType, nodes};
use lspkl_types::{
    AnalysisContext, MemberKind, MemberTable, Modifiers, NodeRef, Type, class_id, class_members,
    find_member, module_id, module_items, module_members,
};

use crate::analyzer::Analyzer;
use crate::feature::{INHERITANCE_RULES, rule_applies};
use crate::holder::DiagnosticsHolder;

pub struct MemberAnalyzer;

impl Analyzer for MemberAnalyzer {
    fn analyze_node(
        &self,
        node: NodeRef,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        let file = node.file;
        let Some(source_tree) = ctx.tree(file) else {
            return false;
        };
        match source_tree.kind(node.value) {
            NodeKind::Module | NodeKind::Class => {
                if rule_applies(ctx, file, &INHERITANCE_RULES) {
                    check_inheritance(node, source_tree, holder, ctx);
                }
                return true;
            }
            NodeKind::ObjectProperty => check_const_assignment(node, source_tree, holder, ctx),
            // Only module-level properties are instance assignments in an
            // amending module. A nested class still declares properties.
            NodeKind::ClassProperty
                if source_tree.is_amend()
                    && source_tree.parent(node.value) == Some(source_tree.root()) =>
            {
                check_const_assignment(node, source_tree, holder, ctx);
            }
            NodeKind::ClassProperty => {}
            _ => return true,
        }
        let node = node.value;

        // Local properties define lexical bindings, not receiver overrides.
        if source_tree.has_modifier(node, lspkl_syntax::ast::TokenType::Local) {
            return true;
        }

        // Defining (has TypeAnnotation child) — never flag.
        if nodes::ClassProperty::cast(source_tree, node)
            .and_then(|property| property.type_annotation(source_tree))
            .or_else(|| {
                nodes::ObjectProperty::cast(source_tree, node)
                    .and_then(|property| property.type_annotation(source_tree))
            })
            .is_some()
        {
            return true;
        }

        let name = source_tree.name(node).unwrap_or("");
        if name.is_empty() {
            return true;
        }

        let this_type =
            lspkl_types::this_type(ctx.db, NodeRef::new(file, node), ctx.root).unaliased(ctx.db);

        // Unknown, Dynamic, and non-Object receivers have no analyzable members.
        if matches!(this_type, Type::Unknown) {
            return true;
        }
        if this_type.class_equals(ctx.base.dynamic_type()) {
            return true;
        }
        if !this_type.is_subtype_of(ctx.base.object_type(), ctx.db) {
            return true;
        }

        // Look up the property name on the this-type. If found → OK; if not → flag.
        if find_member(ctx.db, &this_type, name).is_some() {
            return true;
        }

        let ident_span = source_tree
            .identifier(node)
            .map(|i| source_tree.span(i))
            .unwrap_or_else(|| source_tree.span(node));
        holder.add_warning(ident_span, format!("unresolved property: {name}"));
        true
    }
}

fn table<'a>(ty: &Type, ctx: &'a AnalysisContext) -> Option<&'a MemberTable> {
    match ty.unconstrained() {
        Type::Class { class_id, .. } => Some(class_members(ctx.db, *class_id)),
        Type::Module { module_id } => Some(module_members(ctx.db, *module_id)),
        _ => None,
    }
}

fn check_inheritance(
    node: NodeRef,
    tree: &SyntaxTree,
    holder: &mut DiagnosticsHolder,
    ctx: &AnalysisContext,
) {
    let is_module = *tree.kind(node.value) == NodeKind::Module;
    let (ty, header, abstract_type) = if is_module {
        // Amending modules do not declare a new class.
        if tree.is_amend() {
            return;
        }
        let header = nodes::Module::cast(tree, node.value)
            .and_then(|module| module.header(tree))
            .and_then(|header| tree.child(header, |kind| matches!(kind, NodeKind::ModuleClause)));
        (
            Type::Module {
                module_id: module_id(ctx.db, node.file, ctx.root),
            },
            header.unwrap_or(node.value),
            header.is_some_and(|header| tree.has_modifier(header, TokenType::Abstract)),
        )
    } else {
        let Some(id) = class_id(ctx.db, node, ctx.root) else {
            return;
        };
        (
            Type::class(id),
            node.value,
            tree.has_modifier(node.value, TokenType::Abstract),
        )
    };
    let name = tree
        .name(header)
        .unwrap_or(if is_module { "module" } else { "class" });
    let span = tree
        .identifier(header)
        .map(|id| tree.span(id))
        .unwrap_or_else(|| tree.span(header));
    let parent = lspkl_types::resolver::super_type(&ty, ctx.db);
    let parent_methods = inherited_methods(parent, ctx);
    let own_methods = declared_methods(&ty, ctx);
    for member in &own_methods {
        if member.kind != MemberKind::Method {
            continue;
        }
        let Some(method) = lspkl_types::item_node(ctx.db, member.declaration) else {
            continue;
        };
        if !abstract_type && member.modifiers.contains(Modifiers::ABSTRACT) {
            let method_header = nodes::ClassMethod::cast(tree, method.value)
                .and_then(|method| method.header(tree))
                .unwrap_or(method.value);
            let modifier = tree
                .child(method_header, |kind| {
                    matches!(kind, NodeKind::Terminal(TokenType::Abstract))
                })
                .unwrap_or(method_header);
            holder.add_error(
                tree.span(modifier),
                format!(
                    "Cannot define an abstract method in a non-abstract {}.",
                    if is_module { "module" } else { "class" }
                ),
            );
        }
        if member.modifiers.contains(Modifiers::LOCAL) {
            continue;
        }
        let Some(inherited) = parent_methods
            .iter()
            .find(|entry| entry.name == member.name)
        else {
            continue;
        };
        let lspkl_types::ItemId::Method(own) = member.declaration.item else {
            continue;
        };
        let lspkl_types::ItemId::Method(parent) = inherited.declaration.item else {
            continue;
        };
        let actual = module_items(ctx.db, member.declaration.file).methods[own]
            .type_parameters
            .len();
        let expected = module_items(ctx.db, inherited.declaration.file).methods[parent]
            .type_parameters
            .len();
        if actual != expected {
            let method_span = tree
                .identifier(method.value)
                .map(|id| tree.span(id))
                .unwrap_or_else(|| tree.span(method.value));
            holder.add_error(method_span, format!("Expected method `{}` overriding `{}` to have {expected} type parameter{}, but got {actual}.", member.name.text(ctx.db), inherited.name.text(ctx.db), if expected == 1 { "" } else { "s" }));
        }
    }
    if abstract_type {
        return;
    }
    // Only inherited methods: a directly declared abstract method already has a
    // more precise diagnostic on its offending modifier.
    for member in &parent_methods {
        let overridden = own_methods
            .iter()
            .any(|own| own.name == member.name && !own.modifiers.contains(Modifiers::LOCAL));
        if !overridden && member.modifiers.contains(Modifiers::ABSTRACT) {
            holder.add_error(span, format!("Class `{name}` should either be declared `abstract`, or should implement method `{}`.", method_call_signature(member, ctx)));
        }
    }
}

fn method_call_signature(member: &lspkl_types::MemberDecl, ctx: &AnalysisContext) -> String {
    let lspkl_types::ItemId::Method(index) = member.declaration.item else {
        return member.name.text(ctx.db).to_owned();
    };
    let parameters = module_items(ctx.db, member.declaration.file).methods[index]
        .parameters
        .iter()
        .map(|parameter| parameter.name.map_or("_", |name| name.text(ctx.db)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}({parameters})", member.name.text(ctx.db))
}

fn check_const_assignment(
    node: NodeRef,
    tree: &SyntaxTree,
    holder: &mut DiagnosticsHolder,
    ctx: &AnalysisContext,
) {
    if tree.has_modifier(node.value, TokenType::Local) {
        return;
    }
    let Some(name) = tree.name(node.value) else {
        return;
    };
    let mut ty = lspkl_types::this_type(ctx.db, node, ctx.root);
    if *tree.kind(node.value) == NodeKind::ClassProperty {
        ty = lspkl_types::resolver::super_type(&ty, ctx.db).unwrap_or(Type::Unknown);
    }
    let Some(members) = table(&ty, ctx) else {
        return;
    };
    if !members.entries.iter().any(|member| {
        member.kind == MemberKind::Property
            && member.name.text(ctx.db) == name
            && member.modifiers.contains(Modifiers::CONST)
    }) {
        return;
    }
    let amending = nodes::ObjectProperty::cast(tree, node.value)
        .is_some_and(|p| p.value(tree).is_none())
        || nodes::ClassProperty::cast(tree, node.value).is_some_and(|p| p.value(tree).is_none());
    let span = tree
        .identifier(node.value)
        .map(|id| tree.span(id))
        .unwrap_or_else(|| tree.span(node.value));
    holder.add_error(
        span,
        if amending {
            format!("Cannot amend const property `{name}`.")
        } else {
            format!("Cannot assign to const property `{name}`.")
        },
    );
}

// MemberTable serves name lookup across all declaration kinds. Method obligations
// have a separate namespace and local methods never implement inherited methods.
fn declared_methods(ty: &Type, ctx: &AnalysisContext) -> Vec<lspkl_types::MemberDecl> {
    let (module, class) = match ty.unconstrained() {
        Type::Module { module_id } => (*module_id, None),
        Type::Class { class_id, .. } => (class_id.module(ctx.db), Some(class_id.index(ctx.db))),
        _ => return Vec::new(),
    };
    let file = module.file(ctx.db);
    let items = module_items(ctx.db, file);
    let declarations = class.map_or(&items.declarations, |index| &items.classes[index].members);
    declarations
        .iter()
        .filter_map(|&item| {
            let lspkl_types::ItemId::Method(index) = item else {
                return None;
            };
            let declaration = &items.methods[index].declaration;
            Some(lspkl_types::MemberDecl {
                name: declaration.name,
                declaration: lspkl_types::DeclId {
                    file,
                    root: module.root(ctx.db),
                    item,
                },
                kind: MemberKind::Method,
                modifiers: declaration.modifiers,
            })
        })
        .collect()
}

fn inherited_methods(
    mut parent: Option<Type>,
    ctx: &AnalysisContext,
) -> Vec<lspkl_types::MemberDecl> {
    let mut visited = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    let mut methods = Vec::new();
    while let Some(ty) = parent {
        if !visited.insert(ty.clone()) {
            break;
        }
        for member in declared_methods(&ty, ctx) {
            if !member.modifiers.contains(Modifiers::LOCAL) && names.insert(member.name) {
                methods.push(member);
            }
        }
        parent = lspkl_types::resolver::super_type(&ty, ctx.db);
    }
    methods
}
