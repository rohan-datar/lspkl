//! Unqualified access completion: names in scope at expression and
//! definition positions.

use lsp_types::{CompletionItem, CompletionItemKind};

use pkl_syntax::ast::nodes::{
    ClassBody, ClassMethod, Expr, FunctionLiteralExpr, LetExpr, MethodHeader, Module, ModuleMember,
    ObjectBody, ObjectMember, ObjectMethod, ParameterList, TypedIdentifier,
};
use pkl_syntax::ast::{NodeId, NodeKind};
use pkl_types::compute_expr::compute_this_type;
use pkl_types::infer_expr_type::infer_expr_type_from_context;
use pkl_types::resolver::resolve_unqualified_access;
use pkl_types::{AnalysisContext, Type, TypeBindings};

use super::members::members_of_type;

/// Collect completions for an unqualified position.
pub(crate) fn completions(
    node: NodeId,
    ctx: &AnalysisContext,
    offset: pkl_syntax::TextSize,
) -> Vec<CompletionItem> {
    let tree = ctx.tree;
    let bindings = TypeBindings::new();
    let mut collector: Vec<CompletionItem> = Vec::new();

    let this_type = compute_this_type(node, &bindings, ctx);
    if this_type == Type::Unknown {
        return collector;
    }

    // Definition positions: class bodies, object bodies, and module top level.
    let parent_is_object_element = tree
        .parent(node)
        .map(|p| matches!(tree.kind(p), NodeKind::ObjectElement))
        .unwrap_or(false);
    let in_definition_position = matches!(tree.kind(node), NodeKind::ClassBody)
        || matches!(tree.kind(node), NodeKind::ObjectBody)
        || parent_is_object_element
        || (matches!(tree.kind(node), NodeKind::Module)
            && tree.line_index().line_col(offset).col == 0);

    if in_definition_position {
        let defined = collect_defined_property_names(node, ctx);
        if add_definition_completions(node, &this_type, &defined, ctx, &mut collector) {
            return collector;
        }
    }

    // Expression positions with a (possibly partial) identifier typed.
    // `find_at` usually lands on the access expression's identifier terminal,
    // so climb to the enclosing `UnqualifiedAccessExpr`.
    let access =
        super::ancestor_or_self(node, ctx, |k| matches!(k, NodeKind::UnqualifiedAccessExpr));
    if let Some(access) = access {
        add_inferred_expr_type_completions(access, ctx, &mut collector);
        add_scope_completions(access, ctx, &mut collector);
        collector.extend(super::expression_level_keywords());
    }

    collector
}

fn add_definition_completions(
    node: NodeId,
    this_type: &Type,
    already_defined: &Vec<String>,
    ctx: &AnalysisContext,
    out: &mut Vec<CompletionItem>,
) -> bool {
    match this_type {
        Type::Unknown => false,
        Type::Union { left, right, .. } => {
            let l = add_definition_completions(node, left, already_defined, ctx, out);
            let r = add_definition_completions(node, right, already_defined, ctx, out);
            l && r
        }
        // Typed objects: class instances and modules. Typed objects cannot
        // have elements, so property amendments/assignments are all there is.
        ty @ (Type::Class { .. } | Type::Module { .. }) => {
            let is_class_def = enclosing_def_is_class(node, ctx);
            add_typed_completions(already_defined, is_class_def, ty, ctx, out);
            out.extend(super::definition_level_keywords());

            let tree = ctx.tree;
            if !matches!(tree.kind(node), NodeKind::ObjectBody) && !Expr::can_cast(tree.kind(node))
            {
                if ancestor_matches(node, ctx, |k| matches!(k, NodeKind::Class)) {
                    out.extend(super::class_level_keywords());
                } else if matches!(tree.kind(node), NodeKind::Module) {
                    out.extend(super::module_level_keywords());
                }
            }
            true
        }
        other => {
            let Some(class_ty) = to_class_type(other, ctx) else {
                return false;
            };
            if class_ty.class_equals(ctx.base.mapping_type()) {
                add_mapping_completions(already_defined, &class_ty, ctx, out);
                out.extend(super::definition_level_keywords());
                true // mappings cannot have elements
            } else if class_ty.class_equals(ctx.base.listing_type()) {
                add_listing_completions(already_defined, &class_ty, ctx, out);
                out.extend(super::definition_level_keywords());
                false
            } else if class_ty.class_equals(ctx.base.dynamic_type()) {
                out.extend(super::definition_level_keywords());
                false
            } else {
                false
            }
        }
    }
}

