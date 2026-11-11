//! Every `LowerErrorKind` at least once, with its `Location` display, and collect-all.

use super::*;

fn reg_errs(decls: &str, body: &str) -> Vec<String> {
    let r = registry();
    errs(&load(&snippet(decls, body)), &r)
}

#[test]
fn unknown_extern() {
    assert_eq!(
        reg_errs("", "let x: f64 = 1.0; return ext cos::<(f64) -> f64>(x);"),
        ["fn `top` › stmt 1 › value: unknown extern function `cos`"]
    );
}

#[test]
fn unknown_record() {
    assert_eq!(
        reg_errs("", "return new extern Nope(p).a::<f64>;"),
        [
            "fn `top` › stmt 0 › value: unknown record type `Nope`",
            "fn `top` › stmt 0 › value › base: unknown record type `Nope`",
        ]
    );
    assert_eq!(
        reg_errs("", "let q: extern Nope; return p;"),
        ["fn `top` › local 1: unknown record type `Nope`"]
    );
}

#[test]
fn unknown_field() {
    assert_eq!(
        reg_errs("", "return ambient(origin).w::<f64>;"),
        ["fn `top` › stmt 0 › value: no field `w` on `vec3`"]
    );
    assert_eq!(
        reg_errs("", "return new extern Pair(1.0, 2).c::<f64>;"),
        ["fn `top` › stmt 0 › value: no field `c` on `Pair`"]
    );
    assert_eq!(
        reg_errs("", "return new extern Pair { a: 1.0, c: 2 }.a;"),
        ["fn `top` › stmt 0 › value › base › field `c`: no field `c` on `Pair`"]
    );
}

#[test]
fn field_on_non_struct() {
    assert_eq!(
        reg_errs("", "return p.x::<f64>;"),
        ["fn `top` › stmt 0 › value: field access on `f64`, which is not vec3, pos3 or a record"]
    );
}

#[test]
fn field_on_non_struct_base_type() {
    // The selector comes from the stored base type, not the base.
    assert_eq!(
        reg_errs("", "return ambient(origin).x::<f64; [f64; 3]>;"),
        ["fn `top` › stmt 0 › value: field access on `array[f64; 3]`, which is not vec3, pos3 or a record"]
    );
}

#[test]
fn dynamic_host() {
    assert_eq!(
        reg_errs("host hd dynamic;", "return p;"),
        ["host `#1`: dynamic host inputs are not supported"]
    );
}

#[test]
fn bad_construct() {
    assert_eq!(
        reg_errs("", "return new f64(1.0);"),
        ["fn `top` › stmt 0 › value: can't construct `f64`"]
    );
    assert_eq!(
        reg_errs("", "return new vec3 { x: 1.0 };"),
        ["fn `top` › stmt 0 › value: can't construct `vec3`"]
    );
}

#[test]
fn construct_arity() {
    assert_eq!(
        reg_errs("", "return vec3(1.0, 2.0);"),
        ["fn `top` › stmt 0 › value: construct expects 3 arguments, found 2"]
    );
    assert_eq!(
        reg_errs("", "return new extern Pair(1.0, 2, 3);"),
        ["fn `top` › stmt 0 › value: construct expects 2 arguments, found 3"]
    );
}

#[test]
fn duplicate_parameter() {
    assert_eq!(
        reg_errs("fn g(a, a) -> f64 { vars { a: f64 } return a; }", "return p;"),
        ["fn `g` › param 1: parameter listed twice"]
    );
}

