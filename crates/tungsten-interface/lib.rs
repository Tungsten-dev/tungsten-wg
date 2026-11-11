/*
The interface decides how compute units are orchestrated (which instances exist and in which
waves they run) and fixes the names of everything generated code refers to: kernels, buffers,
host inputs and auto consts. Computed once so callers and callees agree.
*/

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use tungsten_ir::model::{AutoConst, AutoParam, AxisFactor,
    ComputeUnit, Constant, DensityInput, DensityMapping, EntryPoint, Function, HostInput, Name,
    OptimizedProgram, Ref, VariableType,
};

pub mod compiler;
mod guard;
pub mod naming;
mod refine;
pub mod scale;
mod workers;

pub use compiler::{CompileError, Compiler, Inline, Passes};
pub use guard::{Feature, check_implemented};
pub use refine::RefineRun;
pub use scale::Scale;
pub use workers::{Pool, Slot, WorkerLayout};

use naming::{CREATION_RESERVED, auto_const_field_name, host_field_name, unique, unit_base_name};

use crate::naming::NamingRules;

/// One scheduled use of a compute unit: its own buffer is keyed by (unit, dimensions, scales).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Instance {
    pub cu: Ref<ComputeUnit>,
    pub dimensions: (i32, i32, i32),
    origin_bits: [u64; 3],
    position_bits: [u64; 3],
}

fn bits(v: (f64, f64, f64)) -> [u64; 3] {
    [v.0.to_bits(), v.1.to_bits(), v.2.to_bits()]
}

fn unbits(b: [u64; 3]) -> (f64, f64, f64) {
    (
        f64::from_bits(b[0]),
        f64::from_bits(b[1]),
        f64::from_bits(b[2]),
    )
}

impl Instance {
    pub fn new(
        cu: Ref<ComputeUnit>,
        dimensions: (i32, i32, i32),
        scaled_origin: (f64, f64, f64),
        scaled_position: (f64, f64, f64),
    ) -> Self {
        Self {
            cu,
            dimensions,
            origin_bits: bits(scaled_origin),
            position_bits: bits(scaled_position),
        }
    }

    pub fn from_entry(e: &EntryPoint) -> Self {
        Self::new(
            e.density_function,
            e.dimensions,
            e.scaled_origin,
            e.scaled_position,
        )
    }

    pub fn from_input(di: &DensityInput) -> Self {
        Self::new(
            di.density_function,
            di.dimensions,
            di.scaled_origin,
            di.scaled_position,
        )
    }

    pub fn scaled_origin(&self) -> (f64, f64, f64) {
        unbits(self.origin_bits)
    }

    pub fn scaled_position(&self) -> (f64, f64, f64) {
        unbits(self.position_bits)
    }
}

/// Generates device code (density functions and kernels) for compute units in one target language.
pub trait DeviceCodegen {
    /// Generates the unit's functions, helpers and constants; repeated calls are no-ops.
    fn generate_compute_unit(&mut self, cu: Ref<ComputeUnit>);
    /// Generates the kernels running `cus` (implies `generate_compute_unit`).
    fn generate_kernel_of(&mut self, cus: Vec<Ref<ComputeUnit>>); // currently only 1 at a time
    /// Generates the refine kernel of one run (`specs/refinement.md` §6.2), and its unit at the
    /// target's dims. Only called for programs with refinements, which backends refuse until
    /// they implement them (`check_implemented`).
    fn generate_refine_kernel(&mut self, run: &RefineRun) {
        let _ = run;
        panic!("{} does not implement refinement yet", self.codegen_language());
    }
    fn codegen_language(&self) -> &str;
    /// Everything generated so far as source text, wrapped in `module_name` and importing `imports`.
    fn render(&self, module_name: &str, imports: &[String]) -> String;
    /// Non-fatal diagnostics about the generated code (e.g. oversized methods).
    fn warnings(&self) -> Vec<String> {
        vec![]
    }
}

/// Decides what the device codegen generates and produces the glue that runs it.
pub trait OrchestrationCodegen<'a> {
    /// Generates everything for `program`, using the names and schedule in `interface`.
    fn convert_program(&mut self, program: &'a OptimizedProgram, interface: &Interface);
    /// The generated source; call after `convert_program`.
    fn output(&self) -> String;
}