fn enclosing_def_is_class(node: NodeId, ctx: &AnalysisContext) -> bool {
    let tree = ctx.tree;
    let mut current = tree.parent(node);
    while let Some(id) = current {
        match tree.kind(id) {
            k if Expr::can_cast(k) || matches!(k, NodeKind::ObjectBody) => return false,
            NodeKind::Module => return !tree.is_amend(),
            NodeKind::Class => return true,
            _ => current = tree.parent(id),
        }
    }
    false
}

fn ancestor_matches(node: NodeId, ctx: &AnalysisContext, pred: impl Fn(&NodeKind) -> bool) -> bool {
    ctx.tree.ancestor(node, pred).is_some()
}

/// Property amendment / assignment suggestions for class and module
/// this-types
fn add_typed_completions(
    already_defined: &[String],
    is_class_or_module_def: bool,
    this_type: &Type,
    ctx: &AnalysisContext,
    out: &mut Vec<CompletionItem>,
) {
    for member in members_of_type(this_type, &TypeBindings::new(), ctx) {
        if !member.is_property() {
            continue;
        }
        if already_defined.contains(&member.name) {
            continue;
        }
        // fixed/const properties can only be overridden in class or
        // non-amending module bodies.
        if member.is_fixed_or_const(ctx) && !is_class_or_module_def {
            continue;
        }

        let modifiers = property_modifier_prefix(&member.node, ctx);
        let detail = if member.ty != Type::Unknown {
            member.ty.render(ctx)
        } else {
            Default::default()
        };

        let mut amend = super::simple_item(
            &format!("{} {{…}}", member.name),
            "",
            &detail,
            Some(CompletionItemKind::PROPERTY),
        );
        amend.insert_text = Some(format!("{modifiers}{} {{\n  $1\n}}", member.name));
        amend.insert_text_format = Some(lsp_types::InsertTextFormat::SNIPPET);
        out.push(amend);

        let assign = super::simple_item(
            &format!("{} =", member.name),
            &format!("{modifiers}{} = $1", member.name),
            &detail,
            Some(CompletionItemKind::PROPERTY),
        );
        out.push(assign);
    }
}

/// `fixed `/`const ` prefix when the declared property carries those
/// modifiers — the amending element must repeat them.
fn property_modifier_prefix(prop: &pkl_types::NodeRef, ctx: &AnalysisContext) -> String {
    use pkl_syntax::ast::TokenType;
    let Some(tree) = ctx.tree_for(prop.file_id) else {
        return String::new();
    };
    let mut prefix = String::new();
    for modifier in [TokenType::Fixed, TokenType::Const] {
        if tree.has_modifier(prop.node_id, modifier) {
            prefix.push_str(match modifier {
                TokenType::Fixed => "fixed ",
                TokenType::Const => "const ",
                _ => unreachable!(),
            });
        }
    }
    prefix
}

/// `Mapping<K, V>` body completions: `default` plus keyed entries.
fn add_mapping_completions(
    already_defined: &[String],
    mapping: &Type,
    ctx: &AnalysisContext,
    out: &mut Vec<CompletionItem>,
) {
    let Type::Class { type_args, .. } = mapping else {
        return;
    };
    let Some(key_type) = type_args.first() else {
        return;
    };
    let Some(value_type) = type_args.get(1) else {
        return;
    };

    if !already_defined.iter().any(|d| d == "default") {
        let default_detail = base_member_detail(mapping, "default", ctx);
        out.push(super::simple_item(
            "default {…}",
            "default {\n  $1\n}",
            &default_detail,
            Some(CompletionItemKind::PROPERTY),
        ));
        out.push(super::simple_item(
            "default = ",
            "default = $1",
            &default_detail,
            Some(CompletionItemKind::PROPERTY),
        ));
    }

    let key_display = default_key(key_type, ctx, false);
    let key_snippet = default_key(key_type, ctx, true);
    let value_detail = value_type.render(ctx);

    let mut entry_amend = super::simple_item(
        &format!("[{key_display}] {{…}}"),
        "",
        &value_detail,
        Some(CompletionItemKind::FIELD),
    );
    entry_amend.insert_text = Some(format!("[{key_snippet}] {{\n  $2\n}}"));
    entry_amend.insert_text_format = Some(lsp_types::InsertTextFormat::SNIPPET);
    out.push(entry_amend);

    out.push(super::simple_item(
        &format!("[{key_display}] ="),
        &format!("[{key_snippet}] = $2"),
        &value_detail,
        Some(CompletionItemKind::FIELD),
    ));
}

