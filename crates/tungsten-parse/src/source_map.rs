//! Spans for the entities, statements and expressions of an elaborated program.

use std::collections::HashMap;

use tungsten_ir::{
    ComputeUnit, Constant, DensityInput, Function, HostInput, PermutationTableInput, Ref, Var,
};

use crate::span::Span;

/// One step into a statement list. `Index(i)` picks the i-th statement of the current list;
/// `Then`, `Else` and `Body` enter the branch or body of the `If` / `While` / `Repeat` just picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StmtStep {
    Index(usize),
    Then,
    Else,
    Body,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct StmtPath(pub Vec<StmtStep>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExprStep {
    Left,
    Right,
    Operand,
    Arg(usize),
    Base,
    Array,
    Index,
    Elem(usize),
    /// The operand of a cast.
    Inner,
    DensityIndex,
    FieldInit(usize),
}

/// The root (empty path) is the statement's single expression.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ExprPath(pub Vec<ExprStep>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExprOwner {
    Stmt(Ref<Function>, StmtPath),
    Const(Ref<Constant>),
}

/// Valid for the program `elaborate` returned. Refs are not translated across clones; use
/// `Arena::translate` for that. It doesn't survive `perform_inline`.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    pub functions: HashMap<Ref<Function>, Span>,
    pub units: HashMap<Ref<ComputeUnit>, Span>,
    pub constants: HashMap<Ref<Constant>, Span>,
    pub hosts: HashMap<Ref<HostInput>, Span>,
    pub tables: HashMap<Ref<PermutationTableInput>, Span>,
    /// Index = `entry_points` index.
    pub entries: Vec<Span>,
    pub vars: HashMap<(Ref<Function>, Ref<Var>), Span>,
    pub inputs: HashMap<(Ref<ComputeUnit>, Ref<DensityInput>), Span>,
    pub statements: HashMap<(Ref<Function>, StmtPath), Span>,
    pub expressions: HashMap<(ExprOwner, ExprPath), Span>,
}

impl SourceMap {
    pub fn statement(&self, f: Ref<Function>, path: &StmtPath) -> Option<Span> {
        self.statements.get(&(f, path.clone())).copied()
    }

    pub fn expression(&self, f: Ref<Function>, stmt: &StmtPath, expr: &ExprPath) -> Option<Span> {
        self.expressions
            .get(&(ExprOwner::Stmt(f, stmt.clone()), expr.clone()))
            .copied()
    }

    pub fn const_expression(&self, c: Ref<Constant>, expr: &ExprPath) -> Option<Span> {
        self.expressions
            .get(&(ExprOwner::Const(c), expr.clone()))
            .copied()
    }
}