/// A code generator for one target language. A backend is a stateless value holding its config:
/// `generate` returns the output and stores nothing.
pub trait Backend {
    /// What a successful compile returns, e.g. `RustOutput`.
    type Output;

    /// Short stable identifier, e.g. "rust" or "cuda". Used in errors.
    fn name(&self) -> &str;

    /// Naming rules for the current config. Computed on call, so config edits made after `new`
    /// are respected.
    fn naming_rules(&self) -> Cow<'_, NamingRules>;

    /// Checks the current config. [`Compiler::compile`] calls it before anything else.
    fn validate(&self) -> Result<(), BackendError> {
        Ok(())
    }

    /// The passes this backend wants by default, given its current config.
    fn default_passes(&self) -> Passes {
        Passes::default()
    }

    /// Generates everything for `program`. Stores nothing in `self`.
    fn generate(
        &self,
        program: &OptimizedProgram,
        interface: &Interface,
    ) -> Result<Self::Output, BackendError>;

    /// Non-fatal diagnostics in an output. Read by [`Compiler::deny_warnings`].
    fn warnings(_output: &Self::Output) -> &[String] {
        &[]
    }
}

/// How generated code checks reads of density inputs (`specs/workers.md` §5.1). Plain data: each
/// backend maps it to its own checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BufferBounds {
    /// Each read is checked against its input's own length, so a bad index fails exactly where a
    /// separate array per input would.
    PerBuffer,
    /// Reads are checked against the end of the worker's pool only. A bad index may read a
    /// neighbouring input's data.
    Slab,
    /// No checks. A bad index is undefined behaviour where the language allows it (Rust).
    Unchecked,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BackendError {
    #[error("code generation failed: {0}")]
    Codegen(String),
    #[error("invalid backend configuration: {0}")]
    InvalidConfig(String),
}

/// One generated lerp helper: a distinct (input dims, factors, element type) of a `Lerp` input
/// that some reachable unit reads without an index (`specs/density-mappings.md` §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LerpHelper {
    /// Fixed in the module namespace, e.g. `lerp_5x49x5_by_4x8x4_f64`. `r` stands for `Repeat`.
    pub name: String,
    /// Dims of the input being read.
    pub dimensions: (i32, i32, i32),
    pub factors: [AxisFactor; 3],
    /// The input's element type, `f32` or `f64`.
    pub element: VariableType,
}

/// Schedule and names shared by the device and orchestration sides of a code generator.
pub struct Interface {
    instances: Vec<Instance>,
    dependencies: HashMap<Instance, Vec<Instance>>,
    /// Every instance only depends on earlier waves, so a wave can run in parallel.
    waves: Vec<Vec<Instance>>,
    density_fns: HashMap<(Ref<ComputeUnit>, (i32, i32, i32)), String>,
    buffers: HashMap<Instance, String>,
    kernels: HashMap<Instance, String>,
    helpers: HashMap<Ref<Function>, String>,
    constants: HashMap<Ref<Constant>, String>,
    auto_consts: HashMap<Ref<AutoConst>, String>,
    auto_params: HashMap<Ref<AutoParam>, String>,
    host_fields: HashMap<Ref<HostInput>, String>,
    offset_params: HashMap<Ref<DensityInput>, String>,
    lerp_helpers: Vec<LerpHelper>,
    /// Element type of each unit's buffer, to find the helper of an input.
    unit_elements: HashMap<Ref<ComputeUnit>, VariableType>,
    /// Per refinement of the program, in order.
    refinement_names: Vec<String>,
    refine_methods: Vec<String>,
    refine_waves: Vec<Vec<Vec<RefineRun>>>,
    refine_kernels: HashMap<RefineRun, String>,
    /// Refined units, at the dims of the targets they replace.
    with_dims: Vec<(Ref<ComputeUnit>, (i32, i32, i32))>,
    walks: Vec<refine::Walk>,
    workers: workers::Workers,
}

