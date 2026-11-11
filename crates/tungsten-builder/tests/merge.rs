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

#[test]
fn autoconst_arguments_merge_by_bits() {
    let mut pb = ProgramBuilder::new();
    let mk = |pb: &mut ProgramBuilder, args: Vec<AutoArg>| pb.auto_const("v", I64, "make", args);
    let a = mk(&mut pb, vec![0.0.into(), true.into(), "s".into()]);
    assert_eq!(a, mk(&mut pb, vec![0.0.into(), true.into(), "s".into()]));
    // `-0.0` is not `0.0`, NaN equals itself, and each other kind differs.
    assert_ne!(a, mk(&mut pb, vec![(-0.0).into(), true.into(), "s".into()]));
    assert_ne!(a, mk(&mut pb, vec![0.0.into(), false.into(), "s".into()]));
    assert_ne!(a, mk(&mut pb, vec![0.0.into(), true.into(), "t".into()]));
    let n = mk(&mut pb, vec![f64::NAN.into()]);
    assert_eq!(n, mk(&mut pb, vec![f64::NAN.into()]));
    let p = pb.build().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(p.auto_consts.iter().count(), 5);
}

#[test]
fn value_autoconsts_are_read_like_expressions_and_listed() {
    let mut pb = ProgramBuilder::new();
    let seed = pb.param("seed", I64);
    let salt = pb.auto_const("salt", I64, "make_salt", [seed.into(), "caves".into()]);
    let mut f = FnBuilder::new("m", F64);
    f.ret(Expr::from(salt).cast(F64) + 1.0);
    let u = pb.finish_unit(f);
    pb.entry(u, Grid::new((1, 1, 1)));
    let p = pb.build().unwrap_or_else(|e| panic!("{e}"));
    let ComputeUnit::DensityFunction { auto_consts, .. } = p.compute_units.values().next().unwrap();
    assert_eq!(auto_consts.len(), 1, "the unit lists the autoconst its function reads");
}

#[test]
fn mapping_and_previous_separate_inputs() {
    use tungsten_builder::{AxisFactor::Repeat, DensityMapping};
    let mut pb = ProgramBuilder::new();
    let b = base(&mut pb, "b");
    let g = Grid::new((1, 1, 1));
    let cell_zero = DensityMapping::Expand([Repeat; 3]);
    let lerp = DensityMapping::Lerp([Repeat; 3]);

    let plain = pb.input(b, g);
    let expanded = pb.input_with(InputSpec::new(b, g).mapping(cell_zero));
    let lerped = pb.input_with(InputSpec::new(b, g).mapping(lerp));
    let previous = pb.input_with(InputSpec::new(b, g).previous());
    let both = pb.input_with(InputSpec::new(b, g).mapping(cell_zero).previous());
    let all = [plain, expanded, lerped, previous, both];
    for (i, x) in all.iter().enumerate() {
        for y in &all[i + 1..] {
            assert_ne!(x, y);
        }
    }

    // Equal specs still merge, and an explicit `Identity` is the default.
    assert_eq!(expanded, pb.input_with(InputSpec::new(b, g).mapping(cell_zero)));
    assert_eq!(previous, pb.input_with(InputSpec::new(b, g).previous()));
    assert_eq!(plain, pb.input_with(InputSpec::new(b, g).mapping(DensityMapping::Identity)));

    let p = pb.build().unwrap();
    let stored: Vec<_> = p.density_inputs.values().map(|d| (d.mapping, d.previous)).collect();
    assert_eq!(
        stored,
        [
            (DensityMapping::Identity, false),
            (cell_zero, false),
            (lerp, false),
            (DensityMapping::Identity, true),
            (cell_zero, true),
        ]
    );
}

#[test]
fn refinement_is_recorded_as_written() {
    let mut pb = ProgramBuilder::new();
    let (seed, detail, surface) = (base(&mut pb, "seed"), base(&mut pb, "detail"), base(&mut pb, "surface"));
    let g = Grid::new((16, 1, 16)).position_scale((1.0, 8.0, 1.0));
    pb.entry(detail, Grid::new((1, 1, 1)));
    let entry = pb.entry(seed, g);
    pb.refinement(
        "step",
        entry,
        (0, 383, 0),
        [RerunSpec::new(seed, g).with(surface), RerunSpec::new(detail, g)],
    );
    let p = pb.build().unwrap();

    let [r] = &p.refinements[..] else { panic!("one refinement") };
    assert_eq!((r.name.as_str(), r.entry, r.march), ("step", 1, (0, 383, 0)));
    let unit_ref = |name: &str| -> Ref<ComputeUnit> {
        p.compute_units
            .iter()
            .find(|(_, u)| {
                let ComputeUnit::DensityFunction { function, .. } = u;
                p.functions[*function].canonical_name.as_deref() == Some(name)
            })
            .unwrap()
            .0
    };
    assert_eq!(r.reruns[0].cu, unit_ref("seed"));
    assert_eq!(r.reruns[0].with, Some(unit_ref("surface")));
    assert_eq!(r.reruns[0].dimensions, (16, 1, 16));
    assert_eq!(r.reruns[0].scaled_position, (1.0, 8.0, 1.0));
    assert_eq!(r.reruns[1].cu, unit_ref("detail"));
    assert_eq!(r.reruns[1].with, None);
}

#[test]
fn refinement_rejects_foreign_handles() {
    let mut other = ProgramBuilder::new();
    let foreign_unit = base(&mut other, "foreign");
    let foreign_entry = other.entry(foreign_unit, Grid::new((1, 1, 1)));

    let mut pb = ProgramBuilder::new();
    let b = base(&mut pb, "b");
    let g = Grid::new((1, 1, 1));
    let entry = pb.entry(b, g);
    pb.refinement("a", foreign_entry, (0, 0, 0), [RerunSpec::new(b, g)]);
    pb.refinement("b", entry, (0, 0, 0), [RerunSpec::new(foreign_unit, g)]);
    pb.refinement("c", entry, (0, 0, 0), [RerunSpec::new(b, g).with(foreign_unit)]);
    let errors = pb.build().err().expect("three foreign handles").0;
    assert_eq!(errors.len(), 3, "{errors:?}");
    assert!(errors.iter().all(|e| matches!(e.kind, BuildErrorKind::ForeignHandle { .. })), "{errors:?}");
}
