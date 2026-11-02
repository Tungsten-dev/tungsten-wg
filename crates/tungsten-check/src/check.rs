//! Checker errors with names: `tungsten_core::check::CheckError` (core paths, core ids) rendered
//! through the [`LowerMap`] as a [`Location`] and a message.

use std::fmt;

use tungsten_core::check::{self as core_check, CoreItem, CoreStep, CycleMember, Forbidden, IdTable, ScopeList};
use tungsten_core::{BinOp, FieldSel, FnId, Program, Type, UnOp};

use crate::error::{Item, Location, LowerErrors, Step};
use crate::map::LowerMap;

pub use tungsten_core::check::{CheckErrorKind, ReturnHint, UnassignedHint};

/// A program that didn't lower, or lowered but failed the checker.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
    #[error("{0}")]
    Lower(LowerErrors),
    #[error("{0}")]
    Check(CheckErrors),
}

/// All errors of one `check`, in walk order. Never empty.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct CheckErrors(pub Vec<CheckError>);

impl fmt::Display for CheckErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, e) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{e}")?;
        }
        Ok(())
    }
}

/// A checker error: the core kind, where it is, and the kind rendered with names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckError {
    pub kind: CheckErrorKind,
    pub at: Location,
    pub message: String,
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.message)
    }
}

/// Renders core errors against `p` and `map`.
pub(crate) fn render(p: &Program, map: &LowerMap, errors: core_check::CheckErrors) -> CheckErrors {
    let n = Names { p, map };
    CheckErrors(
        errors
            .errors
            .into_iter()
            .map(|e| CheckError {
                at: n.location(&e.at),
                message: n.message(&e.kind, e.at.item),
                kind: e.kind,
            })
            .collect(),
    )
}

struct Names<'a> {
    p: &'a Program,
    map: &'a LowerMap,
}

fn or_id(n: Option<&str>, i: usize) -> String {
    n.map(str::to_string).unwrap_or_else(|| format!("#{i}"))
}

