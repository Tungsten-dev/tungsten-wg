//! New RCL (Rust) generator from optimized SPMT. See INSTRUCTIONS.md for the target output format.
//!
//! The generator is split into a device side and an orchestration side:
//! - Device side ([`RCLCodegen`], implementing [`DeviceCodegen`]): turns compute units into density
//!   functions, helper functions, constants and one kernel per scheduled instance.
//! - Orchestration side ([`orchestration::TransformRclOrchestration`]): defines the structs the device
//!   code is generated against and the `Orchestration` type that runs kernels in wave order.
//!
//! Every density and helper function starts with the same parameters (see [`uniform_parameters`]):
//! `p: PositionIterator, config: &Config, buffers: &InternalBuffers, auto_constants: &AutoConsts,
//! host_consts: &HostConsts`. Then one `usize` offset per input the function reads, directly or
//! through helpers (`Interface::input_params`), then the helper's own parameters. All names come
//! from the shared [`Interface`], so caller and callee always agree.
//!
//! `InternalBuffers` holds one `Box<[T]>` per element type (a pool, see [`pools`]). Each worker
//! sizes the pools for its own entry and passes the offsets of its instances to the kernels
//! (`specs/workers.md`).
//!
//! Refinement (`specs/refinement.md` §6.2): a worker of an entry with refinements also stores the
//! origin of its last `run`, and has one `refine_<name>(offset: Pos3, host_consts)` method per
//! refinement. Each runs the refinement's waves through refine kernels, which are kernels with an
//! extra `offset` that adds to the iterator's `pos` and leaves `idx`, so buffer indices stay put.
//!
//! Modules:
//! - `types`: type and operator conversion, type derivation.
//! - `converter`: per-function state (variables, lookups, type derivation).
//! - `expression` / `statement`: body conversion.
//! - `function`: density functions, helpers, constants and kernels.
//! - `orchestration`: structs, `AutoConsts` initializers, the `Orchestration` impl, module assembly.
//!
//! Assumed runtime (not generated): `Pos3`, `Vec3`, `Config { origin, origin_scale, position_scale,
//! dimensions }`, `iter_3d(&Config)` yielding items with a `pos` field, `index(..)` returning the
//! loop item's flat `usize` index, the autoconst initializer functions and the handle types they
//! return. Runtime contract: handle types are `Send + Sync`, and record types used as a unit output
//! or host are `Copy + Default`.

use std::collections::HashSet;

use tungsten_interface::{DeviceCodegen, Instance, Interface, LerpHelper, RefineRun};
use tungsten_ir::model::{ComputeUnit, Constant, Function, OptimizedProgram, Ref};

use crate::interned::Interned;
use crate::model as rcl;
use crate::model::codegen::RustCodeGenerator;
use crate::pools::Pools;

pub mod backend;
pub mod converter;
pub mod expression;
pub mod function;
pub mod interned;
pub mod mapping;
pub mod model;
pub mod orchestration;
pub mod pools;
pub mod statement;
pub mod transformer;
pub mod types;

/// Names of runtime items and generated structs that the generated code refers to by name.
pub const HOST_CONSTS_STRUCT_NAME: &str = "HostConsts";
pub const AUTO_CONSTS_STRUCT_NAME: &str = "AutoConsts";
pub const CONFIG_STRUCT_NAME: &str = "Config";
pub const INTERNAL_BUFFERS_STRUCT_NAME: &str = "InternalBuffers";
pub const ORCHESTRATION_STRUCT_NAME: &str = "Orchestration";

/// The checked read of `BufferBounds::PerBuffer`, generated into the density module. Reserved by
/// the Rust naming rules.
pub const READ_HELPER: &str = tungsten_interface::naming::RUST_READ_HELPER;

