//! Java generator from optimized SPMT. Mirrors `tungsten-rust` and is validated against its output.
//!
//! The generator is split into a device side and an orchestration side:
//! - Device side ([`JavaCodegen`], implementing [`DeviceCodegen`]): turns compute units into static
//!   density functions, helper functions, constants and one kernel per scheduled instance.
//! - Orchestration side ([`TransformJavaOrchestration`]): the nested `HostConsts`, `AutoConsts`,
//!   `InternalBuffers` and `Orchestration` classes, and assembly of the single compilation unit.
//!
//! Every density and helper function starts with the same parameters (see [`uniform_parameters`]):
//! `PositionIterator p, Config config, InternalBuffers buffers, AutoConsts auto_constants,
//! HostConsts host_consts`. Helper-specific parameters follow them. All names come from the
//! shared [`Interface`] built with [`NamingRules::java`], so caller and callee always agree.
//!
//! Modules:
//! - `types`: type and operator conversion.
//! - `converter`: per-function state (variables, hoisted statements, type derivation).
//! - `expression` / `statement`: body conversion.
//! - `function`: density functions, helpers, constants and kernels.
//! - `orchestration`: nested classes, entry methods, file assembly.
//! - `size`: bytecode size estimates and warnings.
//!
//! Assumed runtime (not generated, imported from the runtime package): `Vec3`, `Pos3`, `Config`,
//! `PositionIterator`, `PerlinNoiseSampler`, `InterpolatedNoiseSampler`, the `*Ref` holders and
//! `Helpers` (extern functions and auto-const initializers). See SPEC.md §6 for the contract.

use std::collections::HashSet;

use tungsten_interface::naming::NamingRules;
use tungsten_interface::{DeviceCodegen, Instance, Interface};
use crate::model as java;
use crate::model::codegen::JavaCodeGenerator;
use tungsten_ir::model::{ComputeUnit, Constant, Function, OptimizedProgram, Ref};

pub mod backend;
pub mod converter;
pub mod expression;
pub mod function;
pub mod model;
pub mod orchestration;
pub mod size;
pub mod statement;
pub mod types;

pub use backend::{JavaBackend, JavaConfig, JavaOutput};
pub use orchestration::TransformJavaOrchestration;

/// Names of runtime and generated classes that the generated code refers to by name.
pub const PERLIN_NOISE_SAMPLER_CLASS: &str = "PerlinNoiseSampler";
pub const BASE3D_NOISE_SAMPLER_CLASS: &str = "InterpolatedNoiseSampler";
pub const HELPERS_CLASS: &str = "Helpers";
pub const CONFIG_CLASS: &str = "Config";
pub const POSITION_ITERATOR_CLASS: &str = "PositionIterator";
pub const HOST_CONSTS_CLASS: &str = "HostConsts";
pub const AUTO_CONSTS_CLASS: &str = "AutoConsts";
pub const INTERNAL_BUFFERS_CLASS: &str = "InternalBuffers";
pub const ORCHESTRATION_CLASS: &str = "Orchestration";

/// Uniform parameter names shared by every generated density and helper function, in order.
pub const PARAM_POS: &str = "p";
pub const PARAM_CONFIG: &str = "config";
pub const PARAM_BUFFERS: &str = "buffers";
pub const PARAM_AUTO_CONSTS: &str = "auto_constants";
pub const PARAM_HOST_CONSTS: &str = "host_consts";

/// The uniform leading parameters of every density and helper function.
pub fn uniform_parameters() -> Vec<(java::Type, String)> {
    let c = java::Type::class;
    vec![
        (c(POSITION_ITERATOR_CLASS), PARAM_POS.into()),
        (c(CONFIG_CLASS), PARAM_CONFIG.into()),
        (c(INTERNAL_BUFFERS_CLASS), PARAM_BUFFERS.into()),
        (c(AUTO_CONSTS_CLASS), PARAM_AUTO_CONSTS.into()),
        (c(HOST_CONSTS_CLASS), PARAM_HOST_CONSTS.into()),
    ]
}

/// The uniform parameters passed through unchanged, as call arguments.
pub fn uniform_arguments() -> Vec<java::Expression> {
    uniform_parameters().into_iter().map(|(_, n)| java::Expression::Name(n)).collect()
}

