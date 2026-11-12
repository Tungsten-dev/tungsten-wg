//! Refinements (`specs/refinement.md`): which instances run in each refine call (§4.1), what
//! `run` computes for them (§4.5), and their validation (§5).

use std::collections::HashSet;

use tungsten_core::InputId;
use tungsten_ir::{ComputeUnit, DensityInput, DensityMapping, Program as IrProgram, Ref, Refinement, Rerun};

use super::Ctx;
use super::mapping::{AXES, arr, listed_inputs, unit_reads, unit_return_type};
use crate::error::{Item, LowerErrorKind};

/// A buffer: unit, dims and scales, with scales compared by bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Inst {
    pub cu: Ref<ComputeUnit>,
    pub dims: (i32, i32, i32),
    pub origin: [u64; 3],
    pub position: [u64; 3],
}

fn bits(v: (f64, f64, f64)) -> [u64; 3] {
    [v.0.to_bits(), v.1.to_bits(), v.2.to_bits()]
}

impl Inst {
    pub fn of_input(d: &DensityInput) -> Self {
        Self { cu: d.density_function, dims: d.dimensions, origin: bits(d.scaled_origin), position: bits(d.scaled_position) }
    }

    pub fn of_entry(e: &tungsten_ir::EntryPoint) -> Self {
        Self {
            cu: e.density_function,
            dims: e.dimensions,
            origin: bits(e.scaled_origin),
            position: bits(e.scaled_position),
        }
    }

    pub fn of_rerun(r: &Rerun) -> Self {
        Self { cu: r.cu, dims: r.dimensions, origin: bits(r.scaled_origin), position: bits(r.scaled_position) }
    }
}

/// One instance that runs in a refine call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Run {
    pub target: Inst,
    /// `with`, or `target.cu`.
    pub unit: Ref<ComputeUnit>,
    /// Index into the refinement's `reruns`.
    pub rerun: usize,
}

/// §4.1 step 1 for one refinement whose entry index is valid.
#[derive(Debug, Clone)]
pub(super) struct Walk {
    /// Index into `program.refinements`.
    pub index: usize,
    pub march: (i32, i32, i32),
    /// In discovery order, the entry's instance first.
    pub running: Vec<Run>,
    /// Instances a running unit reads that aren't rerun, in discovery order, without duplicates.
    pub frozen_reads: Vec<Inst>,
    /// The targets of reruns with a `with` unit, in rerun order.
    pub with_targets: Vec<Inst>,
}

/// The walks of every refinement whose entry exists.
pub(super) struct RefineGraph {
    pub walks: Vec<Walk>,
}

impl RefineGraph {
    pub fn new(p: &IrProgram) -> Self {
        let walks = p
            .refinements
            .iter()
            .enumerate()
            .filter_map(|(index, r)| walk(p, index, r))
            .collect();
        Self { walks }
    }
}

fn walk(p: &IrProgram, index: usize, r: &Refinement) -> Option<Walk> {
    let entry = p.entry_points.get(r.entry)?;
    let reruns: Vec<Inst> = r.reruns.iter().map(Inst::of_rerun).collect();
    // The first listing wins when an instance is listed twice (`RefineDuplicateRerun`).
    let find = |i: &Inst| reruns.iter().position(|x| x == i);
    let start = Inst::of_entry(entry);
    let mut running: Vec<Run> = vec![];
    let mut frozen_reads: Vec<Inst> = vec![];
    if let Some(k) = find(&start) {
        running.push(Run { target: start, unit: r.reruns[k].with.unwrap_or(start.cu), rerun: k });
    }
    let mut i = 0;
    while i < running.len() {
        let run = running[i];
        i += 1;
        for (_, d) in listed_inputs(p, run.unit) {
            if d.previous {
                continue;
            }
            let inst = Inst::of_input(d);
            match find(&inst) {
                Some(k) => {
                    if !running.iter().any(|x| x.target == inst) {
                        running.push(Run { target: inst, unit: r.reruns[k].with.unwrap_or(inst.cu), rerun: k });
                    }
                }
                None => {
                    if !frozen_reads.contains(&inst) {
                        frozen_reads.push(inst);
                    }
                }
            }
        }
    }
    let with_targets = r.reruns.iter().filter(|x| x.with.is_some()).map(Inst::of_rerun).collect();
    Some(Walk { index, march: r.march, running, frozen_reads, with_targets })
}

impl<'p, 'r> Ctx<'p, 'r> {
    fn unit_name(&self, u: Ref<ComputeUnit>) -> String {
        self.map.units.id(u).and_then(|id| self.map.units.name(id)).unwrap_or("?").to_string()
    }

