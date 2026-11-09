//! Ids, types, literals, expressions, statements, functions, units and the program.
//!
//! All fields are `pub`. Exec helpers over these types live in `util.rs`, `check/` and
//! `exec/`.

use vstd::prelude::*;

use crate::registry::{ExternDecl, RecordDecl};

verus! {

// ---------------------------------------------------------------------------------------------
// Ids
// ---------------------------------------------------------------------------------------------

/// Index into `Program.units`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnitId(pub usize);

/// Index into `Program.functions`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FnId(pub usize);

/// Index into `Program.consts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConstId(pub usize);

/// Index into `Program.hosts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostId(pub usize);

/// Index into `Program.tables`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TableId(pub usize);

/// Index into `Program.inputs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InputId(pub usize);

/// Index into `Program.externs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExternId(pub usize);

/// Identifies an opaque handle type. There is no declaration table: core only compares ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HandleTypeId(pub usize);

/// Index into `Program.records`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RecordId(pub usize);

/// Index into the enclosing `Function.locals`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalId(pub usize);

// ---------------------------------------------------------------------------------------------
// Types and literals
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum Type {
    Bool,
    I8,
    I32,
    I64,
    F32,
    F64,
    Vec3,
    Pos3,
    Array(Box<Type>, usize),
    /// From `VariableType::Extern(name)`.
    Record(RecordId),
    /// From `VariableType::Handle(name)`. Only the id is kept: core compares ids and nothing else.
    Handle(HandleTypeId),
}

/// Floats are stored as their bits (`to_bits()`), so equality is exact and NaN payloads are kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lit {
    Bool(bool),
    I8(i8),
    I32(i32),
    I64(i64),
    F32(u32),
    F64(u64),
}

/// Grid dimensions. Unvalidated: `>= 1` is a checker rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dims {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Three `f64`s as bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct F64x3 {
    pub x: u64,
    pub y: u64,
    pub z: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ambient {
    Origin,
    Pos3,
    OriginScale,
    PositionScale,
}

// ---------------------------------------------------------------------------------------------
// Operators
// ---------------------------------------------------------------------------------------------

/// The same variants as `tungsten_ir::BinaryOperator`, in the same order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
    ShiftLeft,
    ShiftRight,
    LogicalShiftRight,
    BitOr,
    BitAnd,
    BitXor,
}

/// `UnaryOperator::Ref(_)` is the identity and is removed during lowering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnOp {
    Negate,
    Not,
}

// ---------------------------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum Expr {
    Lit(Lit),
    Local(LocalId),
    Const(ConstId),
    Host(HostId),
    Table(TableId),
    Ambient(Ambient),
    Density { input: InputId, index: Option<Box<Expr>> },
    Call { func: FnId, args: Vec<Expr> },
    /// `params` and `ret` are the call site's annotation; the checker compares them with the decl.
    ExternCall { ext: ExternId, params: Vec<Type>, ret: Type, args: Vec<Expr> },
    Binary { op: BinOp, left: Box<Expr>, right: Box<Expr> },
    Unary { op: UnOp, operand: Box<Expr> },
    /// `ty` is the annotation (`type_of_field`).
    Field { base: Box<Expr>, sel: FieldSel, ty: Type },
    Index { array: Box<Expr>, index: Box<Expr> },
    MakeVec3 { x: Box<Expr>, y: Box<Expr>, z: Box<Expr> },
    MakePos3 { x: Box<Expr>, y: Box<Expr>, z: Box<Expr> },
    /// Fields in written order. Exactly-once is a checker rule.
    MakeRecord { record: RecordId, fields: Vec<FieldInit> },
    ArrayLit(Vec<Expr>),
    Cast { to: Type, operand: Box<Expr> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldSel {
    Vec3(Axis),
    Pos3(Axis),
    /// Record id and field index.
    Record(RecordId, usize),
}

#[derive(Debug, PartialEq, Eq)]
pub struct FieldInit {
    pub field: usize,
    pub value: Expr,
}

// ---------------------------------------------------------------------------------------------
// Statements and functions
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum Stmt {
    Assign { target: LocalId, value: Expr },
    Return(Expr),
    If { cond: Expr, then_branch: Vec<Stmt>, else_branch: Vec<Stmt> },
    While { cond: Expr, body: Vec<Stmt> },
    Repeat { count: usize, body: Vec<Stmt> },
    Break,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    /// `locals[0..params]` are the parameters, in order.
    pub params: usize,
    pub locals: Vec<Type>,
    pub ret: Type,
    /// `Function.host_inputs`.
    pub hosts: Vec<HostId>,
    pub body: Vec<Stmt>,
}

// ---------------------------------------------------------------------------------------------
// Program
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub main: FnId,
    /// The density inputs the unit may read, in build order.
    pub inputs: Vec<InputId>,
    pub tables: Vec<TableId>,
    pub hosts: Vec<HostId>,
    pub helpers: Vec<FnId>,
    pub consts: Vec<ConstId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDecl {
    /// `DensityInput.density_function`.
    pub source: UnitId,
    pub dims: Dims,
    pub origin_scale: F64x3,
    pub position_scale: F64x3,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub unit: UnitId,
    pub dims: Dims,
    pub origin_scale: F64x3,
    pub position_scale: F64x3,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDecl {
    pub ty: Type,
}

/// An opaque runtime object of the handle type `ty`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableDecl {
    pub ty: HandleTypeId,
}

/// No declared type: the checker derives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstDecl {
    pub init: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub units: Vec<Unit>,
    pub functions: Vec<Function>,
    pub consts: Vec<ConstDecl>,
    pub hosts: Vec<HostDecl>,
    pub tables: Vec<TableDecl>,
    pub inputs: Vec<InputDecl>,
    pub entries: Vec<Entry>,
    pub externs: Vec<ExternDecl>,
    pub records: Vec<RecordDecl>,
}

} // verus!

// `Clone` for the recursive types is written outside `verus!{}`: the pinned Verus rejects a
// derived `Clone` on a type that contains itself (through `Box`/`Vec`) as a possibly
// non-terminating cycle. They delegate to the verified-terminating `clone_*` fns in `util.rs`.

impl Clone for Type {
    fn clone(&self) -> Self {
        crate::util::clone_type(self)
    }
}

impl Clone for Expr {
    fn clone(&self) -> Self {
        crate::util::clone_expr(self)
    }
}

impl Clone for FieldInit {
    fn clone(&self) -> Self {
        crate::util::clone_field_init(self)
    }
}

impl Clone for Stmt {
    fn clone(&self) -> Self {
        crate::util::clone_stmt(self)
    }
}
