//! Which instances run in each refine call (`specs/refinement.md` §4.1, §4.5, §6.1).

use std::collections::HashSet;

use tungsten_ir::model::{ComputeUnit, OptimizedProgram, Ref, Refinement};

use crate::Instance;

/// One instance that runs in a refine call: the buffer it writes, and the unit that writes it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RefineRun {
    /// The buffer written.
    pub target: Instance,
    /// The refined unit (`with`), or `target.cu`.
    pub unit: Ref<ComputeUnit>,
}

/// §4.1 step 1 for one refinement. Tolerant of what validation rejects: an unknown entry or a
/// missing entry rerun gives an empty walk, and a listed instance twice counts once.
#[derive(Debug, Default, Clone)]
pub(crate) struct Walk {
    /// In discovery order, the entry's instance first.
    pub running: Vec<RefineRun>,
    /// Instances a running unit reads that aren't rerun, in discovery order, without duplicates.
    pub frozen: Vec<Instance>,
    /// The targets of reruns with a `with` unit, in rerun order.
    pub with_targets: Vec<Instance>,
}

/// The listed inputs of `cu` as instances, `@previous` ones left out: they add no dependency.
pub(crate) fn normal_inputs(program: &OptimizedProgram, cu: Ref<ComputeUnit>) -> Vec<Instance> {
    let ComputeUnit::DensityFunction { density_inputs, .. } = &program.compute_units[cu];
    let mut out: Vec<Instance> = vec![];
    for &r in density_inputs {
        let d = &program.density_inputs[r];
        if d.previous {
            continue;
        }
        let inst = Instance::from_input(d);
        if !out.contains(&inst) {
            out.push(inst);
        }
    }
    out
}

pub(crate) fn rerun_instance(r: &tungsten_ir::model::Rerun) -> Instance {
    Instance::new(r.cu, r.dimensions, r.scaled_origin, r.scaled_position)
}

pub(crate) fn walk(program: &OptimizedProgram, r: &Refinement) -> Walk {
    let Some(entry) = program.entry_points.get(r.entry) else {
        return Walk::default();
    };
    let reruns: Vec<Instance> = r.reruns.iter().map(rerun_instance).collect();
    // The first listing wins when an instance is listed twice.
    let find = |i: &Instance| reruns.iter().position(|x| x == i);
    let run_of = |k: usize, inst: Instance| RefineRun { target: inst, unit: r.reruns[k].with.unwrap_or(inst.cu) };

    let start = Instance::from_entry(entry);
    let mut running: Vec<RefineRun> = vec![];
    let mut frozen: Vec<Instance> = vec![];
    if let Some(k) = find(&start) {
        running.push(run_of(k, start));
    }
    let mut i = 0;
    while i < running.len() {
        let run = running[i];
        i += 1;
        for inst in normal_inputs(program, run.unit) {
            match find(&inst) {
                Some(k) => {
                    if !running.iter().any(|x| x.target == inst) {
                        running.push(run_of(k, inst));
                    }
                }
                None => {
                    if !frozen.contains(&inst) {
                        frozen.push(inst);
                    }
                }
            }
        }
    }
    let with_targets = r
        .reruns
        .iter()
        .filter(|x| x.with.is_some())
        .map(rerun_instance)
        .collect();
    Walk { running, frozen, with_targets }
}

/// The runs of one refinement in dependency order, one wave at a time.
///
/// Panics on a cycle, like `arrange_waves`: validation rejects it (`RefineCycle`), and the
/// low-level path doesn't validate.
pub(crate) fn refine_waves(program: &OptimizedProgram, running: &[RefineRun]) -> Vec<Vec<RefineRun>> {
    let targets: HashSet<Instance> = running.iter().map(|r| r.target).collect();
    let mut remaining: Vec<(RefineRun, Vec<Instance>)> = running
        .iter()
        .map(|&run| {
            let deps = normal_inputs(program, run.unit).into_iter().filter(|d| targets.contains(d)).collect();
            (run, deps)
        })
        .collect();
    let mut waves = vec![];
    while !remaining.is_empty() {
        let wave: Vec<RefineRun> =
            remaining.iter().filter(|(_, deps)| deps.is_empty()).map(|(run, _)| *run).collect();
        if wave.is_empty() {
            panic!("Cyclic dependency detected between compute unit instances");
        }
        remaining.retain(|(run, _)| !wave.contains(run));
        let done: Vec<Instance> = wave.iter().map(|r| r.target).collect();
        for (_, deps) in &mut remaining {
            deps.retain(|d| !done.contains(d));
        }
        waves.push(wave);
    }
    waves
}

/// What entry `entry`'s `run` has to compute (§4.5): the entry, the frozen instances its
/// refinements' running units read, and every `with` target. Without refinements, only the entry.
pub(crate) fn schedule_seeds(program: &OptimizedProgram, walks: &[Walk], entry: usize) -> Vec<Instance> {
    let mut seeds = vec![Instance::from_entry(&program.entry_points[entry])];
    for (r, w) in program.refinements.iter().zip(walks) {
        if r.entry == entry {
            for i in w.frozen.iter().chain(&w.with_targets) {
                if !seeds.contains(i) {
                    seeds.push(*i);
                }
            }
        }
    }
    seeds
}