/// Hand-built: a stale var, a var of another function, a foreign entry unit, a parameter that
/// isn't in the arena, a density-typed var read, and a stale density input.
#[test]
fn hand_built() {
    let mut p = empty();
    let other = function("other", &[("y", VariableType::F64)], &[], VariableType::F64, |_| vec![]);
    let foreign_var = other.variables.iter().next().unwrap().0;
    p.functions.insert(other);

    let mut f = function(
        "f",
        &[
            ("a", VariableType::F64),
            ("gone", VariableType::F64),
            ("d", VariableType::F64),
        ],
        &[0],
        VariableType::F64,
        |v| {
            vec![
                Statement::Assign {
                    target: v[1],
                    value: Expression::Variable(foreign_var),
                },
                Statement::Return(Expression::Variable(v[2])),
            ]
        },
    );
    let stale = f.variables.iter().nth(1).unwrap().0;
    f.variables.remove(stale);
    // A param whose var is no longer in the arena.
    f.parameters.push(stale);
    let fr = p.functions.insert(f);

    // A stale density input read from a fresh arena.
    let mut inputs = Arena::new();
    let leaf = p.functions.insert(function("leaf", &[], &[], VariableType::F64, |_| vec![]));
    let leaf_u = p.compute_units.insert(unit(leaf));
    let din = inputs.insert(DensityInput {
        name: Name::Anonymous,
        density_function: leaf_u,
        scaled_origin: (1.0, 1.0, 1.0),
        scaled_position: (1.0, 1.0, 1.0),
        dimensions: (1, 1, 1),
        mapping: Default::default(),
        previous: false,
    });
    inputs.remove(din);
    p.functions[fr]
        .body
        .push(Statement::Return(Expression::DensityVariable(din, None)));
    p.compute_units.insert(unit(fr));

    let mut foreign_units: Arena<ComputeUnit> = Arena::new();
    let fu = foreign_units.insert(unit(fr));
    p.entry_points.push(entry(fu));

    assert_eq!(
        errs(&p, &registry()),
        [
            "fn `f` › param 1: parameter not in the function's variables",
            "fn `f` › stmt 0: dangling reference to a variable",
            "fn `f` › stmt 0 › value: dangling reference to a variable",
            "fn `f` › stmt 2 › value: dangling reference to a density input",
            "entry 0 › unit: dangling reference to a compute unit",
        ]
    );
}

#[test]
fn dangling_lists() {
    let mut p = empty();
    let main = p.functions.insert(function("main", &[], &[], VariableType::F64, |_| vec![]));
    let helper = p.functions.insert(function("helper", &[], &[], VariableType::F64, |_| vec![]));
    let h = p.host_inputs.insert(HostInput::Static {
        name: Name::Named("h".into()),
        t: VariableType::F64,
    });
    let c = p.constants.insert(Constant {
        value: ConstExpr::Double(1.0),
        name: "c".into(),
    });
    let t = p.auto_consts.insert(AutoConst {
        name: "t".into(),
        t: VariableType::Handle("Noise"),
        initializer: "make".into(),
        args: vec![],
    });
    let leaf = p.functions.insert(function("leaf", &[], &[], VariableType::F64, |_| vec![]));
    let src = p.compute_units.insert(unit(leaf));
    let d = p.density_inputs.insert(DensityInput {
        name: Name::Anonymous,
        density_function: src,
        scaled_origin: (1.0, 1.0, 1.0),
        scaled_position: (1.0, 1.0, 1.0),
        dimensions: (1, 1, 1),
        mapping: Default::default(),
        previous: false,
    });
    p.functions[main].host_inputs.push(h);
    let mut u = unit(main);
    let ComputeUnit::DensityFunction {
        density_inputs,
        auto_consts,
        host_inputs,
        helper_functions,
        constants,
        ..
    } = &mut u;
    density_inputs.push(d);
    auto_consts.push(t);
    host_inputs.push(h);
    helper_functions.push(helper);
    constants.push(c);
    p.compute_units.insert(u);
    p.density_inputs.remove(d);
    p.functions.remove(helper);
    p.host_inputs.remove(h);
    p.constants.remove(c);
    p.auto_consts.remove(t);
    assert_eq!(
        errs(&p, &registry()),
        [
            "fn `main` › hosts 0: dangling reference to a host input",
            "unit `main` › inputs 0: dangling reference to a density input",
            "unit `main` › autos 0: dangling reference to an autoconst",
            "unit `main` › hosts 0: dangling reference to a host input",
            "unit `main` › helpers 0: dangling reference to a function",
            "unit `main` › consts 0: dangling reference to a constant",
        ]
    );
}

#[test]
fn dangling_input_source_and_main() {
    let mut p = empty();
    let main = p.functions.insert(function("main", &[], &[], VariableType::F64, |_| vec![]));
    let gone = p.compute_units.insert(unit(main));
    p.compute_units.remove(gone);
    let mut u = unit(main);
    let d = p.density_inputs.insert(DensityInput {
        name: Name::Anonymous,
        density_function: gone,
        scaled_origin: (1.0, 1.0, 1.0),
        scaled_position: (1.0, 1.0, 1.0),
        dimensions: (1, 1, 1),
        mapping: Default::default(),
        previous: false,
    });
    let ComputeUnit::DensityFunction { density_inputs, .. } = &mut u;
    density_inputs.push(d);
    p.compute_units.insert(u);
    let gone_fn = p.functions.insert(function("g", &[], &[], VariableType::F64, |_| vec![]));
    p.functions.remove(gone_fn);
    p.compute_units.insert(unit(gone_fn));
    assert_eq!(
        errs(&p, &registry()),
        [
            "unit `#1` › main: dangling reference to a function",
            "input `#0` › source: dangling reference to a compute unit",
        ]
    );
}

