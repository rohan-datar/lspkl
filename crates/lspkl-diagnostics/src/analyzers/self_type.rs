//! Validates where the `this` and `module` types may appear: not in type
//! alias bodies, and not in const members (with Pkl's const-depth and
//! custom-`this` scoping). `module` placements are warnings until Pkl makes
//! them errors.

use lspkl_syntax::ast::{NodeId, NodeKind, SyntaxTree, TokenType, nodes};
use lspkl_types::{AnalysisContext, NodeRef};

use crate::analyzer::Analyzer;
use crate::feature::{SELF_TYPE_RULES, rule_applies};
use crate::holder::DiagnosticsHolder;

pub struct SelfTypeAnalyzer;

impl Analyzer for SelfTypeAnalyzer {
    fn analyze_node(
        &self,
        node: NodeRef,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        let Some(tree) = ctx.tree(node.file) else {
            return false;
        };
        match tree.kind(node.value) {
            // The whole file shares one target version.
            NodeKind::Module => rule_applies(ctx, node.file, &SELF_TYPE_RULES),
            NodeKind::ThisType | NodeKind::ModuleType => {
                check_self_type(node, tree, holder);
                true
            }
            _ => true,
        }
    }
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
