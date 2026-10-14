//! Unqualified access completion: names in scope at expression and
//! definition positions.

use lspkl_types::NodeRef;

use lsp_types::{CompletionItem, CompletionItemKind};

use lspkl_syntax::ast::nodes::Expr;
use lspkl_syntax::ast::{NodeId, NodeKind};

use lspkl_types::{AnalysisContext, ScopeKind, Type, scope_at};

use lspkl_types::members_of_type;

/// Collect completions for an unqualified position.
pub(crate) fn completions(
    node: NodeId,
    tree: &lspkl_syntax::ast::SyntaxTree,
    file: lspkl_db::File,
    ctx: &AnalysisContext,
    offset: lspkl_syntax::TextSize,
) -> Vec<CompletionItem> {
    let mut collector: Vec<CompletionItem> = Vec::new();

    let this_type = lspkl_types::this_type(ctx.db, NodeRef::new(file, node), ctx.root);
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
        let defined = collect_defined_property_names(NodeRef::new(file, node), tree, ctx);
        if add_definition_completions(node, &this_type, &defined, tree, ctx, &mut collector) {
            return collector;
        }
    }

    // Expression positions with a (possibly partial) identifier typed.
    // `find_at` usually lands on the access expression's identifier terminal,
    // so climb to the enclosing `UnqualifiedAccessExpr`.
    let access =
        super::ancestor_or_self(node, tree, |k| matches!(k, NodeKind::UnqualifiedAccessExpr));
    if let Some(access) = access {
        add_inferred_expr_type_completions(access, file, ctx, &mut collector);
        add_scope_completions(NodeRef::new(file, access), ctx, &mut collector);
        collector.extend(super::expression_level_keywords());
    }

    collector
}

