//! `Expr`: an IR expression with its type and the problems found while building it.
//!
//! Typing follows `tungsten-core/CHECK.md` §4.1 to §4.3. A bad operation records a
//! [`BuildError`] at its caller's location and yields an `Expr` whose type is the result type
//! where that's clear and unknown otherwise. An unknown operand raises no further problem, so
//! one mistake gives one error.

use std::panic::Location;
use std::sync::Mutex;

use tungsten_ir::{
    AmbientKind, BinaryOperator, Expression, UnaryOperator, VariableType,
};

use crate::error::{BuildError, BuildErrorKind, FieldProblem};
use crate::function::Local;
use crate::program::{Auto, Constant, Helper, Host, Input};

/// An interned type. Handles stay `Copy` because they hold these instead of `VariableType`s.
pub(crate) type Ty = &'static VariableType;

static TYPES: Mutex<Vec<Ty>> = Mutex::new(Vec::new());
static LISTS: Mutex<Vec<&'static [Ty]>> = Mutex::new(Vec::new());

/// Interns `t`, leaking one copy per distinct type.
pub(crate) fn intern(t: VariableType) -> Ty {
    let mut types = TYPES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = types.iter().find(|x| ***x == t) {
        return found;
    }
    let leaked: Ty = Box::leak(Box::new(t));
    types.push(leaked);
    leaked
}

/// Interns a list of types (a helper's parameters), leaking one copy per distinct list.
pub(crate) fn intern_list(ts: Vec<Ty>) -> &'static [Ty] {
    let mut lists = LISTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = lists
        .iter()
        .find(|l| l.len() == ts.len() && l.iter().zip(&ts).all(|(a, b)| a == b))
    {
        return found;
    }
    let leaked: &'static [Ty] = Box::leak(ts.into_boxed_slice());
    lists.push(leaked);
    leaked
}

pub(crate) fn name(t: &VariableType) -> String {
    format!("{t:?}")
}

fn is_integer(t: &VariableType) -> bool {
    matches!(t, VariableType::I8 | VariableType::I32 | VariableType::I64)
}

fn is_numeric(t: &VariableType) -> bool {
    is_integer(t) || matches!(t, VariableType::F32 | VariableType::F64)
}

/// The result type of `l op r`, or `None` if core rejects it (CHECK.md §4.2).
pub(crate) fn binary_type(op: BinaryOperator, l: Ty, r: Ty) -> Option<Ty> {
    use BinaryOperator as B;
    use VariableType as T;
    match op {
        B::Add | B::Subtract | B::Multiply | B::Divide => match (l, r) {
            (a, b) if a == b && is_numeric(a) => Some(l),
            (T::Vec3, T::Vec3) | (T::Vec3, T::Pos3) | (T::Pos3, T::Vec3) => {
                Some(intern(T::Vec3))
            }
            (T::Pos3, T::Pos3) => Some(l),
            _ => None,
        },
        B::Equal | B::NotEqual => {
            (l == r && (is_numeric(l) || *l == T::Bool)).then(|| intern(T::Bool))
        }
        B::Less | B::LessEqual | B::Greater | B::GreaterEqual => {
            (l == r && is_numeric(l)).then(|| intern(T::Bool))
        }
        B::And | B::Or => (*l == T::Bool && *r == T::Bool).then(|| intern(T::Bool)),
        B::BitAnd | B::BitOr | B::BitXor => {
            (l == r && (is_integer(l) || *l == T::Bool)).then_some(l)
        }
        B::ShiftLeft | B::ShiftRight | B::LogicalShiftRight => {
            (is_integer(l) && *r == T::I32).then_some(l)
        }
    }
}

/// The result type of `op operand`, or `None` if core rejects it (CHECK.md §4.1).
pub(crate) fn unary_type(op: UnaryOperator, t: Ty) -> Option<Ty> {
    match op {
        UnaryOperator::Not => (*t == VariableType::Bool || is_integer(t)).then_some(t),
        UnaryOperator::Negate => {
            (is_numeric(t) || matches!(t, VariableType::Vec3 | VariableType::Pos3)).then_some(t)
        }
        UnaryOperator::Ref(_) => Some(t),
    }
}