/// `Listing<E>` body completions: `default` plus `new {…}` elements.
fn add_listing_completions(
    already_defined: &[String],
    listing: &Type,
    ctx: &AnalysisContext,
    out: &mut Vec<CompletionItem>,
) {
    let Type::Class { type_args, .. } = listing else {
        return;
    };
    let Some(element_type) = type_args.first() else {
        return;
    };

    if !already_defined.iter().any(|d| d == "default") {
        let default_detail = base_member_detail(listing, "default", ctx);
        out.push(super::simple_item(
            "default {…}",
            "default {\n  $1\n}",
            &default_detail,
            Some(CompletionItemKind::PROPERTY),
        ));
        out.push(super::simple_item(
            "default = ",
            "default = $1",
            &default_detail,
            Some(CompletionItemKind::PROPERTY),
        ));
    }

    let mut new_amend = super::simple_item("new {…}", "", &element_type.render(ctx), None);
    new_amend.insert_text = Some("new {\n  $1\n}".to_string());
    new_amend.insert_text_format = Some(lsp_types::InsertTextFormat::SNIPPET);
    out.push(new_amend);
}

fn base_member_detail(container: &Type, member: &str, ctx: &AnalysisContext) -> String {
    pkl_types::resolver::find_member_on_type(container, member, &TypeBindings::new(), ctx)
        .filter(|resolved| resolved.ty != Type::Unknown)
        .map(|resolved| resolved.ty.render(ctx))
        .unwrap_or_default()
}

/// Default literal for an entry key .
fn default_key(key_type: &Type, ctx: &AnalysisContext, is_snippet: bool) -> String {
    if key_type.is_subtype_of(ctx.base.string_type(), ctx) {
        if is_snippet {
            "\"$1\"".into()
        } else {
            "\"key\"".into()
        }
    } else if key_type.is_subtype_of(ctx.base.int_type(), ctx) {
        if is_snippet {
            "$1".into()
        } else {
            "123".into()
        }
    } else if key_type.is_subtype_of(ctx.base.boolean_type(), ctx) {
        if is_snippet {
            "$1".into()
        } else {
            "true".into()
        }
    } else if is_snippet {
        "$1".into()
    } else {
        "key".into()
    }
}

fn to_class_type(ty: &Type, ctx: &AnalysisContext) -> Option<Type> {
    match ty {
        t @ Type::Class { .. } => Some(t.clone()),
        _ => {
            let unaliased = ty.unaliased(ctx);
            match unaliased {
                t @ Type::Class { .. } => Some(t),
                _ => None,
            }
        }
    }
}

