//! Refinement validation (`specs/refinement.md` §5) and refine-step lowering (§7.2).

use super::*;

const G: &str = "@dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1)";

/// `seed` is the entry, rerun `with surface`, which reads its previous value and `detail`.
fn base() -> String {
    format!(
        "program {{
            fn seed_main() -> f64 {{ return 0.0; }}
            fn surf_main() -> f64 {{ return (density(prev) + density(d)); }}
            fn detail_main() -> f64 {{ return 2.0; }}
            unit seed {{ main seed_main; }}
            unit surface {{ main surf_main; inputs [prev, d]; }}
            unit detail {{ main detail_main; }}
            input prev = seed {G} @previous;
            input d = detail {G};
            entry top = seed {G};
            refinement step of top @march(0, 3, 0) {{
                rerun seed {G} with surface;
                rerun detail {G};
            }}
        }}"
    )
}

fn lower_errs(src: &str) -> Vec<String> {
    match crate::lower(&load(src), &registry()) {
        Ok(_) => vec![],
        Err(e) => e.0.iter().map(|e| e.to_string()).collect(),
    }
}

/// The errors of `base()` with `from` replaced by `to`.
fn with(from: &str, to: &str) -> Vec<String> {
    let src = base();
    assert!(src.contains(from), "{from}");
    lower_errs(&src.replacen(from, to, 1))
}

#[test]
fn base_lowers() {
    let l = ok(&load(&base()), &registry());
    assert_eq!(l.steps.len(), 1);
    assert_eq!(l.visible, 1);
}

#[test]
fn duplicate_name() {
    let dup = format!("refinement step of top {{ rerun seed {G}; }}\n        }}");
    let src = base();
    let at = src.rfind('}').unwrap();
    let src = format!("{}{dup}", &src[..at]);
    assert_eq!(lower_errs(&src), ["refinement `step`: entry 0 already has a refinement `step`"]);
}

#[test]
fn entry_not_rerun() {
    assert_eq!(
        with(&format!("rerun seed {G} with surface;"), ""),
        [
            "refinement `step`: the entry's own instance isn't rerun",
            "input `prev`: `@previous` input listed by `surf_main`, which no refinement runs as a `with` unit",
        ]
    );
}

#[test]
fn duplicate_rerun() {
    assert_eq!(
        with(&format!("rerun detail {G};"), &format!("rerun detail {G}; rerun detail {G};")),
        ["refinement `step`: rerun 2 lists the same instance as rerun 1"]
    );
}

#[test]
fn rerun_unreachable() {
    assert_eq!(
        with("inputs [prev, d];", "inputs [prev];").into_iter().filter(|e| e.contains("isn't reached")).collect::<Vec<_>>(),
        ["refinement `step`: rerun 1 (`detail_main`) isn't reached from the entry through reruns"]
    );
}

#[test]
fn type_mismatch() {
    let errs = with("fn surf_main() -> f64 { return (density(prev) + density(d)); }", "fn surf_main() -> i32 { return 1; }");
    assert!(
        errs.contains(&"refinement `step`: rerun 0: `surf_main` returns `i32`, but its target `seed_main` returns `f64`".to_string()),
        "{errs:?}"
    );
}

#[test]
fn march_scale_mismatch() {
    // y marches, so a different y position scale is an error; x doesn't, so it isn't.
    let other = "rerun detail @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 2, 1);";
    let errs = with(&format!("rerun detail {G};"), other);
    assert!(errs.iter().any(|e| e.contains("different position scale on marched axis y")), "{errs:?}");
    let base = base().replace(&format!("input d = detail {G};"), "input d = detail @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(3, 1, 1);");
    let src = base.replace(&format!("rerun detail {G};"), "rerun detail @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(3, 1, 1);");
    assert_eq!(lower_errs(&src), Vec::<String>::new());
}

#[test]
fn cycle() {
    // The refined unit reads its target without `@previous`.
    let src = base().replace("input prev = seed @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @previous;", &format!("input prev = seed {G};"));
    let errs = lower_errs(&src);
    assert!(errs.contains(&"refinement `step`: the instances that run form a cycle through `surf_main`".to_string()), "{errs:?}");
}