/// Whether core allows casting `from` to `to` (CHECK.md §4.3).
pub(crate) fn cast_ok(from: &VariableType, to: &VariableType) -> bool {
    (is_numeric(from) && is_numeric(to))
        || (*from == VariableType::Bool && is_integer(to))
        || (*from == VariableType::Pos3 && *to == VariableType::Vec3)
}

fn op_symbol(op: BinaryOperator) -> &'static str {
    use BinaryOperator as B;
    match op {
        B::Add => "+",
        B::Subtract => "-",
        B::Multiply => "*",
        B::Divide => "/",
        B::Equal => "==",
        B::NotEqual => "!=",
        B::Less => "<",
        B::LessEqual => "<=",
        B::Greater => ">",
        B::GreaterEqual => ">=",
        B::And => "&&",
        B::Or => "||",
        B::ShiftLeft => "<<",
        B::ShiftRight => ">>",
        B::LogicalShiftRight => ">>>",
        B::BitOr => "|",
        B::BitAnd => "&",
        B::BitXor => "^",
    }
}

/// An expression under construction: the IR node, its type if known, and every problem found
/// in it so far. When it reaches a statement, the function takes the problems.
#[derive(Debug, Clone)]
pub struct Expr {
    pub(crate) node: Expression,
    pub(crate) ty: Option<Ty>,
    pub(crate) problems: Vec<BuildError>,
}

impl Expr {
    pub(crate) fn typed(node: Expression, ty: Option<Ty>) -> Self {
        Self {
            node,
            ty,
            problems: Vec::new(),
        }
    }

