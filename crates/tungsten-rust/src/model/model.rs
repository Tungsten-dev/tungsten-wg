/*
ICL (Intermediate C/Rust Language) is a model for representing low-level CPU code
that can be easily converted from SPMT and compiled to Rust library functions.

The idea is to have a flexible intermediate representation that captures:
- Functions with parameters and return types
- Variables with type information
- Control flow (if/else, loops)
- Expressions and operators
- Type information (primitives, pointers, structs)
*/

use std::{fmt::Debug, hash::Hash, rc::Rc};

use crate::interned::Interned;

/// Root structure containing all functions and definitions
#[derive(Debug, Clone)]
pub struct RCL<'m> {
    pub functions: Vec<FunctionRef<'m>>,
    pub structs: Vec<StructRef<'m>>,
    pub main_functions: Vec<FunctionRef<'m>>,
    pub import_statements: Vec<String>,
    pub constants: Vec<Constant<'m>>,
    pub impls: Vec<Impl<'m>>,
}

/// An `impl` block; all methods are emitted as `pub`.
#[derive(Debug, Clone)]
pub struct Impl<'m> {
    pub type_name: String,
    pub methods: Vec<FunctionRef<'m>>,
}

/// How a method receives `self`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receiver {
    Value,
    Ref,
    MutRef,
}

/// A low-level CPU function with typed parameters and return type
#[derive(Debug, Clone)]
pub struct Function<'m> {
    pub name: Option<String>,
    pub parameters: Vec<Parameter>,
    pub return_type: Option<Type>,
    pub body: Vec<Statement<'m>>,
    pub variables: Vec<Rc<Variable>>,
    pub inline: bool,
    /// Set for methods inside an `impl`; printed before `parameters`.
    pub receiver: Option<Receiver>,
}

/// A parameter to a function
#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub t: Type,
}

/// A struct definition for more complex types
#[derive(Debug, Clone)]
pub struct Struct {
    pub name: String,
    pub fields: Vec<(String, Type)>,
    pub is_repr_c: bool,
    pub derivations: Vec<String>,
}

/// Variable with type information
#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    pub name: Option<String>,
    pub t: Type,
    pub mutable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Constant<'m> {
    pub var: Rc<Variable>,
    pub value: Expression<'m>,
}

/// Supported types in the CPU language
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Type {
    // Primitive types
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Bool,
    Void,

    // Composite types
    Pointer(Box<Type>),
    Ref(Box<Type>),
    MutRef(Box<Type>),
    Box(Box<Type>),
    Struct(String),
    Array(Box<Type>, usize),
    ArrayRef(Box<Type>, usize),
    MutArrayRef(Box<Type>, usize),
    Tuple(Vec<Type>),
}

impl Type {
    pub fn is_signed_int(&self) -> bool {
        matches!(self, Type::I8 | Type::I16 | Type::I32 | Type::I64)
    }

    pub fn is_unsigned_int(&self) -> bool {
        matches!(self, Type::U8 | Type::U16 | Type::U32 | Type::U64)
    }

    pub fn is_int(&self) -> bool {
        self.is_signed_int() || self.is_unsigned_int()
    }

    pub fn is_float(&self) -> bool {
        matches!(self, Type::F32 | Type::F64)
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::U8 => write!(f, "u8"),
            Type::U16 => write!(f, "u16"),
            Type::U32 => write!(f, "u32"),
            Type::U64 => write!(f, "u64"),
            Type::I8 => write!(f, "i8"),
            Type::I16 => write!(f, "i16"),
            Type::I32 => write!(f, "i32"),
            Type::I64 => write!(f, "i64"),
            Type::F32 => write!(f, "f32"),
            Type::F64 => write!(f, "f64"),
            Type::Bool => write!(f, "bool"),
            Type::Void => write!(f, "()"),
            Type::Pointer(t) => write!(f, "*{}", t),
            Type::Ref(t) => write!(f, "&{}", t),
            Type::MutRef(t) => write!(f, "&mut {}", t),
            Type::Box(t) => write!(f, "Box<{}>", t),
            Type::Struct(name) => write!(f, "{}", name),
            Type::Array(t, size) => write!(f, "[{}; {}]", t, size),
            Type::ArrayRef(t, size) => write!(f, "&[{}; {}]", t, size),
            Type::MutArrayRef(t, size) => write!(f, "&mut [{}; {}]", t, size),
            Type::Tuple(items) => {
                let item_strs: Vec<String> = items.iter().map(|t| format!("{}", t)).collect();
                write!(f, "({})", item_strs.join(", "))
            }
        }
    }
}

pub type FunctionRef<'m> = Interned<'m, Function<'m>>;
pub type StructRef<'m> = Interned<'m, Struct>;

