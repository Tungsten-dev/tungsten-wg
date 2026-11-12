//! What the inliner does around refinements (`specs/refinement.md` §8.1). The corpus trials
//! check that values survive inlining; these check the program's shape.

use tungsten_ir::model::{ComputeUnit, Program, Ref};
use tungsten_optimizer::{InlineRejection, InlineVerdict, explain_inline, perform_inline};
use tungsten_testsuite::corpus::{corpus_root, load_source};
use tungsten_testsuite::env::TestEnv;

fn load(name: &str) -> Program {
    let path = corpus_root().join(format!("refine/{name}.tw"));
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    load_source(&src, &TestEnv::new()).unwrap_or_else(|e| panic!("{name}: {e}")).program
}

fn unit(p: &Program, main: &str) -> Ref<ComputeUnit> {
    p.compute_units
        .iter()
        .find(|(_, u)| {
            let ComputeUnit::DensityFunction { function, .. } = u;
            p.functions[*function].canonical_name.as_deref() == Some(main)
        })
        .map(|(r, _)| r)
        .unwrap_or_else(|| panic!("no unit with main `{main}`"))
}

fn verdict(p: &Program, main: &str) -> InlineVerdict {
    let r = unit(p, main);
    explain_inline(p).into_iter().find(|(u, _)| *u == r).unwrap().1
}

/// A frozen input read directly by a rerun unit stays a unit.
#[test]
fn frozen_input_of_rerun_unit_is_not_inlined() {
    let mut p = load("inline_frozen_not_inlined");
    assert_eq!(verdict(&p, "f_main"), InlineVerdict::Rejected(InlineRejection::Refinement));
    assert_eq!(perform_inline(&mut p).inlined, 0);
}

/// Rerun and `with` units aren't inlined either.
#[test]
fn rerun_and_with_units_are_not_inlined() {
    let p = load("rerun_chain");
    assert_eq!(verdict(&p, "b_main"), InlineVerdict::Rejected(InlineRejection::Refinement));
    let p = load("schedule_frozen_only_refinement");
    assert_eq!(verdict(&p, "w_main"), InlineVerdict::Rejected(InlineRejection::Refinement));
}

/// Deeper in a frozen closure, inlining still happens.
#[test]
fn deeper_in_frozen_closure_is_inlined() {
    let mut p = load("inline_deeper_frozen");
    assert_eq!(verdict(&p, "f_main"), InlineVerdict::Rejected(InlineRejection::Refinement));
    assert!(matches!(verdict(&p, "g_main"), InlineVerdict::Inlinable { .. }));
    let stats = perform_inline(&mut p);
    assert_eq!(stats.inlined, 1);
    assert_eq!(stats.records[0].callee_name.as_deref(), Some("g_main"));
}
