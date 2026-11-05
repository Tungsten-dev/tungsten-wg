//! Checker errors and core paths. `tungsten-check` renders them with names (`Location`).

use vstd::prelude::*;

use crate::ir::*;

verus! {

/// The item an error is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreItem {
    Function(FnId),
    Const(ConstId),
    Unit(UnitId),
    Host(HostId),
    Input(InputId),
    Entry(usize),
    Extern(ExternId),
    Record(RecordId),
    Program,
}

/// A scoping list of a unit or function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeList {
    Inputs,
    Helpers,
    Tables,
    Hosts,
    Consts,
}

/// Mirrors `tungsten_check::Step`; `Field` carries a record id and field index instead of a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreStep {
    Stmt(usize),
    Then(usize),
    Else(usize),
    Body(usize),
    Cond,
    Value,
    Arg(usize),
    Left,
    Right,
    Operand,
    Base,
    Index,
    Field(RecordId, usize),
    Element(usize),
    Param(usize),
    Local(usize),
    Ret,
    Type,
    Main,
    /// Position `index` in a scoping list.
    List(ScopeList, usize),
    /// A scoping list as a whole (a transitive `ConstNotListed`).
    WholeList(ScopeList),
    Source,
    Unit,
    /// An input's or entry's dims.
    Dims,
    /// An extern's parameter type `k`.
    ParamType(usize),
}

/// An indexed step kind, for walkers that are generic over which list they're in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Stmt,
    Then,
    Else,
    Body,
    Local,
    Element,
    Arg,
    ParamType,
}

