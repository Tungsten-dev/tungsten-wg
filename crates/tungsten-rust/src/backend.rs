use tungsten_interface::naming::NamingRules;
use tungsten_interface::{Backend, BackendError, Interface, OrchestrationCodegen};
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

/// Generated Rust source, available after [`Backend::generate`].
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
    output: Option<RustOutput>,
    naming_rules: NamingRules,
}

impl RustBackend {
    pub fn new(config: RustConfig) -> Self {
        Self {
            config,
            output: None,
            naming_rules: NamingRules::rust(),
        }
    }

    /// The generated code, or `None` before a successful [`Backend::generate`].
    pub fn output(&self) -> Option<&RustOutput> {
        self.output.as_ref()
    }
}

impl Backend for RustBackend {
    fn name(&self) -> &str {
        "rust"
    }

    fn generate(
        &mut self,
        program: &OptimizedProgram,
        interface: &Interface,
    ) -> Result<(), BackendError> {
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
        self.output = Some(RustOutput {
            density_module,
            orchestration_module,
            full_source,
        });
        Ok(())
    }

    fn naming_rules(&self) -> &NamingRules {
        &self.naming_rules
    }
}