/// Checks the index against the input's own length, so a bad index panics with the message of an
/// out-of-bounds index into a separate array, which the test driver maps to `OutOfBounds`.
const READ_HELPER_SOURCE: &str = "#[inline(always)]
fn rd<T: Copy>(pool: &[T], offset: usize, len: usize, i: usize) -> T {
    if i >= len {
        panic!(\"index out of bounds: the len is {} but the index is {}\", len, i);
    }
    pool[offset + i]
}";

/// Uniform parameter names shared by every generated density and helper function, in order.
pub const PARAM_POS: &str = "p";
pub const PARAM_CONFIG: &str = "config";
pub const PARAM_BUFFERS: &str = "buffers";
pub const PARAM_AUTO_CONSTS: &str = "auto_constants";
pub const PARAM_HOST_CONSTS: &str = "host_consts";

/// The uniform leading parameters of every density and helper function.
pub fn uniform_parameters() -> Vec<rcl::Parameter> {
    use rcl::Type;
    let s = |n: &str| Type::Struct(n.to_string());
    vec![
        rcl::Parameter {
            name: PARAM_POS.into(),
            t: s("PositionIterator"),
        },
        rcl::Parameter {
            name: PARAM_CONFIG.into(),
            t: Type::Ref(Box::new(s(CONFIG_STRUCT_NAME))),
        },
        rcl::Parameter {
            name: PARAM_BUFFERS.into(),
            t: Type::Ref(Box::new(s(INTERNAL_BUFFERS_STRUCT_NAME))),
        },
        rcl::Parameter {
            name: PARAM_AUTO_CONSTS.into(),
            t: Type::Ref(Box::new(s(AUTO_CONSTS_STRUCT_NAME))),
        },
        rcl::Parameter {
            name: PARAM_HOST_CONSTS.into(),
            t: Type::Ref(Box::new(s(HOST_CONSTS_STRUCT_NAME))),
        },
    ]
}

/// The uniform leading parameters of every density and helper function.
pub fn uniform_kernel_parameters() -> Vec<rcl::Parameter> {
    use rcl::Type;
    let s = |n: &str| Type::Struct(n.to_string());
    vec![
        rcl::Parameter {
            name: PARAM_POS.into(),
            t: s("PositionIterator"),
        },
        rcl::Parameter {
            name: PARAM_CONFIG.into(),
            t: Type::Ref(Box::new(s(CONFIG_STRUCT_NAME))),
        },
        rcl::Parameter {
            name: PARAM_BUFFERS.into(),
            t: Type::MutRef(Box::new(s(INTERNAL_BUFFERS_STRUCT_NAME))),
        },
        rcl::Parameter {
            name: PARAM_AUTO_CONSTS.into(),
            t: Type::Ref(Box::new(s(AUTO_CONSTS_STRUCT_NAME))),
        },
        rcl::Parameter {
            name: PARAM_HOST_CONSTS.into(),
            t: Type::Ref(Box::new(s(HOST_CONSTS_STRUCT_NAME))),
        },
    ]
}

/// Device-side generator: converts compute units of an optimized program into an RCL model.
///
/// Generation is idempotent per item (the `done_*` sets), so units, helpers and constants shared
/// between instances are emitted once.
pub struct RCLCodegen<'a, 'm> {
    source: &'a OptimizedProgram,
    /// Names and schedule shared with the orchestration side.
    naming: &'a Interface,
    pools: &'a Pools,
    model: rcl::RCL<'m>,
    arena: &'m bumpalo::Bump,
    done_units: HashSet<Ref<ComputeUnit>>,
    done_functions: HashSet<Ref<Function>>,
    done_constants: HashSet<Ref<Constant>>,
    done_kernels: HashSet<Instance>,
    done_refine_kernels: HashSet<RefineRun>,
    done_lerps: HashSet<String>,
} // TODO add config parameters (if necessary)

impl<'a, 'm> RCLCodegen<'a, 'm> {
    /// `interface` must be the one built for `source`, and `pools` built from both; nothing is
    /// generated until asked.
    pub fn new(
        source: &'a OptimizedProgram,
        arena: &'m bumpalo::Bump,
        interface: &'a Interface,
        pools: &'a Pools,
    ) -> Self {
        let mut model = rcl::RCL::new();
        model.raw_items.push(READ_HELPER_SOURCE.to_string());
        Self {
            source,
            naming: interface,
            pools,
            model,
            arena,
            done_units: HashSet::new(),
            done_functions: HashSet::new(),
            done_constants: HashSet::new(),
            done_kernels: HashSet::new(),
            done_refine_kernels: HashSet::new(),
            done_lerps: HashSet::new(),
        }
    }

    pub fn naming(&self) -> &'a Interface {
        self.naming
    }

    /// The RCL model generated so far.
    pub fn model(&self) -> &rcl::RCL<'m> {
        &self.model
    }

    /// Consumes the generator and returns the RCL model.
    pub fn finish(self) -> rcl::RCL<'m> {
        self.model
    }

    fn intern(&self, f: rcl::Function<'m>) -> rcl::FunctionRef<'m> {
        Interned::new(self.arena.alloc(f))
    }

    /// Generates `f`, the helpers it calls, and the constants those use.
    fn generate_helper(&mut self, f: Ref<Function>) {
        let mut pending = vec![f];
        while let Some(f) = pending.pop() {
            if !self.done_functions.insert(f) {
                continue;
            }
            let converted = function::convert_helper(self.source, self.naming, self.pools, f);
            let func = self.intern(converted.function);
            self.model.functions.push(func);
            pending.extend(converted.called);
            self.generate_lerps(converted.lerps);
            for c in converted.constants {
                self.generate_constant(c);
            }
        }
    }

    /// Generates the lerp helpers a converted function calls, once each.
    fn generate_lerps(&mut self, lerps: Vec<LerpHelper>) {
        for h in lerps {
            if self.done_lerps.insert(h.name.clone()) {
                let f = self.intern(mapping::lerp_helper(&h, self.pools.bounds));
                self.model.functions.push(f);
            }
        }
    }

    fn generate_constant(&mut self, c: Ref<Constant>) {
        let mut pending = vec![c];
        while let Some(c) = pending.pop() {
            if !self.done_constants.insert(c) {
                continue;
            }
            let (constant, deps) = function::convert_constant(self.source, &self.naming, c);
            self.model.constants.push(constant);
            pending.extend(deps);
        }
    }

    /// Generates the kernel for one scheduled instance of a compute unit (and the unit itself).
    pub fn generate_kernel_for_instance(&mut self, inst: Instance) {
        self.generate_compute_unit(inst.cu);
        if !self.done_kernels.insert(inst) {
            return;
        }
        let kernel = function::build_kernel(self.source, self.naming, self.pools, &inst);
        let kernel = self.intern(kernel);
        self.model.main_functions.push(kernel);
    }
}

