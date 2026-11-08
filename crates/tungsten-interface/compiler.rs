/*
`Compiler<B>`: one backend per compile, typed output. It validates the backend, checks the
program, runs the passes, builds the `Interface` and calls `generate`. It depends on no backend:
each backend crate adds its own settings as an extension trait on `Compiler<ItsBackend>`.
*/

use std::sync::Arc;

use tungsten_check as check;
use tungsten_ir::model::OptimizedProgram;

use crate::{Backend, BackendError, Interface};

/// The optimization passes `Compiler` runs, in a fixed order. Plain data: a backend proposes
/// them through [`Backend::default_passes`] and the frontend overrides them with
/// [`Compiler::passes`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Passes {
    pub inline: Inline,
}

impl Passes {
    /// Sets the inlining pass. `Passes` is `#[non_exhaustive]`, so other crates can't use a
    /// struct expression.
    pub fn inline(mut self, inline: Inline) -> Self {
        self.inline = inline;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Inline {
    /// No inlining.
    #[default]
    None,
    /// Inline every compute unit that can be inlined.
    All,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CompileError {
    /// A backend failed, or its configuration is invalid. `name` is its [`Backend::name`].
    #[error("backend `{name}` failed")]
    Backend {
        name: String,
        #[source]
        source: BackendError,
    },
    /// The program didn't lower to the core IR, or failed the core checker. No code was generated.
    #[error("program is invalid")]
    Invalid(#[source] check::Invalid),
    /// The program passed the checker but failed it after the passes: a bug in a pass.
    #[error("program became invalid after optimization passes (compiler bug)")]
    InvalidAfterPasses(#[source] check::Invalid),
    /// Warnings were denied and the backend produced some. The output is dropped.
    #[error("backend `{name}` produced {} warning(s) and warnings are denied", warnings.len())]
    Warnings { name: String, warnings: Vec<String> },
}

/// A compile of one program with one backend. Backend settings are extension traits implemented
/// in the backend's crate (`RustCompile`, `CudaCompile`, ...), reached through
/// [`backend_mut`](Self::backend_mut).
pub struct Compiler<B: Backend> {
    program: OptimizedProgram,
    registry: Arc<check::Registry>,
    backend: B,
    /// `None` means `backend.default_passes()`.
    passes: Option<Passes>,
    deny_warnings: bool,
    /// Runs after the real passes. Reports whether it changed the program.
    #[cfg(test)]
    pub(crate) test_pass: Option<fn(&mut OptimizedProgram) -> bool>,
}

impl<B: Backend> Compiler<B> {
    /// `registry` declares every extern and record the program uses. It can be an owned
    /// `Registry`, or an `Arc` shared by the `Compiler`s of several targets.
    pub fn new(
        program: OptimizedProgram,
        registry: impl Into<Arc<check::Registry>>,
        backend: B,
    ) -> Self {
        Self {
            program,
            registry: registry.into(),
            backend,
            passes: None,
            deny_warnings: false,
            #[cfg(test)]
            test_pass: None,
        }
    }

    /// Replaces the backend's default passes entirely. The last call wins.
    pub fn passes(mut self, passes: Passes) -> Self {
        self.passes = Some(passes);
        self
    }

    /// Turns any warning in the output into [`CompileError::Warnings`].
    pub fn deny_warnings(mut self) -> Self {
        self.deny_warnings = true;
        self
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// For extension traits, built-in and custom.
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn compile(mut self) -> Result<B::Output, CompileError> {
        let name = || self.backend.name().to_string();

        self.backend
            .validate()
            .map_err(|source| CompileError::Backend {
                name: name(),
                source,
            })?;

        check::prepare(&self.program, &self.registry).map_err(CompileError::Invalid)?;

        let passes = self
            .passes
            .take()
            .unwrap_or_else(|| self.backend.default_passes());
        let mut changed = false;
        match passes.inline {
            Inline::None => {}
            Inline::All => {
                changed |= tungsten_optimizer::perform_inline(&mut self.program).inlined > 0;
            }
        }
        #[cfg(test)]
        if let Some(pass) = self.test_pass {
            changed |= pass(&mut self.program);
        }
        if changed {
            check::prepare(&self.program, &self.registry)
                .map_err(CompileError::InvalidAfterPasses)?;
        }

        let interface = Interface::build(&self.program, &self.backend.naming_rules());
        let output = self
            .backend
            .generate(&self.program, &interface)
            .map_err(|source| CompileError::Backend {
                name: name(),
                source,
            })?;

        if self.deny_warnings {
            let warnings = B::warnings(&output);
            if !warnings.is_empty() {
                return Err(CompileError::Warnings {
                    name: name(),
                    warnings: warnings.to_vec(),
                });
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::cell::Cell;

    use tungsten_ir::{
        Arena, ComputeUnit, EntryPoint, Expression, Function, Statement, VariableType,
    };

    use super::*;
    use crate::naming::NamingRules;

    /// Counts `generate` calls and returns the warnings it was configured with.
    struct Counting {
        calls: Cell<usize>,
        rules: NamingRules,
        invalid_config: bool,
        passes: Passes,
        warnings: Vec<String>,
    }

    impl Counting {
        fn new() -> Self {
            Counting {
                calls: Cell::new(0),
                rules: NamingRules::inspect(),
                invalid_config: false,
                passes: Passes::default(),
                warnings: vec![],
            }
        }
    }

    struct Out {
        warnings: Vec<String>,
        /// Compute units in the program `generate` saw.
        units: usize,
    }

    impl Backend for Counting {
        type Output = Out;
        fn name(&self) -> &str {
            "counting"
        }
        fn naming_rules(&self) -> Cow<'_, NamingRules> {
            Cow::Borrowed(&self.rules)
        }
        fn validate(&self) -> Result<(), BackendError> {
            if self.invalid_config {
                Err(BackendError::InvalidConfig("bad".into()))
            } else {
                Ok(())
            }
        }
        fn default_passes(&self) -> Passes {
            self.passes.clone()
        }
        fn generate(&self, p: &OptimizedProgram, _: &Interface) -> Result<Out, BackendError> {
            self.calls.set(self.calls.get() + 1);
            Ok(Out {
                warnings: self.warnings.clone(),
                units: p.compute_units.iter().count(),
            })
        }
        fn warnings(o: &Out) -> &[String] {
            &o.warnings
        }
    }

    fn unit(function: tungsten_ir::model::Ref<Function>) -> ComputeUnit {
        ComputeUnit::DensityFunction {
            function,
            density_inputs: vec![],
            permutation_table_inputs: vec![],
            host_inputs: vec![],
            helper_functions: vec![],
            constants: vec![],
            source_hash: 0,
        }
    }

    fn program(valid: bool) -> OptimizedProgram {
        let mut p = OptimizedProgram {
            compute_units: Arena::new(),
            functions: Arena::new(),
            entry_points: vec![],
            constants: Arena::new(),
            host_inputs: Arena::new(),
            permutation_tables: Arena::new(),
            density_inputs: Arena::new(),
        };
        let main = p.functions.insert(Function {
            canonical_name: Some("main".into()),
            host_inputs: vec![],
            body: vec![Statement::Return(Expression::Double(1.0))],
            variables: Arena::new(),
            parameters: vec![],
            return_type: VariableType::F64,
            source_hash: 0,
        });
        let u = p.compute_units.insert(unit(main));
        p.entry_points.push(EntryPoint {
            density_function: u,
            dimensions: (1, 1, 1),
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (1.0, 1.0, 1.0),
        });
        if !valid {
            // A ref from another arena.
            let mut other: Arena<ComputeUnit> = Arena::new();
            p.entry_points[0].density_function = other.insert(unit(main));
        }
        p
    }

    fn registry() -> check::Registry {
        check::RegistryBuilder::new().build().unwrap()
    }

    /// `generate` runs through a shared counter, since `compile` consumes the backend.
    struct Probe<'a>(&'a Cell<usize>, Counting);
    impl Backend for Probe<'_> {
        type Output = Out;
        fn name(&self) -> &str {
            self.1.name()
        }
        fn naming_rules(&self) -> Cow<'_, NamingRules> {
            self.1.naming_rules()
        }
        fn validate(&self) -> Result<(), BackendError> {
            self.1.validate()
        }
        fn default_passes(&self) -> Passes {
            self.1.default_passes()
        }
        fn generate(&self, p: &OptimizedProgram, i: &Interface) -> Result<Out, BackendError> {
            self.0.set(self.0.get() + 1);
            self.1.generate(p, i)
        }
        fn warnings(o: &Out) -> &[String] {
            Counting::warnings(o)
        }
    }

    fn run(
        valid: bool,
        b: Counting,
        f: impl FnOnce(Compiler<Probe<'static>>) -> Compiler<Probe<'static>>,
    ) -> (Result<Out, CompileError>, usize) {
        let reg = registry();
        let calls: &'static Cell<usize> = Box::leak(Box::new(Cell::new(0)));
        let r = f(Compiler::new(program(valid), reg, Probe(calls, b))).compile();
        (r, calls.get())
    }

    #[test]
    fn invalid_program_runs_no_backend() {
        let (r, calls) = run(false, Counting::new(), |c| c);
        assert!(matches!(r, Err(CompileError::Invalid(_))));
        assert_eq!(calls, 0);
    }

    #[test]
    fn valid_program_generates_once() {
        let (r, calls) = run(true, Counting::new(), |c| c);
        assert_eq!(r.unwrap().units, 1);
        assert_eq!(calls, 1);
    }

    #[test]
    fn validate_failure_stops_before_check_and_generate() {
        // The program is invalid too, so reaching the check would give `Invalid`.
        let b = Counting {
            invalid_config: true,
            ..Counting::new()
        };
        let (r, calls) = run(false, b, |c| c);
        match r {
            Err(CompileError::Backend {
                name,
                source: BackendError::InvalidConfig(_),
            }) => {
                assert_eq!(name, "counting")
            }
            _ => panic!("expected InvalidConfig"),
        }
        assert_eq!(calls, 0);
    }

    #[test]
    fn config_edit_through_backend_mut_is_seen() {
        let (r, calls) = run(true, Counting::new(), |mut c| {
            c.backend_mut().1.invalid_config = true;
            c
        });
        assert!(matches!(r, Err(CompileError::Backend { .. })));
        assert_eq!(calls, 0);
    }

    #[test]
    fn deny_warnings() {
        let warn = Counting {
            warnings: vec!["w".into()],
            ..Counting::new()
        };
        let (r, _) = run(
            true,
            Counting {
                warnings: vec!["w".into()],
                ..Counting::new()
            },
            |c| c,
        );
        assert_eq!(r.unwrap().warnings, ["w"]);
        let (r, _) = run(true, warn, |c| c.deny_warnings());
        match r {
            Err(CompileError::Warnings { name, warnings }) => {
                assert_eq!((name.as_str(), warnings.len()), ("counting", 1))
            }
            _ => panic!("expected Warnings"),
        }
        let (r, _) = run(true, Counting::new(), |c| c.deny_warnings());
        assert!(r.is_ok());
    }

    #[test]
    fn passes_override_backend_defaults() {
        let b = Counting {
            passes: Passes::default().inline(Inline::All),
            ..Counting::new()
        };
        let reg = registry();
        let c = Compiler::new(program(true), reg.clone(), b);
        assert_eq!(c.backend().default_passes().inline, Inline::All);
        let c = c
            .passes(Passes::default().inline(Inline::All))
            .passes(Passes::default());
        assert_eq!(
            c.passes,
            Some(Passes::default()),
            "last call wins, and None is explicit"
        );
    }

    #[test]
    fn recheck_only_when_a_pass_changed_the_program() {
        fn corrupt(p: &mut OptimizedProgram) -> bool {
            let mut other: Arena<ComputeUnit> = Arena::new();
            let f = p.functions.iter().next().map(|(r, _)| r).unwrap();
            p.entry_points[0].density_function = other.insert(unit(f));
            true
        }
        fn unchanged(_: &mut OptimizedProgram) -> bool {
            false
        }
        let reg = registry();

        let mut c = Compiler::new(program(true), reg.clone(), Counting::new());
        c.test_pass = Some(corrupt);
        assert!(matches!(
            c.compile(),
            Err(CompileError::InvalidAfterPasses(_))
        ));

        // A pass that changes nothing leaves a valid program valid.
        let mut c = Compiler::new(program(true), reg.clone(), Counting::new());
        c.test_pass = Some(unchanged);
        assert!(c.compile().is_ok());
    }
}
