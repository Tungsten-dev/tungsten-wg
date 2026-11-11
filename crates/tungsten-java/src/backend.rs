use std::borrow::Cow;

use tungsten_interface::naming::{NamingRules, java_sanitize};
use tungsten_interface::{
    Backend, BackendError, BufferBounds, Compiler, Feature, Interface, OrchestrationCodegen,
};
use tungsten_ir::model::OptimizedProgram;

use crate::pools::Pools;
use crate::{JavaCodegen, TransformJavaOrchestration};

/// Names for the generated Java file. An empty `package` emits no package declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaConfig {
    pub package: String,
    /// Sanitized to a legal Java identifier if needed (with a warning).
    pub class_name: String,
    /// Package of the hand-written runtime, imported with `.*`.
    pub runtime_package: String,
    /// How reads of density inputs are checked (`specs/workers.md` §5.1). Default `PerBuffer`.
    /// `Unchecked` behaves like `Slab`: the JVM checks the end of the pool either way.
    pub buffer_bounds: BufferBounds,
}

impl Default for JavaConfig {
    fn default() -> Self {
        Self {
            package: "tungsten.generated".to_string(),
            class_name: "GeneratedWorldgen".to_string(),
            runtime_package: "tungsten.runtime".to_string(),
            buffer_bounds: BufferBounds::PerBuffer,
        }
    }
}

impl JavaConfig {
    pub fn to_backend(self) -> JavaBackend {
        JavaBackend::new(self)
    }
}

/// Generated Java source, returned by [`Backend::generate`] and `Compiler::compile`.
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
}

impl JavaBackend {
    pub fn new(config: JavaConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &JavaConfig {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut JavaConfig {
        &mut self.config
    }
}

impl Backend for JavaBackend {
    type Output = JavaOutput;

    fn name(&self) -> &str {
        "java"
    }

    fn generate(
        &self,
        program: &OptimizedProgram,
        interface: &Interface,
    ) -> Result<JavaOutput, BackendError> {
        tungsten_interface::check_implemented(program, &[Feature::Mappings, Feature::Refinement])?;
        tungsten_rust::mapping::check_emittable(program, interface)?;
        tungsten_rust::backend::check_slots(program, interface)?;
        let class_name = java_sanitize(&self.config.class_name);
        let naming_rules = self.naming_rules();
        let pools = Pools::new(program, interface, self.config.buffer_bounds);
        let device = JavaCodegen::new(program, interface, &pools, &naming_rules, &class_name);
        let mut orchestration = TransformJavaOrchestration::new(
            device,
            pools.clone(),
            &self.config.package,
            &self.config.class_name,
            &self.config.runtime_package,
        );
        orchestration.convert_program(program, interface);
        orchestration.check_limits()?;

        Ok(JavaOutput {
            source: orchestration.output(),
            warnings: orchestration.warnings(),
        })
    }

    /// `java()` with the sanitized class name reserved, so no member collides with it.
    fn naming_rules(&self) -> Cow<'_, NamingRules> {
        Cow::Owned(NamingRules::java().with_reserved([java_sanitize(&self.config.class_name)]))
    }

    fn warnings(output: &JavaOutput) -> &[String] {
        &output.warnings
    }
}

/// Java settings on [`Compiler`]. Each one sets the matching [`JavaConfig`] field.
pub trait JavaCompile: Sized {
    fn package(self, p: impl Into<String>) -> Self;
    fn class_name(self, c: impl Into<String>) -> Self;
    fn runtime_package(self, p: impl Into<String>) -> Self;
    fn buffer_bounds(self, b: BufferBounds) -> Self;
}

impl JavaCompile for Compiler<JavaBackend> {
    fn package(mut self, p: impl Into<String>) -> Self {
        self.backend_mut().config_mut().package = p.into();
        self
    }

    fn class_name(mut self, c: impl Into<String>) -> Self {
        self.backend_mut().config_mut().class_name = c.into();
        self
    }

    fn runtime_package(mut self, p: impl Into<String>) -> Self {
        self.backend_mut().config_mut().runtime_package = p.into();
        self
    }

    fn buffer_bounds(mut self, b: BufferBounds) -> Self {
        self.backend_mut().config_mut().buffer_bounds = b;
        self
    }
}