impl DeviceCodegen for RCLCodegen<'_, '_> {
    /// The refined unit at the target's dims, and the kernel running it. Repeats are no-ops.
    fn generate_refine_kernel(&mut self, run: &RefineRun) {
        self.generate_compute_unit(run.unit);
        self.generate_compute_unit(run.target.cu);
        if !self.done_refine_kernels.insert(*run) {
            return;
        }
        let kernel = function::build_refine_kernel(self.source, self.naming, self.pools, run);
        let kernel = self.intern(kernel);
        self.model.main_functions.push(kernel);
    }

    /// Generates one density function per dimension the unit is scheduled at, plus its helpers and constants.
    fn generate_compute_unit(&mut self, cu: Ref<ComputeUnit>) {
        if !self.done_units.insert(cu) {
            return;
        }
        let source = self.source;
        let ComputeUnit::DensityFunction {
            helper_functions,
            constants,
            ..
        } = &source.compute_units[cu];

        for dimensions in self.naming.dimensions_of(cu) {
            let converted = function::convert_density(source, self.naming, self.pools, cu, dimensions);
            let func = self.intern(converted.function);
            self.model.functions.push(func);
            for h in converted.called {
                self.generate_helper(h);
            }
            self.generate_lerps(converted.lerps);
            for c in converted.constants {
                self.generate_constant(c);
            }
        }
        for h in helper_functions {
            self.generate_helper(*h);
        }
        for c in constants {
            self.generate_constant(*c);
        }
    }

    /// One kernel per scheduled instance of the unit.
    fn generate_kernel_of(&mut self, cus: Vec<Ref<ComputeUnit>>) {
        assert_eq!(
            cus.len(),
            1,
            "kernels with multiple compute units are not supported yet"
        );
        for inst in self.naming.instances_of(cus[0]) {
            self.generate_kernel_for_instance(inst);
        }
    }

    fn codegen_language(&self) -> &str {
        "rust"
    }

    /// Renders the model as a Rust module, adding `imports` to a copy so repeated calls are safe.
    fn render(&self, module_name: &str, imports: &[String]) -> String {
        let mut model = self.model.clone();
        for import in imports {
            model.add_import(import.clone());
        }
        RustCodeGenerator::new().generate_inline_module(&model, module_name)
    }
}

/// Orchestration-side generator; see [`orchestration`] for its [`OrchestrationCodegen`] impl.
pub struct TransformRclOrchestration<'m> {
    device_codegen: Box<dyn DeviceCodegen + 'm>,
    arena: &'m bumpalo::Bump,
    pools: Pools,
    model: rcl::RCL<'m>,
    density_module_name: String,
    orchestration_module_name: String,
}

#[cfg(test)]
mod tests;
