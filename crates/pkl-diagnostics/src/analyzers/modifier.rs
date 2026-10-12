//! Validates modifier combinations on classes, methods, properties, and type
//! aliases.

use pkl_types::NodeRef;

use pkl_syntax::TextRange;
use pkl_syntax::ast::{NodeId, NodeKind, SyntaxTree, TokenType, nodes};
use pkl_types::AnalysisContext;
use semver::Version;

use crate::analyzer::Analyzer;
use crate::holder::DiagnosticsHolder;
use crate::{QuickFix, TextEdit, effective_pkl_version};

pub struct ModifierAnalyzer;

impl Analyzer for ModifierAnalyzer {
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
        let node = node.value;
        use NodeKind::*;
        use TokenType::*;
        let modifiers = source_tree.modifiers(node);

        if !source_tree.has_modifier(node, Local) {
            match source_tree.kind(node) {
                ClassProperty
                    if source_tree.is_amend()
                        && (source_tree.has_modifier(node, Hidden)
                            || has_type_annotation(source_tree, node)) =>
                {
                    holder.add_error_with_fix(
                        source_tree.span(node),
                        "missing modifier `local`",
                        QuickFix::single(
                            "add modifier `local`",
                            TextEdit::insert_before(source_tree.span(node), "local "),
                        ),
                    );
                    return true;
                }
                ObjectProperty if has_type_annotation(source_tree, node) => {
                    holder.add_error_with_fix(
                        source_tree.span(node),
                        "missing modifier `local`",
                        QuickFix::single(
                            "add modifier `local`",
                            TextEdit::insert_before(source_tree.span(node), "local "),
                        ),
                    );
                    return true;
                }
                ObjectMethod => {
                    holder.add_error_with_fix(
                        source_tree.span(node),
                        "missing modifier `local`",
                        QuickFix::single(
                            "add modifier `local`",
                            TextEdit::insert_before(source_tree.span(node), "local "),
                        ),
                    );
                }
                Class | TypeAlias | ClassMethod if source_tree.is_amend() => {
                    holder.add_error_with_fix(
                        source_tree.span(node),
                        "missing modifier `local`",
                        QuickFix::single(
                            "add modifier `local`",
                            TextEdit::insert_before(source_tree.span(node), "local "),
                        ),
                    );
                    return true;
                }
                _ => {}
            }
        }

        if source_tree.has_modifier(node, Abstract) && source_tree.has_modifier(node, Open) {
            holder.add_error(
                modifier_span(source_tree, node, Abstract),
                "modifer `abstract` conflicts with `open`",
            );
            holder.add_error(
                modifier_span(source_tree, node, Open),
                "modifer `open` conflicts with `abstract`",
            );
        }

        if source_tree.has_modifier(node, Const)
            && !source_tree.has_modifier(node, Local)
            && nodes::ObjectMember::can_cast(source_tree.kind(node))
            && effective_pkl_version(ctx, file) >= Version::new(0, 27, 0)
        {
            holder.add_error_with_fix(
                modifier_span(source_tree, node, Const),
                "invalid modifier `const` without `local`",
                QuickFix::single(
                    "add modifier `local`",
                    TextEdit::insert_before(modifier_span(source_tree, node, Const), "local "),
                ),
            );
        }

        let (description, applicable_modifiers) = match source_tree.kind(node) {
            ModuleClause => {
                if source_tree.is_amend() {
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
                let modifier_text = source_tree.text(modifier_node(source_tree, node, modifier));
                holder.add_error_with_fix(
                    modifier_span(source_tree, node, modifier),
                    format!(
                        "modifier `{}` is not applicable for {}",
                        modifier_text, description
                    ),
                    QuickFix::single(
                        format!(
                            "delete `{}`",
                            source_tree.text(modifier_node(source_tree, node, modifier))
                        ),
                        TextEdit::replace(modifier_span(source_tree, node, modifier), ""),
                    ),
                );
            }
        }
        true
    }
}

fn has_type_annotation(tree: &SyntaxTree, node: NodeId) -> bool {
    nodes::ClassProperty::cast(tree, node)
        .and_then(|property| property.type_annotation(tree))
        .or_else(|| {
            nodes::ObjectProperty::cast(tree, node)
                .and_then(|property| property.type_annotation(tree))
        })
        .is_some()
}

// finds the span of the provided modifier. If the modifier is not found, returns the span
// of the original node
fn modifier_span(tree: &SyntaxTree, parent_node: NodeId, modifier: TokenType) -> TextRange {
    tree.span(modifier_node(tree, parent_node, modifier))
}
fn modifier_node(tree: &SyntaxTree, parent_node: NodeId, modifier: TokenType) -> NodeId {
    let holder = match tree.kind(parent_node) {
        NodeKind::ClassMethod | NodeKind::ObjectMethod => {
            nodes::ClassMethod::cast(tree, parent_node)
                .and_then(|method| method.header(tree))
                .or_else(|| {
                    nodes::ObjectMethod::cast(tree, parent_node)
                        .and_then(|method| method.header(tree))
                })
                .unwrap_or(parent_node)
        }
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
