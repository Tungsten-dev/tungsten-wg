//! Instances, per-entry closures, unit-level graph metrics and sharing (SPEC.md §6.2).

use std::collections::{HashMap, HashSet};

use tungsten_interface::Instance;
use tungsten_ir::model::{ComputeUnit, Ref};

use super::schedule::EntrySchedule;
use super::{Ctx, buffers, grid_of_input, grid_of_instance, inline, sort_ids, sort_named};
use crate::model::{EdgeInfo, EntryInfo, InputInfo, InstanceInfo, SharedInstance, UnitInfo};

/// Every scheduled instance, with its input edges.
pub(crate) fn instances(ctx: &Ctx) -> Vec<InstanceInfo> {
    let closures = closures(ctx);
    let displays = instance_displays(ctx);

    let mut out: Vec<InstanceInfo> = ctx
        .interface
        .instances()
        .iter()
        .map(|inst| {
            let ComputeUnit::DensityFunction { density_inputs, .. } =
                &ctx.program.compute_units[inst.cu];
            let consumer_grid = grid_of_instance(inst);
            let mut inputs: Vec<EdgeInfo> = density_inputs
                .iter()
                .map(|(r, di)| {
                    let input_grid = grid_of_input(di);
                    EdgeInfo {
                        input: ctx.inputs[&r].name.clone(),
                        producer: ctx.instance_id(&Instance::from_input(di)),
                        input_grid,
                        consumer_grid,
                        resampled: input_grid != consumer_grid,
                        indexed: ctx.indexed_inputs.contains(&r),
                    }
                })
                .collect();
            inputs.sort_by(|a, b| a.input.cmp(&b.input));

            let mut depends_on: Vec<String> = inputs.iter().map(|e| e.producer.clone()).collect();
            sort_ids(&mut depends_on, &displays);

            let mut entries: Vec<String> = closures
                .iter()
                .enumerate()
                .filter(|(_, c)| c.contains(inst))
                .map(|(n, _)| ctx.entries[n].id.clone())
                .collect();
            sort_ids(&mut entries, &entry_displays(ctx));

            let (x, y, z) = inst.dimensions;
            let cells = (x.max(0) * y.max(0) * z.max(0)) as usize;
            let named = &ctx.instances[inst];
            InstanceInfo {
                id: named.id.clone(),
                display: named.display.clone(),
                unit: ctx.unit_id(inst.cu),
                grid: consumer_grid,
                buffer: ctx.interface.buffer_field(inst).to_string(),
                kernel: ctx.interface.kernel(inst).to_string(),
                density_fn: ctx
                    .interface
                    .density_fn(inst.cu, inst.dimensions)
                    .to_string(),
                bytes: cells * ctx.element_size(ctx.return_type(inst.cu)).unwrap_or(0),
                depends_on,
                inputs,
                entries,
            }
        })
        .collect();
    sort_named(&mut out, |i| (&i.display, &i.id));
    out
}

pub(crate) fn entries(ctx: &Ctx, instances: &[InstanceInfo]) -> Vec<EntryInfo> {
    let by_id: HashMap<&str, &InstanceInfo> =
        instances.iter().map(|i| (i.id.as_str(), i)).collect();
    let displays = instance_displays(ctx);

    let mut out: Vec<EntryInfo> = ctx
        .program
        .entry_points
        .iter()
        .enumerate()
        .map(|(n, e)| {
            let inst = Instance::from_entry(e);
            let schedule = EntrySchedule::of(ctx, &inst);
            let entry_id = &ctx.entries[n].id;

            let mut closure: Vec<String> =
                schedule.steps.iter().map(|i| ctx.instance_id(i)).collect();
            sort_ids(&mut closure, &displays);

            let shared = closure
                .iter()
                .filter_map(|id| {
                    let others: Vec<String> = by_id[id.as_str()]
                        .entries
                        .iter()
                        .filter(|e| *e != entry_id)
                        .cloned()
                        .collect();
                    (!others.is_empty()).then(|| SharedInstance {
                        instance: id.clone(),
                        entries: others,
                    })
                })
                .collect();

            EntryInfo {
                id: entry_id.clone(),
                display: ctx.entries[n].display.clone(),
                index: n,
                instance: ctx.instance_id(&inst),
                instances: closure,
                wave_count: schedule.waves.len(),
                max_wave_width: schedule.waves.iter().map(Vec::len).max().unwrap_or(0),
                buffers: buffers::entry_buffers(ctx, &inst, &schedule, &by_id),
                schedule: schedule.to_model(ctx),
                shared,
            }
        })
        .collect();
    sort_named(&mut out, |e| (&e.display, &e.id));
    out
}

