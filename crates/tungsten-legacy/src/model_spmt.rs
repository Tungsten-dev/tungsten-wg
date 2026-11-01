/*
SPMT (Single Program Multi Target) is a model for representing density functions in a way that can be easily evaluated
on both CPU and GPU.
The idea is to have a single representation of the density function that can be compiled to both CPU and GPU code,
without having to write separate code for each target.

*/

use std::{
    fmt::Debug,
    hash::{Hash, Hasher},
    rc::Rc,
};

#[derive(Debug, Clone)]
pub struct MainDensityFunction<'m> {
    pub density_function: DensityFunctionRef<'m>,
    pub dimensions: (i32, i32, i32),
    pub scaled_origin: (f64, f64, f64),
    pub scaled_position: (f64, f64, f64),
}

pub struct SPMT<'m> {
    pub density_functions: Vec<DensityFunctionRef<'m>>,
    pub functions: Vec<FunctionRef<'m>>,
    pub main_density_functions: Vec<MainDensityFunction<'m>>,
}

pub type Var<'m> = Interned<'m, Variable>;

/// A density function is a function that takes in x, y, z coordinates and returns a density value.
/// its signature is at least f(position: Vec3) -> f32
/// However, it can have other inputs, such as inputs of other density functions
#[derive(Debug, Clone)]
pub struct DensityFunction<'m> {
    pub canonical_name: Option<String>,
    pub density_inputs: Vec<DensityInput<'m>>,
    pub permutation_table_inputs: Vec<PermutationTableInput>,
    pub host_inputs: Vec<HostInput<'m>>,
    pub body: Vec<Statement<'m>>,
    pub variables: Vec<Var<'m>>,
    pub helper_functions: Vec<FunctionRef<'m>>,
    pub constants: Vec<(Var<'m>, Expression<'m>)>,

    /// An identifier for the source of this density function, used to detect changes and cache results.
    pub source_hash: u64,
}

#[derive(Debug, Clone)]
pub struct Function<'m> {
    pub canonical_name: Option<String>,
    pub parameters: Vec<Var<'m>>,
    pub body: Vec<Statement<'m>>,
    pub variables: Vec<Var<'m>>,
    pub return_type: VariableType,
}

pub type DensityFunctionRef<'m> = Interned<'m, DensityFunction<'m>>;
pub type FunctionRef<'m> = Interned<'m, Function<'m>>;

#[derive(PartialEq, Debug, Clone)]
pub struct DensityInput<'m> {
    pub var: Var<'m>,
    pub density_function: DensityFunctionRef<'m>,
    pub scaled_origin: (f64, f64, f64),
    pub scaled_position: (f64, f64, f64),
    pub dimensions: (i32, i32, i32),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PermutationTableInput {
    PerlinNoise {
        ident: String,
        subident: Option<String>,
        subident_index: usize,
    },
    Base3DNoise,
}

/// Input from the host environment, either statically computed, or dynamically computed at runtime.
#[derive(PartialEq, Debug, Clone)]
pub enum HostInput<'m> {
    /// A statically computed input from the host environment.
    /// Asked when starting the orchestration.
    Static(Var<'m>),
    /// A dynamically computed input from the host environment.
    /// Asked during the orchestration as needed. This can pose performance benefits
    /// as it computes while the GPU is busy.
    /// Currently unsupported
    Dynamic(Var<'m>),
}

impl HostInput<'_> {
    pub fn get_name(&self) -> String {
        let name  = match self {
            HostInput::Static(var) => var.name.clone(),
            HostInput::Dynamic(var) => var.name.clone(),
        };
        let Name::Named(name) = name else {
            panic!("Expected a named variable");
        };
        name
    }

    pub fn get_type(&self) -> VariableType {
        match self {
            HostInput::Static(var) => var.t.clone(),
            HostInput::Dynamic(var) => var.t.clone(),
        }
    }
    
}