    /// The expression's type, or `None` if a mistake made it unknown.
    pub fn ty(&self) -> Option<&'static VariableType> {
        self.ty
    }

    /// Whether this expression carries a recorded mistake.
    pub fn has_problems(&self) -> bool {
        !self.problems.is_empty()
    }

    fn problem(&mut self, kind: BuildErrorKind, at: &'static Location<'static>) {
        self.problems.push(BuildError::new(kind, at));
    }

    fn take(&mut self, other: &mut Expr) {
        self.problems.append(&mut other.problems);
    }

    /// Returns a closure that clones this expression, for using it more than once.
    pub fn reusable(self) -> impl Fn() -> Expr {
        move || self.clone()
    }

    #[track_caller]
    pub(crate) fn binary(op: BinaryOperator, l: Expr, r: Expr) -> Expr {
        let at = Location::caller();
        let (mut l, mut r) = (l, r);
        let ty = match (l.ty, r.ty) {
            (Some(a), Some(b)) => binary_type(op, a, b),
            _ => None,
        };
        let known = l.ty.is_some() && r.ty.is_some();
        let ty = ty.or_else(|| fallback_binary(op, l.ty));
        let mut out = Expr::typed(
            Expression::BinaryOp {
                op,
                left: Box::new(std::mem::replace(&mut l.node, Expression::Bool(false))),
                right: Box::new(std::mem::replace(&mut r.node, Expression::Bool(false))),
            },
            ty,
        );
        out.take(&mut l);
        out.take(&mut r);
        if known && binary_type(op, l.ty.unwrap(), r.ty.unwrap()).is_none() {
            out.problem(
                BuildErrorKind::BadBinary {
                    op: op_symbol(op),
                    left: name(l.ty.unwrap()),
                    right: name(r.ty.unwrap()),
                },
                at,
            );
        }
        out
    }

    #[track_caller]
    pub(crate) fn unary(op: UnaryOperator, mut e: Expr) -> Expr {
        let at = Location::caller();
        let operand_ty = e.ty;
        let ty = operand_ty.and_then(|t| unary_type(op, t));
        let mut out = Expr::typed(
            Expression::UnaryOp {
                op,
                operand: Box::new(std::mem::replace(&mut e.node, Expression::Bool(false))),
            },
            ty,
        );
        out.take(&mut e);
        if let Some(t) = operand_ty
            && ty.is_none()
        {
            let sym = match op {
                UnaryOperator::Negate => "-",
                UnaryOperator::Not => "!",
                UnaryOperator::Ref(false) => "&",
                UnaryOperator::Ref(true) => "&mut",
            };
            out.problem(
                BuildErrorKind::BadUnary {
                    op: sym,
                    operand: name(t),
                },
                at,
            );
        }
        out
    }

    /// Builds `name { field: value, ... }` for a record type. The builder doesn't know records'
    /// fields, so the field list is checked by core.
    pub fn record(
        name: &'static str,
        fields: impl IntoIterator<Item = (&'static str, Expr)>,
    ) -> Expr {
        let mut problems = Vec::new();
        let args = fields
            .into_iter()
            .map(|(f, mut e)| {
                problems.append(&mut e.problems);
                (f, e.node)
            })
            .collect();
        Expr {
            node: Expression::ConstructExtern {
                t: VariableType::Extern(name),
                args,
            },
            ty: Some(intern(VariableType::Extern(name))),
            problems,
        }
    }

    /// `vec3(x, y, z)` from three `f64`s, or `pos3(x, y, z)` from three `i32`s. The macros
    /// `vec3!` and `pos3!` call this.
    #[track_caller]
    #[doc(hidden)]
    pub fn construct(t: VariableType, args: [Expr; 3]) -> Expr {
        let at = Location::caller();
        let elem = match t {
            VariableType::Vec3 => VariableType::F64,
            _ => VariableType::I32,
        };
        let tn = name(&t);
        let mut out = Expr::typed(Expression::Bool(false), Some(intern(t.clone())));
        let mut nodes = Vec::with_capacity(3);
        for (index, mut a) in args.into_iter().enumerate() {
            if let Some(found) = a.ty
                && *found != elem
            {
                out.problem(
                    BuildErrorKind::BadConstruct {
                        t: tn.clone(),
                        index,
                        expected: name(&elem),
                        found: name(found),
                    },
                    at,
                );
            }
            out.take(&mut a);
            nodes.push(a.node);
        }
        out.node = Expression::Construct { t, args: nodes };
        out
    }

    /// An array literal. The macro `array!` calls this.
    #[track_caller]
    #[doc(hidden)]
    pub fn array(elems: Vec<Expr>) -> Expr {
        let at = Location::caller();
        let len = elems.len();
        let first = elems.first().and_then(|e| e.ty);
        let mut out = Expr::typed(Expression::Bool(false), None);
        if elems.is_empty() {
            out.problem(BuildErrorKind::EmptyArray, at);
        }
        let mut nodes = Vec::with_capacity(len);
        for (index, mut e) in elems.into_iter().enumerate() {
            if let (Some(expected), Some(found)) = (first, e.ty)
                && expected != found
            {
                out.problem(
                    BuildErrorKind::ArrayElement {
                        index,
                        expected: name(expected),
                        found: name(found),
                    },
                    at,
                );
            }
            out.take(&mut e);
            nodes.push(e.node);
        }
        out.node = Expression::ArrayLiteral(nodes);
        out.ty = first.map(|t| intern(VariableType::Array(Box::new(t.clone()), len)));
        out
    }

    /// An extern call checked against a declared signature. The functions generated by
    /// `extern_fns!` call this; there is deliberately no untyped extern call.
    #[track_caller]
    #[doc(hidden)]
    pub fn __extern_call(
        function_name: &str,
        params: Vec<VariableType>,
        ret: VariableType,
        args: Vec<Expr>,
    ) -> Expr {
        let at = Location::caller();
        let params: Vec<Ty> = params.into_iter().map(intern).collect();
        let mut out = Expr::typed(Expression::Bool(false), Some(intern(ret)));
        let nodes = check_args(&mut out, &params, args, at);
        out.node = Expression::ExternCall {
            function_name: function_name.to_string(),
            parameters: nodes,
            parameter_types: params.iter().map(|t| (*t).clone()).collect(),
            return_type: out.ty.unwrap().clone(),
        };
        out
    }

    // ---- structure -------------------------------------------------------------------

    /// `self[index]`. `self` must be an array and `index` an `i32`.
    #[track_caller]
    pub fn index(self, index: impl Into<Expr>) -> Expr {
        let at = Location::caller();
        let (mut a, mut i) = (self, index.into());
        let ty = match a.ty {
            Some(VariableType::Array(elem, _)) => Some(intern((**elem).clone())),
            _ => None,
        };
        let mut out = Expr::typed(
            Expression::ArrayAccess {
                array: Box::new(std::mem::replace(&mut a.node, Expression::Bool(false))),
                index: Box::new(std::mem::replace(&mut i.node, Expression::Bool(false))),
            },
            ty,
        );
        out.take(&mut a);
        out.take(&mut i);
        if let Some(t) = a.ty
            && ty.is_none()
        {
            out.problem(BuildErrorKind::NotArray { found: name(t) }, at);
        }
        check_index(&mut out, i.ty, at);
        out
    }

    /// `self.field`, stated to have type `t`. On `vec3`/`pos3` the field and type are
    /// verified; on a record they are trusted and core checks them.
    #[track_caller]
    pub fn field(self, field: &str, t: VariableType) -> Expr {
        let at = Location::caller();
        self.field_at(field, Some(t), at)
    }

    /// `.x` of a `vec3` (`f64`) or `pos3` (`i32`).
    #[track_caller]
    pub fn x(self) -> Expr {
        self.field_at("x", None, Location::caller())
    }

    /// `.y` of a `vec3` (`f64`) or `pos3` (`i32`).
    #[track_caller]
    pub fn y(self) -> Expr {
        self.field_at("y", None, Location::caller())
    }

    /// `.z` of a `vec3` (`f64`) or `pos3` (`i32`).
    #[track_caller]
    pub fn z(self) -> Expr {
        self.field_at("z", None, Location::caller())
    }

    fn field_at(
        mut self,
        field: &str,
        stated: Option<VariableType>,
        at: &'static Location<'static>,
    ) -> Expr {
        let base_ty = self.ty;
        let mut problem = None;
        let real: Option<VariableType> = match base_ty {
            Some(VariableType::Vec3) | Some(VariableType::Pos3) => {
                if matches!(field, "x" | "y" | "z") {
                    Some(if *base_ty.unwrap() == VariableType::Vec3 {
                        VariableType::F64
                    } else {
                        VariableType::I32
                    })
                } else {
                    problem = Some(FieldProblem::NoSuchField);
                    None
                }
            }
            Some(VariableType::Extern(_)) => {
                if stated.is_none() {
                    // `.x()` on a record: its fields are unknown here.
                    problem = Some(FieldProblem::NotAStruct);
                }
                stated.clone()
            }
            Some(_) => {
                problem = Some(FieldProblem::NotAStruct);
                None
            }
            None => None,
        };
        if let (Some(real), Some(stated)) = (&real, &stated)
            && real != stated
            && problem.is_none()
        {
            problem = Some(FieldProblem::Annotation {
                annotated: name(stated),
                actual: name(real),
            });
        }
        let ty = real.clone().or(stated.clone()).map(intern);
        let type_of_field = ty.cloned().unwrap_or(VariableType::F64);
        let base = std::mem::replace(&mut self.node, Expression::Bool(false));
        let mut out = Expr::typed(
            Expression::Field {
                base: Box::new(base),
                field: field.to_string(),
                type_of_field,
                base_type: base_ty.cloned().unwrap_or(VariableType::Vec3),
            },
            ty,
        );
        out.take(&mut self);
        if let (Some(problem), Some(b)) = (problem, base_ty) {
            out.problem(
                BuildErrorKind::BadField {
                    field: field.to_string(),
                    base: name(b),
                    problem,
                },
                at,
            );
        }
        out
    }

    // ---- conversion ------------------------------------------------------------------

    /// `self as t` (CHECK.md §4.3). Its type is always `t`.
    #[track_caller]
    pub fn cast(mut self, t: VariableType) -> Expr {
        let at = Location::caller();
        let from = self.ty;
        let to = intern(t.clone());
        let mut out = Expr::typed(
            Expression::ExplicitCast {
                to: t,
                expr: Box::new(std::mem::replace(&mut self.node, Expression::Bool(false))),
            },
            Some(to),
        );
        out.take(&mut self);
        if let Some(from) = from
            && !cast_ok(from, to)
        {
            out.problem(
                BuildErrorKind::BadCast {
                    from: name(from),
                    to: name(to),
                },
                at,
            );
        }
        out
    }

    /// `&self`, for passing to an extern by reference. Same type as `self`.
    #[track_caller]
    pub fn as_ref(self) -> Expr {
        Expr::unary(UnaryOperator::Ref(false), self)
    }

    /// `&mut self`, for an extern that writes through it. Same type as `self`.
    #[track_caller]
    pub fn as_mut_ref(self) -> Expr {
        Expr::unary(UnaryOperator::Ref(true), self)
    }
}

