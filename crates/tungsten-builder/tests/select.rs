//! `select(c, a, b)` (`specs/select.md` §3.4): typing, each `BuildErrorKind`, poisoning, derived
//! lists that include what both arms read, and rejection in constants.

mod common;

use common::*;
use tungsten_builder::BuildErrorKind;
use tungsten_builder::prelude::*;
use tungsten_ir::{ComputeUnit, Expression, Statement};

fn errors(pb: ProgramBuilder) -> BuildErrors {
    match pb.build() {
        Ok(_) => panic!("built"),
        Err(e) => e,
    }
}

/// A unit `m` returning `e`, built in a fresh program, and its errors.
fn kinds(t: VariableType, e: Expr) -> Vec<BuildErrorKind> {
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("m", t);
    f.ret(e);
    pb.finish_unit(f);
    errors(pb).iter().map(|e| e.kind.clone()).collect()
}

#[test]
fn typed_as_the_arms() {
    let c = || pos3().x().lt(0);
    assert_eq!(select(c(), 1.0, 2.0).ty(), Some(&F64));
    assert_eq!(select(c(), 1, 2).ty(), Some(&I32));
    assert_eq!(select(true, 1i64, 2i64).ty(), Some(&I64));
    assert_eq!(select(c(), true, false).ty(), Some(&Bool));
    assert_eq!(select(c(), origin(), origin_scale()).ty(), Some(&Vec3));
    assert_eq!(select(c(), pos3(), pos3!(1, 2, 3)).ty(), Some(&Pos3));
    assert!(!select(c(), 1.0, 2.0).has_problems());
}

#[test]
fn builds_and_checks_with_every_type() {
    let mut pb = ProgramBuilder::new();
    let c = || pos3().x().lt(0);
    let cases: [(VariableType, Expr); 6] = [
        (F64, select(c(), 1.0, 2.0)),
        (F32, select(c(), 1.0f32, 2.0f32)),
        (I32, select(c(), pos3().x(), 2)),
        (I64, select(c(), 1i64, 2i64)),
        (Vec3, select(c(), origin(), origin_scale())),
        (Pos3, select(c(), pos3(), pos3!(1, 2, 3))),
    ];
    for (i, (t, e)) in cases.into_iter().enumerate() {
        let mut f = FnBuilder::new(format!("m{i}"), t);
        f.ret(e);
        let u = pb.finish_unit(f);
        pb.entry(u, Grid::new((2, 1, 1)));
    }
    let p = checked(pb);
    let main = p.functions.values().find(|f| f.canonical_name.as_deref() == Some("m0")).unwrap();
    assert!(matches!(&main.body[0], Statement::Return(Expression::Select { .. })), "{:?}", main.body);
}

#[test]
fn cond_must_be_bool() {
    let e = select(1, 1.0, 2.0);
    assert!(e.has_problems());
    assert_eq!(e.ty(), Some(&F64), "a bad cond doesn't poison the result");
    assert_eq!(kinds(F64, e), [BuildErrorKind::SelectCond { found: "i32".into() }]);
}

#[test]
fn arms_must_match() {
    let e = select(true, 1, 2i64);
    assert_eq!(e.ty(), None, "mismatched arms poison the result");
    assert_eq!(
        kinds(I64, e),
        [BuildErrorKind::SelectMismatch { then: "i32".into(), otherwise: "i64".into() }]
    );
    // No vec3/pos3 promotion either.
    let e = select(true, origin(), pos3());
    assert_eq!(
        kinds(Vec3, e),
        [BuildErrorKind::SelectMismatch { then: "vec3".into(), otherwise: "pos3".into() }]
    );
}

#[test]
fn records_and_arrays_are_rejected() {
    let e = select(true, array![1.0, 2.0], array![3.0, 4.0]);
    assert_eq!(e.ty(), None);
    assert_eq!(kinds(F64, e), [BuildErrorKind::SelectType { found: "array[f64; 2]".into() }]);
    let pair = || Expr::record("Pair", [("a", Expr::from(1.0))]);
    let e = select(true, pair(), pair());
    assert_eq!(kinds(F64, e), [BuildErrorKind::SelectType { found: "Pair".into() }]);
}

#[test]
fn an_unknown_arm_falls_back_to_the_other() {
    // `1.0 + 1` is a bad binary with an unknown type: one error, and the select is `f64`.
    let bad = Expr::from(1.0) + 1;
    assert_eq!(bad.ty(), None);
    let e = select(true, bad, 2.0);
    assert_eq!(e.ty(), Some(&F64));
    let k = kinds(F64, e);
    assert_eq!(k.len(), 1, "{k:?}");
    assert!(matches!(k[0], BuildErrorKind::BadBinary { .. }), "{k:?}");
}

#[test]
fn derived_lists_include_both_arms() {
    let mut pb = ProgramBuilder::new();
    let h = pb.host("h", F64);
    let k = pb.constant("k", 2.0);
    let mut f = FnBuilder::new("base", F64);
    f.ret(pos3().x().cast(F64));
    let base = pb.finish_unit(f);
    let d = pb.input(base, Grid::new((4, 1, 1)));
    let mut f = FnBuilder::new("top", F64);
    // Never true at run time, but both arms run, so both count as read.
    f.ret(select(false, d.at(0) + h, k));
    let top = pb.finish_unit(f);
    pb.entry(top, Grid::new((1, 1, 1)));
    let p = checked(pb);
    let ComputeUnit::DensityFunction { density_inputs, host_inputs, constants, function, .. } = p
        .compute_units
        .values()
        .find(|u| {
            let ComputeUnit::DensityFunction { function, .. } = u;
            p.functions[*function].canonical_name.as_deref() == Some("top")
        })
        .unwrap();
    assert_eq!(density_inputs.len(), 1, "the input read in `then`");
    assert_eq!(host_inputs.len(), 1, "the host read in `then`");
    assert_eq!(constants.len(), 1, "the constant read in `otherwise`");
    assert_eq!(p.functions[*function].host_inputs.len(), 1);
}

#[test]
fn not_allowed_in_constants() {
    let mut pb = ProgramBuilder::new();
    pb.constant("c", select(true, 1.0, 2.0));
    let errs = errors(pb);
    assert_eq!(errs.len(), 1, "{errs}");
    assert_eq!(errs.0[0].kind, BuildErrorKind::NotConstant { name: "c".into(), found: "a select" });
}