impl Interface {
    /// Fixes every name with `rules`; reserved names are never produced.
    pub fn build(program: &OptimizedProgram, rules: &NamingRules) -> Self {
        let walks: Vec<refine::Walk> = program.refinements.iter().map(|r| refine::walk(program, r)).collect();
        let (instances, dependencies) = discover_instances(program);
        // Instances only a refinement reruns have buffers, but no wave: `run` doesn't compute them.
        let scheduled = scheduled_instances(program, &walks, &dependencies);
        let in_waves: Vec<Instance> = instances.iter().filter(|i| scheduled.contains(i)).copied().collect();
        let wave_deps = dependencies.iter().filter(|(i, _)| scheduled.contains(i)).map(|(i, d)| (*i, d.clone()));
        let waves = arrange_waves(&in_waves, wave_deps.collect());
        let refine_waves: Vec<Vec<Vec<RefineRun>>> =
            walks.iter().map(|w| refine::refine_waves(program, &w.running)).collect();
        let mut with_dims: Vec<(Ref<ComputeUnit>, (i32, i32, i32))> = vec![];
        for run in walks.iter().flat_map(|w| &w.running) {
            let key = (run.unit, run.target.dimensions);
            if run.unit != run.target.cu && !with_dims.contains(&key) {
                with_dims.push(key);
            }
        }

        // One namespace for module-level items (functions, constants, kernels).
        let mut used = rules.used_set();
        let mut used_fields = HashSet::new();

        let mut density_fns = HashMap::new();
        let mut buffers = HashMap::new();
        let mut kernels = HashMap::new();
        for inst in &instances {
            let key = (inst.cu, inst.dimensions);
            if !density_fns.contains_key(&key) {
                let (x, y, z) = inst.dimensions;
                let base = format!(
                    "{}_d{}x{}x{}",
                    unit_base_name(rules, program, inst.cu),
                    x,
                    y,
                    z
                );
                density_fns.insert(key, unique(&mut used, base));
            }
            let base = density_fns[&key].clone();
            buffers.insert(*inst, unique(&mut used_fields, format!("{}_output", base)));
            kernels.insert(*inst, unique(&mut used, format!("{}_kernel", base)));
        }

        // Refined units have a density function at the dims of their targets, and no buffer.
        for &(cu, (x, y, z)) in &with_dims {
            density_fns.entry((cu, (x, y, z))).or_insert_with(|| {
                let base = format!("{}_d{}x{}x{}", unit_base_name(rules, program, cu), x, y, z);
                unique(&mut used, base)
            });
        }
        let mut refine_kernels = HashMap::new();
        for run in refine_waves.iter().flatten().flatten() {
            if !refine_kernels.contains_key(run) {
                let base = format!(
                    "{}_{}_refine_kernel",
                    density_fns[&(run.unit, run.target.dimensions)],
                    unit_base_name(rules, program, run.target.cu)
                );
                refine_kernels.insert(*run, unique(&mut used, base));
            }
        }

        let main_functions: HashSet<_> = program
            .compute_units
            .iter()
            .map(|(_, cu)| {
                let ComputeUnit::DensityFunction { function, .. } = cu;
                *function
            })
            .collect();
        let mut helpers = HashMap::new();
        for (r, f) in program.functions.iter() {
            if main_functions.contains(&r) {
                continue;
            }
            let base = f
                .canonical_name
                .as_deref()
                .map(rules.sanitize)
                .unwrap_or_else(|| "function".into());
            helpers.insert(r, unique(&mut used, base));
        }

        let mut constants = HashMap::new();
        for (r, c) in program.constants.iter() {
            constants.insert(r, unique(&mut used, (rules.sanitize)(&c.name)));
        }

        let mut auto_consts = HashMap::new();
        let mut used_auto = rules.used_set();
        for (r, a) in program.auto_consts.iter() {
            auto_consts.insert(r, unique(&mut used_auto, auto_const_field_name(rules, a)));
        }

        // Params are parameters of the creation functions, next to the names those use.
        let mut auto_params = HashMap::new();
        let mut used_params = rules.used_set();
        used_params.extend(CREATION_RESERVED.iter().map(|s| s.to_string()));
        for (r, p) in program.auto_params.iter() {
            auto_params.insert(r, unique(&mut used_params, (rules.sanitize)(&p.name)));
        }

        let mut host_fields = HashMap::new();
        let mut used_host = HashSet::new();
        for (r, h) in program.host_inputs.iter() {
            if matches!(h, HostInput::Static { .. }) {
                host_fields.insert(r, unique(&mut used_host, host_field_name(rules, h)));
            }
        }

        // One namespace: a function can take the offsets of several inputs. The `off_` prefix keeps
        // them apart from the uniform parameters; backends keep body locals apart.
        let mut offset_params = HashMap::new();
        let mut used_offsets = rules.used_set();
        for (r, d) in program.density_inputs.iter() {
            let base = match &d.name {
                Name::Named(n) | Name::Prefixed(n) => format!("off_{n}"),
                Name::Anonymous => "off_input".to_string(),
            };
            offset_params.insert(r, unique(&mut used_offsets, (rules.sanitize)(&base)));
        }

        let mut interface = Self {
            instances,
            dependencies,
            waves,
            density_fns,
            buffers,
            kernels,
            helpers,
            constants,
            auto_consts,
            auto_params,
            host_fields,
            offset_params,
            lerp_helpers: vec![],
            unit_elements: program
                .compute_units
                .iter()
                .map(|(r, cu)| {
                    let ComputeUnit::DensityFunction { function, .. } = cu;
                    (r, program.functions[*function].return_type.clone())
                })
                .collect(),
            refinement_names: {
                let mut used = HashSet::new();
                program.refinements.iter().map(|r| unique(&mut used, (rules.sanitize)(&r.name))).collect()
            },
            refine_methods: naming::refinement_method_names(program, rules, &HashSet::new()),
            refine_waves,
            refine_kernels,
            with_dims,
            walks,
            workers: workers::Workers::default(),
        };
        interface.workers = workers::Workers::build(program, &interface);
        interface.lerp_helpers = collect_lerp_helpers(program, &interface, rules, &mut used);
        interface
    }

    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    pub fn waves(&self) -> &[Vec<Instance>] {
        &self.waves
    }