/// Checks call arguments against `params`, recording `Arity` and `ArgumentType`.
pub(crate) fn check_args(
    out: &mut Expr,
    params: &[Ty],
    args: Vec<Expr>,
    at: &'static Location<'static>,
) -> Vec<Expression> {
    if args.len() != params.len() {
        out.problem(
            BuildErrorKind::Arity {
                expected: params.len(),
                found: args.len(),
            },
            at,
        );
    }
    let mut nodes = Vec::with_capacity(args.len());
    for (index, mut a) in args.into_iter().enumerate() {
        if let (Some(expected), Some(found)) = (params.get(index), a.ty)
            && expected != &found
        {
            out.problem(
                BuildErrorKind::ArgumentType {
                    index,
                    expected: name(expected),
                    found: name(found),
                },
                at,
            );
        }
        out.take(&mut a);
        nodes.push(a.node);
    }
    nodes
}

pub(crate) fn check_index(out: &mut Expr, index: Option<Ty>, at: &'static Location<'static>) {
    if let Some(t) = index
        && *t != VariableType::I32
    {
        out.problem(BuildErrorKind::BadIndex { found: name(t) }, at);
    }
}

/// When a binary operation can't be typed: comparisons and logic are still `bool`, and a shift
/// keeps its left type.
fn fallback_binary(op: BinaryOperator, left: Option<Ty>) -> Option<Ty> {
    use BinaryOperator as B;
    match op {
        B::Equal
        | B::NotEqual
        | B::Less
        | B::LessEqual
        | B::Greater
        | B::GreaterEqual
        | B::And
        | B::Or => Some(intern(VariableType::Bool)),
        B::ShiftLeft | B::ShiftRight | B::LogicalShiftRight => {
            left.filter(|t| is_integer(t))
        }
        _ => None,
    }
}

