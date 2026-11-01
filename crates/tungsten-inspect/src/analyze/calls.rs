//! Functions, the helper call graph and the resource graph (SPEC.md §6.7).

use std::collections::{BTreeMap, HashMap, HashSet};

use tungsten_ir::model::{
    ComputeUnit, Constant, Expression, Function, HostInput, PermutationTableInput, Ref,
};

use super::graph::named_displays;
use super::{Ctx, const_refs, dataflow, ir_text, sort_ids, sort_named, visit_exprs};
use crate::model::{
    CallEdge, CallGraph, ConstantDependency, ConstantInfo, FunctionInfo, FunctionKind,
    HostInputInfo, Parameter, ResourceGraph, ResourceUse, TableInfo,
};

pub(crate) fn functions(ctx: &Ctx) -> Vec<FunctionInfo> {
    let unit_displays = named_displays(&ctx.units);
    let fn_displays = named_displays(&ctx.functions);
    let host_displays = named_displays(&ctx.host_inputs);

    let mut out: Vec<FunctionInfo> = ctx
        .program
        .functions
        .iter()
        .map(|(r, f)| {
            let mut units: Vec<String> = ctx
                .owners(r)
                .into_iter()
                .map(|cu| ctx.unit_id(cu))
                .collect();
            sort_ids(&mut units, &unit_displays);
            let mut host_inputs: Vec<String> = f
                .host_inputs
                .iter()
                .map(|h| ctx.host_inputs[h].id.clone())
                .collect();
            sort_ids(&mut host_inputs, &host_displays);
            let mut calls: Vec<String> = calls_of(f).keys().map(|c| ctx.function_id(*c)).collect();
            sort_ids(&mut calls, &fn_displays);

            let named = &ctx.functions[&r];
            FunctionInfo {
                id: named.id.clone(),
                display: named.display.clone(),
                kind: if ctx.main_of.contains_key(&r) {
                    FunctionKind::Main
                } else {
                    FunctionKind::Helper
                },
                canonical_name: f.canonical_name.clone(),
                units,
                reachable: ctx.reachable_functions.contains(&r),
                parameters: f
                    .parameters
                    .iter()
                    .map(|p| Parameter {
                        name: ctx.var_names[&r][p].clone(),
                        ty: format!("{:?}", f.variables[*p].t),
                    })
                    .collect(),
                return_type: format!("{:?}", f.return_type),
                host_inputs,
                calls,
                ir: ir_text::function(ctx, r),
                dag: ctx.options.dataflow.then(|| dataflow::build(ctx, r)),
            }
        })
        .collect();
    sort_named(&mut out, |f| (&f.display, &f.id));
    out
}

/// Call sites per callee. Keyed by ref, so only use for lookups and counts.
fn calls_of(f: &Function) -> HashMap<Ref<Function>, usize> {
    let mut calls = HashMap::new();
    visit_exprs(&f.body, &mut |e| {
        if let Expression::FunctionCall { function, .. } = e {
            *calls.entry(*function).or_default() += 1;
        }
    });
    calls
}

pub(crate) fn call_graph(ctx: &Ctx) -> CallGraph {
    let mut edges: Vec<(String, String, CallEdge)> = Vec::new();
    for (r, f) in ctx.program.functions.iter() {
        for (callee, count) in calls_of(f) {
            edges.push((
                ctx.functions[&r].display.clone(),
                ctx.functions[&callee].display.clone(),
                CallEdge {
                    from: ctx.function_id(r),
                    to: ctx.function_id(callee),
                    count,
                },
            ));
        }
    }
    edges.sort_by(|a, b| (&a.0, &a.1, &a.2.from, &a.2.to).cmp(&(&b.0, &b.1, &b.2.from, &b.2.to)));
    CallGraph {
        edges: edges.into_iter().map(|(_, _, e)| e).collect(),
    }
}

