//! `eval(unit, p)` (`specs/unit-eval.md` §5.1, §6): the desugar into position-taking clones, the
//! lists a reader gains, and every `Eval*` lowering error with its location.

use super::*;

const S: &str = "@origin_scale(1, 1, 1) @position_scale(1, 1, 1)";
const G: &str = "@dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1)";

fn lower_errs(src: &str) -> Vec<String> {
    match crate::lower(&load(src), &registry()) {
        Ok(_) => vec![],
        Err(e) => e.0.iter().map(|e| e.to_string()).collect(),
    }
}

/// `probe` reads a host, an input by index and an ambient through a helper; `top` evaluates it
/// at two scales, once inside a helper of its own.
fn program() -> String {
    format!(
        "program {{
            host h: f64;
            fn base_main() -> f64 {{ return 1.0; }}
            unit base {{ main base_main; }}
            input b = base {G};
            fn scaled() -> f64 {{ return ambient(position_scale).x; }}
            fn probe_main() -> f64 @hosts(h) {{ return ((density(b, ambient(pos3).x) + scaled()) + host(h)); }}
            unit probe {{ main probe_main; helpers [scaled]; hosts [h]; inputs [b]; }}
            fn twice() -> f64 {{ return eval(probe, pos3(1, 0, 0)) @origin_scale(1, 1, 1) @position_scale(2, 2, 2); }}
            fn top_main() -> f64 {{
                return ((eval(probe, ambient(pos3)) {S} + eval(probe, pos3(0, 0, 0)) {S}) + twice());
            }}
            unit top {{ main top_main; helpers [twice]; }}
            entry top {G};
        }}"
    )
}

#[test]
fn clones_per_key_with_the_position_first() {
    let l = ok(&load(&program()), &registry());
    // Two keys (scale 1 and 2), each cloning `probe_main` and `scaled`.
    assert_eq!(l.map.eval_fns.len(), 4);
    let own = l.program.functions.len() - 4;
    for (k, info) in l.map.eval_fns.iter().enumerate() {
        assert_eq!(info.id, FnId(own + k), "clones follow the program's functions");
        let f = &l.program.functions[info.id.0];
        assert_eq!(f.params, 1, "the position is the only parameter");
        assert_eq!(f.locals[0], tungsten_core::Type::Pos3);
        // Clones keep their source's name for diagnostics.
        let src = l.map.functions.id(info.source).unwrap();
        assert_eq!(l.map.functions.name(info.id), l.map.functions.name(src));
    }
    assert!(l.map.eval_fns[0].main && !l.map.eval_fns[1].main);
    assert_ne!(l.map.eval_fns[0].key, l.map.eval_fns[2].key);
}

#[test]
fn reader_lists_gain_what_evaluated_bodies_use() {
    let l = ok(&load(&program()), &registry());
    let top = l.map.units.iter().find(|(_, _, n)| *n == "top_main").unwrap().0;
    let u = &l.program.units[top.0];
    assert_eq!(u.inputs.len(), 1, "the input `probe` reads by index");
    assert!(u.hosts.contains(&tungsten_core::HostId(0)));
    for info in &l.map.eval_fns {
        assert!(u.helpers.contains(&info.id), "every clone the reader reaches");
    }
}

#[test]
fn clones_read_the_position_and_fold_the_scales() {
    let l = ok(&load(&program()), &registry());
    // The clone of `scaled` for the key with position scale 2 returns `vec3(2, 2, 2).x`.
    let info = l.map.eval_fns.iter().find(|e| !e.main && e.key.position_scale().0 == 2.0).unwrap();
    let body = format!("{:?}", l.program.functions[info.id.0].body);
    assert!(body.contains("MakeVec3"), "{body}");
    assert!(body.contains(&format!("{}", 2.0f64.to_bits())), "{body}");
    assert!(!body.contains("PositionScale"), "{body}");
    // The main clone reads `Local(0)`, not `Pos3`.
    let main = l.map.eval_fns.iter().find(|e| e.main).unwrap();
    let main = format!("{:?}", l.program.functions[main.id.0].body);
    assert!(main.contains("Local(LocalId(0))") && !main.contains("Ambient(Pos3)"), "{main}");
}

#[test]
fn deterministic() {
    let p = load(&program());
    assert!(crate::lower(&p, &registry()).unwrap() == crate::lower(&p, &registry()).unwrap());
}

