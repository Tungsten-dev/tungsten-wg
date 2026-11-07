//! Facade over the `tungsten-*` workspace crates.
//!
//! Build an [`OptimizedProgram`](ir::model::OptimizedProgram), create the backends you need, and
//! pass them to [`compile`]. Each backend keeps its own output.

pub use tungsten_builder as builder;
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

/// A registry from builder declaration groups (`extern_fns!`/`record_decls!` with `group`).
/// Identical declarations in several groups count once; conflicting ones are reported by
/// [`check::RegistryBuilder`].
#[cfg(feature = "check")]
pub fn registry_from(
    groups: impl IntoIterator<Item = builder::Decls>,
) -> Result<check::Registry, check::RegistryError> {
    let mut all = builder::Decls::new();
    for g in groups {
        all.merge(g);
    }
    let mut rb = check::RegistryBuilder::new();
    for e in &all.externs {
        rb = if e.domain_error {
            rb.extern_fn_with_domain_error(&e.name, &e.params, e.ret.clone())
        } else {
            rb.extern_fn(&e.name, &e.params, e.ret.clone())
        };
    }
    for r in &all.records {
        let fields: Vec<(&str, ir::VariableType)> =
            r.fields.iter().map(|(n, t)| (n.as_str(), t.clone())).collect();
        rb = rb.record(&r.name, &fields);
    }
    rb.build()
}

#[cfg(all(test, feature = "check"))]
mod registry_tests {
    use super::*;
    use builder::prelude::*;

    // Paths are explicit: the legacy `extern_fns!` is still re-exported at the facade root.
    tungsten_builder::extern_fns! {
        group group_a;
        twice(x: F64): F64,
        checked(x: F64): F64 @domain_error,
    }

    mod b {
        tungsten_builder::extern_fns! {
            group group_b;
            twice(x: F64): F64,
        }
        pub mod c {
            tungsten_builder::extern_fns! {
                group conflicting;
                twice(x: I32): I32,
            }
        }
        tungsten_builder::record_decls! {
            group records;
            Pair { a: F64, b: I32 },
            Wrap { p: "Pair", v: [F64; 3] },
        }
    }

    #[test]
    fn overlapping_groups_count_once() {
        let r = registry_from([group_a(), b::group_b(), b::records()]).unwrap();
        let (externs, records) = r.decls();
        assert_eq!(externs.len(), 2);
        assert!(externs[1].domain_error);
        assert_eq!(records.len(), 2);
        assert!(r.field_index(r.record_id("Wrap").unwrap(), "v").is_some());
    }

    #[test]
    fn conflicts_are_left_to_the_registry() {
        let e = registry_from([group_a(), b::c::conflicting()]).unwrap_err();
        assert_eq!(e, check::RegistryError::DuplicateExtern("twice".into()));
    }

    /// A generated call checks its arguments, and the program checks against the registry
    /// built from the same declarations.
    #[test]
    fn declared_calls_check() {
        let mut pb = ProgramBuilder::new();
        let mut f = FnBuilder::new("m", F64);
        f.ret(twice(checked(1.0)) + twice(2.0));
        let u = pb.finish_unit(f);
        pb.entry(u, Grid::new((1, 1, 1)));
        let p = pb.build().unwrap();
        let reg = registry_from([group_a()]).unwrap();
        check::prepare(&p, &reg).unwrap();

        let mut pb = ProgramBuilder::new();
        let mut f = FnBuilder::new("m", F64);
        f.ret(twice(1));
        pb.finish_unit(f);
        let Err(errs) = pb.build() else { panic!() };
        assert!(matches!(errs.0[0].kind, builder::BuildErrorKind::ArgumentType { index: 0, .. }));
    }
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

/// Compiles the builder README's examples as doctests. They use `registry_from`.
#[cfg(all(doctest, feature = "check"))]
#[doc = include_str!("../crates/tungsten-builder/README.md")]
pub struct BuilderReadme;

/// Compiles the root README's examples as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct Readme;
