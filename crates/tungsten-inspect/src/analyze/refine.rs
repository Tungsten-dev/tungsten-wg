//! Refinements (`specs/refinement.md` §4.1, §8.4): what each refine call runs and what it leaves
//! frozen.

use std::collections::HashSet;

use tungsten_interface::Instance;
use tungsten_ir::model::ComputeUnit;

use super::graph::instance_displays;
use super::{Ctx, sort_ids};
use crate::model::{RefineRunInfo, RefinementInfo, RerunInfo};

pub(crate) fn refinements(ctx: &Ctx) -> Vec<RefinementInfo> {
    let displays = instance_displays(ctx);
    ctx.program
        .refinements
        .iter()
        .enumerate()
        .map(|(n, r)| {
            let waves = ctx.interface.refine_waves(n);
            let running: HashSet<Instance> =
                waves.iter().flatten().map(|run| run.target).collect();

            // Frozen: read by a running unit, but not rerun. `@previous` reads add nothing.
            let mut frozen: Vec<Instance> = vec![];
            for run in waves.iter().flatten() {
                let ComputeUnit::DensityFunction { density_inputs, .. } =
                    &ctx.program.compute_units[run.unit];
                for d in density_inputs {
                    let di = &ctx.program.density_inputs[*d];
                    let inst = Instance::from_input(di);
                    if !di.previous && !running.contains(&inst) && !frozen.contains(&inst) {
                        frozen.push(inst);
                    }
                }
            }
            let mut frozen: Vec<String> = frozen
                .iter()
                .filter_map(|i| ctx.instances.get(i).map(|n| n.id.clone()))
                .collect();
            sort_ids(&mut frozen, &displays);

            RefinementInfo {
                id: ctx.refinements[n].id.clone(),
                display: ctx.refinements[n].display.clone(),
                index: n,
                entry: ctx.entries[r.entry].id.clone(),
                march: r.march,
                reruns: r
                    .reruns
                    .iter()
                    .map(|rr| RerunInfo {
                        instance: ctx
                            .instances
                            .get(&Instance::new(
                                rr.cu,
                                rr.dimensions,
                                rr.scaled_origin,
                                rr.scaled_position,
                            ))
                            .map(|n| n.id.clone())
                            .unwrap_or_default(),
                        with: rr.with.map(|u| ctx.unit_id(u)),
                    })
                    .collect(),
                waves: waves
                    .iter()
                    .map(|w| {
                        w.iter()
                            .map(|run| RefineRunInfo {
                                instance: ctx.instance_id(&run.target),
                                unit: ctx.unit_id(run.unit),
                            })
                            .collect()
                    })
                    .collect(),
                frozen,
            }
        })
        .collect()
}
