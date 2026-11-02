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
fn density_input_type() {
    assert_eq!(
        reg_errs("fn bad(x: density) -> f64 { return 1.0; }", "return p;"),
        ["fn `bad` › param 0: `density` can't be used as a value type"]
    );
    assert_eq!(
        reg_errs("", "return (p as [density; 2]);"),
        ["fn `top` › stmt 0 › value › type: `density` can't be used as a value type"]
    );
    assert_eq!(
        reg_errs("host hd: density;", "return p;"),
        ["host `hd`: `density` can't be used as a value type"]
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
            ("d", VariableType::DensityInput),
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
        var: foreign_var,
        density_function: leaf_u,
        scaled_origin: (1.0, 1.0, 1.0),
        scaled_position: (1.0, 1.0, 1.0),
        dimensions: (1, 1, 1),
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
            "fn `f` › stmt 1 › value: `density` can't be used as a value type",
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
    let t = p.permutation_tables.insert(PermutationTableInput::Base3DNoise);
    p.functions[main].host_inputs.push(h);
    let mut u = unit(main);
    let ComputeUnit::DensityFunction {
        permutation_table_inputs,
        host_inputs,
        helper_functions,
        constants,
        ..
    } = &mut u;
    permutation_table_inputs.push(t);
    host_inputs.push(h);
    helper_functions.push(helper);
    constants.push(c);
    p.compute_units.insert(u);
    p.functions.remove(helper);
    p.host_inputs.remove(h);
    p.constants.remove(c);
    p.permutation_tables.remove(t);
    assert_eq!(
        errs(&p, &registry()),
        [
            "fn `main` › hosts 0: dangling reference to a host input",
            "unit `main` › tables 0: dangling reference to a permutation table",
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
    let ComputeUnit::DensityFunction { density_inputs, .. } = &mut u;
    let var_owner = function("x", &[("d", VariableType::DensityInput)], &[], VariableType::F64, |_| vec![]);
    density_inputs.insert(DensityInput {
        var: var_owner.variables.iter().next().unwrap().0,
        density_function: gone,
        scaled_origin: (1.0, 1.0, 1.0),
        scaled_position: (1.0, 1.0, 1.0),
        dimensions: (1, 1, 1),
    });
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
