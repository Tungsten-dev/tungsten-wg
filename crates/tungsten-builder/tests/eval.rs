//! `eval(unit, p)` (`specs/unit-eval.md` §3.3): typing, poisoning, `eval_on`, and derived lists
//! that don't absorb the evaluated unit's items.

mod common;

use common::*;
use tungsten_builder::prelude::*;
use tungsten_builder::BuildErrorKind;
use tungsten_ir::{ComputeUnit, Expression, Statement};

/// `probe` reads a host, a constant and an input through a helper. `top` only evaluates it.
fn program() -> (ProgramBuilder, Unit, Unit) {
    let mut pb = ProgramBuilder::new();
    let h = pb.host("h", F64);
    let k = pb.constant("k", 2.0);
    let mut f = FnBuilder::new("base", F64);
    f.ret(pos3().x().cast(F64));
    let base = pb.finish_unit(f);
    let d = pb.input(base, Grid::new((4, 1, 1)));
    let mut f = FnBuilder::new("look", F64);
    f.ret(d.at(pos3().x()) * k);
    let look = pb.finish_helper(f);
    let mut f = FnBuilder::new("probe", F64);
    f.ret(look.call([]) + h);
    let probe = pb.finish_unit(f);

    let mut f = FnBuilder::new("top", F64);
    f.ret(eval(probe, pos3!(pos3().x(), 0, 0), (1.0, 1.0, 1.0), (1.0, 1.0, 1.0)) * 3.0);
    let top = pb.finish_unit(f);
    pb.entry(top, Grid::new((4, 1, 1)));
    (pb, probe, top)
}

#[test]
fn typed_from_the_unit_and_lists_stay_per_unit() {
    let (pb, probe, _) = program();
    assert_eq!(probe.ty(), &F64);
    let p = checked(pb);
    let top = p
        .compute_units
        .values()
        .find(|u| {
            let ComputeUnit::DensityFunction { function, .. } = u;
            p.functions[*function].canonical_name.as_deref() == Some("top")
        })
        .unwrap();
    let ComputeUnit::DensityFunction { density_inputs, host_inputs, helper_functions, constants, .. } = top;
    assert!(density_inputs.is_empty(), "the evaluated unit's input isn't the reader's");
    assert!(host_inputs.is_empty());
    assert!(helper_functions.is_empty());
    assert!(constants.is_empty());
}

#[test]
fn record_type_and_eval_on() {
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("r", Vec3);
    f.ret(origin_scale());
    let r = pb.finish_unit(f);
    let grid = Grid::new((9, 9, 9)).origin_scale(Scale::new(0.5, 2.0, 1.0)).position_scale(Scale::new(3.0, 1.0, 1.0));
    let e = eval_on(r, pos3(), &grid);
    assert_eq!(e.ty(), Some(&Vec3));
    assert!(!e.has_problems());
    let mut f = FnBuilder::new("top", Vec3);
    f.ret(e);
    let top = pb.finish_unit(f);
    pb.entry(top, Grid::new((1, 1, 1)));
    let p = checked(pb);
    let main = p.functions.values().find(|f| f.canonical_name.as_deref() == Some("top")).unwrap();
    let Statement::Return(Expression::Eval { origin_scale, position_scale, .. }) = &main.body[0] else {
        panic!("{:?}", main.body);
    };
    assert_eq!((*origin_scale, *position_scale), ((0.5, 2.0, 1.0), (3.0, 1.0, 1.0)), "dims are ignored");
}

#[test]
fn a_non_pos3_position_poisons() {
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("u", F64);
    f.ret(1.0);
    let u = pb.finish_unit(f);
    let e = eval(u, origin(), (1.0, 1.0, 1.0), (1.0, 1.0, 1.0));
    assert!(e.has_problems());
    assert_eq!(e.ty(), Some(&F64), "the type is still the unit's");
    let mut f = FnBuilder::new("top", F64);
    f.ret(e);
    let top = pb.finish_unit(f);
    pb.entry(top, Grid::new((1, 1, 1)));
    let Err(errs) = pb.build() else { panic!("built") };
    assert_eq!(errs.len(), 1);
    assert_eq!(errs.0[0].kind, BuildErrorKind::EvalPosition { found: "vec3".into() });
}

#[test]
fn a_unit_from_another_builder_is_foreign() {
    let mut other = ProgramBuilder::new();
    let mut f = FnBuilder::new("u", F64);
    f.ret(1.0);
    let u = other.finish_unit(f);
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("top", F64);
    f.ret(eval(u, pos3(), (1.0, 1.0, 1.0), (1.0, 1.0, 1.0)));
    pb.finish_unit(f);
    let Err(errs) = pb.build() else { panic!("built") };
    assert!(errs.iter().any(|e| matches!(e.kind, BuildErrorKind::ForeignHandle { what: "a unit", .. })));
}
