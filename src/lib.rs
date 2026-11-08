//! Facade over the `tungsten-*` workspace crates.
//!
//! Build an [`OptimizedProgram`](ir::model::OptimizedProgram) and a [`check::Registry`] that
//! declares the externs and records it uses, then compile with one backend through
//! [`Compiler`]. The result is that backend's own output type. Backend settings are extension
//! traits on `Compiler<ThatBackend>` (`RustCompile`, `JavaCompile`, `CudaCompile`,
//! `InspectCompile`), and the optimization passes run inside `compile`: the backend proposes
//! them and [`Compiler::passes`] overrides them. Custom backends implement [`Backend`] and
//! their own extension trait, the same way the built-in ones do.

pub use tungsten_builder as builder;
pub use tungsten_check as check;
pub use tungsten_codegen as codegen;
pub use tungsten_interface as interface;
pub use tungsten_ir as ir;
pub use tungsten_optimizer as optimizer;

pub use tungsten_interface::{Backend, BackendError, CompileError, Compiler, Inline, Passes};

#[cfg(feature = "rust")]
pub use tungsten_rust as rust;
#[cfg(feature = "rust")]
pub use tungsten_rust::backend::{RustBackend, RustCompile, RustConfig, RustOutput};

#[cfg(feature = "java")]
pub use tungsten_java as java;
#[cfg(feature = "java")]
pub use tungsten_java::backend::{JavaBackend, JavaCompile, JavaConfig, JavaOutput};

#[cfg(feature = "cuda")]
pub use tungsten_cuda as cuda;
#[cfg(feature = "cuda")]
pub use tungsten_cuda::backend::{CudaBackend, CudaCompile, CudaConfig, CudaOutput};

#[cfg(feature = "inspect")]
pub use tungsten_inspect as inspect;
#[cfg(feature = "inspect")]
pub use tungsten_inspect::{InspectBackend, InspectCompile, InspectConfig, InspectOutput};

#[cfg(feature = "parse")]
pub use tungsten_parse as parse;

/// A registry from builder declaration groups (`extern_fns!`/`record_decls!` with `group`).
/// Identical declarations in several groups count once; conflicting ones are reported by
/// [`check::RegistryBuilder`].
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

#[cfg(test)]
mod registry_tests {
    use super::*;
    use builder::prelude::*;

    // Paths are explicit, so the macro name stays unambiguous.
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

/// Compiles the builder README's examples as doctests. They use `registry_from`.
#[cfg(doctest)]
#[doc = include_str!("../crates/tungsten-builder/README.md")]
pub struct BuilderReadme;

/// Compiles the root README's examples as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct Readme;