    /// The waves restricted to `target` and everything it transitively depends on.
    pub fn waves_for(&self, target: &Instance) -> Vec<Vec<Instance>> {
        self.waves_for_all(&[*target])
    }

    /// The waves restricted to `targets` and everything they transitively depend on.
    pub fn waves_for_all(&self, targets: &[Instance]) -> Vec<Vec<Instance>> {
        let needed = closure(&self.dependencies, targets);
        self.waves
            .iter()
            .map(|w| {
                w.iter()
                    .filter(|i| needed.contains(i))
                    .copied()
                    .collect::<Vec<_>>()
            })
            .filter(|w| !w.is_empty())
            .collect()
    }

    /// The waves that entry `entry`'s worker runs in `run` (`specs/refinement.md` §4.5): the entry's
    /// closure, the frozen instances its refinements read, and the instances their `with` units
    /// replace. Instances only a refinement reruns aren't in it, but have a slot (`worker`).
    pub fn worker_schedule(&self, entry: usize) -> &[Vec<Instance>] {
        &self.workers.schedules[entry]
    }

    /// Where entry `entry`'s worker keeps its buffers.
    pub fn worker(&self, entry: usize) -> &WorkerLayout {
        &self.workers.layouts[entry]
    }

    /// `inst`'s place in entry `entry`'s worker, if that worker computes it.
    pub fn slot(&self, entry: usize, inst: &Instance) -> Option<Slot> {
        let slots = &self.workers.layouts[entry].slots;
        slots.iter().find(|(i, _)| i == inst).map(|(_, s)| *s)
    }

    /// Inputs `f` reads, directly or through the helpers it calls, in program arena order,
    /// deduplicated.
    pub fn input_params(&self, f: Ref<Function>) -> &[Ref<DensityInput>] {
        &self.workers.input_params[&f]
    }

    /// Name of the offset parameter of input `d`, the same in every function that takes it
    /// (`specs/workers.md` §4.3). Starts with `off_`.
    pub fn offset_param(&self, d: Ref<DensityInput>) -> &str {
        &self.offset_params[&d]
    }

    /// The lerp helper that reads `input`, which must be a `Lerp` input read without an index by
    /// a reachable unit.
    pub fn lerp_helper(&self, input: &DensityInput) -> &str {
        let DensityMapping::Lerp(factors) = input.mapping else {
            panic!("input has no lerp mapping");
        };
        let element = &self.unit_elements[&input.density_function];
        self.lerp_helpers
            .iter()
            .find(|h| h.dimensions == input.dimensions && h.factors == factors && h.element == *element)
            .map(|h| h.name.as_str())
            .unwrap_or_else(|| panic!("no lerp helper for {:?} of {:?}", factors, input.dimensions))
    }