impl Ord for PermutationTableInput {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use PermutationTableInput::*;
        match (self, other) {
            (
                PerlinNoise {
                    ident: i1,
                    subident: s1,
                    subident_index: idx1,
                },
                PerlinNoise {
                    ident: i2,
                    subident: s2,
                    subident_index: idx2,
                },
            ) => i1
                .cmp(i2)
                .then_with(|| s1.cmp(s2))
                .then_with(|| idx1.cmp(idx2)),
            (PerlinNoise { .. }, Base3DNoise) => std::cmp::Ordering::Less,
            (Base3DNoise, PerlinNoise { .. }) => std::cmp::Ordering::Greater,
            (Base3DNoise, Base3DNoise) => std::cmp::Ordering::Equal,
        }
    }
}

impl PartialOrd for PermutationTableInput {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: Name,
    pub t: VariableType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Name {
    Anonymous,        // for variables that don't have a name (e.g. temporary variables)
    Prefixed(String), // for variables that have a name, but it's not guaranteed to be unique
    Named(String),    // for variables that have a unique name
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use Name::*;
        match (self, other) {
            (Anonymous, Anonymous) => std::cmp::Ordering::Equal,
            (Anonymous, _) => std::cmp::Ordering::Less,
            (_, Anonymous) => std::cmp::Ordering::Greater,
            (Prefixed(s1), Prefixed(s2)) => s1.cmp(s2),
            (Prefixed(_), Named(_)) => std::cmp::Ordering::Less,
            (Named(_), Prefixed(_)) => std::cmp::Ordering::Greater,
            (Named(s1), Named(s2)) => s1.cmp(s2),
        }
    }
}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum VariableType {
    DensityInput,
    PermutationTable,
    InterpolatedNoiseSampler,
    Vec3,
    Pos3,
    F32,
    F64,
    I32,
    I64,
    Bool,
    Array(Box<VariableType>, usize), // For array types, we can specify the element type and size
    Extern(&'static str), // For external functions, we can use the name of the function as the type
}

impl Debug for VariableType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VariableType::DensityInput => write!(f, "density"),
            VariableType::PermutationTable => write!(f, "perm_table"),
            VariableType::InterpolatedNoiseSampler => write!(f, "interpolated_noise_sampler"),
            VariableType::Vec3 => write!(f, "vec3"),
            VariableType::Pos3 => write!(f, "pos3"),
            VariableType::F32 => write!(f, "f32"),
            VariableType::F64 => write!(f, "f64"),
            VariableType::I32 => write!(f, "i32"),
            VariableType::I64 => write!(f, "i64"),
            VariableType::Extern(name) => write!(f, "{}", name),
            VariableType::Array(element_type, size) => {
                write!(f, "array[{:?}; {}]", element_type, size)
            }
            VariableType::Bool => write!(f, "bool"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement<'m> {
    Assign {
        target: Var<'m>,
        value: Expression<'m>,
    },
    Return(Expression<'m>),
    If {
        condition: Expression<'m>,
        then_branch: Vec<Statement<'m>>,
        else_branch: Vec<Statement<'m>>,
    },
    While {
        condition: Expression<'m>,
        body: Vec<Statement<'m>>,
    },
    Repeat {
        count: usize,
        body: Vec<Statement<'m>>,
    },
    Break,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expression<'m> {
    /// A variable reference
    Variable(Var<'m>),
    /// A literal value (e.g. number)
    Float(f32),
    Double(f64),
    Int(i32),
    Long(i64),
    Bool(bool),
    /// A Function call: function(parameters...)
    FunctionCall {
        function: FunctionRef<'m>,
        parameters: Vec<Expression<'m>>,
    },
    /// A Named function call: function_name(parameters...)
    /// Useful for calling helper functions such as math functions (e.g. sin, cos, etc.)
    ExternCall {
        function_name: String,
        parameters: Vec<Expression<'m>>,
        parameter_types: Vec<VariableType>,
        return_type: VariableType,
    },
    /// A 'call' to another density function, with the given parameters.
    /// This is used to call other density functions from within a density function.
    /// Optionally, the caller can pass in an index for the called density for reading.
    DensityVariable(DensityInput<'m>, Option<Box<Expression<'m>>>),

    // similar to density variable but for permutation tables,
    // this is used to reference the permutation tables that are passed as arguments to noise functions
    PermutationTable(PermutationTableInput),

    BinaryOp {
        op: BinaryOperator,
        left: Box<Expression<'m>>,
        right: Box<Expression<'m>>,
    },

    UnaryOp {
        op: UnaryOperator,
        operand: Box<Expression<'m>>,
    },

    Field {
        base: Box<Expression<'m>>,
        field: String,
        type_of_field: VariableType,
        known_idnex: Option<usize>, // for cases where we know the index of the field (e.g. for vec3.x, vec3.y, vec3.z)
    },

    ArrayAccess {
        array: Box<Expression<'m>>,
        index: Box<Expression<'m>>,
    },

    // MakeVec3 {
    //     x: Box<Expression<'m>>,
    //     y: Box<Expression<'m>>,
    //     z: Box<Expression<'m>>,
    // },
    Construct {
        t: VariableType,
        args: Vec<Expression<'m>>,
    },

    ConstructExtern {
        t: VariableType,
        args: Vec<(&'static str, Expression<'m>)>,
    },

    ArrayLiteral(Vec<Expression<'m>>),
    ExplicitCast {
        to: VariableType,
        expr: Box<Expression<'m>>,
    },
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
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOperator {
    Negate,
    Not,
}

pub struct Interned<'m, T: ?Sized>(&'m T);

impl<'m, T> PartialEq for Interned<'m, T> {
    fn eq(&self, other: &Self) -> bool {
        // pointer equality is sufficient since we intern all functions
        std::ptr::eq(self.0, other.0)
    }
}

impl<'m, T> Eq for Interned<'m, T> {}

impl<'m, T> Hash for Interned<'m, T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // pointer hash is sufficient since we intern all functions
        std::ptr::hash(self.0, state);
    }
}

impl<'m, T> std::ops::Deref for Interned<'m, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<'m, T> Debug for Interned<'m, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Interned({:p})", self.0)
    }
}

