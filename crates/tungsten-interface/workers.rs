//! What each worker computes and where it keeps it (`specs/workers.md` §4).

use std::collections::{HashMap, HashSet};

use tungsten_ir::model::{
    ComputeUnit, DensityInput, Expression, Function, HostInput, OptimizedProgram, Ref, Statement,
    VariableType,
};

use crate::{Instance, Interface};

/// One worker's buffers: one contiguous array per IR element type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLayout {
    /// In first-use order, the entry's pool first.
    pub pools: Vec<Pool>,
    /// The worker's instances, in `Interface::instances` order.
    pub slots: Vec<(Instance, Slot)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pool {
    pub element: VariableType,
    /// In elements.
    pub len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub pool: usize,
    pub offset: usize,
    pub len: usize,
}

/// Per-entry and per-function facts, computed once in `Interface::build`.
#[derive(Default)]
pub(crate) struct Workers {
    /// For each function: the inputs it reads without an index, directly or through helpers.
    pub unindexed_reads: HashMap<Ref<Function>, HashSet<Ref<DensityInput>>>,
    pub schedules: Vec<Vec<Vec<Instance>>>,
    pub layouts: Vec<WorkerLayout>,
    pub input_params: HashMap<Ref<Function>, Vec<Ref<DensityInput>>>,
    pub entry_hosts: Vec<Vec<Ref<HostInput>>>,
}

impl Workers {
    pub fn build(program: &OptimizedProgram, interface: &Interface) -> Self {
        let uses = transitive_uses(program);
        let mut workers = Workers {
            unindexed_reads: uses.iter().map(|(f, u)| (*f, u.unindexed.clone())).collect(),
            input_params: uses
                .iter()
                .map(|(f, u)| (*f, in_arena_order(program.density_inputs.iter(), &u.inputs)))
                .collect(),
            ..Default::default()
        };
        for entry in &program.entry_points {
            let target = Instance::from_entry(entry);
            let schedule = interface.waves_for(&target);
            workers.layouts.push(layout(program, interface.instances(), &schedule, target));
            workers.entry_hosts.push(entry_hosts(program, &schedule, &uses));
            workers.schedules.push(schedule);
        }
        workers
    }
}

