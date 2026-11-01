/*
The interface decides how compute units are orchestrated (which instances exist and in which
waves they run) and fixes the names of everything generated code refers to: kernels, buffers,
host inputs and auto consts. Computed once so callers and callees agree.
*/

use std::collections::{HashMap, HashSet};

use tungsten_ir::model::{
    ComputeUnit, Constant, DensityInput, EntryPoint, Function, HostInput, OptimizedProgram,
    PermutationTableInput, Ref,
};

pub mod naming;
pub mod scale;

pub use scale::Scale;

use naming::{auto_const_field_name, host_field_name, unique, unit_base_name};

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

/// A code generator for one target language. Backends are independent: each one reads the
/// shared `Interface` and stores its own output.
pub trait Backend {
    /// Short stable identifier, e.g. "rust" or "cuda". Used in errors.
    fn name(&self) -> &str;

    /// Generate everything for `program`. Results are stored in `self`.
    fn generate(
        &mut self,
        program: &OptimizedProgram,
        interface: &Interface,
    ) -> Result<(), BackendError>;

    fn naming_rules(&self) -> &NamingRules;
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("code generation failed: {0}")]
    Codegen(String),
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
    auto_consts: HashMap<Ref<PermutationTableInput>, String>,
    host_fields: HashMap<Ref<HostInput>, String>,
}

impl Interface {
    /// Fixes every name with `rules`; reserved names are never produced.
    pub fn build(program: &OptimizedProgram, rules: &NamingRules) -> Self {
        let (instances, dependencies) = discover_instances(program);
        let waves = arrange_waves(&instances, dependencies.clone());

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
        let mut used_auto = HashSet::new();
        for (r, t) in program.permutation_tables.iter() {
            auto_consts.insert(r, unique(&mut used_auto, auto_const_field_name(rules, t)));
        }

        let mut host_fields = HashMap::new();
        let mut used_host = HashSet::new();
        for (r, h) in program.host_inputs.iter() {
            if matches!(h, HostInput::Static { .. }) {
                host_fields.insert(r, unique(&mut used_host, host_field_name(rules, h)));
            }
        }

        Self {
            instances,
            dependencies,
            waves,
            density_fns,
            buffers,
            kernels,
            helpers,
            constants,
            auto_consts,
            host_fields,
        }
    }

    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    pub fn waves(&self) -> &[Vec<Instance>] {
        &self.waves
    }

    /// The waves restricted to `target` and everything it transitively depends on.
    pub fn waves_for(&self, target: &Instance) -> Vec<Vec<Instance>> {
        let mut needed = HashSet::new();
        let mut stack = vec![*target];
        while let Some(i) = stack.pop() {
            if needed.insert(i) {
                stack.extend(self.dependencies[&i].iter().copied());
            }
        }
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

    pub fn instances_of(&self, cu: Ref<ComputeUnit>) -> Vec<Instance> {
        self.instances
            .iter()
            .filter(|i| i.cu == cu)
            .copied()
            .collect()
    }

    /// Distinct dimensions the unit is used at, in discovery order.
    pub fn dimensions_of(&self, cu: Ref<ComputeUnit>) -> Vec<(i32, i32, i32)> {
        let mut dims = Vec::new();
        for i in self.instances.iter().filter(|i| i.cu == cu) {
            if !dims.contains(&i.dimensions) {
                dims.push(i.dimensions);
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

    pub fn auto_const_field(&self, t: Ref<PermutationTableInput>) -> &str {
        &self.auto_consts[&t]
    }

    /// Field of `HostConsts` for a static host input.
    pub fn host_field(&self, h: Ref<HostInput>) -> &str {
        &self.host_fields[&h]
    }
}

/// Everything reachable from the entry points (breadth-first), with each instance's direct inputs.
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
    while i < queue.len() {
        let inst = queue[i];
        i += 1;
        if dependencies.contains_key(&inst) {
            continue;
        }
        instances.push(inst);
        let ComputeUnit::DensityFunction { density_inputs, .. } = &program.compute_units[inst.cu];
        let mut deps: Vec<Instance> = Vec::new();
        for (_, di) in density_inputs.iter() {
            let dep = Instance::from_input(di);
            if !deps.contains(&dep) {
                deps.push(dep);
            }
        }
        queue.extend(deps.iter().copied());
        dependencies.insert(inst, deps);
    }
    (instances, dependencies)
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