    /// Every lerp helper, in discovery order and shared by all readers. Inputs a backend can't emit
    /// (a non-float element, a factor below 1) get none: backends refuse those first.
    pub fn lerp_helpers(&self) -> &[LerpHelper] {
        &self.lerp_helpers
    }

    /// Static hosts read by any unit in entry `entry`'s worker schedule, in arena order.
    pub fn entry_hosts(&self, entry: usize) -> &[Ref<HostInput>] {
        &self.workers.entry_hosts[entry]
    }

    /// Names of the program's refinements, one per `program.refinements` entry, unique in the module.
    pub fn refinement_names(&self) -> &[String] {
        &self.refinement_names
    }

    /// The worker method of refinement `refinement` (`refine_<name>`), unique within its worker.
    pub fn refine_method(&self, refinement: usize) -> &str {
        &self.refine_methods[refinement]
    }

    /// What refinement `refinement` runs per call, in dependency order, one wave at a time
    /// (`specs/refinement.md` §4.1). Frozen instances aren't in it.
    pub fn refine_waves(&self, refinement: usize) -> &[Vec<RefineRun>] {
        &self.refine_waves[refinement]
    }

    /// The refine kernel of `run`. Shared by every refinement that runs the same unit on the same
    /// buffer.
    pub fn refine_kernel(&self, run: &RefineRun) -> &str {
        &self.refine_kernels[run]
    }

    pub fn instances_of(&self, cu: Ref<ComputeUnit>) -> Vec<Instance> {
        self.instances
            .iter()
            .filter(|i| i.cu == cu)
            .copied()
            .collect()
    }

    /// Distinct dimensions the unit is used at, in discovery order. A refined unit is used at the
    /// dims of the targets it replaces.
    pub fn dimensions_of(&self, cu: Ref<ComputeUnit>) -> Vec<(i32, i32, i32)> {
        let mut dims = Vec::new();
        let refined = self.with_dims.iter().filter(|(c, _)| *c == cu).map(|(_, d)| *d);
        for d in self.instances.iter().filter(|i| i.cu == cu).map(|i| i.dimensions).chain(refined) {
            if !dims.contains(&d) {
                dims.push(d);
            }
        }
        dims
    }

    pub fn density_fn(&self, cu: Ref<ComputeUnit>, dimensions: (i32, i32, i32)) -> &str {
        &self.density_fns[&(cu, dimensions)]
    }

    /// Field of `InternalBuffers` holding this instance's output.
    pub fn buffer_field(&self, inst: &Instance) -> &str {
        &self.buffers[inst]
    }

    pub fn kernel(&self, inst: &Instance) -> &str {
        &self.kernels[inst]
    }

    pub fn helper(&self, f: Ref<Function>) -> &str {
        &self.helpers[&f]
    }

    pub fn constant(&self, c: Ref<Constant>) -> &str {
        &self.constants[&c]
    }

    pub fn auto_const_field(&self, a: Ref<AutoConst>) -> &str {
        &self.auto_consts[&a]
    }

    /// Parameter name of an auto param in the creation functions.
    pub fn auto_param(&self, p: Ref<AutoParam>) -> &str {
        &self.auto_params[&p]
    }

    /// Field of `HostConsts` for a static host input.
    pub fn host_field(&self, h: Ref<HostInput>) -> &str {
        &self.host_fields[&h]
    }
}

