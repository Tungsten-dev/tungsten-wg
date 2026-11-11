use std::borrow::Cow;

use tungsten_interface::naming::NamingRules;
use tungsten_interface::{
    Backend, BackendError, BufferBounds, Compiler, Instance, Interface, OrchestrationCodegen,
};
use tungsten_ir::model::{ComputeUnit, OptimizedProgram};

use crate::RCLCodegen;
use crate::TransformRclOrchestration;
use crate::pools::Pools;

/// Settings of the generated Rust code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustConfig {
    /// Names of the two generated modules, which import each other.
    pub density_module_name: String,
    pub orchestration_module_name: String,
    /// How reads of density inputs are checked (`specs/workers.md` §5.1). Default `PerBuffer`.
    pub buffer_bounds: BufferBounds,
}

impl Default for RustConfig {
    fn default() -> Self {
        Self {
            density_module_name: "density_function".to_string(),
            orchestration_module_name: "orchestration".to_string(),
            buffer_bounds: BufferBounds::PerBuffer,
        }
    }
}

impl RustConfig {
    pub fn to_backend(self) -> RustBackend {
        RustBackend::new(self)
    }
}

/// Generated Rust source, returned by [`Backend::generate`] and `Compiler::compile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustOutput {
    /// The density module (`density_function` by default).
    pub density_module: String,
    /// The orchestration module (`orchestration` by default).
    pub orchestration_module: String,
    /// Both modules, density first, as one source text.
    pub full_source: String,
}

/// Rust backend: generates the density and orchestration modules for an optimized program.
#[derive(Debug)]
pub struct RustBackend {
    config: RustConfig,
    naming_rules: NamingRules,
}

impl RustBackend {
    pub fn new(config: RustConfig) -> Self {
        Self {
            config,
            naming_rules: NamingRules::rust(),
        }
    }

    pub fn config(&self) -> &RustConfig {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut RustConfig {
        &mut self.config
    }
}

impl Backend for RustBackend {
    type Output = RustOutput;

    fn name(&self) -> &str {
        "rust"
    }

    fn generate(
        &self,
        program: &OptimizedProgram,
        interface: &Interface,
    ) -> Result<RustOutput, BackendError> {
        tungsten_interface::check_implemented(program, &[])?;
        check_slots(program, interface)?;
        let pools = Pools::new(program, interface, self.config.buffer_bounds);
        // The arena only lives for this call; the output is stored as finished strings.
        let arena = bumpalo::Bump::new();
        let device = RCLCodegen::new(program, &arena, interface, &pools);
        let mut orchestration = TransformRclOrchestration::new(
            Box::new(device),
            &arena,
            pools.clone(),
            &self.config.density_module_name,
            &self.config.orchestration_module_name,
        );
        orchestration.convert_program(program, interface);

        let density_module = orchestration.density_module();
        let orchestration_module = orchestration.orchestration_module();
        let full_source = format!("{}\n\n{}", density_module, orchestration_module);
        Ok(RustOutput {
            density_module,
            orchestration_module,
            full_source,
        })
    }

    fn naming_rules(&self) -> Cow<'_, NamingRules> {
        Cow::Borrowed(&self.naming_rules)
    }
}

/// Every input a scheduled unit reads must have a slot in that worker. The checker guarantees it
/// (a unit reads only its listed inputs), but the low-level path doesn't run the checker.
fn check_slots(program: &OptimizedProgram, interface: &Interface) -> Result<(), BackendError> {
    for e in 0..program.entry_points.len() {
        for inst in interface.worker_schedule(e).iter().flatten() {
            let ComputeUnit::DensityFunction { function, .. } = &program.compute_units[inst.cu];
            for &d in interface.input_params(*function) {
                let input = Instance::from_input(&program.density_inputs[d]);
                if interface.slot(e, &input).is_none() {
                    return Err(BackendError::Codegen(format!(
                        "input `{}` is read by a unit that doesn't list it",
                        interface.offset_param(d).trim_start_matches("off_")
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Rust settings on [`Compiler`]. Each one sets the matching [`RustConfig`] field.
pub trait RustCompile: Sized {
    fn density_module_name(self, n: impl Into<String>) -> Self;
    fn orchestration_module_name(self, n: impl Into<String>) -> Self;
    fn buffer_bounds(self, b: BufferBounds) -> Self;
}

impl RustCompile for Compiler<RustBackend> {
    fn density_module_name(mut self, n: impl Into<String>) -> Self {
        self.backend_mut().config_mut().density_module_name = n.into();
        self
    }

    fn orchestration_module_name(mut self, n: impl Into<String>) -> Self {
        self.backend_mut().config_mut().orchestration_module_name = n.into();
        self
    }

    fn buffer_bounds(mut self, b: BufferBounds) -> Self {
        self.backend_mut().config_mut().buffer_bounds = b;
        self
    }
}