pub(crate) fn resources(
    ctx: &Ctx,
) -> (
    Vec<ConstantInfo>,
    Vec<TableInfo>,
    Vec<HostInputInfo>,
    ResourceGraph,
) {
    // Declared uses per unit, plus helpers' (and main functions') host inputs attributed to
    // their units.
    let mut const_users: HashMap<Ref<Constant>, Vec<Ref<ComputeUnit>>> = HashMap::new();
    let mut table_users: HashMap<Ref<PermutationTableInput>, Vec<Ref<ComputeUnit>>> =
        HashMap::new();
    let mut host_users: HashMap<Ref<HostInput>, Vec<Ref<ComputeUnit>>> = HashMap::new();
    for (r, cu) in ctx.program.compute_units.iter() {
        let ComputeUnit::DensityFunction {
            permutation_table_inputs,
            host_inputs,
            constants,
            ..
        } = cu;
        constants
            .iter()
            .for_each(|c| const_users.entry(*c).or_default().push(r));
        permutation_table_inputs
            .iter()
            .for_each(|t| table_users.entry(*t).or_default().push(r));
        host_inputs
            .iter()
            .for_each(|h| host_users.entry(*h).or_default().push(r));
    }
    for (r, f) in ctx.program.functions.iter() {
        for owner in ctx.owners(r) {
            f.host_inputs
                .iter()
                .for_each(|h| host_users.entry(*h).or_default().push(owner));
        }
    }

    let reachable = Reachable::find(ctx);
    let unit_displays = named_displays(&ctx.units);
    let users_of = |cus: Option<&Vec<Ref<ComputeUnit>>>| {
        let mut ids: Vec<String> = cus.into_iter().flatten().map(|c| ctx.unit_id(*c)).collect();
        sort_ids(&mut ids, &unit_displays);
        ids
    };

    let const_displays = named_displays(&ctx.constants);
    let mut constants: Vec<ConstantInfo> = ctx
        .program
        .constants
        .iter()
        .map(|(r, c)| {
            let mut deps = Vec::new();
            const_refs(&c.value, &mut deps);
            let mut depends_on: Vec<String> =
                deps.iter().map(|d| ctx.constants[d].id.clone()).collect();
            sort_ids(&mut depends_on, &const_displays);
            ConstantInfo {
                id: ctx.constants[&r].id.clone(),
                display: ctx.constants[&r].display.clone(),
                reachable: reachable.constants.contains(&r),
                value: ir_text::const_expr(ctx, &c.value),
                depends_on,
                users: users_of(const_users.get(&r)),
            }
        })
        .collect();
    sort_named(&mut constants, |c| (&c.display, &c.id));

    let mut tables: Vec<TableInfo> = ctx
        .program
        .permutation_tables
        .iter()
        .map(|(r, _)| TableInfo {
            id: ctx.tables[&r].id.clone(),
            display: ctx.tables[&r].display.clone(),
            reachable: reachable.tables.contains(&r),
            users: users_of(table_users.get(&r)),
        })
        .collect();
    sort_named(&mut tables, |t| (&t.display, &t.id));

    let mut host_inputs: Vec<HostInputInfo> = ctx
        .program
        .host_inputs
        .iter()
        .map(|(r, h)| HostInputInfo {
            id: ctx.host_inputs[&r].id.clone(),
            display: ctx.host_inputs[&r].display.clone(),
            ty: format!("{:?}", h.get_type()),
            reachable: reachable.host_inputs.contains(&r),
            users: users_of(host_users.get(&r)),
        })
        .collect();
    sort_named(&mut host_inputs, |h| (&h.display, &h.id));

    // Resource uses, keyed by (unit display, resource display, unit id, resource id) for order.
    let mut uses: BTreeMap<(String, String, String, String), ResourceUse> = BTreeMap::new();
    let mut add = |cu: Ref<ComputeUnit>, named: &super::Named| {
        let unit = &ctx.units[&cu];
        uses.insert(
            (
                unit.display.clone(),
                named.display.clone(),
                unit.id.clone(),
                named.id.clone(),
            ),
            ResourceUse {
                unit: unit.id.clone(),
                resource: named.id.clone(),
            },
        );
    };
    for (c, cus) in &const_users {
        cus.iter().for_each(|cu| add(*cu, &ctx.constants[c]));
    }
    for (t, cus) in &table_users {
        cus.iter().for_each(|cu| add(*cu, &ctx.tables[t]));
    }
    for (h, cus) in &host_users {
        cus.iter().for_each(|cu| add(*cu, &ctx.host_inputs[h]));
    }

    let mut constant_dependencies: Vec<(String, String, ConstantDependency)> = Vec::new();
    for (r, c) in ctx.program.constants.iter() {
        let mut deps = Vec::new();
        const_refs(&c.value, &mut deps);
        deps.dedup();
        for d in deps {
            let (from, to) = (&ctx.constants[&r], &ctx.constants[&d]);
            constant_dependencies.push((
                from.display.clone(),
                to.display.clone(),
                ConstantDependency {
                    constant: from.id.clone(),
                    depends_on: to.id.clone(),
                },
            ));
        }
    }
    constant_dependencies.sort_by(|a, b| {
        (&a.0, &a.1, &a.2.constant, &a.2.depends_on).cmp(&(
            &b.0,
            &b.1,
            &b.2.constant,
            &b.2.depends_on,
        ))
    });
    constant_dependencies.dedup_by(|a, b| a.2 == b.2);

    let graph = ResourceGraph {
        uses: uses.into_values().collect(),
        constant_dependencies: constant_dependencies
            .into_iter()
            .map(|(_, _, d)| d)
            .collect(),
    };
    (constants, tables, host_inputs, graph)
}

/// Resources used by scheduled units (declared) or by reachable functions (referenced), with
/// constants closed under their dependencies.
struct Reachable {
    constants: HashSet<Ref<Constant>>,
    tables: HashSet<Ref<PermutationTableInput>>,
    host_inputs: HashSet<Ref<HostInput>>,
}

impl Reachable {
    fn find(ctx: &Ctx) -> Self {
        let mut r = Self {
            constants: HashSet::new(),
            tables: HashSet::new(),
            host_inputs: HashSet::new(),
        };
        for (cu_ref, cu) in ctx.program.compute_units.iter() {
            if !ctx.unit_reachable(cu_ref) {
                continue;
            }
            let ComputeUnit::DensityFunction {
                permutation_table_inputs,
                host_inputs,
                constants,
                ..
            } = cu;
            r.constants.extend(constants);
            r.tables.extend(permutation_table_inputs);
            r.host_inputs.extend(host_inputs);
        }
        for f in &ctx.reachable_functions {
            let func = &ctx.program.functions[*f];
            r.host_inputs.extend(&func.host_inputs);
            visit_exprs(&func.body, &mut |e| match e {
                Expression::Constant(c) => {
                    r.constants.insert(*c);
                }
                Expression::PermutationTable(t) => {
                    r.tables.insert(*t);
                }
                Expression::HostInput(h) => {
                    r.host_inputs.insert(*h);
                }
                _ => {}
            });
        }
        let mut stack: Vec<Ref<Constant>> = r.constants.iter().copied().collect();
        while let Some(c) = stack.pop() {
            let mut deps = Vec::new();
            const_refs(&ctx.program.constants[c].value, &mut deps);
            for d in deps {
                if r.constants.insert(d) {
                    stack.push(d);
                }
            }
        }
        r
    }
}
