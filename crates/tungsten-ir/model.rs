use std::{
    fmt::Debug,
    hash::Hash,
    marker::PhantomData,
    sync::atomic::{AtomicU32, Ordering},
};

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
    I8,
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
            VariableType::I8 => write!(f, "i8"),
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

static NEXT_TAG: AtomicU32 = AtomicU32::new(1);

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct OwnerTag(u32);
impl OwnerTag {
    pub fn fresh() -> Self {
        Self(NEXT_TAG.fetch_add(1, Ordering::Relaxed))
    }
}

pub struct Ref<T> {
    pub(crate) tag: OwnerTag,
    idx: u32,
    generation: u32,
    _m: PhantomData<fn() -> T>,
}

impl<T> PartialEq for Ref<T> {
    fn eq(&self, other: &Self) -> bool {
        self.tag == other.tag && self.idx == other.idx && self.generation == other.generation
    }
}

impl<T> Eq for Ref<T> {}

impl<T> Clone for Ref<T> {
    fn clone(&self) -> Self {
        Self {
            tag: self.tag,
            idx: self.idx,
            generation: self.generation,
            _m: PhantomData,
        }
    }
}

impl<T> Copy for Ref<T> {}

impl<T> Hash for Ref<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.tag.hash(state);
        self.idx.hash(state);
        self.generation.hash(state);
    }
}

impl<T> std::fmt::Debug for Ref<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ref").field("idx", &self.idx).finish()
    }
}

struct Slot<T> {
    generation: u32,
    value: Option<T>,
}
impl<T: Debug> Debug for Slot<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value.fmt(f)
    }
}

pub struct Arena<T> {
    tag: OwnerTag,
    /// Tag of the arena this one was (transitively) cloned from; its own tag if it is an original.
    origin: OwnerTag,
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> Arena<T> {
    pub fn new() -> Self {
        let tag = OwnerTag::fresh();
        Self {
            tag,
            origin: tag,
            slots: vec![],
            free: vec![],
        }
    }

    pub fn tag(&self) -> OwnerTag {
        self.tag
    }

    /// The same slot as `r` (a ref into `from`), but as a ref into `self`.
    ///
    /// Panics if `r` doesn't belong to `from`, or if `from` and `self` aren't clones of the same
    /// arena. The result may be stale in `self` (e.g. the slot was removed); check with
    /// `try_get` or `contains`.
    pub fn translate(&self, r: Ref<T>, from: &Arena<T>) -> Ref<T> {
        assert_eq!(r.tag, from.tag, "Ref used on the wrong arena");
        assert_eq!(
            from.origin, self.origin,
            "translate between arenas that are not clones of each other"
        );
        Ref { tag: self.tag, ..r }
    }

    /// Clones with a fresh tag (same origin), cloning every value with `f`.
    /// Idx and generation of every slot are preserved.
    pub(crate) fn clone_with(&self, mut f: impl FnMut(&T) -> T) -> Self {
        Self {
            tag: OwnerTag::fresh(),
            origin: self.origin,
            slots: self
                .slots
                .iter()
                .map(|s| Slot {
                    generation: s.generation,
                    value: s.value.as_ref().map(&mut f),
                })
                .collect(),
            free: self.free.clone(),
        }
    }

    pub fn insert(&mut self, v: T) -> Ref<T> {
        let (idx, generation) = if let Some(i) = self.free.pop() {
            let s = &mut self.slots[i as usize];
            s.value = Some(v);
            (i, s.generation)
        } else {
            self.slots.push(Slot {
                generation: 0,
                value: Some(v),
            });
            (self.slots.len() as u32 - 1, 0)
        };
        Ref {
            tag: self.tag,
            idx,
            generation,
            _m: PhantomData,
        }
    }

    pub fn try_get(&self, r: Ref<T>) -> Option<&T> {
        assert_eq!(r.tag, self.tag, "Ref used on the wrong arena"); // misuse: always panic
        let s = self.slots.get(r.idx as usize)?;
        if s.generation == r.generation {
            s.value.as_ref()
        } else {
            None
        } // stale: recoverable
    }
    pub fn get(&self, r: Ref<T>) -> &T {
        self.try_get(r).expect("stale Ref (slot was removed)")
    }

    pub fn try_get_mut(&mut self, r: Ref<T>) -> Option<&mut T> {
        assert_eq!(r.tag, self.tag, "Ref used on the wrong arena");
        let s = self.slots.get_mut(r.idx as usize)?;
        if s.generation == r.generation {
            s.value.as_mut()
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, r: Ref<T>) -> &mut T {
        self.try_get_mut(r).expect("stale Ref (slot was removed)")
    }

    pub fn remove(&mut self, r: Ref<T>) -> Option<T> {
        assert_eq!(r.tag, self.tag, "Ref used on the wrong arena");
        let s = self.slots.get_mut(r.idx as usize)?;
        if s.generation != r.generation {
            return None;
        }
        let v = s.value.take()?;
        s.generation = s.generation.wrapping_add(1);
        self.free.push(r.idx);
        Some(v)
    }

    pub fn contains(&self, r: Ref<T>) -> bool {
        r.tag == self.tag
            && self
                .slots
                .get(r.idx as usize)
                .is_some_and(|s| s.generation == r.generation && s.value.is_some())
    }

    pub fn iter(&self) -> impl Iterator<Item = (Ref<T>, &T)> {
        let tag = self.tag;
        self.slots.iter().enumerate().filter_map(move |(i, s)| {
            s.value.as_ref().map(|v| {
                (
                    Ref {
                        tag,
                        idx: i as u32,
                        generation: s.generation,
                        _m: PhantomData,
                    },
                    v,
                )
            })
        })
    }
}

/// A clone with a fresh tag. Refs stored *inside* the values are not rewritten; the
/// `Clone` impls of the IR types that hold arenas take care of that.
impl<T: Clone> Clone for Arena<T> {
    fn clone(&self) -> Self {
        self.clone_with(T::clone)
    }
}

impl<T> Arena<T> {
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.slots.iter().filter_map(|s| s.value.as_ref())
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.slots.iter_mut().filter_map(|s| s.value.as_mut())
    }
}

impl<T> std::ops::Index<Ref<T>> for Arena<T> {
    type Output = T;