pub fn mk_step(k: StepKind, i: usize) -> CoreStep {
    match k {
        StepKind::Stmt => CoreStep::Stmt(i),
        StepKind::Then => CoreStep::Then(i),
        StepKind::Else => CoreStep::Else(i),
        StepKind::Body => CoreStep::Body(i),
        StepKind::Local => CoreStep::Local(i),
        StepKind::Element => CoreStep::Element(i),
        StepKind::Arg => CoreStep::Arg(i),
        StepKind::ParamType => CoreStep::ParamType(i),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct CorePath {
    pub item: CoreItem,
    pub steps: Vec<CoreStep>,
}

/// The id table an out-of-range id points into (wf errors).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdTable {
    Unit,
    Function,
    Const,
    Host,
    Table,
    Input,
    Extern,
    Record,
    Local,
}

/// What a constant initializer must not contain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forbidden {
    Call,
    Density,
    Ambient,
    Host,
    ExternCall,
    Table,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnassignedHint {
    /// Some loop body earlier on the path assigns the local; loops don't count.
    AssignedOnlyInLoop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReturnHint {
    /// The last reachable statement is a loop that contains a `return`.
    EndsInLoop,
}

/// A member of a dependency cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CycleMember {
    Function(FnId),
    Const(ConstId),
    Input(InputId),
    Unit(UnitId),
}

#[derive(Debug, PartialEq, Eq)]
pub enum CheckErrorKind {
    // --- wf (internal: a lowering bug or a hand-built program) ---
    WfBadId { table: IdTable, id: usize },
    WfParams { params: usize, locals: usize },
    WfField { record: RecordId, field: usize },
    /// Input `input` and unit `unit` disagree about ownership.
    WfInputLink { input: InputId, unit: UnitId },

    // --- types (§6.3) ---
    TypeMismatch { expected: Type, found: Type },
    BadBinary { op: BinOp, left: Type, right: Type },
    BadUnary { op: UnOp, operand: Type },
    BadCast { from: Type, to: Type },
    Arity { expected: usize, found: usize },
    /// An `ExternCall` annotation differs from the registry decl.
    ExternSignature { ext: ExternId },
    /// A `Field` annotation differs from the selected field's type.
    FieldAnnotation { annotated: Type, actual: Type },
    BadFieldBase { sel: FieldSel, found: Type },
    NotArray { found: Type },
    RecordFieldMissing { record: RecordId, field: usize },
    RecordFieldRepeated { record: RecordId, field: usize },
    EmptyArray,
    ConstForbidden { what: Forbidden },
    MainParams { params: usize },

    // --- flow (§6.4) ---
    UnassignedLocal { local: LocalId, hint: Option<UnassignedHint> },
    MissingReturn { hint: Option<ReturnHint> },
    BreakOutsideLoop,

    // --- scoping (§6.5) ---
    HelperNotListed { unit: UnitId, func: FnId },
    HostNotInFn { host: HostId },
    HostNotInUnit { unit: UnitId, host: HostId },
    TableNotListed { unit: UnitId, table: TableId },
    ConstNotListed { unit: UnitId, konst: ConstId },
    InputNotOwned { unit: UnitId, input: InputId },
    InputReaderShared { func: FnId, units: Vec<UnitId> },

    // --- grids and cycles (§6.6) ---
    DimsTooSmall,
    GridTooLarge,
    Cycle { members: Vec<CycleMember> },
}

#[derive(Debug, PartialEq, Eq)]
pub struct CheckError {
    pub kind: CheckErrorKind,
    pub at: CorePath,
}

/// All errors of one `check`, in walk order. Never empty.
#[derive(Debug, PartialEq, Eq)]
pub struct CheckErrors {
    pub errors: Vec<CheckError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseError {
    HostCount { expected: usize, found: usize },
    /// `hosts[host]` doesn't have the host's declared type.
    HostValue { host: HostId },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseErrors {
    pub errors: Vec<CaseError>,
}

pub fn copy_steps(s: &Vec<CoreStep>) -> Vec<CoreStep> {
    let mut out: Vec<CoreStep> = Vec::new();
    let mut i: usize = 0;
    while i < s.len()
        invariant
            i <= s.len(),
        decreases s.len() - i,
    {
        out.push(s[i]);
        i = i + 1;
    }
    out
}

/// Appends an error at `item` / `steps`.
pub fn report(errs: &mut Vec<CheckError>, kind: CheckErrorKind, item: CoreItem, steps: &Vec<CoreStep>)
    ensures
        final(errs)@.len() == old(errs)@.len() + 1,
{
    errs.push(CheckError { kind, at: CorePath { item, steps: copy_steps(steps) } });
}

/// Moves every error of `src` to the end of `dst`, in order.
pub fn append_errors(dst: &mut Vec<CheckError>, src: Vec<CheckError>)
    ensures
        final(dst)@.len() >= old(dst)@.len(),
{
    let mut src = src;
    let mut rev: Vec<CheckError> = Vec::new();
    let n = src.len();
    let mut i: usize = 0;
    while i < n
        invariant
            i <= n,
        decreases n - i,
    {
        match src.pop() {
            Some(e) => rev.push(e),
            None => {},
        }
        i = i + 1;
    }
    let m = rev.len();
    let mut j: usize = 0;
    let ghost d0 = dst@.len();
    while j < m
        invariant
            j <= m,
            dst@.len() >= d0,
            d0 == old(dst)@.len(),
        decreases m - j,
    {
        match rev.pop() {
            Some(e) => dst.push(e),
            None => {},
        }
        j = j + 1;
    }
}

} // verus!

impl Clone for CheckErrorKind {
    fn clone(&self) -> Self {
        match self {
            CheckErrorKind::WfBadId { table, id } => CheckErrorKind::WfBadId { table: *table, id: *id },
            CheckErrorKind::WfParams { params, locals } => CheckErrorKind::WfParams { params: *params, locals: *locals },
            CheckErrorKind::WfField { record, field } => CheckErrorKind::WfField { record: *record, field: *field },
            CheckErrorKind::WfInputLink { input, unit } => CheckErrorKind::WfInputLink { input: *input, unit: *unit },
            CheckErrorKind::TypeMismatch { expected, found } => CheckErrorKind::TypeMismatch { expected: expected.clone(), found: found.clone() },
            CheckErrorKind::BadBinary { op, left, right } => CheckErrorKind::BadBinary { op: op.clone(), left: left.clone(), right: right.clone() },
            CheckErrorKind::BadUnary { op, operand } => CheckErrorKind::BadUnary { op: op.clone(), operand: operand.clone() },
            CheckErrorKind::BadCast { from, to } => CheckErrorKind::BadCast { from: from.clone(), to: to.clone() },
            CheckErrorKind::Arity { expected, found } => CheckErrorKind::Arity { expected: *expected, found: *found },
            CheckErrorKind::ExternSignature { ext } => CheckErrorKind::ExternSignature { ext: *ext },
            CheckErrorKind::FieldAnnotation { annotated, actual } => CheckErrorKind::FieldAnnotation { annotated: annotated.clone(), actual: actual.clone() },
            CheckErrorKind::BadFieldBase { sel, found } => CheckErrorKind::BadFieldBase { sel: sel.clone(), found: found.clone() },
            CheckErrorKind::NotArray { found } => CheckErrorKind::NotArray { found: found.clone() },
            CheckErrorKind::RecordFieldMissing { record, field } => CheckErrorKind::RecordFieldMissing { record: *record, field: *field },
            CheckErrorKind::RecordFieldRepeated { record, field } => CheckErrorKind::RecordFieldRepeated { record: *record, field: *field },
            CheckErrorKind::EmptyArray => CheckErrorKind::EmptyArray,
            CheckErrorKind::ConstForbidden { what } => CheckErrorKind::ConstForbidden { what: *what },
            CheckErrorKind::MainParams { params } => CheckErrorKind::MainParams { params: *params },
            CheckErrorKind::UnassignedLocal { local, hint } => CheckErrorKind::UnassignedLocal { local: *local, hint: *hint },
            CheckErrorKind::MissingReturn { hint } => CheckErrorKind::MissingReturn { hint: *hint },
            CheckErrorKind::BreakOutsideLoop => CheckErrorKind::BreakOutsideLoop,
            CheckErrorKind::HelperNotListed { unit, func } => CheckErrorKind::HelperNotListed { unit: *unit, func: *func },
            CheckErrorKind::HostNotInFn { host } => CheckErrorKind::HostNotInFn { host: *host },
            CheckErrorKind::HostNotInUnit { unit, host } => CheckErrorKind::HostNotInUnit { unit: *unit, host: *host },
            CheckErrorKind::TableNotListed { unit, table } => CheckErrorKind::TableNotListed { unit: *unit, table: *table },
            CheckErrorKind::ConstNotListed { unit, konst } => CheckErrorKind::ConstNotListed { unit: *unit, konst: *konst },
            CheckErrorKind::InputNotOwned { unit, input } => CheckErrorKind::InputNotOwned { unit: *unit, input: *input },
            CheckErrorKind::InputReaderShared { func, units } => CheckErrorKind::InputReaderShared { func: *func, units: units.clone() },
            CheckErrorKind::DimsTooSmall => CheckErrorKind::DimsTooSmall,
            CheckErrorKind::GridTooLarge => CheckErrorKind::GridTooLarge,
            CheckErrorKind::Cycle { members } => CheckErrorKind::Cycle { members: members.clone() },
        }
    }
}

impl Clone for CorePath {
    fn clone(&self) -> Self {
        CorePath { item: self.item, steps: self.steps.clone() }
    }
}

impl Clone for CheckError {
    fn clone(&self) -> Self {
        CheckError { kind: self.kind.clone(), at: self.at.clone() }
    }
}