// ---- ambients -----------------------------------------------------------------------------

/// The current cell's position (`pos3`).
pub fn pos3() -> Expr {
    Expr::typed(Expression::Ambient(AmbientKind::Pos3), Some(intern(VariableType::Pos3)))
}

/// The grid's origin (`vec3`).
pub fn origin() -> Expr {
    Expr::typed(Expression::Ambient(AmbientKind::Origin), Some(intern(VariableType::Vec3)))
}

/// The origin scale (`vec3`).
pub fn origin_scale() -> Expr {
    Expr::typed(
        Expression::Ambient(AmbientKind::OriginScale),
        Some(intern(VariableType::Vec3)),
    )
}

/// The position scale (`vec3`).
pub fn position_scale() -> Expr {
    Expr::typed(
        Expression::Ambient(AmbientKind::PositionScale),
        Some(intern(VariableType::Vec3)),
    )
}

// ---- conversions into Expr -----------------------------------------------------------------

macro_rules! from_lit {
    ($($t:ty => $variant:ident / $vt:ident),* $(,)?) => {$(
        impl From<$t> for Expr {
            fn from(v: $t) -> Self {
                Expr::typed(Expression::$variant(v), Some(intern(VariableType::$vt)))
            }
        }
    )*};
}
from_lit!(f32 => Float / F32, f64 => Double / F64, i8 => Byte / I8, i32 => Int / I32,
          i64 => Long / I64, bool => Bool / Bool);