    fn index(&self, r: Ref<T>) -> &Self::Output {
        self.get(r)
    }
}
impl<T> std::ops::IndexMut<Ref<T>> for Arena<T> {
    fn index_mut(&mut self, r: Ref<T>) -> &mut Self::Output {
        self.get_mut(r)
    }
}

impl<T: Debug> Debug for Arena<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Arena")
            .field("slots", &self.slots)
            .field("free", &self.free)
            .finish()
    }
}

impl<T> PartialEq for Arena<T> {
    fn eq(&self, _other: &Self) -> bool {
        false // Arenas are never considered equal, even if their contents are the same.
    }
}

impl<T> Eq for Arena<T> {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Copy)]
pub struct Dimensions {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Extends the X dimension to provide concurrency over lanes (warps)
    pub lane: i32,
    /// Extends the Y dimension to provide concurrency over the block (thread blocks)
    pub block: i32,
}

impl Dimensions {
    pub fn new(x: i32, y: i32, z: i32, lane: i32, block: i32) -> Self {
        Self {
            x,
            y,
            z,
            lane,
            block,
        }
    }

    pub fn from_tuple(dimensions: (i32, i32, i32)) -> Self {
        Self {
            x: dimensions.0,
            y: dimensions.1,
            z: dimensions.2,
            lane: 1,
            block: 1,
        }
    }
}

#[derive(PartialEq, Debug, Clone)]
pub struct EntryPoint {
    pub density_function: Ref<ComputeUnit>,
    pub dimensions: (i32, i32, i32),
    pub scaled_origin: (f64, f64, f64),
    pub scaled_position: (f64, f64, f64),
}

/// `Clone` gives `density_inputs` a fresh tag and rewrites the refs into it (see `retag`).
#[derive(Debug)]
pub enum ComputeUnit {
    DensityFunction {
        function: Ref<Function>,
        density_inputs: Arena<DensityInput>,
        permutation_table_inputs: Vec<Ref<PermutationTableInput>>,
        host_inputs: Vec<Ref<HostInput>>,
        helper_functions: Vec<Ref<Function>>,
        constants: Vec<Ref<Constant>>,

        /// An identifier for the source of this density function, used to detect changes and cache results.
        source_hash: u64,
    },
}
#[derive(PartialEq, Debug, Clone)]
pub struct DensityInput {
    pub var: Ref<Var>,
    pub density_function: Ref<ComputeUnit>,
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
pub enum HostInput {
    /// A statically computed input from the host environment.
    /// Asked when starting the orchestration.
    Static { name: Name, t: VariableType },
    /// A dynamically computed input from the host environment.
    /// Asked during the orchestration as needed. This can pose performance benefits
    /// as it computes while the GPU is busy.
    /// Currently unsupported
    Dynamic,
}

impl HostInput {
    pub fn get_name(&self) -> String {
        let name = match self {
            HostInput::Static { name, .. } => name.clone(),
            HostInput::Dynamic => panic!("Dynamic host input does not have a name"),
        };
        let Name::Named(name) = name else {
            panic!("Expected a named variable");
        };
        name
    }

