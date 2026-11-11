//! `LowerMap`: core ids ↔ tungsten-ir refs and display names.

use std::collections::HashMap;

use tungsten_core::{ConstId, ExternId, FnId, HostId, InputId, LocalId, RecordId, TableId, UnitId};
use tungsten_ir::{
    AutoConst, AutoParam, ComputeUnit, Constant, DensityInput, Function, HostInput, Ref, Var, VariableType,
};

/// A core id: a dense index.
pub trait CoreId: Copy {
    fn index(self) -> usize;
    fn from_index(i: usize) -> Self;
}

macro_rules! core_ids {
    ($($t:ident),*) => {$(
        impl CoreId for $t {
            fn index(self) -> usize {
                self.0
            }
            fn from_index(i: usize) -> Self {
                $t(i)
            }
        }
    )*};
}
core_ids!(UnitId, FnId, ConstId, HostId, TableId, InputId, ExternId, RecordId, LocalId);

/// Core id → (`Ref`, display name), and `Ref` → core id.
pub struct IdMap<T, I> {
    forward: Vec<(Ref<T>, String)>,
    reverse: HashMap<Ref<T>, I>,
}

// Manual impls: `Ref<T>` is `Clone`/`Eq`/`Debug` for every `T`, but derives would require it of
// `T` (and `ComputeUnit`/`Function` aren't `PartialEq`).
impl<T, I: Clone> Clone for IdMap<T, I> {
    fn clone(&self) -> Self {
        Self {
            forward: self.forward.clone(),
            reverse: self.reverse.clone(),
        }
    }
}

impl<T, I: PartialEq> PartialEq for IdMap<T, I> {
    fn eq(&self, other: &Self) -> bool {
        self.forward == other.forward && self.reverse == other.reverse
    }
}

impl<T, I: std::fmt::Debug> std::fmt::Debug for IdMap<T, I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.forward.iter().map(|(_, n)| n)).finish()
    }
}

impl<T, I: CoreId> Default for IdMap<T, I> {
    fn default() -> Self {
        Self {
            forward: vec![],
            reverse: HashMap::new(),
        }
    }
}

impl<T, I: CoreId> IdMap<T, I> {
    /// Appends `r` with the next id. Refs are unique per map, so a repeat keeps its first id.
    pub(crate) fn push(&mut self, r: Ref<T>, name: String) -> I {
        let id = I::from_index(self.forward.len());
        self.forward.push((r, name));
        self.reverse.entry(r).or_insert(id);
        id
    }

    pub fn get(&self, id: I) -> Option<Ref<T>> {
        self.forward.get(id.index()).map(|(r, _)| *r)
    }

    pub fn name(&self, id: I) -> Option<&str> {
        self.forward.get(id.index()).map(|(_, n)| n.as_str())
    }

    pub fn id(&self, r: Ref<T>) -> Option<I> {
        self.reverse.get(&r).copied()
    }

    pub fn len(&self) -> usize {
        self.forward.len()
    }

    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    /// `(id, ref, name)` in id order.
    pub fn iter(&self) -> impl Iterator<Item = (I, Ref<T>, &str)> {
        self.forward
            .iter()
            .enumerate()
            .map(|(i, (r, n))| (I::from_index(i), *r, n.as_str()))
    }
}

/// Names of a record and its fields, in field order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordNames {
    pub name: String,
    pub fields: Vec<String>,
}

/// Index of an auto param, in declaration order. The core IR has no params.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoParamId(pub usize);

impl CoreId for AutoParamId {
    fn index(self) -> usize {
        self.0
    }
    fn from_index(i: usize) -> Self {
        AutoParamId(i)
    }
}

/// Where the core program keeps an autoconst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoCoreId {
    /// A handle autoconst.
    Table(TableId),
    /// A value autoconst: a host after the program's real hosts.
    Host(HostId),
}