/// Names of properties already declared in the enclosing module/class/object
/// body.
fn collect_defined_property_names(node: NodeId, ctx: &AnalysisContext) -> Vec<String> {
    let tree = ctx.tree;
    let mut current = tree.parent(node);
    while let Some(id) = current {
        match tree.kind(id) {
            NodeKind::Module | NodeKind::Class | NodeKind::ObjectBody => {
                return ModuleMember::children(tree, id)
                    .filter(|&member| {
                        matches!(
                            ModuleMember::cast(tree, member),
                            Some(ModuleMember::ClassProperty(_) | ModuleMember::ClassMethod(_))
                        )
                    })
                    .chain(ObjectMember::children(tree, id).filter(|&member| {
                        matches!(
                            ObjectMember::cast(tree, member),
                            Some(ObjectMember::ObjectProperty(_) | ObjectMember::ObjectMethod(_))
                        )
                    }))
                    .filter_map(|member| tree.name(member).map(str::to_string))
                    .collect();
            }
            _ => current = tree.parent(id),
        }
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// Expression-scope completions.
// ---------------------------------------------------------------------------

/// Suggest lambda literals when the expected type of the access is a
/// function type
fn add_inferred_expr_type_completions(
    node: NodeId,
    ctx: &AnalysisContext,
    out: &mut Vec<CompletionItem>,
) {
    let expected = infer_expr_type_from_context(node, &TypeBindings::new(), ctx);
    add_lambda_completions(&expected, ctx, out);
}

fn add_lambda_completions(expected: &Type, ctx: &AnalysisContext, out: &mut Vec<CompletionItem>) {
    let unaliased = expected.unaliased(ctx);
    match unaliased {
        Type::Union { left, right, .. } => {
            add_lambda_completions(&left, ctx, out);
            add_lambda_completions(&right, ctx, out);
        }
        Type::Class { ref type_args, .. } => {
            // Match against Function0..Function5.
            for n in 0..=5 {
                let Some(fn_ty) = ctx.base.function_n_type(n) else {
                    continue;
                };
                if !unaliased.class_equals(&fn_ty) {
                    continue;
                }
                let param_count = type_args.len().saturating_sub(1).min(n);
                let param_names = lambda_parameter_names(&type_args[..param_count], ctx);
                let display = format!("({}) -> …", param_names.join(", "));
                let placeholders: Vec<String> = param_names
                    .iter()
                    .enumerate()
                    .map(|(i, name)| format!("${{{}:{name}}}", i + 1))
                    .collect();
                let insert = format!(
                    "({}) -> ${{{}}}",
                    placeholders.join(", "),
                    param_names.len() + 1
                );
                let detail = expected.render(ctx);
                out.push(super::simple_item(
                    &display,
                    &insert,
                    &detail,
                    Some(CompletionItemKind::SNIPPET),
                ));
                break;
            }
        }
        _ => {}
    }
}

/// Derive parameter names from parameter types
fn lambda_parameter_names(param_types: &[Type], ctx: &AnalysisContext) -> Vec<String> {
    const INT_NAMES: [char; 5] = ['i', 'j', 'k', 'l', 'm'];
    let mut result: Vec<String> = Vec::new();
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut next_int = 0usize;

    for ty in param_types {
        let name = match ty {
            Type::Class { class_node, .. }
                if class_node.file_id == pkl_types::FileId::BASE
                    && ctx
                        .tree_for(class_node.file_id)
                        .and_then(|t| t.name(class_node.node_id))
                        == Some("Int") =>
            {
                let c = INT_NAMES[next_int % INT_NAMES.len()];
                next_int += 1;
                c.to_string()
            }
            Type::Class { class_node, .. } => ctx
                .tree_for(class_node.file_id)
                .and_then(|t| t.name(class_node.node_id))
                .map(decapitalize)
                .unwrap_or_else(|| "param".to_string()),
            Type::Module { reference_name, .. } => decapitalize(reference_name),
            Type::Variable { type_param, .. } => ctx
                .tree_for(type_param.file_id)
                .and_then(|t| t.name(type_param.node_id))
                .map(decapitalize)
                .unwrap_or_else(|| "param".to_string()),
            _ => "param".to_string(),
        };
        let count = counts.entry(name.clone()).or_insert(0);
        *count += 1;
        result.push(if *count == 1 {
            name
        } else {
            format!("{name}{count}")
        });
    }
    result
}

fn decapitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn add_scope_completions(node: NodeId, ctx: &AnalysisContext, out: &mut Vec<CompletionItem>) {
    let tree = ctx.tree;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Local scope walk (innermost first, so inner bindings shadow outer).
    let mut scopes: Vec<(String, CompletionItemKind)> = Vec::new();
    let mut inherited_scopes = Vec::new();
    let mut current = Some(node);
    while let Some(id) = current {
        match tree.kind(id) {
            NodeKind::LetExpr => {
                if pkl_types::resolver::let_binding_is_visible(id, node, ctx)
                    && let Some(binding) =
                        LetExpr::cast(tree, id).and_then(|expr| expr.binding(tree))
                    && let Some(name) = tree.name(binding)
                {
                    scopes.push((name.to_string(), CompletionItemKind::VARIABLE));
                }
            }
            NodeKind::FunctionLiteralExpr => {
                collect_params(id, ctx, &mut scopes);
            }
            NodeKind::ClassMethod | NodeKind::ObjectMethod => {
                collect_params(id, ctx, &mut scopes);
            }
            NodeKind::ObjectBody | NodeKind::ClassBody => {
                for member in ClassBody::cast(tree, id)
                    .into_iter()
                    .flat_map(|body| body.members(tree))
                    .chain(
                        ObjectBody::cast(tree, id)
                            .into_iter()
                            .flat_map(|body| body.members(tree)),
                    )
                {
                    if let Some(name) = tree.name(member) {
                        let kind = match (
                            ModuleMember::cast(tree, member),
                            ObjectMember::cast(tree, member),
                        ) {
                            (Some(ModuleMember::ClassMethod(_)), _)
                            | (_, Some(ObjectMember::ObjectMethod(_))) => {
                                CompletionItemKind::METHOD
                            }
                            (Some(ModuleMember::ClassProperty(_)), _)
                            | (_, Some(ObjectMember::ObjectProperty(_))) => {
                                CompletionItemKind::PROPERTY
                            }
                            _ => continue,
                        };
                        scopes.push((name.to_string(), kind));
                    }
                }
                if matches!(tree.kind(id), NodeKind::ClassBody)
                    && let Some(class) = tree.parent(id)
                {
                    for member in members_of_type(
                        &Type::class(pkl_types::NodeRef::new(ctx.file_id, class)),
                        &TypeBindings::new(),
                        ctx,
                    ) {
                        inherited_scopes.push((
                            member.name,
                            if matches!(member.kind, super::members::MemberKind::Method) {
                                CompletionItemKind::METHOD
                            } else {
                                CompletionItemKind::PROPERTY
                            },
                        ));
                    }
                }
            }
            NodeKind::Module => {
                for member in members_of_type(
                    &pkl_types::resolver::module_type(
                        pkl_types::NodeRef::new(ctx.file_id, id),
                        ctx,
                    ),
                    &TypeBindings::new(),
                    ctx,
                ) {
                    let kind = match member.kind {
                        super::members::MemberKind::Method => CompletionItemKind::METHOD,
                        super::members::MemberKind::Property => CompletionItemKind::PROPERTY,
                        super::members::MemberKind::Class => CompletionItemKind::CLASS,
                        super::members::MemberKind::TypeAlias => CompletionItemKind::STRUCT,
                    };
                    if member.node.file_id == ctx.file_id
                        && tree.parent(member.node.node_id) == Some(id)
                    {
                        scopes.push((member.name, kind));
                    } else {
                        inherited_scopes.push((member.name, kind));
                    }
                }
                for import in Module::cast(tree, id)
                    .into_iter()
                    .flat_map(|module| module.imports(tree))
                {
                    if let Some(name) = pkl_types::resolver::import_name(tree, import) {
                        scopes.push((name, CompletionItemKind::MODULE));
                    }
                }
            }
            _ => {}
        }
        current = tree.parent(id);
    }
    scopes.extend(inherited_scopes);
    // Base-module classes and typealiases (e.g. `String`, `List`).
    let base_tree = ctx.base.tree();
    for def in ModuleMember::children(base_tree, base_tree.root()).filter(|&member| {
        matches!(
            ModuleMember::cast(base_tree, member),
            Some(ModuleMember::Class(_) | ModuleMember::TypeAlias(_))
        )
    }) {
        if let Some(name) = base_tree.name(def) {
            scopes.push((name.to_string(), CompletionItemKind::CLASS));
        }
    }

    for (name, kind) in scopes {
        if !seen.insert(name.clone()) {
            continue;
        }
        // Enrich with the resolved declaration's type when possible.
        let detail = resolve_unqualified_access(node, &name, &TypeBindings::new(), ctx)
            .filter(|r| r.ty != Type::Unknown)
            .map(|r| r.ty.render(ctx))
            .unwrap_or_default();
        out.push(super::simple_item(&name, &name, &detail, Some(kind)));
    }
}

fn collect_params(
    owner: NodeId,
    ctx: &AnalysisContext,
    out: &mut Vec<(String, CompletionItemKind)>,
) {
    let tree = ctx.tree;
    let params = FunctionLiteralExpr::cast(tree, owner)
        .and_then(|function| function.parameters(tree))
        .or_else(|| {
            ClassMethod::cast(tree, owner)
                .and_then(|method| method.header(tree))
                .or_else(|| ObjectMethod::cast(tree, owner).and_then(|method| method.header(tree)))
                .and_then(|header| MethodHeader::cast(tree, header))
                .and_then(|header| header.parameters(tree))
        });
    let Some(params) = params.and_then(|params| ParameterList::cast(tree, params)) else {
        return;
    };
    for tid in params
        .parameters(tree)
        .filter(|&tid| TypedIdentifier::cast(tree, tid).is_some())
    {
        if let Some(name) = tree.name(tid) {
            out.push((name.to_string(), CompletionItemKind::VARIABLE));
        }
    }
}