/// Helpers for the `Lerp` inputs that reachable units read without an index, in discovery order.
fn collect_lerp_helpers(
    program: &OptimizedProgram,
    interface: &Interface,
    rules: &NamingRules,
    used: &mut HashSet<String>,
) -> Vec<LerpHelper> {
    let mut helpers: Vec<LerpHelper> = vec![];
    let refined = interface.with_dims.iter().map(|(cu, _)| *cu);
    for cu in interface.instances.iter().map(|i| i.cu).chain(refined) {
        let ComputeUnit::DensityFunction { function, density_inputs, .. } = &program.compute_units[cu];
        let read = &interface.workers.unindexed_reads[function];
        for &r in density_inputs {
            let d = &program.density_inputs[r];
            let DensityMapping::Lerp(factors) = d.mapping else { continue };
            let element = &interface.unit_elements[&d.density_function];
            let valid = matches!(element, VariableType::F32 | VariableType::F64)
                && factors.iter().all(|f| !matches!(f, AxisFactor::Factor(n) if *n < 1));
            let known = helpers
                .iter()
                .any(|h| h.dimensions == d.dimensions && h.factors == factors && h.element == *element);
            if !read.contains(&r) || !valid || known {
                continue;
            }
            let (sx, sy, sz) = d.dimensions;
            let factor = |f: AxisFactor| match f {
                AxisFactor::Repeat => "r".to_string(),
                AxisFactor::Factor(n) => n.to_string(),
            };
            let name = format!(
                "lerp_{sx}x{sy}x{sz}_by_{}x{}x{}_{}",
                factor(factors[0]),
                factor(factors[1]),
                factor(factors[2]),
                if *element == VariableType::F32 { "f32" } else { "f64" },
            );
            helpers.push(LerpHelper {
                name: unique(used, (rules.sanitize)(&name)),
                dimensions: d.dimensions,
                factors,
                element: element.clone(),
            });
        }
    }
    helpers
}

/// Everything reachable from the entry points (breadth-first), then what only refinements need,
/// with each instance's direct inputs. `@previous` inputs add no dependency.
fn discover_instances(
    program: &OptimizedProgram,
) -> (Vec<Instance>, HashMap<Instance, Vec<Instance>>) {
    let mut instances = Vec::new();
    let mut dependencies: HashMap<Instance, Vec<Instance>> = HashMap::new();
    let mut queue: Vec<Instance> = program
        .entry_points
        .iter()
        .map(Instance::from_entry)
        .collect();
    let mut i = 0;
    let mut refinements_seeded = false;
    loop {
        while i < queue.len() {
            let inst = queue[i];
            i += 1;
            if dependencies.contains_key(&inst) {
                continue;
            }
            instances.push(inst);
            let deps = refine::normal_inputs(program, inst.cu);
            queue.extend(deps.iter().copied());
            dependencies.insert(inst, deps);
        }
        if refinements_seeded {
            return (instances, dependencies);
        }
        refinements_seeded = true;
        // Instances needed only by refinements come after the others: every rerun, and whatever
        // a `with` unit reads.
        for r in &program.refinements {
            for x in &r.reruns {
                queue.push(refine::rerun_instance(x));
                if let Some(w) = x.with {
                    queue.extend(refine::normal_inputs(program, w));
                }
            }
        }
    }
}

/// Instances some entry's `run` computes: its seeds (`refine::schedule_seeds`) and everything they
/// depend on.
fn scheduled_instances(
    program: &OptimizedProgram,
    walks: &[refine::Walk],
    dependencies: &HashMap<Instance, Vec<Instance>>,
) -> HashSet<Instance> {
    let seeds: Vec<Instance> = (0..program.entry_points.len())
        .flat_map(|e| refine::schedule_seeds(program, walks, e))
        .collect();
    closure(dependencies, &seeds)
}

/// `seeds` and everything they transitively depend on.
fn closure(dependencies: &HashMap<Instance, Vec<Instance>>, seeds: &[Instance]) -> HashSet<Instance> {
    let mut needed = HashSet::new();
    let mut stack = seeds.to_vec();
    while let Some(i) = stack.pop() {
        if needed.insert(i) {
            stack.extend(dependencies[&i].iter().copied());
        }
    }
    needed
}

/// Wave algorithm: repeatedly peel off every instance whose dependencies are already scheduled.
fn arrange_waves(
    instances: &[Instance],
    mut remaining: HashMap<Instance, Vec<Instance>>,
) -> Vec<Vec<Instance>> {
    let mut waves = Vec::new();
    while !remaining.is_empty() {
        // Discovery order keeps the result deterministic.
        let wave: Vec<Instance> = instances
            .iter()
            .filter(|i| remaining.get(i).is_some_and(|deps| deps.is_empty()))
            .copied()
            .collect();
        if wave.is_empty() {
            panic!("Cyclic dependency detected between compute unit instances");
        }
        for inst in &wave {
            remaining.remove(inst);
        }
        for deps in remaining.values_mut() {
            deps.retain(|d| !wave.contains(d));
        }
        waves.push(wave);
    }
    waves
}