impl<'m, T> Clone for Interned<'m, T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<'m, T> Copy for Interned<'m, T> {}

impl<'m, T> Interned<'m, T> {
    pub fn new(value: &'m T) -> Self {
        Interned(value)
    }
}

impl<'m> Function<'m> {
    pub fn add_statement(&mut self, statement: Statement<'m>) {
        self.body.push(statement);
    }

    pub fn add_variable(&mut self, variable: Var<'m>) {
        self.variables.push(variable);
    }
}

pub trait Addr {
    fn addr(&self) -> *const ();
}

impl<T> Addr for &T {
    fn addr(&self) -> *const () {
        *self as *const T as *const ()
    }
}

impl<T> Addr for std::rc::Rc<T> {
    fn addr(&self) -> *const () {
        let ptr: *const T = Rc::as_ptr(self);
        ptr as *const ()
    }
}

impl<T> Addr for Interned<'_, T> {
    fn addr(&self) -> *const () {
        self.0 as *const T as *const ()
    }
}

impl<'m> DensityFunction<'m> {
    pub fn add_statement(&mut self, statement: Statement<'m>) {
        self.body.push(statement);
    }

    pub fn add_variable(&mut self, variable: Var<'m>) {
        for var in &self.variables {
            if var.name == variable.name {
                panic!("Variable with the same name already exists");
            }
        }
        self.variables.push(variable);
    }

    pub fn add_density_input(&mut self, dependency: DensityInput<'m>) {
        for input in &self.density_inputs {
            if input.density_function == dependency.density_function {
                return;
            }
        }
        self.density_inputs.push(dependency);
    }
}
