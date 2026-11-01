/*
Java source model: just enough of Java to express generated density code.
Owned and lifetime-free; printed by `super::codegen::JavaCodeGenerator`.
*/

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Boolean,
    Byte,
    Int,
    Long,
    Float,
    Double,
    Void,
    Class(String),
    Array(Box<Type>),
}

impl Type {
    pub fn class(name: &str) -> Type {
        Type::Class(name.to_string())
    }

    pub fn is_primitive(&self) -> bool {
        matches!(self, Type::Boolean | Type::Byte | Type::Int | Type::Long | Type::Float | Type::Double)
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
    /// `>>>`
    UnsignedShiftRight,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOperator {
    Negate,
    /// `!`
    Not,
    /// `~`
    BitNot,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    BoolLit(bool),
    IntLit(i32),
    LongLit(i64),
    FloatLit(f32),
    DoubleLit(f64),
    Null,
    /// A local, parameter or class name.
    Name(String),
    Field {
        base: Box<Expression>,
        field: String,
    },
    Index {
        base: Box<Expression>,
        index: Box<Expression>,
    },
    /// `target.name(args)`, or `name(args)` without a target.
    Call {
        target: Option<Box<Expression>>,
        name: String,
        args: Vec<Expression>,
    },
    /// `new Class(args)`, optionally followed by `/* comment */`.
    New {
        class: String,
        args: Vec<Expression>,
        comment: Option<String>,
    },
    /// `new T[]{items}`
    NewArray {
        element: Type,
        items: Vec<Expression>,
    },
    /// `new T[len]`
    NewArraySized {
        element: Type,
        len: usize,
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
    Cast {
        to: Type,
        expr: Box<Expression>,
    },
    Conditional {
        cond: Box<Expression>,
        then: Box<Expression>,
        other: Box<Expression>,
    },
}

impl Expression {
    pub fn name(n: &str) -> Expression {
        Expression::Name(n.to_string())
    }

    pub fn field(base: Expression, field: &str) -> Expression {
        Expression::Field { base: Box::new(base), field: field.to_string() }
    }

    pub fn call(target: Option<Expression>, name: &str, args: Vec<Expression>) -> Expression {
        Expression::Call { target: target.map(Box::new), name: name.to_string(), args }
    }

    pub fn binary(op: BinaryOperator, left: Expression, right: Expression) -> Expression {
        Expression::Binary { op, left: Box::new(left), right: Box::new(right) }
    }

    pub fn cast(to: Type, expr: Expression) -> Expression {
        Expression::Cast { to, expr: Box::new(expr) }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Declare {
        t: Type,
        name: String,
        init: Option<Expression>,
        is_final: bool,
    },
    /// `target = value;` where target is a name, field or index.
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
    /// `for (int var = 0; var < end; var++) { body }`
    ForRange {
        var: String,
        end: Expression,
        body: Vec<Statement>,
    },
    Break,
    Block(Vec<Statement>),
    Comment(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub modifiers: String,
    pub t: Type,
    pub name: String,
    pub init: Option<Expression>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Method {
    pub doc: Option<String>,
    pub modifiers: String,
    /// `None` for a constructor.
    pub return_type: Option<Type>,
    pub name: String,
    pub params: Vec<(Type, String)>,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Class {
    pub doc: Option<String>,
    pub modifiers: String,
    pub name: String,
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
    pub nested: Vec<Class>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompilationUnit {
    pub header: Vec<String>,
    pub package: Option<String>,
    pub imports: Vec<String>,
    pub class: Class,
}
