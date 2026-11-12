//! What the inliner does with mapped reads (`specs/density-mappings.md` §5.4). The corpus trials
//! check that values survive inlining; these check the program's shape.

use tungsten_ir::model::{ComputeUnit, Program, Ref};
use tungsten_optimizer::{InlineRejection, InlineVerdict, explain_inline, perform_inline};
use tungsten_testsuite::corpus::{corpus_root, load_source};
use tungsten_testsuite::env::TestEnv;

fn load(name: &str) -> Program {
    let path = corpus_root().join(format!("density/{name}.tw"));
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    load_source(&src, &TestEnv::new()).unwrap_or_else(|e| panic!("{name}: {e}")).program
}

/// The unit whose main function is `main`.
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

fn listed(p: &Program, main: &str) -> Vec<String> {
    let ComputeUnit::DensityFunction { density_inputs, .. } = &p.compute_units[unit(p, main)];
    density_inputs.iter().map(|r| format!("{:?}", p.density_inputs[*r].name)).collect()
}

/// A `(1,1,1)` input with all `*`, read from a `(1,1,1)` reader, is inlined.
#[test]
fn all_repeat_on_unit_grid_is_inlined() {
    let mut p = load("map_inline_identity_one");
    assert!(matches!(verdict(&p, "leaf"), InlineVerdict::Inlinable { .. }));
    assert_eq!(perform_inline(&mut p).inlined, 1);
}

/// Factor 1 and `*` on size-1 axes are the identity on the grid.
#[test]
fn identity_on_grid_is_inlined() {
    let mut p = load("map_inline_identity_grid");
    assert!(matches!(verdict(&p, "leaf"), InlineVerdict::Inlinable { .. }));
    assert_eq!(perform_inline(&mut p).inlined, 1);
}

/// A mapping that moves cells blocks inlining, naming the function that reads it.
#[test]
fn mapped_read_is_not_inlined() {
    let mut p = load("map_inline_mapped");
    let top = p.compute_units[unit(&p, "top")].clone();
    let ComputeUnit::DensityFunction { function, .. } = top;
    assert_eq!(verdict(&p, "leaf"), InlineVerdict::Rejected(InlineRejection::MappedRead { function }));
    assert_eq!(perform_inline(&mut p).inlined, 0);
}

/// A callee with its own mapped input is inlined, and the caller lists that input afterwards.
#[test]
fn callee_with_mapped_input_is_inlined() {
    let mut p = load("map_inline_callee_mapped");
    assert!(matches!(verdict(&p, "mid_main"), InlineVerdict::Inlinable { .. }));
    assert!(matches!(
        verdict(&p, "leaf"),
        InlineVerdict::Rejected(InlineRejection::MappedRead { .. })
    ));
    let stats = perform_inline(&mut p);
    assert_eq!(stats.inlined, 1);
    assert_eq!(stats.records[0].callee_name.as_deref(), Some("mid_main"));
    assert_eq!(listed(&p, "top"), [r#"Named("m")"#]);
    assert!(matches!(
        verdict(&p, "leaf"),
        InlineVerdict::Rejected(InlineRejection::MappedRead { .. })
    ));
}
