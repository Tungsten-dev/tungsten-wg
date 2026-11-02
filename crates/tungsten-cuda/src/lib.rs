//! CUDA C++ generator from optimized SPMT. Mirrors `tungsten-rust` and is validated against its
//! output downstream. Contracts and design decisions are in this crate's `README.md`.
//!
//! The generator is split into a device side and an orchestration side:
//! - Device side ([`CudaCodegen`], implementing [`DeviceCodegen`]): `__device__` density functions,
//!   helpers, constants, and per scheduled instance a kernel body plus an `extern "C"` wrapper.
//!   Rendered as `density.cu`. It knows nothing about the host class, so a different host (e.g.
//!   Rust via NVRTC) can drive it.
//! - Orchestration side ([`TransformCudaOrchestration`]): `interface.cuh` (the structs the device
//!   code is generated against), the CUDA-free public header `orchestration.h` and the host source
//!   `orchestration.cu`.
//!
//! Every density and helper function starts with the same parameters (see [`uniform_parameters`]):
//! `tw::PositionIterator p, const Ctx& ctx`. Helper-specific parameters follow them. All names come
//! from the shared [`Interface`] built with [`NamingRules::cuda`], plus the two derived names
//! [`kernel_symbol`] and [`kernel_body_name`].
//!
//! Modules:
//! - `types`: type and operator conversion, integer-semantics helpers.
//! - `converter`: per-function state (variables, type derivation, warnings).
//! - `expression` / `statement`: body conversion.
//! - `function`: density functions, helpers, constants and kernels.
//! - `launch`: block and grid shape per instance.
//! - `orchestration`: `interface.cuh`, `orchestration.h`, `orchestration.cu`.
//!
//! Assumed runtime (not generated, `tw::` namespace): vector operators, `tw::Array`, `tw::Config`,
//! `tw::PositionIterator`, samplers, math externs and host initializers. See the runtime contract in `README.md`.

use std::collections::{HashMap, HashSet};

use tungsten_interface::naming::NamingRules;
use tungsten_interface::{DeviceCodegen, Instance, Interface};
use tungsten_ir::model::{ComputeUnit, Constant, Function, OptimizedProgram, Ref};

use crate::model as cuda;
use crate::model::codegen::CudaCodeGenerator;

pub mod backend;
pub mod converter;
pub mod expression;
pub mod function;
pub mod launch;
pub mod model;
pub mod orchestration;
pub mod statement;
pub mod types;

pub use backend::{CudaBackend, CudaConfig, CudaOutput, InlinePolicy, Profile};
pub use orchestration::TransformCudaOrchestration;

/// Runtime types the generated code refers to by name.
pub const PERLIN_NOISE_SAMPLER_TYPE: &str = "tw::PerlinNoiseSampler";
pub const BASE3D_NOISE_SAMPLER_TYPE: &str = "tw::InterpolatedNoiseSampler";
pub const CONFIG_TYPE: &str = "tw::Config";
pub const POSITION_ITERATOR_TYPE: &str = "tw::PositionIterator";

/// Generated structs in `<namespace>::dev` (declared in `interface.cuh`).
pub const CTX_STRUCT: &str = "Ctx";
pub const AUTO_CONSTS_STRUCT: &str = "AutoConsts";
pub const DEVICE_HOST_CONSTS_STRUCT: &str = "DeviceHostConsts";
pub const BUFFER_LAYOUT: &str = "BufferLayout";
/// Namespace of the device code inside the configured namespace.
pub const DEVICE_NAMESPACE: &str = "dev";

/// Uniform parameter names shared by every generated density and helper function, in order.
pub const PARAM_POS: &str = "p";
pub const PARAM_CTX: &str = "ctx";

/// The uniform leading parameters of every density and helper function.
pub fn uniform_parameters() -> Vec<(cuda::Type, String)> {
    vec![
        (cuda::Type::named(POSITION_ITERATOR_TYPE), PARAM_POS.into()),
        (cuda::Type::ConstRef(Box::new(cuda::Type::named(CTX_STRUCT))), PARAM_CTX.into()),
    ]
}

/// The uniform parameters passed through unchanged, as call arguments.
pub fn uniform_arguments() -> Vec<cuda::Expression> {
    uniform_parameters().into_iter().map(|(_, n)| cuda::Expression::Name(n)).collect()
}

/// The `extern "C"` symbol of an instance's kernel: `<kernel_prefix><interface.kernel(inst)>`.
/// Kernels live in the global symbol namespace, so the prefix keeps programs apart.
pub fn kernel_symbol(config: &CudaConfig, interface: &Interface, inst: &Instance) -> String {
    format!("{}{}", config.effective_kernel_prefix(), interface.kernel(inst))
}