    /// §5, except the mapping rules, which `mapping_readers` applies to refined units too.
    pub(super) fn refinements(&mut self) {
        let p = self.p;
        let graph = RefineGraph::new(p);
        let mut names: HashSet<(usize, &str)> = HashSet::new();
        let mut with_units: Vec<Ref<ComputeUnit>> = vec![];
        for (index, r) in p.refinements.iter().enumerate() {
            self.enter(Item::Refinement { index, name: r.name.clone() });
            if r.entry >= p.entry_points.len() {
                self.err(LowerErrorKind::RefineBadEntry { entry: r.entry });
            } else if !names.insert((r.entry, r.name.as_str())) {
                self.err(LowerErrorKind::RefineDuplicateName { entry: r.entry, name: r.name.clone() });
            }
            for (k, m) in arr(r.march).into_iter().enumerate() {
                if m < 0 {
                    self.err(LowerErrorKind::RefineBadMarch { axis: AXES[k], march: m });
                }
            }
            let reruns: Vec<Inst> = r.reruns.iter().map(Inst::of_rerun).collect();
            for (i, inst) in reruns.iter().enumerate() {
                if let Some(first) = reruns[..i].iter().position(|x| x == inst) {
                    self.err(LowerErrorKind::RefineDuplicateRerun { index: i, first });
                }
            }
            for x in &r.reruns {
                if let Some(w) = x.with
                    && !with_units.contains(&w)
                {
                    with_units.push(w);
                }
            }
            let Some(walk) = graph.walks.iter().find(|w| w.index == index) else {
                continue;
            };
            if walk.running.is_empty() {
                self.err(LowerErrorKind::RefineEntryNotRerun);
            }
            for (i, x) in r.reruns.iter().enumerate() {
                let reached = walk.running.iter().any(|run| run.rerun == i);
                let duplicate = reruns[..i].contains(&reruns[i]);
                if !reached && !duplicate && !walk.running.is_empty() {
                    let unit = self.unit_name(x.cu);
                    self.err(LowerErrorKind::RefineRerunUnreachable { index: i, unit });
                }
            }
            for (i, x) in r.reruns.iter().enumerate() {
                let Some(w) = x.with else { continue };
                let (Some(expected), Some(found)) = (unit_return_type(p, x.cu), unit_return_type(p, w)) else {
                    continue;
                };
                if expected != found {
                    self.err(LowerErrorKind::RefineTypeMismatch {
                        index: i,
                        unit: self.unit_name(x.cu),
                        with: self.unit_name(w),
                        expected: format!("{expected:?}"),
                        found: format!("{found:?}"),
                    });
                }
            }
            // Position scales on marched axes, against the first rerun.
            if let Some(first) = reruns.first() {
                for (i, inst) in reruns.iter().enumerate().skip(1) {
                    let axis = (0..3).find(|&k| arr(r.march)[k] > 0 && inst.position[k] != first.position[k]);
                    if let Some(k) = axis {
                        let unit = self.unit_name(r.reruns[i].cu);
                        self.err(LowerErrorKind::RefineMarchScaleMismatch { index: i, unit, axis: AXES[k] });
                    }
                }
            }
            if let Some(unit) = self.running_cycle(walk) {
                self.err(LowerErrorKind::RefineCycle { unit });
            }
        }

        self.enter(Item::Program);
        for &w in &with_units {
            let why = if p.entry_points.iter().any(|e| e.density_function == w) {
                Some("an entry")
            } else if p.density_inputs.values().any(|d| d.density_function == w) {
                Some("the source of an input")
            } else {
                None
            };
            if let Some(why) = why {
                let unit = self.unit_name(w);
                self.err(LowerErrorKind::RefinedUnitInNormalGraph { unit, why: why.into() });
            }
        }
        self.previous_inputs(&with_units);
    }

    /// A cycle among the instances of one refine call, following the inputs of the unit that
    /// runs for each, `@previous` edges left out. Gives the unit where the walk first closes it.
    fn running_cycle(&self, walk: &Walk) -> Option<String> {
        let p = self.p;
        let index = |i: &Inst| walk.running.iter().position(|r| r.target == *i);
        let edges: Vec<Vec<usize>> = walk
            .running
            .iter()
            .map(|run| {
                listed_inputs(p, run.unit)
                    .into_iter()
                    .filter(|(_, d)| !d.previous)
                    .filter_map(|(_, d)| index(&Inst::of_input(d)))
                    .collect()
            })
            .collect();
        // 0 = unvisited, 1 = on the stack, 2 = done.
        let mut state = vec![0u8; edges.len()];
        fn visit(n: usize, edges: &[Vec<usize>], state: &mut [u8]) -> Option<usize> {
            state[n] = 1;
            for &m in &edges[n] {
                match state[m] {
                    1 => return Some(m),
                    0 => {
                        if let Some(c) = visit(m, edges, state) {
                            return Some(c);
                        }
                    }
                    _ => {}
                }
            }
            state[n] = 2;
            None
        }
        let n = (0..edges.len()).find_map(|n| if state[n] == 0 { visit(n, &edges, &mut state) } else { None })?;
        Some(self.unit_name(walk.running[n].unit))
    }

    /// The `@previous` rules, per input in arena order.
    fn previous_inputs(&mut self, with_units: &[Ref<ComputeUnit>]) {
        let p = self.p;
        for (i, (r, d)) in p.density_inputs.iter().enumerate() {
            if !d.previous {
                continue;
            }
            let id = InputId(i);
            let name = self.map.inputs.name(id).unwrap_or_default().to_string();
            self.enter(Item::Input { id: Some(id), name });
            if d.mapping != DensityMapping::Identity {
                self.err(LowerErrorKind::PreviousMapped);
            }
            let listers: Vec<Ref<ComputeUnit>> = p
                .compute_units
                .iter()
                .map(|(u, _)| u)
                .filter(|&u| listed_inputs(p, u).iter().any(|(x, _)| *x == r))
                .collect();
            for &u in &listers {
                if !with_units.contains(&u) {
                    let unit = self.unit_name(u);
                    self.err(LowerErrorKind::PreviousOutsideRefinedUnit { unit });
                }
            }
            let inst = Inst::of_input(d);
            for refinement in &p.refinements {
                for x in &refinement.reruns {
                    let Some(w) = x.with else { continue };
                    if listers.contains(&w) && Inst::of_rerun(x) != inst {
                        self.err(LowerErrorKind::PreviousNotTarget {
                            with: self.unit_name(w),
                            refinement: refinement.name.clone(),
                        });
                    }
                }
            }
            for &u in &listers {
                if unit_reads(p, u).indexed.contains(&r) {
                    let unit = self.unit_name(u);
                    self.err(LowerErrorKind::PreviousIndexed { unit });
                }
            }
        }
    }
}