#[test]
fn refined_unit_in_normal_graph() {
    let errs = with("entry top = seed", "entry other = surface @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);\n entry top = seed");
    assert!(errs.iter().any(|e| e == "program: refined unit `surf_main` is also an entry"), "{errs:?}");
    let errs = with("input d = detail", &format!("input s = surface {G};\n input d = detail"));
    assert!(errs.iter().any(|e| e == "program: refined unit `surf_main` is also the source of an input"), "{errs:?}");
}

#[test]
fn previous_rules() {
    // Listed by a unit that isn't a `with` unit.
    let errs = with("unit detail { main detail_main; }", "unit detail { main detail_main; inputs [prev]; }");
    assert!(errs.iter().any(|e| e.contains("listed by `detail_main`, which no refinement runs as a `with` unit")), "{errs:?}");
    // Its instance isn't the target of the rerun whose `with` unit lists it.
    let src = base().replace(
        "input prev = seed @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @previous;",
        "input prev = seed @dims(2, 1, 1) @origin_scale(2, 2, 2) @position_scale(1, 1, 1) @previous;",
    );
    let errs = lower_errs(&src);
    assert!(errs.iter().any(|e| e.contains("which refinement `step` runs for a different instance")), "{errs:?}");
    // Mapped.
    let errs = with("@position_scale(1, 1, 1) @previous;", "@position_scale(1, 1, 1) @previous @expand(1, *, *);");
    assert!(errs.iter().any(|e| e == "input `prev`: a `@previous` input must have no mapping"), "{errs:?}");
    // Indexed.
    let errs = with("density(prev) + density(d)", "density(prev, 0) + density(d)");
    assert_eq!(errs, ["input `prev`: `surf_main` reads this `@previous` input with an explicit index"]);
}

/// The mapping reader rule includes the march: y reads up to (1 - 1 + 3) / 2 + 1 = 2, past a
/// 2-high grid.
#[test]
fn mapped_read_out_of_range_only_with_march() {
    let src = base()
        .replace("fn detail_main() -> f64 { return 2.0; }", "fn detail_main() -> f64 { return (density(g) + 2.0); }")
        .replace("unit detail { main detail_main; }", "unit detail { main detail_main; inputs [g]; }")
        .replace(
            "input d = detail",
            "fn grid_main() -> f64 { return 1.0; }\n unit grid { main grid_main; }\n input g = grid @dims(2, 2, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @lerp(1, 2, *);\n input d = detail",
        );
    assert_eq!(
        lower_errs(&src),
        ["input `g`: unit `detail_main` reads it at 1 on axis y (march 3), past the input's 2 cells on that axis"]
    );
    // Without the march, the same grid is fine.
    assert_eq!(lower_errs(&src.replace("@march(0, 3, 0)", "")), Vec::<String>::new());
}

/// Hand-built: an entry index no label can name, and a negative march the grammar refuses.
#[test]
fn bad_entry_and_march() {
    let mut p = load(&base());
    p.refinements[0].march = (0, -1, 0);
    p.refinements.push(tungsten_ir::Refinement { name: "far".into(), entry: 7, march: (0, 0, 0), reruns: vec![] });
    assert_eq!(
        errs(&p, &registry()),
        ["refinement `step`: march -1 on axis y is negative", "refinement `far`: entry 7 doesn't exist"]
    );
}

/// The step program: `Pos3` is offset, the `@previous` read is a buffer, `detail` stays an input
/// and the entry runs `surface`.
#[test]
fn step_program_shape() {
    use tungsten_core::{Expr, HostId, Type};
    let l = ok(&load(&base()), &registry());
    let step = &l.steps[0];
    assert_eq!(step.name, "step");
    assert_eq!(step.offset, HostId(0));
    assert_eq!(step.buffers.len(), 1);
    assert_eq!(step.buffers[0].0, HostId(1));
    assert_eq!(step.program.hosts.len(), 2);
    assert_eq!(step.program.hosts[0].ty, Type::Pos3);
    assert_eq!(step.program.hosts[1].ty, Type::Array(Box::new(Type::F64), 2));
    assert_eq!(step.running.len(), 2);
    let entry = &step.program.entries[0];
    let surf = l.map.units.iter().find(|(_, _, n)| *n == "surf_main").unwrap().0;
    assert_eq!(entry.unit, surf);
    // `surface` lists only `d` now, and its main reads the buffer.
    assert_eq!(step.program.units[surf.0].inputs.len(), 1);
    let main = &step.program.functions[step.program.units[surf.0].main.0];
    let tungsten_core::Stmt::Return(Expr::Binary { left, .. }) = &main.body[0] else { panic!("{:?}", main.body) };
    assert!(matches!(&**left, Expr::Index { array, .. } if **array == Expr::Host(HostId(1))), "{left:?}");
    assert_eq!(step.map.host_name(HostId(0)), Some("refine offset"));
}