/// The `__device__` function holding an instance's per-instance kernel logic.
pub fn kernel_body_name(interface: &Interface, inst: &Instance) -> String {
    format!("{}_body", interface.kernel(inst))
}

/// Every namespace-level name of the device code, plus the reserved names. Locals may not reuse
/// any of them, since a C++ local shadows a function or constant of the same name.
fn namespace_names(source: &OptimizedProgram, naming: &Interface, rules: &NamingRules, config: &CudaConfig) -> HashSet<String> {
    let mut names = rules.used_set();
    for inst in naming.instances() {
        names.insert(naming.density_fn(inst.cu, inst.dimensions).to_string());
        names.insert(naming.kernel(inst).to_string());
        names.insert(kernel_body_name(naming, inst));
        names.insert(kernel_symbol(config, naming, inst));
    }
    let main_functions: HashSet<_> = source
        .compute_units
        .iter()
        .map(|(_, cu)| {
            let ComputeUnit::DensityFunction { function, .. } = cu;
            *function
        })
        .collect();
    for (f, _) in source.functions.iter() {
        if !main_functions.contains(&f) {
            names.insert(naming.helper(f).to_string());
        }
    }
    for (c, _) in source.constants.iter() {
        names.insert(naming.constant(c).to_string());
    }
    names
}

/// Device-side generator: converts compute units of an optimized program into `density.cu`.
///
/// Generation is idempotent per item (the `done_*` sets), so units, helpers and constants shared
/// between instances are emitted once.
pub struct CudaCodegen<'a> {
    source: &'a OptimizedProgram,
    /// Names and schedule shared with the orchestration side.
    naming: &'a Interface,
    rules: &'a NamingRules,
    config: &'a CudaConfig,
    /// Names locals must avoid (see [`namespace_names`]).
    namespace_names: HashSet<String>,
    /// Density and helper functions, in generation order.
    functions: Vec<cuda::Function>,
    kernel_bodies: Vec<cuda::Function>,
    kernel_wrappers: Vec<cuda::Function>,
    /// Converted constants; emitted in dependency order.
    constants: HashMap<Ref<Constant>, cuda::Item>,
    warnings: Vec<String>,
    done_units: HashSet<Ref<ComputeUnit>>,
    done_functions: HashSet<Ref<Function>>,
    done_kernels: HashSet<Instance>,
}

impl<'a> CudaCodegen<'a> {
    /// `naming` must be the interface built for `source` with `rules`; nothing is generated until
    /// asked.
    pub fn new(source: &'a OptimizedProgram, naming: &'a Interface, rules: &'a NamingRules, config: &'a CudaConfig) -> Self {
        let namespace_names = namespace_names(source, naming, rules, config);
        Self {
            source,
            naming,
            rules,
            config,
            namespace_names,
            functions: Vec::new(),
            kernel_bodies: Vec::new(),
            kernel_wrappers: Vec::new(),
            constants: HashMap::new(),
            warnings: Vec::new(),
            done_units: HashSet::new(),
            done_functions: HashSet::new(),
            done_kernels: HashSet::new(),
        }
    }

