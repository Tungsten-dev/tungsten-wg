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

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum VariableType {
    /// An opaque runtime type, built once by the runtime and only passed to externs.
    Handle(&'static str),
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
            VariableType::Handle(name) => write!(f, "handle {}", name),
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

#[derive(Debug, Clone)]
pub enum ComputeUnit {
    DensityFunction {
        function: Ref<Function>,
        /// The program inputs this unit lists, in build order.
        density_inputs: Vec<Ref<DensityInput>>,
        /// The autoconsts this unit lists, in first-use order.
        auto_consts: Vec<Ref<AutoConst>>,
        host_inputs: Vec<Ref<HostInput>>,
        helper_functions: Vec<Ref<Function>>,
        constants: Vec<Ref<Constant>>,

        /// An identifier for the source of this density function, used to detect changes and cache results.
        source_hash: u64,
    },
}

/// How one axis of an un-indexed read maps the reader's coordinate to the input's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AxisFactor {
    /// The input has size 1 on this axis. Every reader coordinate reads coordinate 0.
    Repeat,
    /// Each input coordinate covers `n` reader coordinates. `n >= 1`.
    Factor(i32),
}

/// How an un-indexed read maps the reader's cell to the input's buffer. A read with an explicit
/// index ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DensityMapping {
    #[default]
    Identity,
    Expand([AxisFactor; 3]),
    Lerp([AxisFactor; 3]),
}

impl DensityMapping {
    /// Whether a read through this mapping, on an input of `input_dims`, reads the reader's own
    /// cell. `Repeat` only counts on an axis of size 1, the only place it is valid.
    pub fn is_identity_on(&self, input_dims: (i32, i32, i32)) -> bool {
        let factors = match self {
            DensityMapping::Identity => return true,
            DensityMapping::Expand(f) | DensityMapping::Lerp(f) => f,
        };
        let sizes = [input_dims.0, input_dims.1, input_dims.2];
        factors.iter().zip(sizes).all(|(f, size)| match f {
            AxisFactor::Repeat => size == 1,
            AxisFactor::Factor(n) => *n == 1,
        })
    }
}

#[derive(PartialEq, Debug, Clone)]
pub struct DensityInput {
    pub name: Name,
    pub density_function: Ref<ComputeUnit>,
    pub scaled_origin: (f64, f64, f64),
    pub scaled_position: (f64, f64, f64),
    pub dimensions: (i32, i32, i32),
    pub mapping: DensityMapping,
    /// Reads this instance's output from before the current call. Only refined units may list it.
    pub previous: bool,
}

/// A second generated call on an entry's worker that reruns part of its closure, with `Pos3`
/// moved by an offset. Everything not listed in `reruns` stays frozen.
#[derive(Debug, Clone, PartialEq)]
pub struct Refinement {
    pub name: String,
    /// Index into `entry_points`.
    pub entry: usize,
    /// Largest allowed offset per axis, each >= 0. An axis with 0 doesn't march.
    pub march: (i32, i32, i32),
    pub reruns: Vec<Rerun>,
}

/// One instance a refinement reruns.
#[derive(Debug, Clone, PartialEq)]
pub struct Rerun {
    /// The buffer written, identified as an instance: unit, dims and scales.
    pub cu: Ref<ComputeUnit>,
    pub dimensions: (i32, i32, i32),
    pub scaled_origin: (f64, f64, f64),
    pub scaled_position: (f64, f64, f64),
    /// The refined unit that runs instead of `cu`. `None` reruns `cu`.
    pub with: Option<Ref<ComputeUnit>>,
}

/// A value supplied once, when the orchestration is created. Programs can't read it directly;
/// only autoconst initializers take it as an argument.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AutoParam {
    pub name: String,
    /// `I64`, `F64` or `Bool`.
    pub t: VariableType,
}

/// A value built once, when the orchestration is created, by calling the runtime function
/// `initializer` with `args`. The IR doesn't know what the initializer computes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AutoConst {
    /// Field-name hint and display name.
    pub name: String,
    /// `Handle` or a scalar.
    pub t: VariableType,
    /// Bare name of a runtime function.
    pub initializer: String,
    pub args: Vec<AutoConstArg>,
}

/// The type of an initializer argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutoArgType {
    I64,
    F64,
    Bool,
    Str,
}

impl std::fmt::Display for AutoArgType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AutoArgType::I64 => "i64",
            AutoArgType::F64 => "f64",
            AutoArgType::Bool => "bool",
            AutoArgType::Str => "str",
        })
    }
}

/// An argument of an initializer. `F64` compares and hashes by bit pattern, so `0.0` and `-0.0`
/// are different arguments and NaN equals itself.
#[derive(Debug, Clone)]
pub enum AutoConstArg {
    Param(Ref<AutoParam>),
    I64(i64),
    F64(f64),
    Bool(bool),
    Str(String),
}

impl PartialEq for AutoConstArg {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Param(a), Self::Param(b)) => a == b,
            (Self::I64(a), Self::I64(b)) => a == b,
            (Self::F64(a), Self::F64(b)) => a.to_bits() == b.to_bits(),
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Str(a), Self::Str(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for AutoConstArg {}

impl std::hash::Hash for AutoConstArg {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Param(r) => r.hash(state),
            Self::I64(v) => v.hash(state),
            Self::F64(v) => v.to_bits().hash(state),
            Self::Bool(v) => v.hash(state),
            Self::Str(v) => v.hash(state),
        }
    }
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

/// `Clone` is fully independent: every arena (including the ones nested in functions) gets a
/// fresh tag, and every embedded `Ref` is rewritten to it.
///
/// Any new `Ref`-holding field anywhere in the IR must be added to the walker in `retag.rs`.
pub struct OptimizedProgram {
    pub compute_units: Arena<ComputeUnit>,
    pub functions: Arena<Function>,
    pub entry_points: Vec<EntryPoint>,
    pub constants: Arena<Constant>,
    pub host_inputs: Arena<HostInput>,
    /// Creation-time inputs, in declaration order. The creation API takes one argument per param.
    pub auto_params: Arena<AutoParam>,
    /// Values built at creation time, in creation order.
    pub auto_consts: Arena<AutoConst>,
    pub density_inputs: Arena<DensityInput>,
    pub refinements: Vec<Refinement>,
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

    /// A value built at creation time (see [`AutoConst`]).
    AutoConst(Ref<AutoConst>),
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