impl From<Local> for Expr {
    fn from(l: Local) -> Self {
        Expr::typed(Expression::Variable(l.var), Some(l.ty))
    }
}

impl From<Host> for Expr {
    fn from(h: Host) -> Self {
        Expr::typed(Expression::HostInput(h.r), Some(h.ty))
    }
}

impl From<Auto> for Expr {
    fn from(t: Auto) -> Self {
        Expr::typed(Expression::AutoConst(t.r), Some(t.ty))
    }
}

impl From<Constant> for Expr {
    fn from(c: Constant) -> Self {
        Expr::typed(Expression::Constant(c.r), c.ty)
    }
}

impl From<Input> for Expr {
    fn from(i: Input) -> Self {
        Expr::typed(Expression::DensityVariable(i.r, None), i.ty)
    }
}

impl Input {
    /// Reads the input at the current cell.
    pub fn read(self) -> Expr {
        self.into()
    }

    /// Reads the input at an explicit flat index, which must be an `i32`.
    #[track_caller]
    pub fn at(self, index: impl Into<Expr>) -> Expr {
        let at = Location::caller();
        let mut i = index.into();
        let mut out = Expr::typed(
            Expression::DensityVariable(
                self.r,
                Some(Box::new(std::mem::replace(&mut i.node, Expression::Bool(false)))),
            ),
            self.ty,
        );
        out.take(&mut i);
        check_index(&mut out, i.ty, at);
        out
    }
}

impl Helper {
    /// Calls the helper. Its type is the helper's return type, even when the arguments are
    /// wrong.
    #[track_caller]
    pub fn call(self, args: impl IntoIterator<Item = Expr>) -> Expr {
        let at = Location::caller();
        let mut out = Expr::typed(Expression::Bool(false), Some(self.ret));
        let nodes = check_args(&mut out, self.params, args.into_iter().collect(), at);
        out.node = Expression::FunctionCall {
            function: self.r,
            parameters: nodes,
        };
        out
    }
}

// ---- methods shared by Expr and the handles ------------------------------------------------

macro_rules! named_binops {
    ($($name:ident => $op:ident),* $(,)?) => {
        // The names are the legacy builder's; `&`, `|`, `<<`… stay methods like the comparisons.
        #[allow(clippy::should_implement_trait)]
        impl Expr {$(
            #[track_caller]
            pub fn $name(self, rhs: impl Into<Expr>) -> Expr {
                Expr::binary(BinaryOperator::$op, self, rhs.into())
            }
        )*}
    };
}
named_binops!(lt => Less, le => LessEqual, gt => Greater, ge => GreaterEqual,
    eq_expr => Equal, ne => NotEqual, and => And, or => Or,
    bitand => BitAnd, bitor => BitOr, bitxor => BitXor,
    shl => ShiftLeft, shr => ShiftRight, ushr => LogicalShiftRight);

