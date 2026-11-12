//! Slices: real frontend patterns rewritten with the builder (spec §10). Each slice builds a
//! program that must be `isomorphic` to its twin `.tw` file in the testsuite corpus, where the
//! corpus runs the twin on the interpreter and every backend against hand-computed values. Each
//! slice also checks with core and round-trips through the printer.
//!
//! `REVIEW.md` next to this file places each slice beside the frontend code it came from and
//! lists the friction points found.

mod aquifer;
mod noise;
mod recursive;
mod shared;
mod top_surface;

use tungsten_builder::Program;
use tungsten_ir::isomorphic;
use tungsten_testsuite::corpus::{corpus_root, load_source};
use tungsten_testsuite::env::TestEnv;

/// The twin at `corpus/<path>.tw`.
fn twin(path: &str) -> Program {
    let file = corpus_root().join(format!("{path}.tw"));
    let src = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    load_source(&src, &TestEnv::new())
        .unwrap_or_else(|e| panic!("{path}: {e}"))
        .program
}

/// Checks a built slice: core accepts it, it equals its twin, and it survives printing.
fn check_slice(built: Result<Program, tungsten_builder::BuildErrors>, path: &str) {
    let p = built.unwrap_or_else(|e| panic!("{path}: build failed:\n{e}"));
    let env = TestEnv::new();
    let registry = env.spec().build().unwrap();
    if let Err(e) = tungsten_check::prepare(&p, &registry) {
        panic!("{path}: core rejected the slice:\n{e}\n{}", tungsten_parse::print(&p));
    }
    let t = twin(path);
    assert!(
        isomorphic(&p, &t),
        "{path}: the slice differs from its twin\nbuilt:\n{}\ntwin:\n{}",
        tungsten_parse::print(&p),
        tungsten_parse::print(&t)
    );
    let text = tungsten_parse::print(&p);
    let back = load_source(&text, &env).unwrap_or_else(|e| panic!("{path}: {e}\n{text}"));
    assert!(isomorphic(&p, &back.program), "{path}: print round-trip");
    assert_eq!(tungsten_parse::print(&back.program), text);
}
