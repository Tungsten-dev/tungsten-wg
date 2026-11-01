//! The analysis model. Everything the renderers show comes from here, and `inspection.json` is
//! this struct serialized, so the files and the JSON always agree.
//!
//! The model never stores `Ref`s. Every entity has a stable string id (`cu:…`, `inst:…`,
//! `entry:<n>:…`, `fn:…`, `const:…`, `table:…`, `host:…`) and a display name (SPEC.md §6.1).
//! Lists are sorted by display name with the id as tiebreak, except where the order is the
//! information (schedules, DAG construction).

use std::collections::BTreeMap;

use serde::Serialize;

/// Version of the `inspection.json` schema.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Inspection {
    pub version: u32,
    pub summary: Summary,
    pub entries: Vec<EntryInfo>,
    pub units: Vec<UnitInfo>,
    pub instances: Vec<InstanceInfo>,
    pub functions: Vec<FunctionInfo>,
    pub constants: Vec<ConstantInfo>,
    pub tables: Vec<TableInfo>,
    pub host_inputs: Vec<HostInputInfo>,
    pub call_graph: CallGraph,
    pub resources: ResourceGraph,
    /// Non-fatal remarks, e.g. unsupported buffer element types.
    pub notes: Vec<String>,
    /// Id → output path (relative to the output root). Filled by the renderers.
    pub paths: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Summary {
    pub units: usize,
    pub reachable_units: usize,
    /// Functions that are not the main function of a compute unit.
    pub helpers: usize,
    pub constants: usize,
    pub tables: usize,
    pub host_inputs: usize,
    pub entries: usize,
    pub instances: usize,
    pub unreachable: Unreachable,
}

/// Ids of everything no entry point reaches.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Unreachable {
    pub units: Vec<String>,
    pub functions: Vec<String>,
    pub constants: Vec<String>,
    pub tables: Vec<String>,
    pub host_inputs: Vec<String>,
}

/// Dimensions and scales a compute unit is evaluated over.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Grid {
    pub dimensions: (i32, i32, i32),
    pub scaled_origin: (f64, f64, f64),
    pub scaled_position: (f64, f64, f64),
}

