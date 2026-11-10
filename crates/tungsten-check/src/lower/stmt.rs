//! Statements, and locals as assignment targets and reads.

use tungsten_core::{LocalId, Stmt};
use tungsten_ir::{Ref, Statement, Var, VariableType};

use super::{Ctx, get};
use crate::error::{EntityKind, LowerErrorKind, Step};

impl<'p, 'r> Ctx<'p, 'r> {
    /// Lowers a block; `step(i)` is the path step of statement `i`.
    pub(super) fn block(&mut self, stmts: &[Statement], step: fn(usize) -> Step) -> Vec<Stmt> {
        stmts
            .iter()
            .enumerate()
            .map(|(i, s)| self.at(step(i), |cx| cx.stmt(s)))
            .collect()
    }

    fn stmt(&mut self, s: &Statement) -> Stmt {
        match s {
            Statement::Assign { target, value } => {
                let target = self.local(*target);
                let value = self.at(Step::Value, |cx| cx.expr(value));
                Stmt::Assign { target, value }
            }
            Statement::Return(e) => Stmt::Return(self.at(Step::Value, |cx| cx.expr(e))),
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => Stmt::If {
                cond: self.at(Step::Cond, |cx| cx.expr(condition)),
                then_branch: self.block(then_branch, Step::Then),
                else_branch: self.block(else_branch, Step::Else),
            },
            Statement::While { condition, body } => Stmt::While {
                cond: self.at(Step::Cond, |cx| cx.expr(condition)),
                body: self.block(body, Step::Body),
            },
            Statement::Repeat { count, body } => Stmt::Repeat {
                count: *count,
                body: self.block(body, Step::Body),
            },
            Statement::Break => Stmt::Break,
        }
    }

    /// A var read or assignment target in the current function.
    pub(super) fn local(&mut self, r: Ref<Var>) -> LocalId {
        if let Some(id) = self.locals.get(&r) {
            return *id;
        }
        let var = self.func.and_then(|f| get(&f.variables, r));
        match var {
            Some(v) if v.t == VariableType::DensityInput => {
                self.err(LowerErrorKind::DensityInputType)
            }
            // A var in the arena that isn't a local: a `density`-typed parameter (already
            // reported), so nothing more to say.
            Some(_) => {}
            None => self.err(LowerErrorKind::DanglingRef {
                kind: EntityKind::Var,
            }),
        }
        LocalId(0)
    }
}