    pub fn get_type(&self) -> VariableType {
        match self {
            HostInput::Static { t, .. } => t.clone(),
            HostInput::Dynamic => panic!("Dynamic host input does not have a type"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var {
    pub name: Name,
    pub t: VariableType,
}

/// `Clone` gives `variables` a fresh tag and rewrites the refs into it (see `retag`).
#[derive(Debug)]
pub struct Function {
    pub canonical_name: Option<String>,
    // density_inputs: Vec<DensityInput>,
    // permutation_table_inputs: Vec<PermutationTableInput>,
    pub host_inputs: Vec<Ref<HostInput>>,
    pub body: Vec<Statement>,
    pub variables: Arena<Var>,
    pub parameters: Vec<Ref<Var>>,
    pub return_type: VariableType,
    // helper_functions: Vec<Ref<Function>>,
    // constants: Vec<Ref<Constant>>,
    /// An identifier for the source of this density function, used to detect changes and cache results.
    pub source_hash: u64,
}

/// `Clone` is fully independent: every arena (including the ones nested in compute units and
/// functions) gets a fresh tag, and every embedded `Ref` is rewritten to it.
///
/// Any new `Ref`-holding field anywhere in the IR must be added to the walker in `retag.rs`.
pub struct OptimizedProgram {
    pub compute_units: Arena<ComputeUnit>,
    pub functions: Arena<Function>,
    pub entry_points: Vec<EntryPoint>,
    pub constants: Arena<Constant>,
    pub host_inputs: Arena<HostInput>,
    pub permutation_tables: Arena<PermutationTableInput>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Assign {
        target: Ref<Var>,
        value: Expression,
    },
    Return(Expression),
    If {
        condition: Expression,
        then_branch: Vec<Statement>,
        else_branch: Vec<Statement>,
    },
    While {
        condition: Expression,
        body: Vec<Statement>,
    },
    Repeat {
        count: usize,
        body: Vec<Statement>,
    },
    Break,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    /// A constant reference
    Constant(Ref<Constant>),
    /// A variable reference
    Variable(Ref<Var>),
    /// A literal value (e.g. number)
    Float(f32),
    Double(f64),
    Byte(i8),
    Int(i32),
    Long(i64),
    Bool(bool),
    /// A Function call: function(parameters...)
    FunctionCall {
        function: Ref<Function>,
        parameters: Vec<Expression>,
    },
    /// A Named function call: function_name(parameters...)
    /// Useful for calling helper functions such as math functions (e.g. sin, cos, etc.)
    ExternCall {
        function_name: String,
        parameters: Vec<Expression>,
        parameter_types: Vec<VariableType>,
        return_type: VariableType,
    },
    /// A 'call' to another density function, with the given parameters.
    /// This is used to call other density functions from within a density function.
    /// Optionally, the caller can pass in an index for the called density for reading.
    DensityVariable(Ref<DensityInput>, Option<Box<Expression>>),

    // similar to density variable but for permutation tables,
    // this is used to reference the permutation tables that are passed as arguments to noise functions
    PermutationTable(Ref<PermutationTableInput>),
    HostInput(Ref<HostInput>),
    /// A read-only value supplied by the evaluation context (origin, position, scales).
    Ambient(AmbientKind),

    BinaryOp {
        op: BinaryOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },

    UnaryOp {
        op: UnaryOperator,
        operand: Box<Expression>,
    },

    Field {
        base: Box<Expression>,
        field: String,
        type_of_field: VariableType,
        /// Type of `base`; used to resolve `field`.
        base_type: VariableType,
    },

    ArrayAccess {
        array: Box<Expression>,
        index: Box<Expression>,
    },

    // MakeVec3 {
    //     x: Box<Expression>,
    //     y: Box<Expression>,
    //     z: Box<Expression>,
    // },
    Construct {
        t: VariableType,
        args: Vec<Expression>,
    },

    ConstructExtern {
        t: VariableType,
        args: Vec<(&'static str, Expression)>,
    },

    ArrayLiteral(Vec<Expression>),
    ExplicitCast {
        to: VariableType,
        expr: Box<Expression>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AmbientKind {
    Origin,
    Pos3,
    OriginScale,
    PositionScale,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstExpr {
    Constant(Ref<Constant>),
    Float(f32),
    Double(f64),
    Byte(i8),
    Int(i32),
    Long(i64),
    Bool(bool),

    BinaryOp {
        op: BinaryOperator,
        left: Box<ConstExpr>,
        right: Box<ConstExpr>,
    },

    UnaryOp {
        op: UnaryOperator,
        operand: Box<ConstExpr>,
    },

    ExplicitCast {
        to: VariableType,
        expr: Box<ConstExpr>,
    },

    Field {
        base: Box<ConstExpr>,
        field: String,
        type_of_field: VariableType,
        /// Type of `base`; used to resolve `field`.
        base_type: VariableType,
    },

    ArrayAccess {
        array: Box<ConstExpr>,
        index: Box<ConstExpr>,
    },

    ArrayLiteral(Vec<ConstExpr>),

    Construct {
        t: VariableType,
        args: Vec<ConstExpr>,
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
    ShiftLeft,
    ShiftRight,
    /// Unsigned (logical) right shift: `>>>` in Java. Last, so existing discriminants (and source hashes) stay stable.
    LogicalShiftRight,
    BitOr,
    BitAnd,
    BitXor,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOperator {
    Negate,
    Not,
    Ref(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Constant {
    pub value: ConstExpr,
    pub name: String,
}