/// Device-side generator: converts compute units of an optimized program into Java members.
///
/// Generation is idempotent per item (the `done_*` sets), so units, helpers and constants shared
/// between instances are emitted once.
pub struct JavaCodegen<'a> {
    source: &'a OptimizedProgram,
    /// Names and schedule shared with the orchestration side.
    naming: &'a Interface,
    rules: &'a NamingRules,
    /// The generated top-level class; constants are qualified with it.
    class_name: String,
    /// Density and helper functions, in generation order.
    functions: Vec<java::Method>,
    kernels: Vec<java::Method>,
    /// Constants in generation order; emitted in dependency order.
    constants: Vec<Ref<Constant>>,
    done_units: HashSet<Ref<ComputeUnit>>,
    done_functions: HashSet<Ref<Function>>,
    done_constants: HashSet<Ref<Constant>>,
    done_kernels: HashSet<Instance>,
}

impl<'a> JavaCodegen<'a> {
    /// `naming` must be the interface built for `source` with `rules`; nothing is generated until
    /// asked. `class_name` is the (already sanitized) generated top-level class, which `rules` must
    /// reserve so no member collides with it.
    pub fn new(
        source: &'a OptimizedProgram,
        naming: &'a Interface,
        rules: &'a NamingRules,
        class_name: &str,
    ) -> Self {
        Self {
            source,
            naming,
            rules,
            class_name: class_name.to_string(),
            functions: Vec::new(),
            kernels: Vec::new(),
            constants: Vec::new(),
            done_units: HashSet::new(),
            done_functions: HashSet::new(),
            done_constants: HashSet::new(),
            done_kernels: HashSet::new(),
        }
    }

    pub fn naming(&self) -> &'a Interface {
        self.naming
    }

    pub fn rules(&self) -> &'a NamingRules {
        self.rules
    }

    /// Constants in dependency order, as `static final` fields.
    pub fn constant_fields(&self) -> Vec<java::Field> {
        function::sort_constants(self.source, &self.constants)
            .into_iter()
            .map(|c| function::convert_constant(self.source, self.naming, self.rules, &self.class_name, c))
            .collect()
    }

    /// Density functions and helpers, followed by kernels.
    pub fn methods(&self) -> Vec<java::Method> {
        self.functions.iter().chain(&self.kernels).cloned().collect()
    }

    /// A class holding only the device members (constants, functions, kernels).
    fn device_class(&self, name: &str) -> java::Class {
        java::Class {
            modifiers: "final".into(),
            name: name.to_string(),
            fields: self.constant_fields(),
            methods: self.methods(),
            ..Default::default()
        }
    }

    /// Generates `f`, the helpers it calls, and the constants those use.
    fn generate_helper(&mut self, f: Ref<Function>) {
        let mut pending = vec![f];
        while let Some(f) = pending.pop() {
            if !self.done_functions.insert(f) {
                continue;
            }
            let converted = function::convert_helper(self.source, self.naming, self.rules, &self.class_name, f);
            self.functions.push(converted.method);
            pending.extend(converted.called);
            for c in converted.constants {
                self.generate_constant(c);
            }
        }
    }

    fn generate_constant(&mut self, c: Ref<Constant>) {
        let mut pending = vec![c];
        while let Some(c) = pending.pop() {
            if !self.done_constants.insert(c) {
                continue;
            }
            self.constants.push(c);
            tungsten_rust::expression::const_refs(&self.source.constants[c].value, &mut pending);
        }
    }

    /// Generates the kernel for one scheduled instance of a compute unit (and the unit itself).
    pub fn generate_kernel_for_instance(&mut self, inst: Instance) {
        self.generate_compute_unit(inst.cu);
        if !self.done_kernels.insert(inst) {
            return;
        }
        self.kernels.push(function::build_kernel(self.source, self.naming, &inst));
    }
}

impl DeviceCodegen for JavaCodegen<'_> {
    /// Generates one density function per dimension the unit is scheduled at, plus its helpers and constants.
    fn generate_compute_unit(&mut self, cu: Ref<ComputeUnit>) {
        if !self.done_units.insert(cu) {
            return;
        }
        let source = self.source;
        let ComputeUnit::DensityFunction { helper_functions, constants, .. } = &source.compute_units[cu];

        for dimensions in self.naming.dimensions_of(cu) {
            let converted =
                function::convert_density(source, self.naming, self.rules, &self.class_name, cu, dimensions);
            self.functions.push(converted.method);
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
        "java"
    }

    /// The device members alone, as a class named `module_name` importing `imports`.
    /// The full program is assembled by [`TransformJavaOrchestration`].
    fn render(&self, module_name: &str, imports: &[String]) -> String {
        let unit = java::CompilationUnit {
            header: vec![],
            package: None,
            imports: imports.to_vec(),
            class: self.device_class(module_name),
        };
        JavaCodeGenerator::new().generate(&unit)
    }

    /// Size warnings for the device members.
    fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        size::check_class(&self.device_class(&self.class_name), &mut warnings);
        warnings
    }
}

#[cfg(test)]
mod tests;