/// Statements form the body of functions
#[derive(Debug, Clone, PartialEq)]
pub enum Statement<'m> {
    // Assignment: variable = expression
    Assign {
        target: Rc<Variable>,
        value: Expression<'m>,
    },

    ArrayAssign {
        target: Rc<Variable>,
        index: Expression<'m>,
        value: Expression<'m>,
    },

    // Assignment to an arbitrary place, e.g. `buffers.x[i] = v` or `auto_consts.a = v`
    AssignPlace {
        target: Expression<'m>,
        value: Expression<'m>,
    },

    // Return statement
    Return(Option<Expression<'m>>),

    // Conditional statement
    If {
        condition: Expression<'m>,
        then_branch: Vec<Statement<'m>>,
        else_branch: Option<Vec<Statement<'m>>>,
    },

    // While loop
    While {
        condition: Expression<'m>,
        body: Vec<Statement<'m>>,
    },

    // For loop: for (init; condition; increment) { body }
    For {
        init: Option<Box<Statement<'m>>>,
        condition: Option<Expression<'m>>,
        increment: Option<Box<Statement<'m>>>,
        body: Vec<Statement<'m>>,
    },

    // Iter loop: for variable in iterable { body }
    ForIn {
        variable: Rc<Variable>,
        iterable: Expression<'m>,
        body: Vec<Statement<'m>>,
    },

    Break,
    Continue,

    // Variable declaration with optional initialization
    Declare {
        variable: Rc<Variable>,
        init: Option<Expression<'m>>,
        mutable: bool,
    },

    // Function call as statement
    FunctionCall {
        function: FunctionRef<'m>,
        arguments: Vec<Expression<'m>>,
    },

    // Block scope
    Block(Vec<Statement<'m>>),
    InlineRust(String),
}

/// Expressions produce values
#[derive(Debug, Clone, PartialEq)]
pub enum Expression<'m> {
    // Variable reference
    Variable(Rc<Variable>),

    // Literal values
    I8Literal(i8),
    I32Literal(i32),
    I64Literal(i64),
    F32Literal(f32),
    F64Literal(f64),
    BoolLiteral(bool),

    // Binary operations
    BinaryOp {
        op: BinaryOperator,
        left: Box<Expression<'m>>,
        right: Box<Expression<'m>>,
    },

    // Unary operations
    UnaryOp {
        op: UnaryOperator,
        operand: Box<Expression<'m>>,
    },

    // Function call
    FunctionCall {
        function: FunctionRef<'m>,
        arguments: Vec<Expression<'m>>,
    },

    // Late bound call. This is used for function calls where we only know the name and argument types,
    // but not a direct reference to a function definition.
    LateBoundCall {
        function_name: String,
        argument_types: Vec<Type>,
        return_type: Type,
        arguments: Vec<Expression<'m>>,
    },

    // Field/member access
    Field {
        base: Box<Expression<'m>>,
        field: String,
    },

    // Array indexing
    Index {
        base: Box<Expression<'m>>,
        index: Box<Expression<'m>>,
    },

    // Dereference pointer
    Deref(Box<Expression<'m>>),

    // Take address
    Ref(Box<Expression<'m>>),
    MutRef(Box<Expression<'m>>),

    // Cast expression
    Cast {
        expr: Box<Expression<'m>>,
        to_type: Type,
    },

    // Struct initialization
    StructInit {
        struct_name: String,
        fields: Vec<(String, Expression<'m>)>,
    },

    ArrayInit {
        element_type: Type,
        element: Box<Expression<'m>>,
        count: usize,
    },

    ArrayLiteral(Vec<Expression<'m>>),

    TupleInit(Vec<Expression<'m>>),

    Construct {
        t: Type,
        args: Vec<(&'static str, Expression<'m>)>,
    },

    // Raw inline Rust code
    InlineRust(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    // Arithmetic
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,

    // Comparison
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,

    // Logical
    And,
    Or,

    // Bitwise
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    LeftShift,
    RightShift,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    // Arithmetic
    Negate,

    // Logical
    Not,

    // Bitwise
    BitwiseNot,

    // Reference
    Ref(bool),
}

impl<'m> Function<'m> {
    pub fn new(name: Option<String>, return_type: Option<Type>) -> Self {
        Function {
            name,
            parameters: Vec::new(),
            return_type,
            body: Vec::new(),
            variables: Vec::new(),
            inline: false,
            receiver: None,
        }
    }

    pub fn add_parameter(&mut self, name: String, t: Type) {
        for param in &self.parameters {
            if param.name == name {
                panic!("Parameter already exists: {}", name);
            }
        }
        self.parameters.push(Parameter { name, t });
    }

    pub fn add_statement(&mut self, statement: Statement<'m>) {
        self.body.push(statement);
    }

    pub fn add_variable(&mut self, variable: Rc<Variable>) {
        // debug check!
        for var in &self.variables {
            if Rc::ptr_eq(var, &variable) {
                panic!("Variable already exists: {:?}", variable);
            }
            if var.name == variable.name {
                panic!("Variable with the same name already exists: {:?}", variable);
            }
        }
        self.variables.push(variable);
    }
}

impl Struct {
    pub fn new(name: String) -> Self {
        Struct {
            name,
            fields: Vec::new(),
            is_repr_c: false,
            derivations: Vec::new(),
        }
    }

    pub fn set_repr_c(&mut self, is_repr_c: bool) {
        self.is_repr_c = is_repr_c;
    }

    pub fn add_field(&mut self, name: String, t: Type) {
        for field in &self.fields {
            if field.0 == name {
                panic!("Field already exists: {}", name);
            }
        }

        self.fields.push((name, t));
    }
}

impl<'m> RCL<'m> {
    pub fn new() -> Self {
        RCL {
            functions: Vec::new(),
            structs: Vec::new(),
            main_functions: Vec::new(),
            import_statements: Vec::new(),
            constants: Vec::new(),
            impls: Vec::new(),
        }
    }

    pub fn add_import(&mut self, import: String) {
        for existing_import in &self.import_statements {
            if existing_import == &import {
                panic!("Import already exists: {}", import);
            }
        }
        self.import_statements.push(import);
    }
}
