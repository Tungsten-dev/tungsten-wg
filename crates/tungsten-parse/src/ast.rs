//! The syntax tree. Every node carries a span. Nothing here is resolved.

use tungsten_ir::{AmbientKind, BinaryOperator, UnaryOperator, VariableType};

use crate::span::{FileId, Span};
pub use crate::tables::NumKind;

/// The nesting limit for expressions (AST depth: a leaf is 1, parentheses don't count),
/// statement blocks (`if`/`while`/`repeat` bodies and `else if` links), types and envelope values.
/// Each kind is counted on its own. P1 holds for every program within it.
pub const MAX_DEPTH: u32 = 256;

/// The parser's recursion guard. It sits far above anything `MAX_DEPTH` allows and only stops
/// pathological input such as thousands of nested parentheses.
pub const MAX_RECURSION: u32 = 2048;

#[derive(Debug, Clone)]
pub struct File {
    pub file: FileId,
    pub src: String,
    /// In source order, before and after the program.
    pub directives: Vec<Directive>,
    pub program: Program,
}

#[derive(Debug, Clone)]
pub struct Program {
    pub span: Span,
    pub items: Vec<Item>,
}

/// `name`, `name#N`, `name##N`, `` `quoted` ``, or `%N`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Label {
    pub name: String,
    pub quoted: bool,
    pub kind: LabelKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LabelKind {
    Plain,
    Hash(u64),
    HashHash(u64),
    Anon(u64),
}

impl Label {
    /// Identity within a namespace: the text of the label, ignoring how it was quoted.
    pub fn key(&self) -> LabelKey {
        LabelKey {
            name: self.name.clone(),
            kind: self.kind,
        }
    }

