//! Lowering errors and their locations.

use std::fmt;

use tungsten_core::{ConstId, FnId, HostId, InputId, TableId, UnitId};

/// All errors of one [`lower`](crate::lower) call, in walk order. Never empty.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct LowerErrors(pub Vec<LowerError>);

impl fmt::Display for LowerErrors {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LowerError {
    pub kind: LowerErrorKind,
    pub at: Location,
}

impl fmt::Display for LowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.kind)
    }
}

/// The kind of entity a dangling ref points to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    ComputeUnit,
    Function,
    Constant,
    HostInput,
    AutoConst,
    AutoParam,
    DensityInput,
    Var,
}

impl EntityKind {
    fn article(self) -> &'static str {
        match self {
            EntityKind::AutoConst | EntityKind::AutoParam => "an",
            _ => "a",
        }
    }
}

impl fmt::Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            EntityKind::ComputeUnit => "compute unit",
            EntityKind::Function => "function",
            EntityKind::Constant => "constant",
            EntityKind::HostInput => "host input",
            EntityKind::AutoConst => "autoconst",
            EntityKind::AutoParam => "auto param",
            EntityKind::DensityInput => "density input",
            EntityKind::Var => "variable",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterProblem {
    /// The same var is listed twice in `parameters`.
    Duplicate,
    /// The var isn't in the function's own `variables` arena.
    NotInArena,
}

impl fmt::Display for ParameterProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ParameterProblem::Duplicate => "listed twice",
            ParameterProblem::NotInArena => "not in the function's variables",
        })
    }
}

/// Only what the core IR can't represent (see README.md). Type errors and the like are kept for
/// the checker.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LowerErrorKind {
    #[error("dangling reference to {} {kind}", kind.article())]
    DanglingRef { kind: EntityKind },
    #[error("unknown extern function `{0}`")]
    UnknownExtern(String),
    #[error("unknown record type `{0}`")]
    UnknownRecord(String),
    #[error("no field `{field}` on `{base}`")]
    UnknownField { base: String, field: String },
    #[error("field access on `{base}`, which is not vec3, pos3 or a record")]
    FieldOnNonStruct { base: String },
    #[error("dynamic host inputs are not supported")]
    DynamicHost,
    #[error("can't construct `{t}`")]
    BadConstruct { t: String },
    #[error("construct expects {expected} arguments, found {found}")]
    ConstructArity { expected: usize, found: usize },
    #[error("parameter {problem}")]
    Parameter { problem: ParameterProblem },
    #[error("auto param type `{t}` isn't i64, f64 or bool")]
    BadAutoParamType { t: String },
    #[error("auto param `{0}` is declared twice")]
    DuplicateAutoParam(String),
    #[error("autoconst type `{t}` isn't a handle or one of bool, i8, i32, i64, f32, f64")]
    BadAutoConstType { t: String },
    #[error("host type `{t}` is a handle type")]
    HandleHost { t: String },
    #[error("unknown auto initializer `{0}`")]
    UnknownAutoInitializer(String),
    #[error("auto initializer `{initializer}` takes {expected} arguments, found {found}")]
    AutoArgCount { initializer: String, expected: usize, found: usize },
    #[error("argument {index} of auto initializer `{initializer}` is `{expected}`, found `{found}`")]
    AutoArgType { index: usize, initializer: String, expected: String, found: String },
    #[error("auto initializer `{initializer}` returns `{returns}`, but the autoconst is `{declared}`")]
    AutoInitializerReturn { initializer: String, returns: String, declared: String },
    #[error("string argument {index} contains a NUL character")]
    AutoArgHasNul { index: usize },
    #[error("mapping factor {factor} on axis {axis} is below 1")]
    MappingBadFactor { axis: char, factor: i32 },
    #[error("mapping repeats axis {axis}, but the input is {size} wide on it")]
    MappingRepeatOnWideAxis { axis: char, size: i32 },
    #[error("`@lerp` needs an `f32` or `f64` input, but its unit returns `{t}`")]
    MappingNotFloat { t: String },
    #[error("unit `{unit}` reads it un-indexed at {reader} on axis {axis}, but it is {input} on that axis")]
    MappingDimsMismatch { unit: String, axis: char, input: i32, reader: i32 },
    #[error(
        "unit `{unit}` reads it at {reader} on axis {axis} (march {march}), past the input's {input} cells on that axis"
    )]
    MappingOutOfRange { unit: String, axis: char, input: i32, reader: i32, march: i32 },
    #[error("entry {entry} doesn't exist")]
    RefineBadEntry { entry: usize },
    #[error("entry {entry} already has a refinement `{name}`")]
    RefineDuplicateName { entry: usize, name: String },
    #[error("march {march} on axis {axis} is negative")]
    RefineBadMarch { axis: char, march: i32 },
    #[error("the entry's own instance isn't rerun")]
    RefineEntryNotRerun,
    #[error("rerun {index} lists the same instance as rerun {first}")]
    RefineDuplicateRerun { index: usize, first: usize },
    #[error("rerun {index} (`{unit}`) isn't reached from the entry through reruns")]
    RefineRerunUnreachable { index: usize, unit: String },
    #[error("rerun {index}: `{with}` returns `{found}`, but its target `{unit}` returns `{expected}`")]
    RefineTypeMismatch { index: usize, unit: String, with: String, expected: String, found: String },
    #[error("rerun {index} (`{unit}`) has a different position scale on marched axis {axis}")]
    RefineMarchScaleMismatch { index: usize, unit: String, axis: char },
    #[error("the instances that run form a cycle through `{unit}`")]
    RefineCycle { unit: String },
    #[error("refined unit `{unit}` is also {why}")]
    RefinedUnitInNormalGraph { unit: String, why: String },
    #[error("`@previous` input listed by `{unit}`, which no refinement runs as a `with` unit")]
    PreviousOutsideRefinedUnit { unit: String },
    #[error("`@previous` input listed by `{with}`, which refinement `{refinement}` runs for a different instance")]
    PreviousNotTarget { with: String, refinement: String },
    #[error("a `@previous` input must have no mapping")]
    PreviousMapped,
    #[error("`{unit}` reads this `@previous` input with an explicit index")]
    PreviousIndexed { unit: String },
}