/// Three independent errors give three errors, in walk order (hosts, constants, functions).
#[test]
fn collects_all_in_walk_order() {
    let r = registry();
    let src = snippet(
        "host hd dynamic;
         const k = vec3(1.0).x::<f64>;",
        "return ext nope::<() -> f64>();",
    );
    assert_eq!(
        errs(&load(&src), &r),
        [
            "host `#1`: dynamic host inputs are not supported",
            "const `k` › base: construct expects 3 arguments, found 1",
            "fn `top` › stmt 0 › value: unknown extern function `nope`",
        ]
    );
}

#[test]
fn errors_display_one_per_line() {
    let r = registry();
    let e = crate::lower(
        &load(&snippet("host hd dynamic;", "return ext nope::<() -> f64>();")),
        &r,
    )
    .unwrap_err();
    assert_eq!(
        e.to_string(),
        "host `#1`: dynamic host inputs are not supported\n\
         fn `top` › stmt 0 › value: unknown extern function `nope`"
    );
}

/// `specs/density-mappings.md` §4.1: every shape error, on unread inputs (no reader rule).
#[test]
fn mapping_shape() {
    assert_eq!(
        reg_errs(
            "fn ileaf() -> i32 { return 1; }
            unit iu { main ileaf; }
            input z = leaf_u @dims(2, 1, 2) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @expand(0, *, 1);
            input w = leaf_u @dims(2, 3, 2) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @lerp(4, *, *);
            input n = iu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @lerp(*, *, *);",
            "return p;",
        ),
        [
            "input `z`: mapping factor 0 on axis x is below 1",
            "input `w`: mapping repeats axis y, but the input is 3 wide on it",
            "input `w`: mapping repeats axis z, but the input is 2 wide on it",
            "input `n`: `@lerp` needs an `f32` or `f64` input, but its unit returns `i32`",
        ]
    );
}

/// `specs/density-mappings.md` §4.2: the reader rule, once per (input, reader dims).
#[test]
fn mapping_readers() {
    let prog = |input: &str, mapping: &str, reader: &str| {
        format!(
            "program {{
                fn leaf() -> f64 {{ return 1.0; }}
                unit leaf_u {{ main leaf; }}
                input d = leaf_u @dims({input}) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) {mapping};
                fn top() -> f64 {{ return (density(d) + density(d)); }}
                unit t {{ main top; inputs [d]; }}
                entry e = t @dims({reader}) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
                entry f = t @dims({reader}) @origin_scale(2, 2, 2) @position_scale(1, 1, 1);
            }}"
        )
    };
    let lower = |src: &str| match crate::lower(&load(src), &registry()) {
        Ok(_) => vec![],
        Err(e) => e.0.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
    };
    let e = |input, mapping, reader| lower(&prog(input, mapping, reader));
    assert_eq!(
        e("1, 1, 1", "", "2, 1, 1"),
        ["input `d`: unit `top` reads it un-indexed at 2 on axis x, but it is 1 on that axis"]
    );
    // Expand: (r - 1) / n < s.
    assert!(e("4, 1, 4", "@expand(4, *, 4)", "16, 3, 16").is_empty());
    assert_eq!(
        e("4, 1, 4", "@expand(4, *, 4)", "16, 3, 17"),
        ["input `d`: unit `top` reads it at 17 on axis z (march 0), past the input's 4 cells on that axis"]
    );
    // Lerp: the high corner counts even at weight 0, so 5 wide with factor 4 fits 16, not 17.
    assert!(e("5, 49, 5", "@lerp(4, 8, 4)", "16, 384, 16").is_empty());
    assert_eq!(
        e("5, 1, 5", "@lerp(4, *, 4)", "17, 1, 16"),
        ["input `d`: unit `top` reads it at 17 on axis x (march 0), past the input's 5 cells on that axis"]
    );
    // Lerp with factor 1: r - 1 < s. A smaller reader is legal.
    assert!(e("3, 1, 1", "@lerp(1, *, *)", "2, 1, 1").is_empty());
    assert_eq!(
        e("3, 1, 1", "@lerp(1, *, *)", "4, 1, 1"),
        ["input `d`: unit `top` reads it at 4 on axis x (march 0), past the input's 3 cells on that axis"]
    );
    // An explicit index has no reader rule.
    let indexed = prog("1, 1, 1", "", "2, 1, 1").replace("(density(d) + density(d))", "density(d, 0)");
    assert!(lower(&indexed).is_empty());
}