    pub fn display(&self) -> String {
        self.key().display()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LabelKey {
    pub name: String,
    pub kind: LabelKind,
}

impl LabelKey {
    pub fn display(&self) -> String {
        match self.kind {
            LabelKind::Plain => self.name.clone(),
            LabelKind::Hash(n) => format!("{}#{n}", self.name),
            LabelKind::HashHash(n) => format!("{}##{n}", self.name),
            LabelKind::Anon(n) => format!("%{n}"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Item {
    Use(UseItem),
    Host(HostItem),
    Table(TableItem),
    Const(ConstItem),
    Fn(FnItem),
    Unit(UnitItem),
    Entry(Box<EntryItem>),
}

#[derive(Debug, Clone)]
pub struct UseItem {
    pub span: Span,
    pub library: String,
    pub library_span: Span,
    pub names: Vec<Label>,
}

#[derive(Debug, Clone)]
pub struct HostItem {
    pub span: Span,
    pub label: Label,
    /// `None` for `dynamic`.
    pub ty: Option<Ty>,
}

#[derive(Debug, Clone)]
pub struct TableItem {
    pub span: Span,
    pub label: Label,
    pub def: TableDef,
}

#[derive(Debug, Clone)]
pub enum TableDef {
    Perlin {
        ident: String,
        subident: Option<String>,
        index: u64,
    },
    Base3d,
}

#[derive(Debug, Clone)]
pub struct ConstItem {
    pub span: Span,
    pub label: Label,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub label: Label,
    pub ty: Option<Ty>,
}

#[derive(Debug, Clone)]
pub struct FnItem {
    pub span: Span,
    pub label: Label,
    pub params: Vec<Param>,
    pub ret: Ty,
    pub hash: Option<u64>,
    /// `@hosts(…)`, with the span of the attribute.
    pub hosts: Option<(Vec<Label>, Span)>,
    pub vars: Vec<(Label, Ty)>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub struct UnitItem {
    pub span: Span,
    pub label: Label,
    pub hash: Option<u64>,
    pub main: Option<Label>,
    pub helpers: Option<Vec<Label>>,
    pub tables: Option<Vec<Label>>,
    pub hosts: Option<Vec<Label>>,
    pub consts: Option<Vec<Label>>,
    pub inputs: Vec<InputItem>,
}

#[derive(Debug, Clone)]
pub struct InputItem {
    pub span: Span,
    pub label: Label,
    pub target: Label,
    pub grid: GridAttrs,
    /// `@var(f.v)`
    pub var: Option<(Label, Label)>,
}

#[derive(Debug, Clone)]
pub struct EntryItem {
    pub span: Span,
    pub label: Option<Label>,
    pub unit: Label,
    pub grid: GridAttrs,
}

/// Each of the three is optional here so the elaborator can report a missing one.
#[derive(Debug, Clone)]
pub struct GridAttrs {
    pub span: Span,
    pub dims: Option<([i64; 3], Span)>,
    pub origin_scale: Option<([Lit; 3], Span)>,
    pub position_scale: Option<([Lit; 3], Span)>,
}

#[derive(Debug, Clone)]
pub struct Ty {
    pub span: Span,
    pub kind: TyKind,
}

#[derive(Debug, Clone)]
pub enum TyKind {
    /// Anything but `Array` and `Extern`.
    Prim(VariableType),
    Array(Box<Ty>, usize),
    Extern(String),
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub span: Span,
    pub kind: StmtKind,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Let {
        label: Label,
        ty: Ty,
        init: Option<Expr>,
    },
    Assign {
        target: Label,
        value: Expr,
    },
    Return(Expr),
    Break,
    If {
        cond: Expr,
        then: Vec<Stmt>,
        els: Option<Vec<Stmt>>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    Repeat {
        count: usize,
        body: Vec<Stmt>,
    },
}

impl Stmt {
    /// The expressions directly in this statement (not in nested blocks).
    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        match &self.kind {
            StmtKind::Let { init, .. } => init.as_ref(),
            StmtKind::Assign { value: e, .. }
            | StmtKind::Return(e)
            | StmtKind::If { cond: e, .. }
            | StmtKind::While { cond: e, .. } => Some(e),
            StmtKind::Break | StmtKind::Repeat { .. } => None,
        }
        .into_iter()
    }

    /// The blocks directly in this statement.
    pub fn blocks(&self) -> impl Iterator<Item = &[Stmt]> {
        let (a, b): (Option<&[Stmt]>, Option<&[Stmt]>) = match &self.kind {
            StmtKind::If { then, els, .. } => (Some(then), els.as_deref()),
            StmtKind::While { body, .. } | StmtKind::Repeat { body, .. } => (Some(body), None),
            _ => (None, None),
        };
        a.into_iter().chain(b)
    }
}

/// Calls `f` on every statement of `stmts`, nested ones included, in textual order.
pub fn try_for_each_stmt<'a, E>(
    stmts: &'a [Stmt],
    f: &mut impl FnMut(&'a Stmt) -> Result<(), E>,
) -> Result<(), E> {
    for s in stmts {
        f(s)?;
        for b in s.blocks() {
            try_for_each_stmt(b, f)?;
        }
    }
    Ok(())
}

/// The `let` declarations of a body, in textual order (depth-first).
pub fn lets(stmts: &[Stmt]) -> Vec<(&Label, &Ty)> {
    let mut out = vec![];
    let _ = try_for_each_stmt(stmts, &mut |s| {
        if let StmtKind::Let { label, ty, .. } = &s.kind {
            out.push((label, ty));
        }
        Ok::<(), ()>(())
    });
    out
}

/// An unconverted numeric literal. `digits` has underscores removed and no radix prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawNum {
    pub digits: String,
    pub radix: u32,
    pub kind: NumKind,
    /// A suffix was written (`b`, `L`, `f`, `_i8`, …).
    pub suffixed: bool,
}

/// A literal as written. Conversion to a value happens against a kind, see `numlit`.
#[derive(Debug, Clone, PartialEq)]
pub struct Lit {
    pub span: Span,
    pub kind: LitKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LitKind {
    Bool(bool),
    /// A numeric literal. `neg` is the glued sign.
    Num {
        raw: RawNum,
        neg: bool,
    },
    /// `inf`, `-inf`, `inf_f32`, `-inf_f32`
    Inf {
        neg: bool,
        f32: bool,
    },
    /// `nan`, `nan(0x…)`, `nan_f32`, `nan_f32(0x…)`. The bits are exact.
    Nan {
        bits: u64,
        f32: bool,
        explicit: bool,
    },
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub span: Span,
    pub kind: ExprKind,
    /// Written in parentheses.
    pub paren: bool,
    /// AST depth: 1 for a leaf.
    pub depth: u32,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Lit(Lit),
    /// A bare name: a var, or else a constant.
    Name(Label),
    Const(Label),
    Host(Label),
    Table(Label),
    Density {
        unit: Option<Label>,
        input: Label,
        index: Option<Box<Expr>>,
    },
    Ambient(AmbientKind),
    Call {
        callee: Label,
        args: Vec<Expr>,
    },
    ExtCall {
        name: String,
        sig: Option<(Vec<Ty>, Ty)>,
        args: Vec<Expr>,
    },
    Field {
        base: Box<Expr>,
        name: String,
        name_span: Span,
        /// `::<T>` (field type) or `::<T; B>` (field type, base type)
        ty: Option<(Ty, Option<Ty>)>,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    /// `vec3(…)`, `pos3(…)`, `new T(…)`
    Construct {
        ty: Ty,
        args: Vec<Expr>,
    },
    ConstructExtern {
        ty: Ty,
        fields: Vec<(String, Span, Expr)>,
    },
    Array(Vec<Expr>),
    Cast(Box<Expr>, Ty),
    Unary(UnaryOperator, Box<Expr>),
    Binary(BinaryOperator, Box<Expr>, Box<Expr>),
}

impl ExprKind {
    /// Calls `f` on each direct subexpression, in order.
    pub fn for_each_child<'a>(&'a self, mut f: impl FnMut(&'a Expr)) {
        match self {
            ExprKind::Lit(_)
            | ExprKind::Name(_)
            | ExprKind::Const(_)
            | ExprKind::Host(_)
            | ExprKind::Table(_)
            | ExprKind::Ambient(_) => {}
            ExprKind::Density { index, .. } => index.iter().for_each(|e| f(e)),
            ExprKind::Call { args, .. }
            | ExprKind::ExtCall { args, .. }
            | ExprKind::Construct { args, .. }
            | ExprKind::Array(args) => args.iter().for_each(f),
            ExprKind::ConstructExtern { fields, .. } => fields.iter().for_each(|(_, _, e)| f(e)),
            ExprKind::Field { base: e, .. } | ExprKind::Cast(e, _) | ExprKind::Unary(_, e) => f(e),
            ExprKind::Index { base: l, index: r } | ExprKind::Binary(_, l, r) => {
                f(l);
                f(r);
            }
        }
    }

    /// Calls `f` on every subexpression (not on `self`), depth-first, stopping at the first error.
    pub fn try_for_each_descendant<'a, E>(
        &'a self,
        f: &mut impl FnMut(&'a Expr) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut r = Ok(());
        self.for_each_child(|c| {
            if r.is_ok() {
                r = f(c).and_then(|()| c.kind.try_for_each_descendant(f));
            }
        });
        r
    }
}

// ---------------------------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Directive {
    pub span: Span,
    pub kind: DirectiveKind,
}

#[derive(Debug, Clone)]
pub enum DirectiveKind {
    KnownFail {
        backend: String,
        reason: String,
        case: Option<String>,
        stage: Option<String>,
    },
    Fuel(u64),
    Tier(String),
    Cells {
        sample: u64,
        seed: u64,
    },
    Case {
        name: String,
        origin: [Lit; 3],
        seed: u64,
        hosts: Vec<(Label, AstValue)>,
    },
    ExpectReject(Option<String>),
    Expect {
        case: String,
        case_span: Span,
        expectation: Expectation,
    },
}

#[derive(Debug, Clone)]
pub enum Expectation {
    Error(String),
    Cell {
        entry: Label,
        pos: [i64; 3],
        value: AstValue,
    },
    All {
        entry: Label,
        value: AstValue,
    },
    Grid {
        entry: Label,
        values: Vec<AstValue>,
    },
}

#[derive(Debug, Clone)]
pub struct AstValue {
    pub span: Span,
    pub kind: AstValueKind,
}

#[derive(Debug, Clone)]
pub enum AstValueKind {
    Lit(Lit),
    Vec3(Vec<AstValue>),
    Pos3(Vec<AstValue>),
    Array(Vec<AstValue>),
    Record(Ty, Vec<(String, Span, AstValue)>),
}
