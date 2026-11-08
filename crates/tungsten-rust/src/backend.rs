use std::borrow::Cow;

use tungsten_interface::naming::NamingRules;
use tungsten_interface::{Backend, BackendError, Compiler, Interface, OrchestrationCodegen};
use tungsten_ir::model::OptimizedProgram;

use crate::RCLCodegen;
use crate::TransformRclOrchestration;

/// Module names for the generated Rust code. The two modules import each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustConfig {
    pub density_module_name: String,
    pub orchestration_module_name: String,
}

impl Default for RustConfig {
    fn default() -> Self {
        Self {
            density_module_name: "density_function".to_string(),
            orchestration_module_name: "orchestration".to_string(),
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
        // The arena only lives for this call; the output is stored as finished strings.
        let arena = bumpalo::Bump::new();
        let device = RCLCodegen::new(program, &arena, interface);
        let mut orchestration = TransformRclOrchestration::new(
            Box::new(device),
            &arena,
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

/// Rust settings on [`Compiler`]. Each one sets the matching [`RustConfig`] field.
pub trait RustCompile: Sized {
    fn density_module_name(self, n: impl Into<String>) -> Self;
    fn orchestration_module_name(self, n: impl Into<String>) -> Self;
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
}