#[test]
fn cycle_errors_name_the_cycle() {
    let src = format!(
        "program {{
            fn a_main() -> f64 {{ return eval(b, ambient(pos3)) {S}; }}
            unit a {{ main a_main; }}
            fn b_main() -> f64 {{ return eval(a, ambient(pos3)) {S}; }}
            unit b {{ main b_main; }}
            entry a {G};
        }}"
    );
    assert_eq!(lower_errs(&src), ["program: `eval` forms a cycle: `a_main` → `b_main` → `a_main`"]);
    let src = format!(
        "program {{
            fn a_main() -> f64 {{ return eval(a, ambient(pos3)) {S}; }}
            unit a {{ main a_main; }}
            entry a {G};
        }}"
    );
    assert_eq!(lower_errs(&src), ["program: `eval` forms a cycle: `a_main` → `a_main`"]);
}

#[test]
fn input_cycles_without_eval_are_left_to_the_checker() {
    let src = format!(
        "program {{
            input x = a {G};
            fn a_main() -> f64 {{ return density(x); }}
            unit a {{ main a_main; inputs [x]; }}
            entry a {G};
        }}"
    );
    assert_eq!(lower_errs(&src), Vec::<String>::new());
}

#[test]
fn unindexed_and_previous_reads() {
    let src = format!(
        "program {{
            fn base_main() -> f64 {{ return 1.0; }}
            unit base {{ main base_main; }}
            input b = base {G};
            fn read() -> f64 {{ return density(b); }}
            fn probe_main() -> f64 {{ return (read() + density(b)); }}
            unit probe {{ main probe_main; helpers [read]; inputs [b]; }}
            fn top_main() -> f64 {{ return eval(probe, ambient(pos3)) {S}; }}
            unit top {{ main top_main; }}
            entry top {G};
        }}"
    );
    // Once per key and input, at the first function that reads it.
    assert_eq!(
        lower_errs(&src),
        ["unit `probe_main`: `probe_main` is evaluated, but `probe_main` reads input `b` without an index"]
    );
    // A body that is never evaluated isn't checked.
    let unevaluated = src.replace(&format!("eval(probe, ambient(pos3)) {S}"), "1.0");
    assert_eq!(lower_errs(&unevaluated), Vec::<String>::new());

    let src = format!(
        "program {{
            fn seed_main() -> f64 {{ return 0.0; }}
            unit seed {{ main seed_main; }}
            input prev = seed {G} @previous;
            fn w_main() -> f64 {{ return (density(prev) + eval(w, pos3(0, 0, 0)) {S}); }}
            unit w {{ main w_main; inputs [prev]; }}
            entry seed {G};
            refinement step of seed {{ rerun seed {G} with w; }}
        }}"
    );
    let errs = lower_errs(&src);
    assert!(errs.contains(&"unit `w_main`: `w_main` is evaluated, but it reads `@previous` input `prev`".to_string()), "{errs:?}");
}

#[test]
fn bad_scale_once_per_key_at_the_site() {
    let site = "eval(probe, ambient(pos3)) @origin_scale(1, nan, 1) @position_scale(inf, 1, 1)";
    let src = format!(
        "program {{
            fn probe_main() -> f64 {{ return 1.0; }}
            unit probe {{ main probe_main; }}
            fn top_main() -> f64 {{ return ({site} + {site}); }}
            unit top {{ main top_main; }}
            entry top {G};
        }}"
    );
    assert_eq!(
        lower_errs(&src),
        [
            "fn `top_main` › stmt 0 › value › left: `eval` origin scale component y is NaN, which isn't finite",
            "fn `top_main` › stmt 0 › value › left: `eval` position scale component x is inf, which isn't finite",
        ]
    );
}

#[test]
fn unknown_unit() {
    let mut p = load(&format!(
        "program {{
            fn probe_main() -> f64 {{ return 1.0; }}
            unit probe {{ main probe_main; }}
            fn top_main() -> f64 {{ return eval(probe, ambient(pos3)) {S}; }}
            unit top {{ main top_main; }}
            entry top {G};
        }}"
    ));
    let other = load(&format!("program {{ fn f() -> f64 {{ return 1.0; }} unit u {{ main f; }} entry u {G}; }}"));
    let foreign = other.compute_units.iter().next().unwrap().0;
    for f in p.functions.values_mut() {
        if let Some(Statement::Return(Expression::Eval { unit, .. })) = f.body.first_mut() {
            *unit = foreign;
        }
    }
    assert_eq!(errs(&p, &registry()), ["fn `top_main` › stmt 0 › value: `eval` names a unit that isn't in the program"]);
}