fn f64s(r: &tungsten_core::exec::CaseResult, entry: usize) -> Vec<f64> {
    r.entries[entry]
        .iter()
        .map(|c| match c {
            Ok(tungsten_core::Value::F64(b)) => f64::from_bits(*b),
            other => panic!("{other:?}"),
        })
        .collect()
}

fn sequence(src: &str, offsets: &[[i32; 3]]) -> crate::SequenceResult {
    use crate::{EvalOptions, NamedCase, RefineCall, SequenceCase, prepare};
    let prep = prepare(&load(src), &registry()).unwrap_or_else(|e| panic!("{e}"));
    let run = NamedCase { origin: [0.0; 3], hosts: vec![], auto_values: vec![] };
    let steps = offsets.iter().map(|&offset| RefineCall { refinement: 0, offset, hosts: vec![] }).collect();
    let case = SequenceCase { run, steps };
    let a = prep.eval_sequence(&case, &super::eval::Toy(&prep), EvalOptions::default()).unwrap();
    let b = prep.eval_sequence(&case, &super::eval::Toy(&prep), EvalOptions { memo: false, ..EvalOptions::default() }).unwrap();
    assert_eq!(a, b, "memo on and off differ");
    a
}

/// `surface` adds `detail` (2.0) to its previous value, starting from `seed`'s 0.0.
#[test]
fn sequence_accumulates_through_previous() {
    let r = sequence(&base(), &[[0, 0, 0], [0, 3, 0], [0, 1, 0]]);
    assert_eq!(f64s(&r.run, 0), [0.0, 0.0]);
    assert_eq!(r.steps.iter().map(|s| f64s(s, 0)).collect::<Vec<_>>(), [[2.0, 2.0], [4.0, 4.0], [6.0, 6.0]]);
}

/// `Pos3` moves by the offset, the buffer index doesn't, and a frozen buffer keeps its value.
#[test]
fn sequence_marches_pos3_and_keeps_frozen() {
    let src = base()
        .replace("fn detail_main() -> f64 { return 2.0; }", "fn detail_main() -> f64 { return ((ambient(pos3).y * 10) as f64); }")
        .replace(
            "fn surf_main() -> f64 { return (density(prev) + density(d)); }",
            "fn surf_main() -> f64 { return ((density(prev) + density(d)) + (density(fz) + (ambient(pos3).x as f64))); }",
        )
        .replace("unit surface { main surf_main; inputs [prev, d]; }", "unit surface { main surf_main; inputs [prev, d, fz]; }")
        .replace(
            "input d = detail",
            "fn frozen_main() -> f64 { return (100.0 + ((ambient(pos3).y * 1000) as f64)); }\n unit frozen { main frozen_main; }\n input fz = frozen @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);\n input d = detail",
        );
    let r = sequence(&src, &[[0, 3, 0], [0, 2, 0]]);
    // Step 1: prev 0, d = 30, frozen 100 (computed by orchestrate at offset 0), x = 0 or 1.
    assert_eq!(f64s(&r.steps[0], 0), [130.0, 131.0]);
    // Step 2: prev 130/131, d = 20, frozen still 100.
    assert_eq!(f64s(&r.steps[1], 0), [250.0, 252.0]);
    // `detail` ran too: entry 1 of the step is its buffer.
    assert_eq!(f64s(&r.steps[1], 1), [20.0, 20.0]);
}

/// A failing step ends the sequence.
#[test]
fn sequence_stops_at_a_failing_step() {
    let src = base().replace(
        "fn detail_main() -> f64 { return 2.0; }",
        "fn detail_main() -> f64 { return ((1 / (2 - ambient(pos3).y)) as f64); }",
    );
    let r = sequence(&src, &[[0, 1, 0], [0, 2, 0], [0, 3, 0]]);
    assert_eq!(r.steps.len(), 2);
    assert!(r.steps[1].entries[0].iter().all(|c| c == &Err(tungsten_core::EvalError::DivByZero)));
}
