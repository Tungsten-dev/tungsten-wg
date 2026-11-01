use tungsten_interface::naming::{NamingRules, java_sanitize};
use tungsten_interface::{Backend, BackendError, Interface, OrchestrationCodegen};
use tungsten_ir::model::OptimizedProgram;

use crate::{JavaCodegen, TransformJavaOrchestration};

/// Names for the generated Java file. An empty `package` emits no package declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaConfig {
    pub package: String,
    /// Sanitized to a legal Java identifier if needed (with a warning).
    pub class_name: String,
    /// Package of the hand-written runtime, imported with `.*`.
    pub runtime_package: String,
}

impl Default for JavaConfig {
    fn default() -> Self {
        Self {
            package: "tungsten.generated".to_string(),
            class_name: "GeneratedWorldgen".to_string(),
            runtime_package: "tungsten.runtime".to_string(),
        }
    }
}

impl JavaConfig {
    pub fn to_backend(self) -> JavaBackend {
        JavaBackend::new(self)
    }
}

/// Generated Java source, available after [`Backend::generate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaOutput {
    /// The single compilation unit.
    pub source: String,
    /// Non-fatal diagnostics: class-name sanitizing and oversized methods.
    pub warnings: Vec<String>,
}

/// Java backend: generates one Java 21 source file for an optimized program.
#[derive(Debug)]
pub struct JavaBackend {
    config: JavaConfig,
    output: Option<JavaOutput>,
    /// Reserves the generated class name so no member collides with it.
    naming_rules: NamingRules,
}

impl JavaBackend {
    pub fn new(config: JavaConfig) -> Self {
        let naming_rules = NamingRules::java().with_reserved([java_sanitize(&config.class_name)]);
        Self {
            config,
            output: None,
            naming_rules,
        }
    }

    /// The generated code, or `None` before a successful [`Backend::generate`].
    pub fn output(&self) -> Option<&JavaOutput> {
        self.output.as_ref()
    }
}

impl Backend for JavaBackend {
    fn name(&self) -> &str {
        "java"
    }

    fn generate(
        &mut self,
        program: &OptimizedProgram,
        interface: &Interface,
    ) -> Result<(), BackendError> {
        let class_name = java_sanitize(&self.config.class_name);
        let device = JavaCodegen::new(program, interface, &self.naming_rules, &class_name);
        let mut orchestration = TransformJavaOrchestration::new(
            device,
            &self.config.package,
            &self.config.class_name,
            &self.config.runtime_package,
        );
        orchestration.convert_program(program, interface);

        self.output = Some(JavaOutput {
            source: orchestration.output(),
            warnings: orchestration.warnings(),
        });
        Ok(())
    }

    fn naming_rules(&self) -> &NamingRules {
        &self.naming_rules
    }
}