/// An argument of an autoconst initializer, with params as indices into `auto_params`.
#[derive(Debug, Clone)]
pub enum AutoArgSpec {
    Param(AutoParamId),
    I64(i64),
    F64(f64),
    Bool(bool),
    Str(String),
}

/// `F64` compares by bit pattern, as in the IR.
impl PartialEq for AutoArgSpec {
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

impl Eq for AutoArgSpec {}

/// An autoconst as declared, with its place in the core program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoConstInfo {
    pub core: AutoCoreId,
    pub name: String,
    pub t: VariableType,
    pub initializer: String,
    pub args: Vec<AutoArgSpec>,
}

/// The side table of a lowering: everything the core IR dropped that callers still need.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LowerMap {
    pub units: IdMap<ComputeUnit, UnitId>,
    pub functions: IdMap<Function, FnId>,
    pub consts: IdMap<Constant, ConstId>,
    pub hosts: IdMap<HostInput, HostId>,
    /// By `HostId`: the `Name::Named` string, which `host(name = …)` bindings resolve against.
    pub host_names: Vec<Option<String>>,
    /// The handle autoconsts, by `TableId`.
    pub tables: IdMap<AutoConst, TableId>,
    /// The value autoconsts and the hosts they lower to. Their ids follow the real hosts.
    pub auto_hosts: HashMap<Ref<AutoConst>, HostId>,
    /// By value autoconst, in order: the name of host `hosts.len() + i`.
    pub auto_host_names: Vec<String>,
    /// A refine step's own hosts, after the value autoconsts: the offset, then the buffers.
    pub step_host_names: Vec<String>,
    /// Auto params in declaration order.
    pub auto_params: IdMap<AutoParam, AutoParamId>,
    /// By `AutoParamId`.
    pub auto_param_types: Vec<VariableType>,
    /// Every autoconst in arena order, read or not.
    pub auto_consts: Vec<AutoConstInfo>,
    pub inputs: IdMap<DensityInput, InputId>,
    /// By `HandleTypeId`: the handle type names, in order of first appearance.
    pub handle_types: Vec<String>,
    /// By `FnId`: that function's locals.
    pub locals: Vec<IdMap<Var, LocalId>>,
    /// By `ExternId`.
    pub externs: Vec<String>,
    /// By `RecordId`.
    pub records: Vec<RecordNames>,
}

/// A buffer: a unit at dims and scales, with the scales as bits. Refine steps keep one store
/// of these per entry (`specs/refinement.md` §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Instance {
    pub unit: UnitId,
    pub dims: (i32, i32, i32),
    pub origin_scale: [u64; 3],
    pub position_scale: [u64; 3],
}

impl LowerMap {
    /// The display name of a host: its name, or for a value autoconst's host, the autoconst's,
    /// or for a refine step's own host, its description.
    pub fn host_name(&self, h: HostId) -> Option<&str> {
        match self.hosts.name(h) {
            Some(n) => Some(n),
            None => {
                let k = h.0.checked_sub(self.hosts.len())?;
                match self.auto_host_names.get(k) {
                    Some(n) => Some(n),
                    None => self.step_host_names.get(k - self.auto_host_names.len()).map(String::as_str),
                }
            }
        }
    }

    /// Whether `h` is the host of a value autoconst rather than a program host.
    pub fn is_auto_host(&self, h: HostId) -> bool {
        h.0 >= self.hosts.len() && h.0 - self.hosts.len() < self.auto_host_names.len()
    }

    pub fn extern_name(&self, e: ExternId) -> Option<&str> {
        self.externs.get(e.0).map(String::as_str)
    }

    pub fn record_name(&self, r: RecordId) -> Option<&str> {
        self.records.get(r.0).map(|r| r.name.as_str())
    }

    pub fn field_name(&self, r: RecordId, field: usize) -> Option<&str> {
        self.records.get(r.0)?.fields.get(field).map(String::as_str)
    }

    pub fn local(&self, f: FnId, r: Ref<Var>) -> Option<LocalId> {
        self.locals.get(f.0)?.id(r)
    }
}
