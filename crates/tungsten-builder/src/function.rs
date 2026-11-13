//! `FnBuilder`: one function under construction. It is owned and borrows nothing, so several can
//! be open at once. Whether it becomes a unit or a helper is decided by which
//! `ProgramBuilder::finish_*` it is passed to.

use std::collections::HashSet;
use std::panic::Location;

use tungsten_ir::{Arena, Expression, Name, Ref, Statement, Var, VariableType};

use crate::error::{BuildError, BuildErrorKind};
use crate::expr::{Expr, Ty, intern, name};

/// A parameter or local of one function. Using it in another function is `ForeignHandle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    pub(crate) var: Ref<Var>,
    pub(crate) ty: Ty,
}

impl Local {
    pub fn ty(&self) -> &'static VariableType {
        self.ty
    }
}

/// Statements captured by [`FnBuilder::block`], to be added later with [`FnBuilder::append`].
#[derive(Debug, Clone, Default)]
pub struct Block(pub(crate) Vec<Statement>);

impl Block {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A checked condition (see `FnBuilder::__condition`).
#[doc(hidden)]
#[derive(Debug)]
pub struct Condition(Expression);

#[derive(Debug)]
pub struct FnBuilder {
    pub(crate) name: String,
    pub(crate) ret: Ty,
    pub(crate) vars: Arena<Var>,
    pub(crate) params: Vec<Ref<Var>>,
    named: HashSet<String>,
    pub(crate) body: Vec<Statement>,
    pub(crate) errors: Vec<BuildError>,
    pub(crate) source_hash: u64,
}

impl FnBuilder {
    pub fn new(name: impl Into<String>, ret: VariableType) -> Self {
        Self {
            name: name.into(),
            ret: intern(ret),
            vars: Arena::new(),
            params: Vec::new(),
            named: HashSet::new(),
            body: Vec::new(),
            errors: Vec::new(),
            source_hash: 0,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn return_type(&self) -> &'static VariableType {
        self.ret
    }

    pub fn set_return_type(&mut self, ret: VariableType) {
        self.ret = intern(ret);
    }

    /// Sets the function's `source_hash` (default 0).
    pub fn source_hash(&mut self, hash: u64) {
        self.source_hash = hash;
    }

    fn error(&mut self, kind: BuildErrorKind, at: &'static Location<'static>) {
        let mut e = BuildError::new(kind, at);
        e.function = Some(self.name.clone());
        self.errors.push(e);
    }

    // ---- variables -----------------------------------------------------------------------

    #[track_caller]
    fn named_var(&mut self, n: &str, t: VariableType) -> Local {
        if !self.named.insert(n.to_string()) {
            self.error(
                BuildErrorKind::DuplicateLocal {
                    name: n.to_string(),
                },
                Location::caller(),
            );
        }
        self.var(Name::Named(n.to_string()), t)
    }

    fn var(&mut self, n: Name, t: VariableType) -> Local {
        let ty = intern(t.clone());
        let var = self.vars.insert(Var { name: n, t });
        Local { var, ty }
    }

    /// A parameter, named uniquely. A function with parameters can only be a helper.
    #[track_caller]
    pub fn param(&mut self, name: &str, t: VariableType) -> Local {
        let l = self.named_var(name, t);
        self.params.push(l.var);
        l
    }

    /// A uniquely named local (`Name::Named`). A repeated name is `DuplicateLocal`.
    #[track_caller]
    pub fn local(&mut self, name: &str, t: VariableType) -> Local {
        self.named_var(name, t)
    }

    /// A temporary whose name is only a hint (`Name::Prefixed`). Never a duplicate.
    pub fn temp(&mut self, prefix: &str, t: VariableType) -> Local {
        self.var(Name::Prefixed(prefix.to_string()), t)
    }

    /// A local without a name (`Name::Anonymous`).
    pub fn anon(&mut self, t: VariableType) -> Local {
        self.var(Name::Anonymous, t)
    }

    // ---- statements ----------------------------------------------------------------------

    /// Takes `e`'s problems, and reports a local of another function in it.
    fn take(&mut self, mut e: Expr, at: &'static Location<'static>) -> (Expression, Option<Ty>) {
        for mut p in e.problems.drain(..) {
            p.function = Some(self.name.clone());
            self.errors.push(p);
        }
        if foreign_local(&self.vars, &e.node) {
            self.foreign(at);
        }
        (e.node, e.ty)
    }

    fn foreign(&mut self, at: &'static Location<'static>) {
        self.error(
            BuildErrorKind::ForeignHandle {
                what: "a local",
                owner: "function",
            },
            at,
        );
    }

    fn push(&mut self, s: Statement) {
        self.body.push(s);
    }

    /// `target = value;`
    #[track_caller]
    pub fn assign(&mut self, target: Local, value: impl Into<Expr>) {
        let at = Location::caller();
        let (value, ty) = self.take(value.into(), at);
        if !self.vars.contains(target.var) {
            self.foreign(at);
        } else if let Some(found) = ty
            && found != target.ty
        {
            self.error(
                BuildErrorKind::AssignType {
                    expected: name(target.ty),
                    found: name(found),
                },
                at,
            );
        }
        self.push(Statement::Assign {
            target: target.var,
            value,
        });
    }

    /// `return value;`
    #[track_caller]
    pub fn ret(&mut self, value: impl Into<Expr>) {
        let at = Location::caller();
        let (value, ty) = self.take(value.into(), at);
        if let Some(found) = ty
            && found != self.ret
        {
            self.error(
                BuildErrorKind::ReturnType {
                    expected: name(self.ret),
                    found: name(found),
                },
                at,
            );
        }
        self.push(Statement::Return(value));
    }

    /// `break;`. Whether it is inside a loop is checked by core.
    pub fn break_(&mut self) {
        self.push(Statement::Break);
    }

    fn condition(&mut self, cond: Expr, at: &'static Location<'static>) -> Expression {
        let (cond, ty) = self.take(cond, at);
        if let Some(found) = ty
            && *found != VariableType::Bool
        {
            self.error(BuildErrorKind::ConditionType { found: name(found) }, at);
        }
        cond
    }

    /// Runs `f` with an empty body and returns what it added.
    fn nested(&mut self, f: impl FnOnce(&mut Self)) -> Vec<Statement> {
        let saved = std::mem::take(&mut self.body);
        f(self);
        std::mem::replace(&mut self.body, saved)
    }

    /// `if cond { then }`
    #[track_caller]
    pub fn if_then(&mut self, cond: impl Into<Expr>, then: impl FnOnce(&mut Self)) {
        let condition = self.condition(cond.into(), Location::caller());
        let then_branch = self.nested(then);
        self.push(Statement::If {
            condition,
            then_branch,
            else_branch: Vec::new(),
        });
    }

    /// `if cond { then } else { other }`
    #[track_caller]
    pub fn if_else(
        &mut self,
        cond: impl Into<Expr>,
        then: impl FnOnce(&mut Self),
        other: impl FnOnce(&mut Self),
    ) {
        let condition = self.condition(cond.into(), Location::caller());
        let then_branch = self.nested(then);
        let else_branch = self.nested(other);
        self.push(Statement::If {
            condition,
            then_branch,
            else_branch,
        });
    }

    /// `if cond { then } else { other }`, from blocks captured with [`block`](Self::block).
    ///
    /// Each branch is built in its own step, so lowering code can use `&mut ProgramBuilder` in
    /// both, which the two closures of [`if_else`](Self::if_else) can't do. A block from another
    /// function is `ForeignHandle` if it uses that function's locals.
    #[track_caller]
    pub fn if_blocks(&mut self, cond: impl Into<Expr>, then: Block, other: Block) {
        let at = Location::caller();
        let condition = self.condition(cond.into(), at);
        if then
            .0
            .iter()
            .chain(&other.0)
            .any(|s| foreign_in_stmt(&self.vars, s))
        {
            self.foreign(at);
        }
        self.push(Statement::If {
            condition,
            then_branch: then.0,
            else_branch: other.0,
        });
    }

    /// `while cond { body }`
    #[track_caller]
    pub fn while_(&mut self, cond: impl Into<Expr>, body: impl FnOnce(&mut Self)) {
        let condition = self.condition(cond.into(), Location::caller());
        let body = self.nested(body);
        self.push(Statement::While { condition, body });
    }

    /// `repeat count { body }`
    pub fn repeat(&mut self, count: usize, body: impl FnOnce(&mut Self)) {
        let body = self.nested(body);
        self.push(Statement::Repeat { count, body });
    }

    /// Checks a condition now, for an `if` assembled later from blocks. `body!`'s `if`/`else`
    /// uses this, so that each branch is a separate closure and both can use the program
    /// builder.
    #[doc(hidden)]
    #[track_caller]
    pub fn __condition(&mut self, cond: impl Into<Expr>) -> Condition {
        Condition(self.condition(cond.into(), Location::caller()))
    }

    #[doc(hidden)]
    pub fn __if(&mut self, cond: Condition, then: Block, other: Block) {
        self.push(Statement::If {
            condition: cond.0,
            then_branch: then.0,
            else_branch: other.0,
        });
    }

    /// Captures the statements `body` adds, without appending them.
    pub fn block(&mut self, body: impl FnOnce(&mut Self)) -> Block {
        Block(self.nested(body))
    }

    /// Appends a captured block. A block from another function is `ForeignHandle` if it uses
    /// that function's locals.
    #[track_caller]
    pub fn append(&mut self, block: Block) {
        if block.0.iter().any(|s| foreign_in_stmt(&self.vars, s)) {
            self.foreign(Location::caller());
        }
        self.body.extend(block.0);
    }
}

fn foreign_in_stmt(vars: &Arena<Var>, s: &Statement) -> bool {
    match s {
        Statement::Assign { target, value } => {
            !vars.contains(*target) || foreign_local(vars, value)
        }
        Statement::Return(e) => foreign_local(vars, e),
        Statement::If {
            condition,
            then_branch,
            else_branch,
        } => {
            foreign_local(vars, condition)
                || then_branch
                    .iter()
                    .chain(else_branch)
                    .any(|s| foreign_in_stmt(vars, s))
        }
        Statement::While { condition, body } => {
            foreign_local(vars, condition) || body.iter().any(|s| foreign_in_stmt(vars, s))
        }
        Statement::Repeat { body, .. } => body.iter().any(|s| foreign_in_stmt(vars, s)),
        Statement::Break => false,
    }
}

/// Whether `e` reads a local that isn't in `vars`.
fn foreign_local(vars: &Arena<Var>, e: &Expression) -> bool {
    let mut found = false;
    crate::derive::each_child(e, &mut |e| {
        if let Expression::Variable(v) = e
            && !vars.contains(*v)
        {
            found = true;
        }
    });
    found
}