impl Names<'_> {
    fn func(&self, f: FnId) -> String {
        or_id(self.map.functions.name(f), f.0)
    }
    fn unit(&self, u: tungsten_core::UnitId) -> String {
        or_id(self.map.units.name(u), u.0)
    }
    fn konst(&self, c: tungsten_core::ConstId) -> String {
        or_id(self.map.consts.name(c), c.0)
    }
    fn host(&self, h: tungsten_core::HostId) -> String {
        or_id(self.map.hosts.name(h), h.0)
    }
    fn table(&self, t: tungsten_core::TableId) -> String {
        or_id(self.map.tables.name(t), t.0)
    }
    fn input(&self, i: tungsten_core::InputId) -> String {
        or_id(self.map.inputs.name(i), i.0)
    }
    fn record(&self, r: tungsten_core::RecordId) -> String {
        or_id(self.map.record_name(r), r.0)
    }
    fn field(&self, r: tungsten_core::RecordId, i: usize) -> String {
        or_id(self.map.field_name(r, i), i)
    }
    fn ext(&self, e: tungsten_core::ExternId) -> String {
        or_id(self.map.extern_name(e), e.0)
    }
    fn local(&self, item: CoreItem, l: tungsten_core::LocalId) -> String {
        match item {
            CoreItem::Function(f) => or_id(self.map.locals.get(f.0).and_then(|m| m.name(l)), l.0),
            _ => format!("#{}", l.0),
        }
    }

    fn ty(&self, t: &Type) -> String {
        match t {
            Type::Bool => "bool".into(),
            Type::I8 => "i8".into(),
            Type::I32 => "i32".into(),
            Type::I64 => "i64".into(),
            Type::F32 => "f32".into(),
            Type::F64 => "f64".into(),
            Type::Vec3 => "vec3".into(),
            Type::Pos3 => "pos3".into(),
            Type::Array(inner, n) => format!("[{}; {n}]", self.ty(inner)),
            Type::Record(r) => format!("extern {}", self.record(*r)),
            Type::PermTable => "perm_table".into(),
            Type::Sampler => "interpolated_noise_sampler".into(),
        }
    }

    fn item(&self, i: CoreItem) -> Item {
        match i {
            CoreItem::Function(f) => Item::Function { id: Some(f), name: self.func(f) },
            CoreItem::Const(c) => Item::Constant { id: Some(c), name: self.konst(c) },
            CoreItem::Unit(u) => Item::Unit { id: Some(u), name: self.unit(u) },
            CoreItem::Host(h) => Item::Host { id: Some(h), name: self.host(h) },
            CoreItem::Input(x) => Item::Input { id: Some(x), name: self.input(x) },
            CoreItem::Entry(e) => Item::Entry { index: e },
            CoreItem::Extern(_) | CoreItem::Record(_) => Item::Registry,
            CoreItem::Program => Item::Program,
        }
    }

    fn step(&self, s: &CoreStep) -> Step {
        fn list(l: &ScopeList) -> &'static str {
            match l {
                ScopeList::Inputs => "inputs",
                ScopeList::Helpers => "helpers",
                ScopeList::Tables => "tables",
                ScopeList::Hosts => "hosts",
                ScopeList::Consts => "consts",
            }
        }
        match s {
            CoreStep::Stmt(i) => Step::Stmt(*i),
            CoreStep::Then(i) => Step::Then(*i),
            CoreStep::Else(i) => Step::Else(*i),
            CoreStep::Body(i) => Step::Body(*i),
            CoreStep::Cond => Step::Cond,
            CoreStep::Value => Step::Value,
            CoreStep::Arg(i) => Step::Arg(*i),
            CoreStep::Left => Step::Left,
            CoreStep::Right => Step::Right,
            CoreStep::Operand => Step::Operand,
            CoreStep::Base => Step::Base,
            CoreStep::Index => Step::Index,
            CoreStep::Field(r, i) => Step::Field(self.field(*r, *i)),
            CoreStep::Element(i) => Step::Element(*i),
            CoreStep::Param(i) => Step::Param(*i),
            CoreStep::Local(i) => Step::Local(*i),
            CoreStep::Ret => Step::Ret,
            CoreStep::Type => Step::Type,
            CoreStep::Main => Step::Main,
            CoreStep::List(l, i) => Step::List { list: list(l), index: *i },
            CoreStep::WholeList(l) => Step::WholeList { list: list(l) },
            CoreStep::Source => Step::Source,
            CoreStep::Unit => Step::Unit,
            CoreStep::Dims => Step::Dims,
            CoreStep::ParamType(i) => Step::Param(*i),
        }
    }

    fn location(&self, at: &core_check::CorePath) -> Location {
        Location {
            item: self.item(at.item),
            path: at.steps.iter().map(|s| self.step(s)).collect(),
        }
    }

    fn member(&self, m: &CycleMember) -> String {
        match m {
            CycleMember::Function(f) => format!("fn `{}`", self.func(*f)),
            CycleMember::Const(c) => format!("const `{}`", self.konst(*c)),
            CycleMember::Input(i) => format!("input `{}`", self.input(*i)),
            CycleMember::Unit(u) => format!("unit `{}`", self.unit(*u)),
        }
    }

    fn sig(&self, params: &[Type], ret: &Type) -> String {
        let ps: Vec<String> = params.iter().map(|t| self.ty(t)).collect();
        format!("({}) -> {}", ps.join(", "), self.ty(ret))
    }

    fn message(&self, k: &CheckErrorKind, item: CoreItem) -> String {
        use CheckErrorKind as K;
        match k {
            K::WfBadId { table, id } => format!("internal: {} id {id} is out of range", id_table(*table)),
            K::WfParams { params, locals } => format!("internal: {params} params but only {locals} locals"),
            K::WfField { record, field } => format!("internal: record `{}` has no field {field}", self.record(*record)),
            K::WfInputLink { input, unit } => format!(
                "internal: input `{}` and unit `{}` disagree about ownership",
                self.input(*input),
                self.unit(*unit)
            ),
            K::TypeMismatch { expected, found } => format!("expected `{}`, found `{}`", self.ty(expected), self.ty(found)),
            K::BadBinary { op, left, right } => format!(
                "`{}` can't be applied to `{}` and `{}`",
                binop(op),
                self.ty(left),
                self.ty(right)
            ),
            K::BadUnary { op, operand } => format!("`{}` can't be applied to `{}`", unop(op), self.ty(operand)),
            K::BadCast { from, to } => format!("can't cast `{}` to `{}`", self.ty(from), self.ty(to)),
            K::Arity { expected, found } => format!("expected {expected} arguments, found {found}"),
            K::ExternSignature { ext } => {
                let name = self.ext(*ext);
                match self.p.externs.get(ext.0) {
                    Some(d) => format!(
                        "the annotation of `{name}` doesn't match its registry signature `{}`",
                        self.sig(&d.params, &d.ret)
                    ),
                    None => format!("the annotation of `{name}` doesn't match its registry signature"),
                }
            }
            K::FieldAnnotation { annotated, actual } => format!(
                "field annotated as `{}`, but it is `{}`",
                self.ty(annotated),
                self.ty(actual)
            ),
            K::BadFieldBase { sel, found } => {
                let (name, want) = match sel {
                    FieldSel::Vec3(a) => (axis(a).to_string(), "vec3".to_string()),
                    FieldSel::Pos3(a) => (axis(a).to_string(), "pos3".to_string()),
                    FieldSel::Record(r, i) => (self.field(*r, *i), format!("extern {}", self.record(*r))),
                };
                format!("field `{name}` needs a `{want}` base, found `{}`", self.ty(found))
            }
            K::NotArray { found } => format!("indexing needs an array, found `{}`", self.ty(found)),
            K::RecordFieldMissing { record, field } => format!(
                "field `{}` of `{}` is missing",
                self.field(*record, *field),
                self.record(*record)
            ),
            K::RecordFieldRepeated { record, field } => format!(
                "field `{}` of `{}` is given more than once",
                self.field(*record, *field),
                self.record(*record)
            ),
            K::EmptyArray => "empty array literal".into(),
            K::ConstForbidden { what } => format!("a constant can't contain {}", forbidden(*what)),
            K::MainParams { params } => format!("a unit's main can't have parameters (it has {params})"),
            K::UnassignedLocal { local, hint } => {
                let base = format!("`{}` may be read before it is assigned", self.local(item, *local));
                match hint {
                    Some(UnassignedHint::AssignedOnlyInLoop) => format!(
                        "{base}; it is only assigned inside a loop, and loops don't count for definite \
                         assignment: assign it before the loop"
                    ),
                    None => base,
                }
            }
            K::MissingReturn { hint } => match hint {
                Some(ReturnHint::EndsInLoop) => "the end of the body is reachable without a `return`; the body \
                     ends in a loop that returns, but loops don't count: add a `return` after the loop"
                    .into(),
                None => "the end of the body is reachable without a `return`".into(),
            },
            K::BreakOutsideLoop => "`break` outside a loop".into(),
            K::HelperNotListed { unit, func } => format!(
                "`{}` is called but isn't in the helpers of unit `{}`",
                self.func(*func),
                self.unit(*unit)
            ),
            K::HostNotInFn { host } => format!("host `{}` is read but isn't in this fn's `@hosts`", self.host(*host)),
            K::HostNotInUnit { unit, host } => format!(
                "host `{}` isn't in the hosts of unit `{}`",
                self.host(*host),
                self.unit(*unit)
            ),
            K::TableNotListed { unit, table } => format!(
                "table `{}` isn't in the tables of unit `{}`",
                self.table(*table),
                self.unit(*unit)
            ),
            K::ConstNotListed { unit, konst } => format!(
                "const `{}` isn't in the consts of unit `{}`",
                self.konst(*konst),
                self.unit(*unit)
            ),
            K::InputNotOwned { unit, input } => {
                let owner = self
                    .p
                    .inputs
                    .get(input.0)
                    .map(|d| self.unit(d.owner))
                    .unwrap_or_else(|| "?".into());
                format!(
                    "input `{}` belongs to unit `{owner}`, not to unit `{}`",
                    self.input(*input),
                    self.unit(*unit)
                )
            }
            K::InputReaderShared { func, units } => {
                let us: Vec<String> = units.iter().map(|u| format!("`{}`", self.unit(*u))).collect();
                format!(
                    "`{}` reads a density input, so only one unit may list it, but {} do",
                    self.func(*func),
                    us.join(", ")
                )
            }
            K::DimsTooSmall => "every dimension must be at least 1".into(),
            K::GridTooLarge => "the grid has more than i32::MAX cells".into(),
            K::Cycle { members } => {
                let ms: Vec<String> = members.iter().map(|m| self.member(m)).collect();
                format!("dependency cycle through {}", ms.join(", "))
            }
        }
    }
}

