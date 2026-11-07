//! Facade over the `tungsten-*` workspace crates.
//!
//! Build an [`OptimizedProgram`](ir::model::OptimizedProgram), create the backends you need, and
//! pass them to [`compile`]. Each backend keeps its own output.

pub use tungsten_codegen as codegen;
pub use tungsten_interface as interface;
pub use tungsten_ir as ir;
pub use tungsten_legacy as spmt;
pub use tungsten_optimizer as optimizer;

pub use tungsten_legacy::{body, build, extern_fns, pos3, vec3};

#[cfg(feature = "rust")]
pub use tungsten_rust as rust;
#[cfg(feature = "rust")]
pub use tungsten_rust::backend::{RustBackend, RustConfig, RustOutput};

#[cfg(feature = "java")]
pub use tungsten_java as java;
#[cfg(feature = "java")]
pub use tungsten_java::backend::{JavaBackend, JavaConfig, JavaOutput};

#[cfg(feature = "cuda")]
pub use tungsten_cuda as cuda;
#[cfg(feature = "cuda")]
pub use tungsten_cuda::backend::{CudaBackend, CudaConfig, CudaOutput};

#[cfg(feature = "inspect")]
pub use tungsten_inspect as inspect;
#[cfg(feature = "inspect")]
pub use tungsten_inspect::{InspectBackend, InspectConfig, InspectOutput};

#[cfg(feature = "parse")]
pub use tungsten_parse as parse;

#[cfg(feature = "check")]
pub use tungsten_check as check;

pub use tungsten_interface::{Backend, BackendError};

use tungsten_interface::Interface;
use tungsten_ir::model::OptimizedProgram;

/// Errors from [`compile`] (and `compile_checked`).
#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    /// A backend failed. `name` is its [`Backend::name`].
    #[error("backend `{name}` failed")]
    Backend {
        name: String,
        #[source]
        source: BackendError,
    },
    /// The program didn't lower to the core IR, or failed the core checker. No backend ran.
    #[cfg(feature = "check")]
    #[error("program is invalid")]
    Invalid(#[source] check::Invalid),
}

/// Compiles an optimized program with the given backends.
///
/// Each backend receives the same shared [`Interface`] built from the program and its naming rules.
/// Stops at the first failing backend. Backends that already ran keep their output.
pub fn compile(
    program: &OptimizedProgram,
    backends: &mut [&mut dyn Backend],
) -> Result<(), CompileError> {
    for backend in backends.iter_mut() {
        let interface = Interface::build(program, backend.naming_rules());
        backend
            .generate(program, &interface)
            .map_err(|source| CompileError::Backend {
                name: backend.name().to_string(),
                source,
            })?;
    }
    Ok(())
}

/// Lowers `program` to the core IR against `registry` (the externs and records the program may
/// use), runs tungsten-core's checker (types, flow, scoping, grids, cycles), and only compiles
/// if both succeed.
#[cfg(feature = "check")]
pub fn compile_checked(
    program: &OptimizedProgram,
    registry: &check::Registry,
    backends: &mut [&mut dyn Backend],
) -> Result<(), CompileError> {
    check::prepare(program, registry).map_err(CompileError::Invalid)?;
    compile(program, backends)
}

#[cfg(all(test, feature = "check"))]
mod check_tests {
    use super::*;
    use tungsten_interface::naming::NamingRules;
    use tungsten_ir::{
        Arena, ComputeUnit, EntryPoint, Expression, Function, Statement, VariableType,
    };

    struct Counting(usize, NamingRules);

    impl Counting {
        fn new() -> Self {
            Counting(0, NamingRules::inspect())
        }
    }

    impl Backend for Counting {
        fn name(&self) -> &str {
            "counting"
        }
        fn naming_rules(&self) -> &NamingRules {
            &self.1
        }
        fn generate(
            &mut self,
            _: &OptimizedProgram,
            _: &Interface,
        ) -> Result<(), BackendError> {
            self.0 += 1;
            Ok(())
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
        let unit = p.compute_units.insert(ComputeUnit::DensityFunction {
            function: main,
            density_inputs: vec![],
            permutation_table_inputs: vec![],
            host_inputs: vec![],
            helper_functions: vec![],
            constants: vec![],
            source_hash: 0,
        });
        p.entry_points.push(EntryPoint {
            density_function: unit,
            dimensions: (1, 1, 1),
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (1.0, 1.0, 1.0),
        });
        if !valid {
            // A ref from another arena.
            let mut other: Arena<ComputeUnit> = Arena::new();
            let foreign = other.insert(ComputeUnit::DensityFunction {
                function: main,
                density_inputs: vec![],
                permutation_table_inputs: vec![],
                host_inputs: vec![],
                helper_functions: vec![],
                constants: vec![],
                source_hash: 0,
            });
            p.entry_points[0].density_function = foreign;
        }
        p
    }

    fn empty_registry() -> check::Registry {
        check::RegistryBuilder::new().build().unwrap()
    }

    #[test]
    fn invalid_program_runs_no_backend() {
        let (mut a, mut b) = (Counting::new(), Counting::new());
        let r = compile_checked(&program(false), &empty_registry(), &mut [&mut a, &mut b]);
        assert!(matches!(r, Err(CompileError::Invalid(_))), "{r:?}");
        assert_eq!((a.0, b.0), (0, 0));
    }

    #[test]
    fn valid_program_runs_every_backend() {
        let (mut a, mut b) = (Counting::new(), Counting::new());
        compile_checked(&program(true), &empty_registry(), &mut [&mut a, &mut b]).unwrap();
        assert_eq!((a.0, b.0), (1, 1));
    }
}
