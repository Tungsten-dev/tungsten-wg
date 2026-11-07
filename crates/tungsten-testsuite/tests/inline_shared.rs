//! What the inliner does with shared, duplicate and unlisted inputs (program-level-inputs §6.4,
//! §8). The corpus trials check that values survive inlining; these check the program's shape.

use tungsten_ir::model::{ComputeUnit, OptimizedProgram, Ref};
use tungsten_optimizer::{InlineRejection, InlineVerdict, explain_inline, perform_inline};
use tungsten_testsuite::corpus::{corpus_root, load_source};
use tungsten_testsuite::env::TestEnv;

fn load(name: &str) -> OptimizedProgram {
    let path = corpus_root().join(format!("density/{name}.tw"));
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    load_source(&src, &TestEnv::new()).unwrap_or_else(|e| panic!("{name}: {e}")).program
}

/// The unit whose main function is `main`.
fn unit(p: &OptimizedProgram, main: &str) -> Ref<ComputeUnit> {
    p.compute_units
        .iter()
        .find(|(_, u)| {
            let ComputeUnit::DensityFunction { function, .. } = u;
            p.functions[*function].canonical_name.as_deref() == Some(main)
        })
        .map(|(r, _)| r)
        .unwrap_or_else(|| panic!("no unit with main `{main}`"))
}

fn verdict(p: &OptimizedProgram, main: &str) -> InlineVerdict {
    let r = unit(p, main);
    explain_inline(p).into_iter().find(|(u, _)| *u == r).unwrap().1
}

fn listed(p: &OptimizedProgram, main: &str) -> Vec<String> {
    let ComputeUnit::DensityFunction { density_inputs, .. } = &p.compute_units[unit(p, main)];
    density_inputs.iter().map(|r| format!("{:?}", p.density_inputs[*r].name)).collect()
}

/// An input listed by two units counts as two uses of its source.
#[test]
fn shared_input_blocks_inlining() {
    let mut p = load("shared_helper");
    assert_eq!(verdict(&p, "base_main"), InlineVerdict::Rejected(InlineRejection::MultipleUses(2)));
    assert_eq!(perform_inline(&mut p).inlined, 0);
}

/// `inputs [a, a]` is one use, so its source is inlined like the one behind `inputs [c]`.
#[test]
fn duplicate_listing_counts_once() {
    let mut p = load("shared_duplicates");
    assert!(matches!(verdict(&p, "base_main"), InlineVerdict::Inlinable { .. }));
    assert_eq!(perform_inline(&mut p).inlined, 2);
    assert_eq!(p.density_inputs.iter().count(), 0);
}

/// `mid` and its caller `top` both list `x`: after the merge, `top` lists it once, and `x` is the
/// only input left.
#[test]
fn absorbed_input_already_listed() {
    let mut p = load("shared_inline_listed");
    assert!(matches!(
        verdict(&p, "leaf_main"),
        InlineVerdict::Rejected(InlineRejection::MultipleUses(2))
    ));
    let stats = perform_inline(&mut p);
    assert_eq!(stats.inlined, 1);
    assert_eq!(stats.records[0].callee_name.as_deref(), Some("mid_main"));
    assert_eq!(listed(&p, "top_main"), [r#"Named("x")"#]);
    assert_eq!(p.density_inputs.iter().count(), 1);
    assert!(matches!(
        verdict(&p, "leaf_main"),
        InlineVerdict::Rejected(InlineRejection::GridMismatch { .. })
    ));
}

/// An unlisted input whose source is the callee doesn't count as a use, and leaves with it.
#[test]
fn unlisted_input_of_callee_is_removed() {
    let mut p = load("shared_inline_unlisted");
    assert!(matches!(verdict(&p, "mid_main"), InlineVerdict::Inlinable { .. }));
    assert_eq!(p.density_inputs.iter().count(), 2);
    assert_eq!(perform_inline(&mut p).inlined, 1);
    assert_eq!(p.density_inputs.iter().count(), 0);
    assert!(listed(&p, "top_main").is_empty());
}

/// An input no unit lists isn't a use of its source.
#[test]
fn unlisted_input_is_no_use() {
    let p = load("shared_unlisted");
    assert_eq!(verdict(&p, "bad_main"), InlineVerdict::Rejected(InlineRejection::Unused));
}