fn add_definition_completions(
    node: NodeId,
    this_type: &Type,
    already_defined: &Vec<String>,
    tree: &lspkl_syntax::ast::SyntaxTree,
    ctx: &AnalysisContext,
    out: &mut Vec<CompletionItem>,
) -> bool {
    match this_type.unconstrained() {
        Type::Unknown => false,
        Type::Union { members } => {
            let mut typed_only = true;
            for member in members.iter() {
                typed_only &=
                    add_definition_completions(node, member, already_defined, tree, ctx, out);
            }
            typed_only
        }
        // Typed objects: class instances and modules. Typed objects cannot
        // have elements, so property amendments/assignments are all there is.
        ty @ (Type::Class { .. } | Type::Module { .. }) => {
            let is_class_def = enclosing_def_is_class(node, tree);
            add_typed_completions(already_defined, is_class_def, ty, ctx, out);
            out.extend(super::definition_level_keywords());
            if !matches!(tree.kind(node), NodeKind::ObjectBody) && !Expr::can_cast(tree.kind(node))
            {
                if ancestor_matches(node, tree, |k| matches!(k, NodeKind::Class)) {
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

fn enclosing_def_is_class(node: NodeId, tree: &lspkl_syntax::ast::SyntaxTree) -> bool {
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

fn ancestor_matches(
    node: NodeId,
    tree: &lspkl_syntax::ast::SyntaxTree,
    pred: impl Fn(&NodeKind) -> bool,
) -> bool {
    tree.ancestor(node, pred).is_some()
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
    for member in members_of_type(ctx.db, this_type) {
        if !member.is_property() {
            continue;
        }
        if already_defined.contains(&member.name) {
            continue;
        }
        // fixed/const properties can only be overridden in class or
        // non-amending module bodies.
        if member.is_fixed_or_const() && !is_class_or_module_def {
            continue;
        }

        let modifiers = property_modifier_prefix(&member.node, ctx);
        let detail = if member.ty != Type::Unknown {
            member.ty.render(ctx.db)
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
fn property_modifier_prefix(prop: &lspkl_types::NodeRef, ctx: &AnalysisContext) -> String {
    use lspkl_syntax::ast::TokenType;
    let Some(tree) = ctx.tree(prop.file) else {
        return String::new();
    };
    let mut prefix = String::new();
    for modifier in [TokenType::Fixed, TokenType::Const] {
        if tree.has_modifier(prop.value, modifier) {
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
    let Type::Class { type_args, .. } = mapping.unconstrained() else {
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
    let value_detail = value_type.render(ctx.db);

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
    let Type::Class { type_args, .. } = listing.unconstrained() else {
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

    let mut new_amend = super::simple_item("new {…}", "", &element_type.render(ctx.db), None);
    new_amend.insert_text = Some("new {\n  $1\n}".to_string());
    new_amend.insert_text_format = Some(lsp_types::InsertTextFormat::SNIPPET);
    out.push(new_amend);
}

fn base_member_detail(container: &Type, member: &str, ctx: &AnalysisContext) -> String {
    lspkl_types::find_member(ctx.db, container, member)
        .filter(|resolved| resolved.ty != Type::Unknown)
        .map(|resolved| resolved.ty.render(ctx.db))
        .unwrap_or_default()
}

/// Default literal for an entry key .
fn default_key(key_type: &Type, ctx: &AnalysisContext, is_snippet: bool) -> String {
    if key_type.is_subtype_of(ctx.base.string_type(), ctx.db) {
        if is_snippet {
            "\"$1\"".into()
        } else {
            "\"key\"".into()
        }
    } else if key_type.is_subtype_of(ctx.base.int_type(), ctx.db) {
        if is_snippet {
            "$1".into()
        } else {
            "123".into()
        }
    } else if key_type.is_subtype_of(ctx.base.boolean_type(), ctx.db) {
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
    match ty.unconstrained() {
        t @ Type::Class { .. } => Some(t.clone()),
        _ => {
            let unaliased = ty.unaliased(ctx.db);
            match unaliased.unconstrained() {
                t @ Type::Class { .. } => Some(t.clone()),
                _ => None,
            }
        }
    }
}

/// Names declared in the nearest definition container, excluding inherited members.
fn collect_defined_property_names(
    node: NodeRef,
    tree: &lspkl_syntax::ast::SyntaxTree,
    ctx: &AnalysisContext,
) -> Vec<String> {
    let container = |id| {
        super::ancestor_or_self(id, tree, |kind| {
            matches!(
                kind,
                NodeKind::Module | NodeKind::ClassBody | NodeKind::ObjectBody
            )
        })
    };
    let Some(owner) = container(node.value) else {
        return Vec::new();
    };
    scope_at(node, ctx)
        .entries()
        .filter(|entry| matches!(entry.kind, ScopeKind::Property | ScopeKind::Method))
        .filter(|entry| {
            entry.node(ctx).is_some_and(|source| {
                source.file == node.file && container(source.value) == Some(owner)
            })
        })
        .map(|entry| entry.name.text(ctx.db).to_owned())
        .collect()
}

// ---------------------------------------------------------------------------
// Expression-scope completions.
// ---------------------------------------------------------------------------

/// Suggest lambda literals when the expected type of the access is a
/// function type
fn add_inferred_expr_type_completions(
    node: NodeId,
    file: lspkl_db::File,
    ctx: &AnalysisContext,
    out: &mut Vec<CompletionItem>,
) {
    let expected = NodeRef::new(file, node)
        .cast(ctx, lspkl_syntax::ast::nodes::Expr::cast)
        .map_or(Type::Unknown, |expr| {
            lspkl_types::expected_type(ctx.db, expr.map(|expr| expr.id()), ctx.root)
        });
    add_lambda_completions(&expected, ctx, out);
}

fn add_lambda_completions(expected: &Type, ctx: &AnalysisContext, out: &mut Vec<CompletionItem>) {
    let unaliased = expected.unaliased(ctx.db);
    match unaliased.unconstrained() {
        Type::Union { members } => {
            for member in members.iter() {
                add_lambda_completions(member, ctx, out);
            }
        }
        Type::Class { type_args, .. } => {
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
                let detail = expected.render(ctx.db);
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
        let name = match ty.unconstrained() {
            Type::Class { class_id, .. }
                if class_id.module(ctx.db).file(ctx.db) == ctx.base.file_id()
                    && lspkl_types::module_items(ctx.db, class_id.module(ctx.db).file(ctx.db))
                        .classes[class_id.index(ctx.db)]
                    .declaration
                    .name
                    .text(ctx.db)
                        == "Int" =>
            {
                let c = INT_NAMES[next_int % INT_NAMES.len()];
                next_int += 1;
                c.to_string()
            }
            Type::Class { class_id, .. } => decapitalize(
                lspkl_types::module_items(ctx.db, class_id.module(ctx.db).file(ctx.db)).classes
                    [class_id.index(ctx.db)]
                .declaration
                .name
                .text(ctx.db),
            ),
            Type::Module { module_id } => lspkl_types::module_items(ctx.db, module_id.file(ctx.db))
                .name
                .map(|name| decapitalize(name.text(ctx.db)))
                .unwrap_or_else(|| "param".to_string()),
            Type::Variable { .. } => decapitalize(&ty.render(ctx.db)),
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

fn add_scope_completions(node: NodeRef, ctx: &AnalysisContext, out: &mut Vec<CompletionItem>) {
    for entry in scope_at(node, ctx).entries() {
        let name = entry.name.text(ctx.db);
        let kind = match entry.kind {
            ScopeKind::Property => CompletionItemKind::PROPERTY,
            ScopeKind::Method => CompletionItemKind::METHOD,
            ScopeKind::Class => CompletionItemKind::CLASS,
            ScopeKind::TypeAlias => CompletionItemKind::STRUCT,
            ScopeKind::Import => CompletionItemKind::MODULE,
            ScopeKind::Local => CompletionItemKind::VARIABLE,
        };
        let ty = entry.ty(ctx);
        let detail = if ty == Type::Unknown {
            String::new()
        } else {
            ty.render(ctx.db)
        };
        out.push(super::simple_item(name, name, &detail, Some(kind)));
    }
}