fn id_table(t: IdTable) -> &'static str {
    match t {
        IdTable::Unit => "unit",
        IdTable::Function => "function",
        IdTable::Const => "const",
        IdTable::Host => "host",
        IdTable::Table => "table",
        IdTable::Input => "input",
        IdTable::Extern => "extern",
        IdTable::Record => "record",
        IdTable::Local => "local",
    }
}

fn forbidden(w: Forbidden) -> &'static str {
    match w {
        Forbidden::Call => "a function call",
        Forbidden::Density => "a density read",
        Forbidden::Ambient => "an ambient",
        Forbidden::Host => "a host input",
        Forbidden::ExternCall => "an extern call",
        Forbidden::Table => "a table",
    }
}

fn axis(a: &tungsten_core::Axis) -> &'static str {
    match a {
        tungsten_core::Axis::X => "x",
        tungsten_core::Axis::Y => "y",
        tungsten_core::Axis::Z => "z",
    }
}

fn binop(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Subtract => "-",
        BinOp::Multiply => "*",
        BinOp::Divide => "/",
        BinOp::Equal => "==",
        BinOp::NotEqual => "!=",
        BinOp::Less => "<",
        BinOp::LessEqual => "<=",
        BinOp::Greater => ">",
        BinOp::GreaterEqual => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::ShiftLeft => "<<",
        BinOp::ShiftRight => ">>",
        BinOp::LogicalShiftRight => ">>>",
        BinOp::BitOr => "|",
        BinOp::BitAnd => "&",
        BinOp::BitXor => "^",
    }
}

fn unop(op: &UnOp) -> &'static str {
    match op {
        UnOp::Negate => "-",
        UnOp::Not => "!",
    }
}
