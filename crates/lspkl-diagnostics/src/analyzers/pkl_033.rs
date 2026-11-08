//! Static declaration checks introduced by Pkl 0.33.
//!
//! Reuse the semantic member tables (including imported and SDK ancestors), never
//! resolve inheritance by walking another file's syntax with local node IDs.
use lspkl_syntax::ast::{NodeId, NodeKind, SyntaxTree, TokenType, nodes};
use lspkl_types::{
    AnalysisContext, MemberKind, MemberTable, Modifiers, NodeRef, Type, class_id, class_members,
    module_id, module_items, module_members,
};
use semver::Version;

use crate::{Analyzer, DiagnosticsHolder};

pub struct Pkl033Analyzer;

impl Analyzer for Pkl033Analyzer {
    fn analyze_node(
        &self,
        node: NodeRef,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        let Some(tree) = ctx.tree(node.file) else {
            return false;
        };
        // A missing declaration means current language behavior, NOT the default
        // version used by UnsupportedFeatureAnalyzer for historical syntax gates.
        if *tree.kind(node.value) == NodeKind::Module
            && crate::feature::declared_pkl_version(ctx, node.file)
                .is_some_and(|version| version < Version::new(0, 33, 0))
        {
            return false;
        }
        match tree.kind(node.value) {
            NodeKind::Module | NodeKind::Class => check_type(node, tree, holder, ctx),
            NodeKind::ThisType | NodeKind::ModuleType => check_self_type(node, tree, holder),
            NodeKind::ObjectProperty => check_const_assignment(node, tree, holder, ctx),
            // Only module-level properties are instance assignments in an
            // amending module. A nested class still declares properties.
            NodeKind::ClassProperty
                if tree.is_amend() && tree.parent(node.value) == Some(tree.root()) =>
            {
                check_const_assignment(node, tree, holder, ctx);
            }
            _ => {}
        }
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

fn check_type(
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

// Pkl flattens lambdas nested in lambdas when numbering their display names.
fn lambda_scope_name(tree: &SyntaxTree, member: NodeId, lambda: NodeId) -> String {
    let mut pending = vec![member];
    let mut count = 0;
    while let Some(node) = pending.pop() {
        if *tree.kind(node) == NodeKind::FunctionLiteralExpr {
            count += 1;
        }
        if node == lambda {
            break;
        }
        if node != member
            && matches!(
                tree.kind(node),
                NodeKind::ClassProperty
                    | NodeKind::ObjectProperty
                    | NodeKind::ClassMethod
                    | NodeKind::ObjectMethod
            )
        {
            continue;
        }
        pending.extend(tree.children(node).iter().rev().copied());
    }
    format!("{}.<function#{count}>", tree.name(member).unwrap_or(""))
}

fn check_self_type(node: NodeRef, tree: &SyntaxTree, holder: &mut DiagnosticsHolder) {
    let module_type = *tree.kind(node.value) == NodeKind::ModuleType;
    let word = if module_type { "module" } else { "this" };
    let mut child = node.value;
    let mut current = tree.parent(node.value);
    let mut const_member = None;
    let mut invalid_const_this = false;
    let mut context = None;
    let mut object_scope = false;
    let mut custom_this = false;
    // Eager generator expressions execute outside their immediately enclosing
    // object. Lambdas/lets/methods do not introduce a const-depth barrier.
    let mut skip_object = false;
    let mut method_scope = false;
    let mut lambda_scope = None;
    while let Some(parent) = current {
        match tree.kind(parent) {
            NodeKind::ObjectBody => {
                if skip_object {
                    skip_object = false;
                } else if !tree.parent(parent).is_some_and(|owner| {
                    matches!(
                        tree.kind(owner),
                        NodeKind::ForGenerator | NodeKind::WhenGenerator
                    )
                }) {
                    object_scope = true;
                }
            }
            NodeKind::FunctionLiteralExpr => {
                method_scope = true;
                lambda_scope.get_or_insert(parent);
            }
            NodeKind::ObjectEntry
                if nodes::ObjectEntry::cast(tree, parent).and_then(|entry| entry.key(tree))
                    == Some(child) =>
            {
                skip_object = true;
                method_scope = true;
            }
            NodeKind::ObjectSpread => {
                skip_object = true;
                method_scope = true;
            }
            NodeKind::WhenGenerator
                if nodes::WhenGenerator::cast(tree, parent)
                    .and_then(|generator| generator.condition(tree))
                    == Some(child) =>
            {
                skip_object = true;
                method_scope = true;
            }
            NodeKind::ForGenerator
                if nodes::ForGenerator::cast(tree, parent)
                    .and_then(|generator| generator.iterable(tree))
                    == Some(child) =>
            {
                skip_object = true;
                method_scope = true;
            }
            NodeKind::MemberPredicate
                if nodes::MemberPredicate::cast(tree, parent)
                    .and_then(|predicate| predicate.condition(tree))
                    == Some(child) =>
            {
                custom_this = true;
                skip_object = true;
                method_scope = true;
            }
            // Custom-this scopes reset the inherited const level in Pkl.
            NodeKind::ConstrainedType => {
                if nodes::ConstrainedType::cast(tree, parent).and_then(|ty| ty.inner(tree))
                    != Some(child)
                {
                    custom_this = true;
                }
            }
            NodeKind::ClassProperty
            | NodeKind::ClassMethod
            | NodeKind::ObjectProperty
            | NodeKind::ObjectMethod => {
                if !custom_this && tree.has_modifier(parent, TokenType::Const) {
                    const_member = Some(parent);
                    if !object_scope && !custom_this {
                        invalid_const_this = true;
                    }
                }
            }
            NodeKind::TypeAlias => {
                if !custom_this && (module_type || !object_scope) {
                    context = Some("a type alias body");
                }
                break;
            }
            NodeKind::Class => {
                if module_type && !custom_this {
                    context = Some("a class body");
                }
                break;
            }
            NodeKind::Annotation => {
                if module_type && !custom_this {
                    context = Some("an annotation body");
                }
                break;
            }
            _ => {}
        }
        child = parent;
        current = tree.parent(parent);
    }
    let message = if let Some(member) = const_member.filter(|_| module_type || invalid_const_this) {
        let kind = if (!module_type && method_scope)
            || matches!(
                tree.kind(member),
                NodeKind::ClassMethod | NodeKind::ObjectMethod
            ) {
            "method"
        } else {
            "property"
        };
        Some(format!(
            "Cannot reference `{word}` type from const {kind} `{}`.",
            lambda_scope.filter(|_| !module_type).map_or_else(
                || tree.name(member).unwrap_or("").to_owned(),
                |lambda| lambda_scope_name(tree, member, lambda)
            )
        ))
    } else {
        context.map(|context| format!("Cannot reference `{word}` type within {context}."))
    };
    if let Some(message) = message {
        if module_type {
            holder.add_warning(
                tree.span(node.value),
                format!("{message} This will be an error in a future release."),
            );
        } else {
            holder.add_error(tree.span(node.value), message);
        }
    }
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
    let mut ty = if *tree.kind(node.value) == NodeKind::ObjectProperty {
        object_parent_type(node.file, node.value, tree, ctx)
    } else {
        lspkl_types::this_type(ctx.db, node, ctx.root)
    };
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

// this_type deliberately recovers object-local scopes as Unknown. For this
// check we only need the explicit parent, not the object's inferred members.
fn object_parent_type(
    file: lspkl_db::File,
    property: NodeId,
    tree: &SyntaxTree,
    ctx: &AnalysisContext,
) -> Type {
    let Some(body) = tree.ancestor(property, |kind| matches!(kind, NodeKind::ObjectBody)) else {
        return Type::Unknown;
    };
    let Some(parent) = tree.parent(body) else {
        return Type::Unknown;
    };
    let expression = if let Some(amend) = nodes::AmendExpr::cast(tree, parent) {
        amend.parent(tree)
    } else if nodes::NewExpr::cast(tree, parent).is_some() {
        Some(parent)
    } else if matches!(
        tree.kind(parent),
        NodeKind::ObjectProperty | NodeKind::ClassProperty
    ) {
        // Each recursive call moves to a strict ancestor body, so recursion is
        // bounded by syntax nesting, not inheritance or import graph size.
        let value = nodes::ObjectProperty::cast(tree, parent)
            .and_then(|p| p.value(tree))
            .or_else(|| nodes::ClassProperty::cast(tree, parent).and_then(|p| p.value(tree)));
        if let Some(value) = value {
            Some(value)
        } else {
            let receiver = if *tree.kind(parent) == NodeKind::ObjectProperty {
                object_parent_type(file, parent, tree, ctx)
            } else {
                // The declaration being amended shadows its inherited member
                // with an unavailable body type. Resolve on the superclass.
                let this = lspkl_types::this_type(ctx.db, NodeRef::new(file, parent), ctx.root);
                lspkl_types::resolver::super_type(&this, ctx.db).unwrap_or(Type::Unknown)
            };
            return tree
                .name(parent)
                .and_then(|name| lspkl_types::find_member(ctx.db, &receiver, name))
                .map(|member| member.ty.unaliased(ctx.db))
                .unwrap_or(Type::Unknown);
        }
    } else {
        None
    };
    expression
        .and_then(|id| NodeRef::new(file, id).cast(ctx, nodes::Expr::cast))
        .map(|expr| lspkl_types::expr_type(ctx.db, expr, ctx.root).unaliased(ctx.db))
        .unwrap_or(Type::Unknown)
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
