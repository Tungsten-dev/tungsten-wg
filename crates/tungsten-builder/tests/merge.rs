//! Merging (spec §3.2): each row of the table, `-0.0`, first name wins, `HostTypeConflict`, and
//! a merged input on the inliner.

mod common;

use common::*;
use tungsten_builder::BuildErrorKind;
use tungsten_builder::prelude::*;
use tungsten_ir::{ComputeUnit, Name, OptimizedProgram, Ref};
use tungsten_optimizer::{InlineRejection, InlineVerdict, explain_inline};

fn base(pb: &mut ProgramBuilder, name: &str) -> Unit {
    let mut f = FnBuilder::new(name, F64);
    f.ret(1.0);
    pb.finish_unit(f)
}

#[test]
fn equal_inputs_merge() {
    let mut pb = ProgramBuilder::new();
    let b = base(&mut pb, "b");
    let g = Grid::new((2, 1, 1));
    let a1 = pb.input_with(InputSpec::new(b, g).named("first"));
    let a2 = pb.input_with(InputSpec::new(b, g).named("second"));
    let a3 = pb.input(b, g);
    assert_eq!(a1, a2);
    assert_eq!(a1, a3);

    // Each key component separates.
    let other = base(&mut pb, "other");
    let distinct = [
        pb.input(other, g),
        pb.input(b, Grid::new((2, 2, 1))),
        pb.input(b, g.origin_scale((2.0, 1.0, 1.0))),
        pb.input(b, g.position_scale((1.0, 1.0, 0.5))),
        pb.input(b, g.origin_scale((1.0, -0.0, 1.0))),
        pb.input(b, g.origin_scale((1.0, 0.0, 1.0))),
    ];
    for (i, x) in distinct.iter().enumerate() {
        assert_ne!(*x, a1, "{i}");
        for y in &distinct[..i] {
            assert_ne!(x, y, "{i}");
        }
    }
    // Scales compare by bits: -0.0 and 0.0 are two inputs, but each merges with itself.
    assert_eq!(pb.input(b, g.origin_scale((1.0, -0.0, 1.0))), distinct[4]);

    let p = pb.build().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(p.density_inputs.iter().count(), 7);
    let names: Vec<_> = p.density_inputs.values().map(|d| d.name.clone()).collect();
    assert_eq!(names[0], Name::Named("first".into()), "the first name wins");
    assert_eq!(names[1], Name::Anonymous);
}

#[test]
fn hosts_merge_by_name() {
    let mut pb = ProgramBuilder::new();
    let h1 = pb.host("h", F64);
    let h2 = pb.host("h", F64);
    let g = pb.host("g", F64);
    assert_eq!(h1, h2);
    assert_ne!(h1, g);
    let h3 = pb.host("h", I32);
    assert_eq!(h3, h1, "a conflict still returns the existing host");
    let Err(errs) = pb.build() else {
        panic!("expected HostTypeConflict")
    };
    assert_eq!(errs.len(), 1);
    assert_eq!(
        errs.0[0].kind,
        BuildErrorKind::HostTypeConflict {
            name: "h".into(),
            existing: "f64".into(),
            requested: "i32".into()
        }
    );
    assert_eq!(errs.0[0].location.file(), file!());
}

#[test]
fn autoconsts_merge_by_value() {
    let mut pb = ProgramBuilder::new();
    let seed = pb.param("seed", I64);
    let other = pb.param("other", I64);
    let h = |pb: &mut ProgramBuilder, name: &str, init: &str, args: Vec<AutoArg>| {
        pb.auto_const(name, Handle("Noise"), init, args)
    };
    let a = h(&mut pb, "first", "make", vec![seed.into(), 1i64.into()]);
    // Equal type, initializer and arguments: the existing one, and the first name wins.
    assert_eq!(a, h(&mut pb, "second", "make", vec![seed.into(), 1i64.into()]));
    assert_ne!(a, h(&mut pb, "a", "make", vec![seed.into(), 2i64.into()]));
    assert_ne!(a, h(&mut pb, "a", "make", vec![other.into(), 1i64.into()]));
    assert_ne!(a, h(&mut pb, "a", "other_init", vec![seed.into(), 1i64.into()]));
    assert_ne!(a, pb.auto_const("a", Handle("Other"), "make", [seed.into(), 1i64.into()]));
    let p = pb.build().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(p.auto_consts.iter().count(), 5);
    assert_eq!(p.auto_consts.values().next().unwrap().name, "first");
}

#[test]
fn params_merge_by_name() {
    let mut pb = ProgramBuilder::new();
    let a = pb.param("seed", I64);
    assert_eq!(a, pb.param("seed", I64));
    assert_ne!(a, pb.param("salt", I64));
    pb.param("seed", F64);
    let Err(errs) = pb.build() else { panic!("a conflicting param type must be an error") };
    assert!(matches!(errs.0[0].kind, BuildErrorKind::AutoParamConflict { .. }), "{errs}");
}

#[test]
fn constants_helpers_and_units_are_not_merged() {
    let mut pb = ProgramBuilder::new();
    let c1 = pb.constant("c", 1.0);
    let c2 = pb.constant("c", 1.0);
    assert_ne!(c1, c2);
    let mut f = FnBuilder::new("h", F64);
    f.ret(1.0);
    let h1 = pb.finish_helper(f);
    let mut f = FnBuilder::new("h", F64);
    f.ret(1.0);
    let h2 = pb.finish_helper(f);
    assert_ne!(h1, h2);
    assert_ne!(base(&mut pb, "u"), base(&mut pb, "u"));
    let p = pb.build().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(p.constants.iter().count(), 2);
    assert_eq!(p.functions.iter().count(), 4);
    assert_eq!(p.compute_units.iter().count(), 2);
}

fn unit(p: &OptimizedProgram, main: &str) -> Ref<ComputeUnit> {
    p.compute_units
        .iter()
        .find(|(_, u)| {
            let ComputeUnit::DensityFunction { function, .. } = u;
            p.functions[*function].canonical_name.as_deref() == Some(main)
        })
        .map(|(r, _)| r)
        .unwrap()
}

#[test]
fn merged_input_read_by_two_units_blocks_inlining() {
    let mut pb = ProgramBuilder::new();
    let b = base(&mut pb, "b");
    let g = Grid::new((1, 1, 1));
    for name in ["left", "right"] {
        // Each unit asks for the input itself; they get the same one.
        let d = pb.input(b, g);
        let mut f = FnBuilder::new(name, F64);
        f.ret(d.read() + 1.0);
        let u = pb.finish_unit(f);
        pb.entry(u, g);
    }
    let p = checked(pb);
    assert_eq!(p.density_inputs.iter().count(), 1);
    let r = unit(&p, "b");
    let verdict = explain_inline(&p).into_iter().find(|(u, _)| *u == r).unwrap().1;
    assert_eq!(
        verdict,
        InlineVerdict::Rejected(InlineRejection::MultipleUses(2))
    );
}
