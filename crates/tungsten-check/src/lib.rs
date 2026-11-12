//! Lowers a tungsten-ir [`IrProgram`] to the core IR of `tungsten-core`
//! (re-exported as [`core_ir`]). See this crate's `README.md`.
//!
//! The lowering only resolves names and normalizes shapes. Anything that can be *wrong* (type
//! annotations, field coverage, scoping, acyclicity) stays in the IR for the checker to judge.
//! [`lower`] collects every error, each with a [`Location`], and never panics, whatever refs the
//! program holds.
//!
//! [`prepare`] lowers and then runs the core checker; [`Prepared::eval`] runs the core
//! interpreter on one case, and [`evaluate`] does both in one go.
//!
//! Externs and records resolve strictly against a [`Registry`] (an unknown name is an error),
//! which the caller builds with [`RegistryBuilder`].

pub use tungsten_core as core_ir;

mod auto;
mod check;
mod error;
mod eval;
mod lower;
mod map;
mod registry;

#[cfg(test)]
mod tests;

pub use error::{
    EntityKind, Item, Location, LowerError, LowerErrorKind, LowerErrors,
    ParameterProblem, Step,
};
pub use auto::{AutoArgValue, ParamBindError, ResolvedAutoConst};
pub use check::{CheckError, CheckErrorKind, CheckErrors, Invalid, ReturnHint, UnassignedHint};
pub use eval::{
    CaseBindError, DEFAULT_FUEL, EvalFailure, EvalOptions, EvaluateError, NamedCase, Prepared, RefineCall,
    SequenceCase, SequenceResult, check_lowered, evaluate, prepare,
};
pub use map::{
    AutoArgSpec, AutoConstInfo, AutoCoreId, AutoParamId, CoreId, IdMap, Instance, LowerMap, RecordNames,
};
pub use tungsten_core::exec::{CaseResult, Failure};
pub use tungsten_core::{Case, EvalError, ExternFail, Externs, StuckReason, Value};
pub use registry::{AutoArgType, InitializerDecl, Registry, RegistryBuilder, RegistryError};

use tungsten_ir::Program as IrProgram;

use crate::lower::{run, run_step};
use tungsten_core::HostId;

/// A lowered program and its side table.
#[derive(Debug, Clone, PartialEq)]
pub struct Lowered {
    pub program: core_ir::Program,
    pub map: LowerMap,
    /// By core entry: the instance it computes. The first `visible` are the IR's entries; the
    /// rest are hidden entries for what refine calls read from `run`
    /// (`specs/refinement.md` §7.1), which are left out of comparisons.
    pub instances: Vec<Instance>,
    pub visible: usize,
    /// One per `program.refinements`, in order.
    pub steps: Vec<LoweredStep>,
}

/// One refine call as an ordinary core program (`specs/refinement.md` §7.2).
#[derive(Debug, Clone, PartialEq)]
pub struct LoweredStep {
    /// The refinement's name.
    pub name: String,
    /// Index into the IR's `entry_points`: the worker whose store the step reads and writes.
    pub entry: usize,
    pub program: core_ir::Program,
    pub map: LowerMap,
    /// The `Pos3` host holding the call's offset.
    pub offset: HostId,
    /// The array hosts for frozen and `@previous` reads, and the instance each holds.
    pub buffers: Vec<(HostId, Instance)>,
    /// By step entry: the instance it writes, the refinement's entry first.
    pub running: Vec<Instance>,
}

/// Lowers `p` against `registry`. On success the program and its refine steps satisfy
/// `tungsten_core::wf::wf_program`, and their extern and record tables are the registry's. On
/// failure, every error of the normal program is returned, in walk order.
pub fn lower(p: &IrProgram, registry: &Registry) -> Result<Lowered, LowerErrors> {
    let out = run(p, registry);
    if !out.errors.is_empty() {
        return Err(LowerErrors(out.errors));
    }
    let (program, map) = finish(out.program, out.map, registry);
    let mut steps = vec![];
    for (i, r) in p.refinements.iter().enumerate() {
        let step = run_step(p, registry, i);
        if !step.errors.is_empty() {
            return Err(LowerErrors(step.errors));
        }
        let (program, map) = finish(step.program, step.map, registry);
        steps.push(LoweredStep {
            name: r.name.clone(),
            entry: r.entry,
            program,
            map,
            offset: step.offset.expect("a step has an offset host"),
            buffers: step.buffers,
            running: step.instances,
        });
    }
    Ok(Lowered { program, map, instances: out.instances, visible: p.entry_points.len(), steps })
}

fn finish(mut program: core_ir::Program, mut map: LowerMap, registry: &Registry) -> (core_ir::Program, LowerMap) {
    map.externs = registry.extern_names.clone();
    map.records = registry
        .record_names
        .iter()
        .zip(&registry.field_names)
        .map(|(name, fields)| RecordNames {
            name: name.clone(),
            fields: fields.clone(),
        })
        .collect();
    (program.externs, program.records) = registry.decls();
    (program, map)
}
