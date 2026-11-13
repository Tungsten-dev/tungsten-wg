//! Facade over the `tungsten-*` workspace crates.
//!
//! Build an [`Program`](ir::model::Program) and a [`check::Registry`] that
//! declares the externs and records it uses, then compile with one backend through
//! [`Compiler`]. The result is that backend's own output type. Backend settings are extension
//! traits on `Compiler<ThatBackend>` (`RustCompile`, `JavaCompile`, `CudaCompile`,
//! `InspectCompile`), and the optimization passes run inside `compile`: the backend proposes
//! them and [`Compiler::passes`] overrides them. Custom backends implement [`Backend`] and
//! their own extension trait, the same way the built-in ones do.

pub use tungsten_builder as builder;
pub use tungsten_check as check;
#[doc(hidden)]
pub use tungsten_codegen as codegen;
pub use tungsten_interface as interface;
pub use tungsten_ir as ir;
#[doc(hidden)]
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

/// A registry from builder declaration groups (`extern_fns!`, `record_decls!` and
/// `auto_initializers!` with `group`).
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
        let fields: Vec<(&str, ir::VariableType)> = r
            .fields
            .iter()
            .map(|(n, t)| (n.as_str(), t.clone()))
            .collect();
        rb = rb.record(&r.name, &fields);
    }
    for i in &all.initializers {
        rb = rb.auto_initializer(&i.name, &i.params, i.ret.clone());
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
        tungsten_builder::auto_initializers! {
            group initializers;
            make_table(I64, Str): handle "Table",
            derive_salt(I64, F64, Bool): I64,
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
    fn initializers_reach_the_registry() {
        let r = registry_from([group_a(), b::initializers(), b::initializers()]).unwrap();
        let t = r.initializer("make_table").unwrap();
        assert_eq!(t.params, [ir::AutoArgType::I64, ir::AutoArgType::Str]);
        assert!(matches!(t.ret, check::core_ir::Type::Handle(_)));
        let s = r.initializer("derive_salt").unwrap();
        assert_eq!(
            s.params,
            [
                ir::AutoArgType::I64,
                ir::AutoArgType::F64,
                ir::AutoArgType::Bool
            ]
        );
        assert_eq!(s.ret, check::core_ir::Type::I64);
        assert!(r.initializer("nothing").is_none());

        // The registry checks a program built from the same declarations.
        let mut pb = ProgramBuilder::new();
        let seed = pb.param("seed", I64);
        let salt = pb.auto_const(
            "salt",
            I64,
            "derive_salt",
            [seed.into(), 1.5.into(), true.into()],
        );
        let mut f = FnBuilder::new("m", F64);
        f.ret(builder::Expr::from(salt).cast(F64));
        let u = pb.finish_unit(f);
        pb.entry(u, Grid::new((1, 1, 1)));
        check::prepare(&pb.build().unwrap(), &r).unwrap();
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
        assert!(matches!(
            errs.0[0].kind,
            builder::BuildErrorKind::ArgumentType { index: 0, .. }
        ));
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

/// Compiles the book's getting-started snippets as doctests.
#[cfg(doctest)]
#[doc = include_str!("../book/src/getting-started.md")]
pub struct BookGettingStarted;