    pub fn naming(&self) -> &'a Interface {
        self.naming
    }

    pub fn rules(&self) -> &'a NamingRules {
        self.rules
    }

    /// Generates `f`, the helpers it calls, and the constants those use.
    fn generate_helper(&mut self, f: Ref<Function>) {
        let mut pending = vec![f];
        while let Some(f) = pending.pop() {
            if !self.done_functions.insert(f) {
                continue;
            }
            let converted = function::convert_helper(self.source, self.naming, self.config, &self.namespace_names, f);
            self.functions.push(converted.function);
            self.warnings.extend(converted.warnings);
            pending.extend(converted.called);
            for c in converted.constants {
                self.generate_constant(c);
            }
        }
    }

    fn generate_constant(&mut self, c: Ref<Constant>) {
        let mut pending = vec![c];
        while let Some(c) = pending.pop() {
            if self.constants.contains_key(&c) {
                continue;
            }
            let (item, warnings) = function::convert_constant(self.source, self.naming, self.config, c);
            self.constants.insert(c, item);
            self.warnings.extend(warnings);
            tungsten_rust::expression::const_refs(&self.source.constants[c].value, &mut pending);
        }
    }

    /// Generates the kernel for one scheduled instance of a compute unit (and the unit itself).
    pub fn generate_kernel_for_instance(&mut self, inst: Instance) {
        self.generate_compute_unit(inst.cu);
        if !self.done_kernels.insert(inst) {
            return;
        }
        let shape = launch::launch_shape(inst.dimensions, self.config.block_size);
        if shape.threads_per_block() < 32 {
            self.warnings.push(format!(
                "cuda: kernel {} uses {} threads per block (a partial warp); a candidate for wave fusion",
                kernel_symbol(self.config, self.naming, &inst),
                shape.threads_per_block()
            ));
        }
        self.kernel_bodies.push(function::build_kernel_body(self.source, self.naming, self.config, &inst));
        self.kernel_wrappers.push(function::build_kernel_wrapper(self.naming, self.config, &inst, true));
    }

    /// Panics if a kernel body name collides with a generated function or constant (the `_body`
    /// suffix isn't from the `Interface`).
    fn check_body_names(&self) {
        let mut taken: HashSet<&str> = self.functions.iter().map(|f| f.name.as_str()).collect();
        for item in self.constants.values() {
            if let cuda::Item::Variable { name, .. } = item {
                taken.insert(name);
            }
        }
        for body in &self.kernel_bodies {
            if taken.contains(body.name.as_str()) {
                panic!("kernel body `{}` collides with a generated function or constant", body.name);
            }
        }
    }

    /// The namespace contents of `density.cu`: constants, forward declarations, functions, kernel
    /// bodies, kernel wrappers.
    fn device_items(&self) -> Vec<cuda::Item> {
        let mut items = Vec::new();
        let order: Vec<_> = self.constants.keys().copied().collect();
        let sorted = function::sort_constants(self.source, &order);
        if !sorted.is_empty() {
            items.push(cuda::Item::Comment("Constants, in dependency order.".into()));
            items.extend(sorted.iter().map(|c| self.constants[c].clone()));
            items.push(cuda::Item::Blank);
        }
        if !self.functions.is_empty() {
            items.push(cuda::Item::Comment("Forward declarations.".into()));
            items.extend(self.functions.iter().map(|f| cuda::Item::Function(f.declaration())));
            items.push(cuda::Item::Blank);
        }
        for f in self.functions.iter().chain(&self.kernel_bodies).chain(&self.kernel_wrappers) {
            items.push(cuda::Item::Function(f.clone()));
            items.push(cuda::Item::Blank);
        }
        // The namespace closes with its own blank line.
        if items.last() == Some(&cuda::Item::Blank) {
            items.pop();
        }
        items
    }
}

impl DeviceCodegen for CudaCodegen<'_> {
    /// Generates one density function per dimension the unit is scheduled at, plus its helpers and constants.
    fn generate_compute_unit(&mut self, cu: Ref<ComputeUnit>) {
        if !self.done_units.insert(cu) {
            return;
        }
        let source = self.source;
        let ComputeUnit::DensityFunction { helper_functions, constants, .. } = &source.compute_units[cu];

        for dimensions in self.naming.dimensions_of(cu) {
            let converted =
                function::convert_density(source, self.naming, self.config, &self.namespace_names, cu, dimensions);
            self.functions.push(converted.function);
            self.warnings.extend(converted.warnings);
            for h in converted.called {
                self.generate_helper(h);
            }
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
        assert_eq!(cus.len(), 1, "kernels with multiple compute units are not supported yet");
        for inst in self.naming.instances_of(cus[0]) {
            self.generate_kernel_for_instance(inst);
        }
    }

    fn codegen_language(&self) -> &str {
        "cuda"
    }

    /// `density.cu`: the device code in namespace `module_name`, after the profile define, the
    /// runtime and profile headers, and `imports` (as `#include "..."`).
    fn render(&self, module_name: &str, imports: &[String]) -> String {
        self.check_body_names();
        let profile = self.config.profile;
        let mut header = format!(
            "// Generated by tungsten-wg {}. Do not edit.\n\
             // Device code: density functions, helpers, constants and kernels.\n\
             // Profile: {}.\n",
            env!("CARGO_PKG_VERSION"),
            profile.name()
        );
        header.push_str(&format!("// Required nvcc flags: {}\n", profile.required_flags_comment()));
        if self.config.debug_checks {
            header.push_str("// Debug checks are on (TW_CHECK_INDEX / TW_CHECK_DIV).\n");
        }
        // The profile's externs use runtime types, so the runtime comes first.
        let mut includes = format!(
            "#define {} 1\n#include \"{}\"\n#include \"{}\"\n",
            profile.define(),
            self.config.runtime_include,
            self.config.profile_include()
        );
        for i in imports {
            includes.push_str(&format!("#include \"{}\"\n", i));
        }
        let file = cuda::File {
            items: vec![
                cuda::Item::Raw(header),
                cuda::Item::Blank,
                cuda::Item::Raw(includes),
                cuda::Item::Blank,
                cuda::Item::Namespace { name: module_name.to_string(), items: self.device_items() },
            ],
        };
        CudaCodeGenerator::new().generate(&file)
    }

    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
}

#[cfg(test)]
mod tests;