pub(crate) fn units(ctx: &Ctx, instances: &[InstanceInfo]) -> Vec<UnitInfo> {
    let mut reads: HashMap<Ref<ComputeUnit>, Vec<Ref<ComputeUnit>>> = HashMap::new();
    let mut read_by: HashMap<Ref<ComputeUnit>, Vec<Ref<ComputeUnit>>> = HashMap::new();
    for (r, cu) in ctx.program.compute_units.iter() {
        let ComputeUnit::DensityFunction { density_inputs, .. } = cu;
        for di in density_inputs.values() {
            reads.entry(r).or_default().push(di.density_function);
            read_by.entry(di.density_function).or_default().push(r);
        }
    }
    let unit_displays: HashMap<String, String> = ctx
        .units
        .values()
        .map(|n| (n.id.clone(), n.display.clone()))
        .collect();
    let ids_of = |cus: Option<&Vec<Ref<ComputeUnit>>>| {
        let mut ids: Vec<String> = cus.into_iter().flatten().map(|c| ctx.unit_id(*c)).collect();
        sort_ids(&mut ids, &unit_displays);
        ids
    };

    let mut depth = HashMap::new();
    let verdicts = inline::verdicts(ctx);
    let instance_displays = instance_displays(ctx);
    let entry_displays = entry_displays(ctx);

    let mut out: Vec<UnitInfo> = ctx
        .program
        .compute_units
        .iter()
        .map(|(r, cu)| {
            let ComputeUnit::DensityFunction {
                function,
                density_inputs,
                permutation_table_inputs,
                host_inputs,
                helper_functions,
                constants,
                source_hash,
            } = cu;
            let id = ctx.unit_id(r);

            let mut unit_instances: Vec<String> = ctx
                .interface
                .instances_of(r)
                .iter()
                .map(|i| ctx.instance_id(i))
                .collect();
            sort_ids(&mut unit_instances, &instance_displays);

            let mut unit_entries: Vec<String> = instances
                .iter()
                .filter(|i| i.unit == id)
                .flat_map(|i| i.entries.iter().cloned())
                .collect();
            sort_ids(&mut unit_entries, &entry_displays);

            let unit_reads = ids_of(reads.get(&r));
            let unit_read_by = ids_of(read_by.get(&r));

            let mut tables: Vec<String> = permutation_table_inputs
                .iter()
                .map(|t| ctx.tables[t].id.clone())
                .collect();
            sort_ids(&mut tables, &named_displays(&ctx.tables));
            let mut hosts: Vec<String> = host_inputs
                .iter()
                .map(|h| ctx.host_inputs[h].id.clone())
                .collect();
            sort_ids(&mut hosts, &named_displays(&ctx.host_inputs));
            let mut helpers: Vec<String> = helper_functions
                .iter()
                .map(|f| ctx.function_id(*f))
                .collect();
            sort_ids(&mut helpers, &named_displays(&ctx.functions));
            let mut consts: Vec<String> = constants
                .iter()
                .map(|c| ctx.constants[c].id.clone())
                .collect();
            sort_ids(&mut consts, &named_displays(&ctx.constants));

            UnitInfo {
                display: ctx.units[&r].display.clone(),
                function: ctx.function_id(*function),
                return_type: format!("{:?}", ctx.return_type(r)),
                source_hash: *source_hash,
                reachable: ctx.unit_reachable(r),
                instances: unit_instances,
                entries: unit_entries,
                fan_in: unit_reads.len(),
                fan_out: unit_read_by.len(),
                reads: unit_reads,
                read_by: unit_read_by,
                depth: unit_depth(r, &reads, &mut depth, &mut HashSet::new()),
                inputs: density_inputs
                    .iter()
                    .map(|(ir, di)| InputInfo {
                        name: ctx.inputs[&ir].name.clone(),
                        producer: ctx.unit_id(di.density_function),
                        grid: grid_of_input(di),
                        indexed: ctx.indexed_inputs.contains(&ir),
                    })
                    .collect(),
                tables,
                host_inputs: hosts,
                helpers,
                constants: consts,
                inline_verdict: verdicts[&r].clone(),
                id,
            }
        })
        .collect();
    sort_named(&mut out, |u| (&u.display, &u.id));
    out
}

/// Longest chain of unit edges down to a leaf. A cycle (invalid input) counts as depth 0 where
/// it closes, so this terminates.
fn unit_depth(
    cu: Ref<ComputeUnit>,
    reads: &HashMap<Ref<ComputeUnit>, Vec<Ref<ComputeUnit>>>,
    memo: &mut HashMap<Ref<ComputeUnit>, usize>,
    visiting: &mut HashSet<Ref<ComputeUnit>>,
) -> usize {
    if let Some(d) = memo.get(&cu) {
        return *d;
    }
    if !visiting.insert(cu) {
        return 0;
    }
    let d = reads
        .get(&cu)
        .into_iter()
        .flatten()
        .map(|r| 1 + unit_depth(*r, reads, memo, visiting))
        .max()
        .unwrap_or(0);
    visiting.remove(&cu);
    memo.insert(cu, d);
    d
}

/// Each entry's transitive closure, by entry index.
fn closures(ctx: &Ctx) -> Vec<HashSet<Instance>> {
    ctx.program
        .entry_points
        .iter()
        .map(|e| {
            ctx.interface
                .waves_for(&Instance::from_entry(e))
                .into_iter()
                .flatten()
                .collect()
        })
        .collect()
}

fn instance_displays(ctx: &Ctx) -> HashMap<String, String> {
    named_displays(&ctx.instances)
}

fn entry_displays(ctx: &Ctx) -> HashMap<String, String> {
    ctx.entries
        .iter()
        .map(|n| (n.id.clone(), n.display.clone()))
        .collect()
}

/// Id → display, for sorting id lists.
pub(crate) fn named_displays<K>(named: &HashMap<K, super::Named>) -> HashMap<String, String> {
    named
        .values()
        .map(|n| (n.id.clone(), n.display.clone()))
        .collect()
}