/// Where an error is: an item and a path inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub item: Item,
    pub path: Vec<Step>,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.item)?;
        for s in &self.path {
            write!(f, " › {s}")?;
        }
        Ok(())
    }
}

/// Names come from `canonical_name`, the `Name`, or `#<id>` when there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Function { id: Option<FnId>, name: String },
    Constant { id: Option<ConstId>, name: String },
    Unit { id: Option<UnitId>, name: String },
    Host { id: Option<HostId>, name: String },
    Input { id: Option<InputId>, name: String },
    AutoConst { id: Option<TableId>, name: String },
    AutoParam { name: String },
    Entry { index: usize },
    /// `program.refinements[index]`.
    Refinement { index: usize, name: String },
    /// Before the walk enters an item. No lowering error is reported here; checker wf errors
    /// in extern and record decls are.
    Registry,
    /// The whole program (dependency cycles).
    Program,
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Item::Function { name, .. } => write!(f, "fn `{name}`"),
            Item::Constant { name, .. } => write!(f, "const `{name}`"),
            Item::Unit { name, .. } => write!(f, "unit `{name}`"),
            Item::Host { name, .. } => write!(f, "host `{name}`"),
            Item::Input { name, .. } => write!(f, "input `{name}`"),
            Item::AutoConst { name, .. } => write!(f, "autoconst `{name}`"),
            Item::AutoParam { name } => write!(f, "auto param `{name}`"),
            Item::Entry { index } => write!(f, "entry {index}"),
            Item::Refinement { name, .. } => write!(f, "refinement `{name}`"),
            Item::Registry => write!(f, "registry"),
            Item::Program => write!(f, "program"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A top-level statement of a function body.
    Stmt(usize),
    Then(usize),
    Else(usize),
    /// A statement of a `while` or `repeat` body.
    Body(usize),
    Cond,
    Value,
    Arg(usize),
    Left,
    Right,
    Operand,
    /// The base of a field access, or the array of an index.
    Base,
    Index,
    Field(String),
    Element(usize),
    Param(usize),
    /// A non-parameter local, by `LocalId`.
    Local(usize),
    /// A function's return type.
    Ret,
    /// A type annotation or cast target.
    Type,
    /// A unit's `main`.
    Main,
    /// Position `index` in a scoping list (`helpers`, `tables`, `hosts`, `consts`).
    List { list: &'static str, index: usize },
    /// A scoping list as a whole (a const only reachable through other consts' inits).
    WholeList { list: &'static str },
    /// An input's or entry's dims.
    Dims,
    /// An input's source unit.
    Source,
    /// An entry's unit.
    Unit,
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::Stmt(i) => write!(f, "stmt {i}"),
            Step::Then(i) => write!(f, "then {i}"),
            Step::Else(i) => write!(f, "else {i}"),
            Step::Body(i) => write!(f, "body {i}"),
            Step::Cond => write!(f, "cond"),
            Step::Value => write!(f, "value"),
            Step::Arg(i) => write!(f, "arg {i}"),
            Step::Left => write!(f, "left"),
            Step::Right => write!(f, "right"),
            Step::Operand => write!(f, "operand"),
            Step::Base => write!(f, "base"),
            Step::Index => write!(f, "index"),
            Step::Field(n) => write!(f, "field `{n}`"),
            Step::Element(i) => write!(f, "element {i}"),
            Step::Param(i) => write!(f, "param {i}"),
            Step::Local(i) => write!(f, "local {i}"),
            Step::Ret => write!(f, "return type"),
            Step::Type => write!(f, "type"),
            Step::Main => write!(f, "main"),
            Step::List { list, index } => write!(f, "{list} {index}"),
            Step::WholeList { list } => write!(f, "{list}"),
            Step::Dims => write!(f, "dims"),
            Step::Source => write!(f, "source"),
            Step::Unit => write!(f, "unit"),
        }
    }
}