// ---------------------------------------------------------------------------------------------
// Entries, units, instances
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct EntryInfo {
    pub id: String,
    pub display: String,
    /// Index in `OptimizedProgram::entry_points`.
    pub index: usize,
    /// The instance this entry point returns.
    pub instance: String,
    /// The transitive closure of `instance`, sorted.
    pub instances: Vec<String>,
    pub schedule: Schedule,
    /// Critical path, in waves.
    pub wave_count: usize,
    pub max_wave_width: usize,
    /// Instances of this entry that other entries need too.
    pub shared: Vec<SharedInstance>,
    pub buffers: EntryBuffers,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SharedInstance {
    pub instance: String,
    /// The other entries that need it.
    pub entries: Vec<String>,
}

/// The entry's pruned schedule, in execution order (not sorted).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Schedule {
    /// CPU axis: step `k` is the k-th kernel launch.
    pub steps: Vec<String>,
    /// GPU axis: instances of a wave run concurrently.
    pub waves: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct UnitInfo {
    pub id: String,
    pub display: String,
    /// Id of the main function.
    pub function: String,
    pub return_type: String,
    pub source_hash: u64,
    pub reachable: bool,
    /// The distinct grids it is used at.
    pub instances: Vec<String>,
    /// Entries that reach it.
    pub entries: Vec<String>,
    /// Distinct units it reads.
    pub reads: Vec<String>,
    /// Distinct units that read it.
    pub read_by: Vec<String>,
    pub fan_in: usize,
    pub fan_out: usize,
    /// Longest dependency chain to a leaf, in unit edges.
    pub depth: usize,
    /// Its density inputs, in arena order.
    pub inputs: Vec<InputInfo>,
    pub tables: Vec<String>,
    pub host_inputs: Vec<String>,
    pub helpers: Vec<String>,
    pub constants: Vec<String>,
    pub inline_verdict: InlineVerdictInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct InputInfo {
    /// Local name of the input, unique within its unit.
    pub name: String,
    pub producer: String,
    pub grid: Grid,
    /// Some read of it (in any function) carries an index.
    pub indexed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct InstanceInfo {
    pub id: String,
    pub display: String,
    pub unit: String,
    pub grid: Grid,
    /// Interface names, as secondary info.
    pub buffer: String,
    pub kernel: String,
    pub density_fn: String,
    /// Element size × cell count.
    pub bytes: usize,
    /// Distinct instances it reads, sorted.
    pub depends_on: Vec<String>,
    /// One edge per density input of the unit (parallel inputs to one producer stay separate).
    pub inputs: Vec<EdgeInfo>,
    /// Entries whose closure contains it.
    pub entries: Vec<String>,
}

/// Consumer instance ← producer instance, for one density input.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct EdgeInfo {
    pub input: String,
    pub producer: String,
    pub input_grid: Grid,
    pub consumer_grid: Grid,
    /// `input_grid != consumer_grid` (dimensions or either scale differ).
    pub resampled: bool,
    /// Some read of this input in the consumer's functions carries an index.
    pub indexed: bool,
}

// ---------------------------------------------------------------------------------------------
// Buffers
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct EntryBuffers {
    pub cpu: BufferAxis,
    pub gpu: BufferAxis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Kernel steps, one instance each.
    Cpu,
    /// Waves, all instances concurrent.
    Gpu,
}

/// Where the slot assignment comes from. Only the simulation exists today; the interface will
/// provide the real one once buffer reuse lands (SPEC.md §6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentSource {
    Simulated,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct BufferAxis {
    pub axis: Axis,
    pub assignment_source: AssignmentSource,
    /// Number of time units (steps or waves).
    pub units: usize,
    /// Sum of all buffers: today's layout, without reuse.
    pub total_bytes: usize,
    /// Max over time units of live bytes; a lower bound for any assignment (aliasing aside).
    pub peak_live_bytes: usize,
    /// Sum of the simulated slot sizes.
    pub slot_bytes: usize,
    pub slot_count: usize,
    /// Size of each simulated slot, by slot index.
    pub slots: Vec<usize>,
    /// Sorted by `start`, then display name.
    pub buffers: Vec<BufferInfo>,
    /// Live bytes per time unit.
    pub live_bytes: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct BufferInfo {
    pub instance: String,
    pub bytes: usize,
    pub start: usize,
    /// Inclusive. `None` (`null`) when pinned: the buffer lives past the end of the run.
    pub end: Option<usize>,
    pub slot: usize,
    /// The input whose slot this buffer took over in place.
    pub aliases: Option<String>,
    /// The entry's own output.
    pub pinned: bool,
}

// ---------------------------------------------------------------------------------------------
// Functions and dataflow
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct FunctionInfo {
    pub id: String,
    pub display: String,
    pub kind: FunctionKind,
    /// `Function::canonical_name`; `None` for anonymous functions.
    pub canonical_name: Option<String>,
    /// Units that own it: the unit of a main function, or every unit listing a helper.
    pub units: Vec<String>,
    pub reachable: bool,
    pub parameters: Vec<Parameter>,
    pub return_type: String,
    pub host_inputs: Vec<String>,
    /// Distinct functions it calls.
    pub calls: Vec<String>,
    /// The function as IR text (SPEC.md §7.5), without ref indices.
    pub ir: String,
    /// `None` when the dataflow view is disabled.
    pub dag: Option<FunctionDag>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionKind {
    Main,
    Helper,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Parameter {
    pub name: String,
    pub ty: String,
}

/// Dataflow graph of one function body: reaching definitions with implicit φ (several edges
/// on one port), and whole-function CSE. Loop-carried edges make it cyclic; they are marked.
///
/// Node and region ids are indices into `nodes` / `regions`, in construction (program) order.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct FunctionDag {
    /// `regions[0]` is the function body.
    pub regions: Vec<Region>,
    pub nodes: Vec<DagNode>,
    pub edges: Vec<DagEdge>,
    pub summary: DagSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Region {
    pub id: usize,
    pub kind: RegionKind,
    pub parent: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionKind {
    Body,
    If,
    Then,
    Else,
    While,
    Repeat {
        count: usize,
    },
    /// Statements after a `return` or `break` in the same block.
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DagNode {
    pub id: usize,
    pub kind: NodeKind,
    /// The innermost region containing every occurrence of the node.
    pub region: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    // Leaves
    Param {
        var: String,
    },
    /// A variable read that no assignment or parameter reaches, e.g. the implicit `pos3` and
    /// `origin` of density functions.
    Free {
        var: String,
    },
    Literal(Literal),
    Constant {
        constant: String,
    },
    HostInput {
        host_input: String,
    },
    PermTable {
        table: String,
    },
    /// An un-indexed density read.
    DensityRead(DensityReadInfo),

    // Operators
    Binary {
        op: String,
    },
    Unary {
        op: String,
    },
    Field {
        name: String,
    },
    Index,
    Construct {
        ty: String,
    },
    ConstructExtern {
        ty: String,
        fields: Vec<String>,
    },
    ArrayLiteral,
    Cast {
        ty: String,
    },
    Extern {
        name: String,
        parameter_types: Vec<String>,
        return_type: String,
    },
    /// Opaque call; see the callee's own DAG.
    Call {
        function: String,
    },
    /// A density read with an `idx` input edge.
    IndexedDensityRead(DensityReadInfo),

    // Definitions and sinks
    /// Labeled `var#version`; versions count per variable from 1 in statement order.
    Def {
        var: String,
        version: usize,
    },
    Return,
    /// The condition of the `if` / `while` region the node is in.
    Cond,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DensityReadInfo {
    /// Local name of the input in its unit.
    pub input: String,
    /// The unit whose input this is (for helpers, not necessarily the function's own unit).
    pub unit: String,
    pub producer_unit: String,
    /// `None` when the producer instance isn't scheduled (unreachable code).
    pub producer_instance: Option<String>,
    pub grid: Grid,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Literal {
    Float(f32),
    Double(f64),
    Byte(i8),
    Int(i32),
    Long(i64),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DagEdge {
    pub from: usize,
    pub to: usize,
    /// `l`/`r` for binary operators, `0..n` for arguments, `idx`, `base`, `cond`, `value`.
    pub port: String,
    /// Introduced by the loop fixpoint (a value from a previous iteration).
    pub loop_carried: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DagSummary {
    /// Nodes that would exist without CSE.
    pub nodes_before_cse: usize,
    pub nodes_after_cse: usize,
    /// Reads per density input (`<unit id>/<input>`), indexed or not, before CSE.
    pub density_reads: BTreeMap<String, usize>,
    /// Calls per helper id, before CSE.
    pub helper_calls: BTreeMap<String, usize>,
    /// Calls per extern function name, before CSE.
    pub extern_calls: BTreeMap<String, usize>,
}

// ---------------------------------------------------------------------------------------------
// Resources and program graphs
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ConstantInfo {
    pub id: String,
    pub display: String,
    pub reachable: bool,
    /// The value expression as IR text.
    pub value: String,
    /// Constants its value refers to.
    pub depends_on: Vec<String>,
    /// Units that declare it.
    pub users: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct TableInfo {
    pub id: String,
    pub display: String,
    pub reachable: bool,
    /// Units that declare it.
    pub users: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct HostInputInfo {
    pub id: String,
    pub display: String,
    pub ty: String,
    pub reachable: bool,
    /// Units that declare it, or own a function that declares it.
    pub users: Vec<String>,
}

/// Nodes are all functions (see `functions`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CallGraph {
    pub edges: Vec<CallEdge>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CallEdge {
    pub from: String,
    pub to: String,
    /// Call sites in `from`.
    pub count: usize,
}

/// Bipartite between units and tables / host inputs / constants, plus constant → constant.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ResourceGraph {
    pub uses: Vec<ResourceUse>,
    pub constant_dependencies: Vec<ConstantDependency>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ResourceUse {
    pub unit: String,
    pub resource: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ConstantDependency {
    pub constant: String,
    pub depends_on: String,
}

// ---------------------------------------------------------------------------------------------
// Inlining
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct InlineVerdictInfo {
    pub verdict: Verdict,
    /// One line for humans, e.g. `blocked: entry point (must stay addressable)`.
    pub explanation: String,
}

/// `tungsten_optimizer::InlineVerdict`, with refs replaced by ids.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Inlinable {
        into: String,
        site: String,
        reads: usize,
    },
    EntryPoint,
    Unused,
    MultipleUses {
        uses: usize,
    },
    SelfUse,
    CallerTooComplex,
    CallerGridAmbiguous,
    GridMismatch {
        caller: Grid,
        input: Grid,
    },
    IndexedRead {
        function: String,
    },
    ReadInSeveralFunctions,
    NotRead,
}
