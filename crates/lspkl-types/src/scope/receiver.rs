//! Constraint receivers use stable member tables; member types remain demand-driven.
use super::{Entry, ScopeKind, Target};
use crate::{
    AnalysisContext, MemberTable, NodeRef, Type, class_members, compute_expr::compute_this_type,
    module_members, ty::TypeBindings,
};

pub(super) struct ReceiverEntries<'a> {
    ctx: &'a AnalysisContext<'a>,
    owner: NodeRef,
    current: Option<Type>,
    unions: Vec<std::vec::IntoIter<Type>>,
    table: Option<&'a MemberTable>,
    index: usize,
    limit: usize,
    inherited: bool,
}

impl<'a> ReceiverEntries<'a> {
    pub(super) fn new(owner: NodeRef, inherited: bool, ctx: &'a AnalysisContext<'a>) -> Self {
        Self {
            ctx,
            owner,
            current: Some(compute_this_type(owner, &TypeBindings::new(), ctx)),
            unions: Vec::new(),
            table: None,
            index: 0,
            limit: 0,
            inherited,
        }
    }
}

impl Iterator for ReceiverEntries<'_> {
    type Item = Entry;

    fn next(&mut self) -> Option<Entry> {
        loop {
            if self.index < self.limit
                && let Some(member) = self.table.and_then(|table| table.entries.get(self.index))
            {
                self.index += 1;
                return Some(Entry {
                    name: member.name,
                    kind: ScopeKind::from(member.kind),
                    target: Target::Receiver {
                        declaration: member.declaration,
                        owner: self.owner,
                    },
                });
            }
            self.table = None;
            self.index = 0;
            let ty = if let Some(ty) = self.current.take() {
                ty
            } else {
                loop {
                    let alternatives = self.unions.last_mut()?;
                    if let Some(ty) = alternatives.next() {
                        break ty;
                    }
                    self.unions.pop();
                }
            };
            match ty {
                Type::Constrained { base, .. } => self.current = Some(*base),
                alias @ Type::Alias { .. } => self.current = Some(alias.unaliased(self.ctx.db)),
                Type::Union { members } => self.unions.push(members.into_iter()),
                Type::Class { class_id, .. } => {
                    self.table = Some(class_members(self.ctx.db, class_id).as_ref());
                }
                Type::Module { module_id } => {
                    self.table = Some(module_members(self.ctx.db, module_id).as_ref());
                }
                _ => {}
            }
            if let Some(table) = self.table {
                self.index = if self.inherited { table.own_len } else { 0 };
                self.limit = if self.inherited {
                    table.entries.len()
                } else {
                    table.own_len
                };
            }
        }
    }
}
