//! Validates modifier combinations on classes, methods, properties, and type
//! aliases.

use pkl_syntax::TextRange;
use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree, TokenType};
use pkl_types::AnalysisContext;
use semver::Version;

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;
use crate::{QuickFix, TextEdit, effective_pkl_version};

pub struct ModifierAnalyzer;

impl Analyzer for ModifierAnalyzer {
    fn analyze_node(
        &self,
        node: NodeId,
        holder: &mut DiagnosticsHolder,
        ctx: &AnalysisContext,
    ) -> bool {
        use NodeKind::*;
        use TokenType::*;
        let modifiers = ctx.tree.modifiers(node);

        if !ctx.tree.has_modifier(node, Local) {
            match ctx.tree.kind(node) {
                ClassProperty
                    if ctx.tree.is_amend()
                        && (ctx.tree.has_modifier(node, Hidden)
                            || has_type_annotation(ctx.tree, node)) =>
                {
                    holder.add_error_with_fix(
                        ctx.tree.span(node),
                        "missing modifier `local`",
                        QuickFix::single(
                            "add modifier `local`",
                            TextEdit::insert_before(ctx.tree.span(node), "local "),
                        ),
                    );
                    return true;
                }
                ObjectProperty if has_type_annotation(ctx.tree, node) => {
                    holder.add_error_with_fix(
                        ctx.tree.span(node),
                        "missing modifier `local`",
                        QuickFix::single(
                            "add modifier `local`",
                            TextEdit::insert_before(ctx.tree.span(node), "local "),
                        ),
                    );
                    return true;
                }
                ObjectMethod => {
                    holder.add_error_with_fix(
                        ctx.tree.span(node),
                        "missing modifier `local`",
                        QuickFix::single(
                            "add modifier `local`",
                            TextEdit::insert_before(ctx.tree.span(node), "local "),
                        ),
                    );
                }
                Class | TypeAlias | ClassMethod if ctx.tree.is_amend() => {
                    holder.add_error_with_fix(
                        ctx.tree.span(node),
                        "missing modifier `local`",
                        QuickFix::single(
                            "add modifier `local`",
                            TextEdit::insert_before(ctx.tree.span(node), "local "),
                        ),
                    );
                    return true;
                }
                _ => {}
            }
        }

        if ctx.tree.has_modifier(node, Abstract) && ctx.tree.has_modifier(node, Open) {
            holder.add_error(
                modifier_span(ctx.tree, node, Abstract),
                "modifer `abstract` conflicts with `open`",
            );
            holder.add_error(
                modifier_span(ctx.tree, node, Open),
                "modifer `open` conflicts with `abstract`",
            );
        }

        if ctx.tree.has_modifier(node, Const)
            && !ctx.tree.has_modifier(node, Local)
            && ctx.tree.kind(node).is_object_member()
            && effective_pkl_version(ctx) >= Version::new(0, 27, 0)
        {
            holder.add_error_with_fix(
                modifier_span(ctx.tree, node, Const),
                "invalid modifier `const` without `local`",
                QuickFix::single(
                    "add modifier `local`",
                    TextEdit::insert_before(modifier_span(ctx.tree, node, Const), "local "),
                ),
            );
        }

        let (description, applicable_modifiers) = match ctx.tree.kind(node) {
            ModuleClause => {
                if ctx.tree.is_amend() {
                    ("amending modules", vec![])
                } else {
                    ("modules", vec![Abstract, Open])
                }
            }
            Class => ("classes", vec![Abstract, Open, External, Local]),
            TypeAlias => ("typealiases", vec![External, Local]),
            ClassMethod => ("class methods", vec![Abstract, External, Local, Const]),
            ClassProperty => (
                "class properties",
                vec![Abstract, External, Hidden, Local, Fixed, Const],
            ),
            ObjectMethod => ("object methods", vec![Local, Const]),
            ObjectProperty => ("object properties", vec![Local, Const]),
            _ => {
                return true;
            }
        };

        for modifier in modifiers {
            if !applicable_modifiers.contains(&modifier) {
                let modifier_text = ctx.tree.text(modifier_node(ctx.tree, node, modifier));
                holder.add_error_with_fix(
                    modifier_span(ctx.tree, node, modifier),
                    format!(
                        "modifier `{}` is not applicable for {}",
                        modifier_text, description
                    ),
                    QuickFix::single(
                        format!(
                            "delete `{}`",
                            ctx.tree.text(modifier_node(ctx.tree, node, modifier))
                        ),
                        TextEdit::replace(modifier_span(ctx.tree, node, modifier), ""),
                    ),
                );
            }
        }
        true
    }
}

fn has_type_annotation(tree: &SyntaxTree, node: NodeId) -> bool {
    tree.child(node, |c| matches!(c, NodeKind::TypeAnnotation))
        .is_some()
}

// finds the span of the provided modifier. If the modifier is not found, returns the span
// of the original node
fn modifier_span(tree: &SyntaxTree, parent_node: NodeId, modifier: TokenType) -> TextRange {
    tree.span(modifier_node(tree, parent_node, modifier))
}
fn modifier_node(tree: &SyntaxTree, parent_node: NodeId, modifier: TokenType) -> NodeId {
    let holder = match tree.kind(parent_node) {
        NodeKind::ClassMethod | NodeKind::ObjectMethod => tree
            .child(parent_node, |k| matches!(k, NodeKind::MethodHeader))
            .unwrap_or(parent_node),
        _ => parent_node,
    };
    let Some(m) = tree.child(
        holder,
        |c| matches!(c, NodeKind::Terminal(tt) if *tt == modifier),
    ) else {
        return parent_node;
    };
    m
}