fn in_arena_order<'a, T: 'a>(
    arena: impl Iterator<Item = (Ref<T>, &'a T)>,
    wanted: &HashSet<Ref<T>>,
) -> Vec<Ref<T>> {
    arena.map(|(r, _)| r).filter(|r| wanted.contains(r)).collect()
}

fn main_function(program: &OptimizedProgram, cu: Ref<ComputeUnit>) -> Ref<Function> {
    let ComputeUnit::DensityFunction { function, .. } = &program.compute_units[cu];
    *function
}

/// Cells of a grid. Dims below 1 occur on the unchecked path, and count as empty.
fn cells((x, y, z): (i32, i32, i32)) -> usize {
    [x, y, z]
        .into_iter()
        .fold(1, |n: usize, d| n.saturating_mul(d.max(0) as usize))
}

/// The entry's output first, at offset 0 of pool 0, then the others in `instances` order.
fn layout(
    program: &OptimizedProgram,
    instances: &[Instance],
    schedule: &[Vec<Instance>],
    target: Instance,
) -> WorkerLayout {
    let members: HashSet<Instance> = schedule.iter().flatten().copied().collect();
    let mut pools: Vec<Pool> = vec![];
    let mut place = |inst: Instance| {
        let element = &program.functions[main_function(program, inst.cu)].return_type;
        let pool = pools.iter().position(|p| p.element == *element).unwrap_or_else(|| {
            pools.push(Pool { element: element.clone(), len: 0 });
            pools.len() - 1
        });
        let len = cells(inst.dimensions);
        let offset = pools[pool].len;
        pools[pool].len = offset.saturating_add(len);
        Slot { pool, offset, len }
    };
    let entry_slot = place(target);
    let slots = instances
        .iter()
        .filter(|i| members.contains(i))
        .map(|&i| (i, if i == target { entry_slot } else { place(i) }))
        .collect();
    WorkerLayout { pools, slots }
}

fn entry_hosts(
    program: &OptimizedProgram,
    schedule: &[Vec<Instance>],
    uses: &HashMap<Ref<Function>, Uses>,
) -> Vec<Ref<HostInput>> {
    let mut read = HashSet::new();
    for inst in schedule.iter().flatten() {
        if let Some(u) = uses.get(&main_function(program, inst.cu)) {
            read.extend(u.hosts.iter().copied());
        }
    }
    let statics = program
        .host_inputs
        .iter()
        .filter(|(_, h)| matches!(h, HostInput::Static { .. }));
    in_arena_order(statics, &read)
}

/// What a function's body reads.
#[derive(Default)]
struct Uses {
    inputs: HashSet<Ref<DensityInput>>,
    /// The inputs among `inputs` that some read takes without an explicit index.
    unindexed: HashSet<Ref<DensityInput>>,
    hosts: HashSet<Ref<HostInput>>,
}

/// Each function's own reads plus those of everything it calls. Tolerates call cycles and stale
/// callees, which only the checker rejects.
fn transitive_uses(program: &OptimizedProgram) -> HashMap<Ref<Function>, Uses> {
    let direct: HashMap<Ref<Function>, (Uses, HashSet<Ref<Function>>)> =
        program.functions.iter().map(|(r, f)| (r, direct_uses(f))).collect();
    direct
        .keys()
        .map(|&start| {
            let mut total = Uses::default();
            let mut seen = HashSet::from([start]);
            let mut stack = vec![start];
            while let Some(f) = stack.pop() {
                let Some((uses, calls)) = direct.get(&f) else { continue };
                total.inputs.extend(uses.inputs.iter().copied());
                total.unindexed.extend(uses.unindexed.iter().copied());
                total.hosts.extend(uses.hosts.iter().copied());
                stack.extend(calls.iter().copied().filter(|c| seen.insert(*c)));
            }
            (start, total)
        })
        .collect()
}

/// Uses an explicit stack: expressions can nest deeper than the thread's stack allows.
fn direct_uses(f: &Function) -> (Uses, HashSet<Ref<Function>>) {
    let mut uses = Uses::default();
    let mut calls = HashSet::new();
    let mut stmts: Vec<&Statement> = f.body.iter().collect();
    let mut exprs: Vec<&Expression> = vec![];
    while let Some(s) = stmts.pop() {
        match s {
            Statement::Assign { value: e, .. } | Statement::Return(e) => exprs.push(e),
            Statement::If { condition, then_branch, else_branch } => {
                exprs.push(condition);
                stmts.extend(then_branch.iter().chain(else_branch));
            }
            Statement::While { condition, body } => {
                exprs.push(condition);
                stmts.extend(body);
            }
            Statement::Repeat { body, .. } => stmts.extend(body),
            Statement::Break => {}
        }
    }
    while let Some(e) = exprs.pop() {
        match e {
            Expression::DensityVariable(r, index) => {
                uses.inputs.insert(*r);
                if index.is_none() {
                    uses.unindexed.insert(*r);
                }
                exprs.extend(index.as_deref());
            }
            Expression::HostInput(r) => {
                uses.hosts.insert(*r);
            }
            Expression::FunctionCall { function, parameters } => {
                calls.insert(*function);
                exprs.extend(parameters);
            }
            Expression::ExternCall { parameters: args, .. }
            | Expression::Construct { args, .. }
            | Expression::ArrayLiteral(args) => exprs.extend(args),
            Expression::ConstructExtern { args, .. } => exprs.extend(args.iter().map(|(_, a)| a)),
            Expression::BinaryOp { left, right, .. } => exprs.extend([&**left, &**right]),
            Expression::ArrayAccess { array, index } => exprs.extend([&**array, &**index]),
            Expression::UnaryOp { operand: x, .. }
            | Expression::Field { base: x, .. }
            | Expression::ExplicitCast { expr: x, .. } => exprs.push(x),
            Expression::Constant(_)
            | Expression::Variable(_)
            | Expression::Float(_)
            | Expression::Double(_)
            | Expression::Byte(_)
            | Expression::Int(_)
            | Expression::Long(_)
            | Expression::Bool(_)
            | Expression::AutoConst(_)
            | Expression::Ambient(_) => {}
        }
    }
    (uses, calls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::NamingRules;
    use tungsten_ir::model::{Arena, AxisFactor, DensityMapping, EntryPoint, Name};

    fn empty() -> OptimizedProgram {
        OptimizedProgram {
            compute_units: Arena::new(),
            functions: Arena::new(),
            entry_points: vec![],
            refinements: vec![],
            constants: Arena::new(),
            host_inputs: Arena::new(),
            auto_params: Arena::new(),
            auto_consts: Arena::new(),
            density_inputs: Arena::new(),
        }
    }

    fn function(p: &mut OptimizedProgram, t: VariableType, body: Vec<Statement>) -> Ref<Function> {
        p.functions.insert(Function {
            canonical_name: None,
            host_inputs: vec![],
            body,
            variables: Arena::new(),
            parameters: vec![],
            return_type: t,
            source_hash: 0,
        })
    }

    fn unit(p: &mut OptimizedProgram, f: Ref<Function>, inputs: Vec<Ref<DensityInput>>) -> Ref<ComputeUnit> {
        p.compute_units.insert(ComputeUnit::DensityFunction {
            function: f,
            density_inputs: inputs,
            auto_consts: vec![],
            host_inputs: vec![],
            helper_functions: vec![],
            constants: vec![],
            source_hash: 0,
        })
    }

    fn input(p: &mut OptimizedProgram, cu: Ref<ComputeUnit>, dims: (i32, i32, i32)) -> Ref<DensityInput> {
        p.density_inputs.insert(DensityInput {
            name: Name::Anonymous,
            density_function: cu,
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (1.0, 1.0, 1.0),
            dimensions: dims,
            mapping: DensityMapping::Identity,
            previous: false,
        })
    }

    fn entry(p: &mut OptimizedProgram, cu: Ref<ComputeUnit>, dims: (i32, i32, i32)) {
        p.entry_points.push(EntryPoint {
            density_function: cu,
            dimensions: dims,
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (1.0, 1.0, 1.0),
        });
    }

    fn read(d: Ref<DensityInput>) -> Expression {
        Expression::DensityVariable(d, None)
    }

    fn ret(e: Expression) -> Vec<Statement> {
        vec![Statement::Return(e)]
    }

    fn build(p: &OptimizedProgram) -> Interface {
        Interface::build(p, &NamingRules::rust())
    }

    fn inst(cu: Ref<ComputeUnit>, dims: (i32, i32, i32)) -> Instance {
        Instance::new(cu, dims, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0))
    }

    /// Entry 0 (`first`, f64) reads `leaf` (i32). Entry 1 (`second`, i32) reads `other` (f64) and
    /// `leaf`, so its own instance is discovered after instances of entry 0.
    struct Two {
        p: OptimizedProgram,
        first: Ref<ComputeUnit>,
        second: Ref<ComputeUnit>,
        leaf: Ref<ComputeUnit>,
        other: Ref<ComputeUnit>,
    }

    fn two() -> Two {
        use VariableType::{F64, I32};
        let mut p = empty();
        let f = function(&mut p, I32, ret(Expression::Int(1)));
        let leaf = unit(&mut p, f, vec![]);
        let f = function(&mut p, F64, ret(Expression::Double(1.0)));
        let other = unit(&mut p, f, vec![]);
        let leaf_in = input(&mut p, leaf, (2, 1, 1));
        let other_in = input(&mut p, other, (3, 1, 1));
        let f = function(&mut p, F64, ret(read(leaf_in)));
        let first = unit(&mut p, f, vec![leaf_in]);
        let f = function(&mut p, I32, ret(read(other_in)));
        let second = unit(&mut p, f, vec![other_in, leaf_in]);
        entry(&mut p, first, (4, 1, 1));
        entry(&mut p, second, (5, 1, 1));
        Two { p, first, second, leaf, other }
    }

    #[test]
    fn entry_output_is_first_in_pool_zero() {
        let t = two();
        let i = build(&t.p);
        let first = inst(t.first, (4, 1, 1));
        let second = inst(t.second, (5, 1, 1));
        let leaf = inst(t.leaf, (2, 1, 1));
        let other = inst(t.other, (3, 1, 1));

        let w = i.worker(0);
        assert_eq!(w.pools[0], Pool { element: VariableType::F64, len: 4 });
        assert_eq!(w.pools[1], Pool { element: VariableType::I32, len: 2 });
        assert_eq!(i.slot(0, &first), Some(Slot { pool: 0, offset: 0, len: 4 }));
        assert_eq!(i.slot(0, &leaf), Some(Slot { pool: 1, offset: 0, len: 2 }));

        // Entry 1 comes after entry 0's instances in `instances()`, and still gets offset 0.
        let w = i.worker(1);
        assert_eq!(w.pools[0], Pool { element: VariableType::I32, len: 7 });
        assert_eq!(w.pools[1], Pool { element: VariableType::F64, len: 3 });
        assert_eq!(i.slot(1, &second), Some(Slot { pool: 0, offset: 0, len: 5 }));
        assert_eq!(i.slot(1, &leaf), Some(Slot { pool: 0, offset: 5, len: 2 }));
        assert_eq!(i.slot(1, &other), Some(Slot { pool: 1, offset: 0, len: 3 }));

        // `slots` keeps `instances()` order.
        let order: Vec<Instance> = i.instances().iter().filter(|x| i.slot(1, x).is_some()).copied().collect();
        assert_eq!(w.slots.iter().map(|(x, _)| *x).collect::<Vec<_>>(), order);
    }

    #[test]
    fn only_the_closure_gets_slots() {
        let t = two();
        let i = build(&t.p);
        assert_eq!(i.slot(0, &inst(t.other, (3, 1, 1))), None);
        assert_eq!(i.slot(0, &inst(t.second, (5, 1, 1))), None);
        assert_eq!(i.worker(0).slots.len(), 2);
        assert_eq!(i.worker(1).slots.len(), 3);
        let scheduled: usize = i.worker_schedule(0).iter().map(Vec::len).sum();
        assert_eq!(scheduled, 2);
        assert_eq!(i.worker_schedule(0), i.waves_for(&inst(t.first, (4, 1, 1))));
    }

    #[test]
    fn building_twice_gives_the_same_layouts() {
        let t = two();
        let (a, b) = (build(&t.p), build(&t.p));
        for e in 0..2 {
            assert_eq!(a.worker(e), b.worker(e));
            assert_eq!(a.worker_schedule(e), b.worker_schedule(e));
            assert_eq!(a.entry_hosts(e), b.entry_hosts(e));
        }
    }

    #[test]
    fn bad_dims_do_not_panic() {
        let mut p = empty();
        let f = function(&mut p, VariableType::F64, ret(Expression::Double(1.0)));
        let u = unit(&mut p, f, vec![]);
        entry(&mut p, u, (-1, 4, 16));
        entry(&mut p, u, (i32::MAX, i32::MAX, i32::MAX));
        let i = build(&p);
        assert_eq!(i.worker(0).pools[0].len, 0);
        assert_eq!(i.worker(1).pools[0].len, usize::MAX, "saturates instead of overflowing");
    }

    #[test]
    fn input_params_are_transitive_in_arena_order_and_deduplicated() {
        use VariableType::F64;
        let mut p = empty();
        let f = function(&mut p, F64, ret(Expression::Double(1.0)));
        let leaf = unit(&mut p, f, vec![]);
        let a = input(&mut p, leaf, (1, 1, 1));
        let b = input(&mut p, leaf, (2, 1, 1));
        let c = input(&mut p, leaf, (3, 1, 1));
        let unread = input(&mut p, leaf, (4, 1, 1));

        // `inner` reads `a`, nested in the index expression of a read of `c`.
        let index = Expression::ExplicitCast { to: VariableType::I32, expr: Box::new(read(a)) };
        let inner = function(&mut p, F64, ret(Expression::DensityVariable(c, Some(Box::new(index)))));
        // `outer` reads `b` in a loop, and `c` again through `inner`.
        let call = Expression::FunctionCall { function: inner, parameters: vec![] };
        let outer = function(
            &mut p,
            F64,
            vec![
                Statement::Repeat { count: 2, body: ret(read(b)) },
                Statement::Return(Expression::BinaryOp {
                    op: tungsten_ir::model::BinaryOperator::Add,
                    left: Box::new(call),
                    right: Box::new(read(c)),
                }),
            ],
        );
        let i = build(&p);
        assert_eq!(i.input_params(inner), [a, c]);
        assert_eq!(i.input_params(outer), [a, b, c]);
        assert!(!i.input_params(outer).contains(&unread));
        assert_eq!(i.input_params(f), []);
    }

    #[test]
    fn helper_call_cycles_terminate() {
        use VariableType::F64;
        let mut p = empty();
        let f = function(&mut p, F64, ret(Expression::Double(1.0)));
        let leaf = unit(&mut p, f, vec![]);
        let d = input(&mut p, leaf, (1, 1, 1));
        let a = function(&mut p, F64, ret(read(d)));
        let b = function(&mut p, F64, ret(Expression::FunctionCall { function: a, parameters: vec![] }));
        p.functions[a].body = ret(Expression::FunctionCall { function: b, parameters: vec![] });
        let i = build(&p);
        assert_eq!(i.input_params(a), []);
        p.functions[a].body.push(Statement::Return(read(d)));
        let i = build(&p);
        assert_eq!(i.input_params(a), [d]);
        assert_eq!(i.input_params(b), [d]);
    }

    fn host(p: &mut OptimizedProgram, name: &str) -> Ref<HostInput> {
        p.host_inputs.insert(HostInput::Static {
            name: Name::Named(name.into()),
            t: VariableType::F64,
        })
    }

    /// Hosts come from the worker's closure, through helpers, in arena order. Entry 0 reads `b`
    /// directly and `a` through an input, entry 1 reads `c` through a helper.
    #[test]
    fn entry_hosts_follow_the_closure() {
        use VariableType::F64;
        let mut p = empty();
        let [a, b, c] = ["a", "b", "c"].map(|n| host(&mut p, n));
        let unused = host(&mut p, "unused");
        let dynamic = p.host_inputs.insert(HostInput::Dynamic);

        let f = function(&mut p, F64, ret(Expression::HostInput(a)));
        let leaf = unit(&mut p, f, vec![]);
        let d = input(&mut p, leaf, (1, 1, 1));
        let body = vec![
            Statement::Assign { target: dangling_var(), value: read(d) },
            Statement::Return(Expression::HostInput(b)),
        ];
        let f = function(&mut p, F64, body);
        let top = unit(&mut p, f, vec![d]);
        entry(&mut p, top, (1, 1, 1));

        let helper = function(&mut p, F64, ret(Expression::HostInput(c)));
        let dyn_read = function(&mut p, F64, ret(Expression::HostInput(dynamic)));
        let f = function(
            &mut p,
            F64,
            vec![Statement::If {
                condition: Expression::Bool(true),
                then_branch: ret(Expression::FunctionCall { function: helper, parameters: vec![] }),
                else_branch: ret(Expression::FunctionCall { function: dyn_read, parameters: vec![] }),
            }],
        );
        let other = unit(&mut p, f, vec![]);
        entry(&mut p, other, (1, 1, 1));

        let i = build(&p);
        assert_eq!(i.entry_hosts(0), [a, b]);
        assert_eq!(i.entry_hosts(1), [c]);
        assert!(!i.entry_hosts(0).contains(&unused));
    }

    /// Assign targets are never followed, so any ref will do.
    fn dangling_var() -> Ref<tungsten_ir::model::Var> {
        Arena::new().insert(tungsten_ir::model::Var {
            name: Name::Anonymous,
            t: VariableType::F64,
        })
    }

    /// One name per input, program-wide: `off_<name>`, unique, and legal for anonymous inputs.
    #[test]
    fn offset_params_are_prefixed_and_unique() {
        let mut p = empty();
        let f = function(&mut p, VariableType::F64, ret(Expression::Double(1.0)));
        let leaf = unit(&mut p, f, vec![]);
        let [a, b, anon, anon2] = [0, 1, 2, 3].map(|_| input(&mut p, leaf, (1, 1, 1)));
        p.density_inputs[a].name = Name::Named("x:y".into());
        p.density_inputs[b].name = Name::Prefixed("x_y".into());
        let i = build(&p);
        assert_eq!(i.offset_param(a), "off_x_y");
        assert_eq!(i.offset_param(b), "off_x_y_1");
        assert_eq!(i.offset_param(anon), "off_input");
        assert_eq!(i.offset_param(anon2), "off_input_1");
    }

    fn lerp_input(
        p: &mut OptimizedProgram,
        cu: Ref<ComputeUnit>,
        dims: (i32, i32, i32),
        factors: [AxisFactor; 3],
    ) -> Ref<DensityInput> {
        let d = input(p, cu, dims);
        p.density_inputs[d].mapping = DensityMapping::Lerp(factors);
        d
    }

    /// A unit of element type `t` with no inputs, and a unit reading `reads` (listed, un-indexed)
    /// that is the only entry.
    fn lerp_program(
        t: VariableType,
        shapes: &[((i32, i32, i32), [AxisFactor; 3])],
    ) -> (OptimizedProgram, Vec<Ref<DensityInput>>) {
        let mut p = empty();
        let f = function(&mut p, t, ret(Expression::Double(1.0)));
        let leaf = unit(&mut p, f, vec![]);
        let inputs: Vec<_> = shapes.iter().map(|(dims, fs)| lerp_input(&mut p, leaf, *dims, *fs)).collect();
        let body = inputs.iter().map(|d| Statement::Return(read(*d))).collect();
        let f = function(&mut p, VariableType::F64, body);
        let top = unit(&mut p, f, inputs.clone());
        entry(&mut p, top, (1, 1, 1));
        (p, inputs)
    }

    fn names(i: &Interface) -> Vec<&str> {
        i.lerp_helpers().iter().map(|h| h.name.as_str()).collect()
    }

    /// `specs/density-mappings.md` §5.2: names carry dims, factors and element; `r` is `Repeat`.
    #[test]
    fn lerp_helpers_are_named_by_shape() {
        use AxisFactor::{Factor, Repeat};
        let (p, inputs) = lerp_program(
            VariableType::F64,
            &[((5, 49, 5), [Factor(4), Factor(8), Factor(4)]), ((5, 1, 5), [Factor(4), Repeat, Factor(4)])],
        );
        let i = build(&p);
        assert_eq!(names(&i), ["lerp_5x49x5_by_4x8x4_f64", "lerp_5x1x5_by_4xrx4_f64"]);
        assert_eq!(i.lerp_helper(&p.density_inputs[inputs[0]]), "lerp_5x49x5_by_4x8x4_f64");
        assert_eq!(i.lerp_helper(&p.density_inputs[inputs[1]]), "lerp_5x1x5_by_4xrx4_f64");
        let h = &i.lerp_helpers()[0];
        assert_eq!((h.dimensions, h.element.clone()), ((5, 49, 5), VariableType::F64));
    }

    /// One helper per (dims, factors, element), whatever the number of inputs and readers.
    #[test]
    fn lerp_helpers_are_shared_by_shape() {
        use AxisFactor::Factor;
        let shape = [Factor(2); 3];
        let (mut p, inputs) = lerp_program(
            VariableType::F64,
            &[((3, 3, 3), shape), ((3, 3, 3), shape), ((3, 3, 3), [Factor(2), Factor(2), Factor(3)])],
        );
        // A second unit lists the first input too.
        let f = function(&mut p, VariableType::F64, ret(read(inputs[0])));
        let second = unit(&mut p, f, vec![inputs[0]]);
        entry(&mut p, second, (1, 1, 1));
        let i = build(&p);
        assert_eq!(names(&i), ["lerp_3x3x3_by_2x2x2_f64", "lerp_3x3x3_by_2x2x3_f64"]);
        for d in &inputs[..2] {
            assert_eq!(i.lerp_helper(&p.density_inputs[*d]), "lerp_3x3x3_by_2x2x2_f64");
        }

        // The same shape over an `f32` unit is another helper.
        let (p, inputs) = lerp_program(VariableType::F32, &[((3, 3, 3), shape)]);
        let i = build(&p);
        assert_eq!(names(&i), ["lerp_3x3x3_by_2x2x2_f32"]);
        assert_eq!(i.lerp_helper(&p.density_inputs[inputs[0]]), "lerp_3x3x3_by_2x2x2_f32");
    }

    /// An input only read with an explicit index needs no helper, one read inside a helper function
    /// does, and an unlisted or unreachable one is never discovered.
    #[test]
    fn lerp_helpers_follow_unindexed_reads() {
        use AxisFactor::Factor;
        let shape = [Factor(2); 3];
        let mut p = empty();
        let f = function(&mut p, VariableType::F64, ret(Expression::Double(1.0)));
        let leaf = unit(&mut p, f, vec![]);
        let indexed = lerp_input(&mut p, leaf, (2, 2, 2), shape);
        let in_helper = lerp_input(&mut p, leaf, (3, 3, 3), shape);
        let unreachable = lerp_input(&mut p, leaf, (4, 4, 4), shape);
        let explicit = Expression::DensityVariable(indexed, Some(Box::new(Expression::Int(0))));
        let helper = function(&mut p, VariableType::F64, ret(read(in_helper)));
        let call = Expression::FunctionCall { function: helper, parameters: vec![] };
        let f = function(
            &mut p,
            VariableType::F64,
            vec![Statement::Assign { target: dangling_var(), value: explicit }, Statement::Return(call)],
        );
        let top = unit(&mut p, f, vec![indexed, in_helper]);
        entry(&mut p, top, (1, 1, 1));
        let f = function(&mut p, VariableType::F64, ret(read(unreachable)));
        unit(&mut p, f, vec![unreachable]);
        let i = build(&p);
        assert_eq!(names(&i), ["lerp_3x3x3_by_2x2x2_f64"]);
    }

    /// What no backend can emit gets no helper and no panic. Backends refuse those programs.
    #[test]
    fn lerp_helpers_skip_what_cannot_be_emitted() {
        use AxisFactor::{Factor, Repeat};
        let (p, _) = lerp_program(VariableType::I32, &[((2, 2, 2), [Factor(2); 3])]);
        assert_eq!(names(&build(&p)), Vec::<&str>::new());
        let (p, _) = lerp_program(VariableType::F64, &[((2, 2, 2), [Factor(0), Repeat, Factor(-3)])]);
        assert_eq!(names(&build(&p)), Vec::<&str>::new());
    }

    /// Helper names share the module namespace with everything else, and stay deterministic.
    #[test]
    fn lerp_helper_names_are_unique_and_deterministic() {
        use AxisFactor::Factor;
        let (mut p, _) = lerp_program(VariableType::F64, &[((2, 2, 2), [Factor(2); 3])]);
        // A helper function holds the name already, so the lerp helper moves aside.
        let taken = function(&mut p, VariableType::F64, vec![]);
        p.functions[taken].canonical_name = Some("lerp_2x2x2_by_2x2x2_f64".into());
        let (a, b) = (build(&p), build(&p));
        assert_eq!(names(&a), names(&b));
        assert_eq!(a.helper(taken), "lerp_2x2x2_by_2x2x2_f64");
        assert_eq!(names(&a), ["lerp_2x2x2_by_2x2x2_f64_1"]);
    }

    /// Record types share a pool exactly when their names are equal.
    #[test]
    fn pools_are_keyed_by_ir_type() {
        let mut p = empty();
        let f = function(&mut p, VariableType::Extern("Pair"), vec![]);
        let b = unit(&mut p, f, vec![]);
        let f = function(&mut p, VariableType::Extern("Other"), vec![]);
        let c = unit(&mut p, f, vec![]);
        let (bi, ci) = (input(&mut p, b, (2, 1, 1)), input(&mut p, c, (2, 1, 1)));
        let f = function(&mut p, VariableType::Extern("Pair"), vec![]);
        let top = unit(&mut p, f, vec![bi, ci]);
        entry(&mut p, top, (1, 1, 1));
        let i = build(&p);
        let names: Vec<_> = i.worker(0).pools.iter().map(|p| (p.element.clone(), p.len)).collect();
        assert_eq!(
            names,
            [(VariableType::Extern("Pair"), 3), (VariableType::Extern("Other"), 2)]
        );
    }
}