/// Gives a handle the same methods as `Expr`.
macro_rules! handle_methods {
    ($($h:ty),* $(,)?) => {$(
        #[allow(clippy::should_implement_trait)]
        impl $h {
            #[track_caller] pub fn lt(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).lt(rhs) }
            #[track_caller] pub fn le(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).le(rhs) }
            #[track_caller] pub fn gt(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).gt(rhs) }
            #[track_caller] pub fn ge(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).ge(rhs) }
            #[track_caller] pub fn eq_expr(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).eq_expr(rhs) }
            #[track_caller] pub fn ne(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).ne(rhs) }
            #[track_caller] pub fn and(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).and(rhs) }
            #[track_caller] pub fn or(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).or(rhs) }
            #[track_caller] pub fn bitand(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).bitand(rhs) }
            #[track_caller] pub fn bitor(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).bitor(rhs) }
            #[track_caller] pub fn bitxor(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).bitxor(rhs) }
            #[track_caller] pub fn shl(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).shl(rhs) }
            #[track_caller] pub fn shr(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).shr(rhs) }
            #[track_caller] pub fn ushr(self, rhs: impl Into<Expr>) -> Expr { Expr::from(self).ushr(rhs) }
            #[track_caller] pub fn index(self, i: impl Into<Expr>) -> Expr { Expr::from(self).index(i) }
            #[track_caller] pub fn field(self, f: &str, t: VariableType) -> Expr { Expr::from(self).field(f, t) }
            #[track_caller] pub fn x(self) -> Expr { Expr::from(self).x() }
            #[track_caller] pub fn y(self) -> Expr { Expr::from(self).y() }
            #[track_caller] pub fn z(self) -> Expr { Expr::from(self).z() }
            #[track_caller] pub fn cast(self, t: VariableType) -> Expr { Expr::from(self).cast(t) }
            #[track_caller] pub fn as_ref(self) -> Expr { Expr::from(self).as_ref() }
            #[track_caller] pub fn as_mut_ref(self) -> Expr { Expr::from(self).as_mut_ref() }
            pub fn reusable(self) -> impl Fn() -> Expr { Expr::from(self).reusable() }
        }
    )*};
}
handle_methods!(Local, Host, Auto, Constant, Input);

// ---- operators -----------------------------------------------------------------------------

macro_rules! arith {
    ($($trait:ident :: $method:ident => $op:ident),* $(,)?) => {$(
        impl<R: Into<Expr>> std::ops::$trait<R> for Expr {
            type Output = Expr;
            #[track_caller]
            fn $method(self, rhs: R) -> Expr {
                Expr::binary(BinaryOperator::$op, self, rhs.into())
            }
        }
        arith!(@handles $trait $method $op; Local, Host, Auto, Constant, Input);
        arith!(@lits $trait $method $op; f32, f64, i8, i32, i64);
    )*};
    (@handles $trait:ident $method:ident $op:ident; $($h:ty),*) => {$(
        impl<R: Into<Expr>> std::ops::$trait<R> for $h {
            type Output = Expr;
            #[track_caller]
            fn $method(self, rhs: R) -> Expr {
                Expr::binary(BinaryOperator::$op, self.into(), rhs.into())
            }
        }
    )*};
    (@lits $trait:ident $method:ident $op:ident; $($l:ty),*) => {$(
        arith!(@lit $trait $method $op $l; Expr, Local, Host, Auto, Constant, Input);
    )*};
    (@lit $trait:ident $method:ident $op:ident $l:ty; $($r:ty),*) => {$(
        impl std::ops::$trait<$r> for $l {
            type Output = Expr;
            #[track_caller]
            fn $method(self, rhs: $r) -> Expr {
                Expr::binary(BinaryOperator::$op, self.into(), rhs.into())
            }
        }
    )*};
}
arith!(Add::add => Add, Sub::sub => Subtract, Mul::mul => Multiply, Div::div => Divide);

macro_rules! unops {
    ($($h:ty),*) => {$(
        impl std::ops::Neg for $h {
            type Output = Expr;
            #[track_caller]
            fn neg(self) -> Expr {
                Expr::unary(UnaryOperator::Negate, self.into())
            }
        }
        impl std::ops::Not for $h {
            type Output = Expr;
            #[track_caller]
            fn not(self) -> Expr {
                Expr::unary(UnaryOperator::Not, self.into())
            }
        }
    )*};
}
unops!(Expr, Local, Host, Auto, Constant, Input);
