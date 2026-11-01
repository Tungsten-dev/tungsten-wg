/*
CUDA C++ source model: just enough of C++ to express generated density code and its host glue.
Owned and lifetime-free; printed by `super::codegen::CudaCodeGenerator`.
*/

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Bool,
    /// `signed char`: plain `char` has implementation-defined signedness.
    SignedChar,
    Int,
    LongLong,
    Unsigned,
    UnsignedLongLong,
    Float,
    Double,
    Char,
    Void,
    /// A named type, printed as is (`double3`, `tw::Config`, `Ctx`, ...).
    Named(String),
    /// `T*`, `const T*`, optionally `__restrict__`.
    Ptr {
        inner: Box<Type>,
        is_const: bool,
        restrict: bool,
    },
    /// `const T&`
    ConstRef(Box<Type>),
    /// `tw::Array<T, N>`
    Array(Box<Type>, usize),
    /// A C array; only valid as a declarator (`T name[N]`).
    CArray(Box<Type>, usize),
}

impl Type {
    pub fn named(name: &str) -> Type {
        Type::Named(name.to_string())
    }

    pub fn const_ptr(inner: Type) -> Type {
        Type::Ptr { inner: Box::new(inner), is_const: true, restrict: false }
    }

    pub fn const_restrict_ptr(inner: Type) -> Type {
        Type::Ptr { inner: Box::new(inner), is_const: true, restrict: true }
    }

    pub fn ptr(inner: Type) -> Type {
        Type::Ptr { inner: Box::new(inner), is_const: false, restrict: false }
    }

    pub fn is_pointer(&self) -> bool {
        matches!(self, Type::Ptr { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinaryOperator {
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
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOperator {
    Negate,
    /// `!`
    Not,
    /// `~`
    BitNot,
    /// `&`
    AddressOf,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    BoolLit(bool),
    IntLit(i32),
    LongLit(i64),
    FloatLit(f32),
    DoubleLit(f64),
    /// Printed verbatim as a primary expression (macros, hex literals, ...).
    Raw(String),
    /// A local, parameter or (qualified) name.
    Name(String),
    /// `base.field`, or `base->field` with `arrow`.
    Field {
        base: Box<Expression>,
        field: String,
        arrow: bool,
    },
    Index {
        base: Box<Expression>,
        index: Box<Expression>,
    },
    /// `callee<template_args>(args)`
    Call {
        callee: String,
        template_args: Vec<Type>,
        args: Vec<Expression>,
    },
    Binary {
        op: BinaryOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    Unary {
        op: UnaryOperator,
        operand: Box<Expression>,
    },
    /// `(T)expr`
    Cast {
        to: Type,
        expr: Box<Expression>,
    },
    /// `T{items}`
    BraceInit {
        t: Type,
        items: Vec<Expression>,
    },
    /// `{items}` without a type, e.g. the inner list of `tw::Array<T, N>{{...}}`.
    Braces(Vec<Expression>),
    /// `T{.name = value, ...}`
    DesignatedInit {
        t: Type,
        fields: Vec<(String, Expression)>,
    },
}

impl Expression {
    pub fn name(n: &str) -> Expression {
        Expression::Name(n.to_string())
    }

    pub fn raw(s: impl Into<String>) -> Expression {
        Expression::Raw(s.into())
    }

    pub fn field(base: Expression, field: &str) -> Expression {
        Expression::Field { base: Box::new(base), field: field.to_string(), arrow: false }
    }

    pub fn arrow(base: Expression, field: &str) -> Expression {
        Expression::Field { base: Box::new(base), field: field.to_string(), arrow: true }
    }

    pub fn index(base: Expression, index: Expression) -> Expression {
        Expression::Index { base: Box::new(base), index: Box::new(index) }
    }

    pub fn call(callee: &str, args: Vec<Expression>) -> Expression {
        Expression::Call { callee: callee.to_string(), template_args: vec![], args }
    }

    pub fn template_call(callee: &str, template_args: Vec<Type>, args: Vec<Expression>) -> Expression {
        Expression::Call { callee: callee.to_string(), template_args, args }
    }

    pub fn binary(op: BinaryOperator, left: Expression, right: Expression) -> Expression {
        Expression::Binary { op, left: Box::new(left), right: Box::new(right) }
    }

    pub fn unary(op: UnaryOperator, operand: Expression) -> Expression {
        Expression::Unary { op, operand: Box::new(operand) }
    }

    pub fn cast(to: Type, expr: Expression) -> Expression {
        Expression::Cast { to, expr: Box::new(expr) }
    }

    pub fn address_of(expr: Expression) -> Expression {
        Expression::unary(UnaryOperator::AddressOf, expr)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    /// `T name = init;`, or `T name{};` without `init`. `is_const` prefixes `const`.
    Declare {
        t: Type,
        name: String,
        init: Option<Expression>,
        is_const: bool,
    },
    /// `target = value;`
    Assign {
        target: Expression,
        value: Expression,
    },
    Expr(Expression),
    Return(Option<Expression>),
    If {
        cond: Expression,
        then: Vec<Statement>,
        other: Vec<Statement>,
    },
    While {
        cond: Expression,
        body: Vec<Statement>,
    },
    /// `for (int var = 0; var < end; ++var) { body }`
    ForRange {
        var: String,
        end: Expression,
        body: Vec<Statement>,
    },
    Break,
    /// `// text`, one line per `\n`.
    Comment(String),
    /// Printed verbatim, one line per `\n` (macro uses, launches, ...).
    Raw(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub doc: Option<String>,
    /// Before the return type: `__device__`, `extern "C" __global__`, `static`, ...
    pub qualifiers: String,
    pub return_type: Type,
    /// Between the return type and the name, e.g. `__launch_bounds__(128)`.
    pub attributes: String,
    pub name: String,
    pub params: Vec<(Type, String)>,
    /// `None` prints a declaration (`...;`).
    pub body: Option<Vec<Statement>>,
}

impl Function {
    /// The same function as a declaration only.
    pub fn declaration(&self) -> Function {
        Function { doc: None, body: None, ..self.clone() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Struct {
    pub doc: Option<String>,
    pub name: String,
    pub members: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// Printed verbatim, one line per `\n`.
    Raw(String),
    /// `// text`, one line per `\n`.
    Comment(String),
    Blank,
    Function(Function),
    /// `<qualifiers> T name = init;`
    Variable {
        qualifiers: String,
        t: Type,
        name: String,
        init: Expression,
    },
    /// `T name;` (a struct member).
    Field {
        t: Type,
        name: String,
    },
    Struct(Struct),
    Namespace {
        name: String,
        items: Vec<Item>,
    },
}

/// One generated file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct File {
    pub items: Vec<Item>,
}
