//! Autoconsts: the registry checks of lowering (specs/auto-consts.md §3.2), value autoconsts as
//! core hosts, and binding their values to a case (§4.3, §4.4).

use tungsten_core::{HostId, Value};

use super::eval::eval_registry;
use super::*;
use crate::{
    AutoArgType, AutoArgValue as V, AutoCoreId, CaseBindError, EvalOptions, NamedCase, ParamBindError,
    RegistryBuilder, prepare,
};

/// The corpus initializers, the `sample` extern, and the value initializers below.
fn reg() -> Registry {
    use AutoArgType::*;
    super::corpus_initializers(RegistryBuilder::new())
        .extern_fn(
            "sample",
            &[VariableType::Handle("PerlinNoiseSampler"), VariableType::F64],
            VariableType::F64,
        )
        .auto_initializer("make_salt", &[I64, Str], VariableType::I64)
        .auto_initializer("make_scale", &[F64, Bool], VariableType::F64)
        .auto_initializer("make_i8", &[I64], VariableType::I8)
        .build()
        .unwrap()
}

fn base() -> OptimizedProgram {
    load(&snippet("", "return 1.0;"))
}

fn one(p: &OptimizedProgram) -> String {
    let e = errs(p, &reg());
    assert_eq!(e.len(), 1, "{e:?}");
    e.into_iter().next().unwrap()
}

fn src_errs(src: &str) -> Vec<String> {
    errs(&load(src), &reg())
}

fn prog(body: &str) -> String {
    format!(
        "program {{ {body}
            fn m() -> f64 {{ return 1.0; }}
            unit mu {{ main m; }}
            entry mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1); }}"
    )
}

// --- §3.2 ----------------------------------------------------------------------------------

#[test]
fn bad_auto_param_type() {
    assert_eq!(
        src_errs(&prog("param x: f32;")),
        ["auto param `x`: auto param type `f32` isn't i64, f64 or bool"]
    );
}

#[test]
fn duplicate_auto_param() {
    let mut p = base();
    let dup = p.auto_params.values().next().unwrap().clone();
    p.auto_params.insert(dup);
    assert_eq!(one(&p), "auto param `seed`: auto param `seed` is declared twice");
}

#[test]
fn bad_auto_const_type() {
    let e = src_errs(&prog("auto v: vec3 = make_i8(1L);"));
    assert_eq!(e.len(), 1);
    assert!(e[0].starts_with("autoconst `v`: autoconst type `vec3` isn't a handle"), "{e:?}");
}

#[test]
fn handle_host() {
    let mut p = base();
    p.host_inputs.insert(HostInput::Static {
        name: Name::Named("hh".into()),
        t: VariableType::Handle("PerlinNoiseSampler"),
    });
    assert_eq!(one(&p), "host `hh`: host type `handle PerlinNoiseSampler` is a handle type");
}

#[test]
fn unknown_initializer() {
    let e = src_errs(&prog("auto v: i64 = make_nothing(1L);"));
    assert_eq!(e, ["autoconst `v`: unknown auto initializer `make_nothing`"]);
}

#[test]
fn arg_count() {
    let e = src_errs(&prog("auto v: i64 = make_salt(1L);"));
    assert_eq!(e, ["autoconst `v`: auto initializer `make_salt` takes 2 arguments, found 1"]);
}

#[test]
fn arg_type() {
    let e = src_errs(&prog("auto v: i64 = make_salt(1L, 2L);"));
    assert_eq!(
        e,
        ["autoconst `v` › arg 1: argument 1 of auto initializer `make_salt` is `str`, found `i64`"]
    );
    // A param has its own type.
    let e = src_errs(&prog("param f: f64; auto v: i64 = make_salt(param(f), \"a\");"));
    assert_eq!(
        e,
        ["autoconst `v` › arg 0: argument 0 of auto initializer `make_salt` is `i64`, found `f64`"]
    );
    let e = src_errs(&prog("auto v: f64 = make_scale(1L, true);"));
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("argument 0") && e[0].contains("`f64`, found `i64`"), "{e:?}");
}

#[test]
fn initializer_return() {
    let e = src_errs(&prog("auto v: i32 = make_salt(1L, \"a\");"));
    assert_eq!(
        e,
        ["autoconst `v`: auto initializer `make_salt` returns `i64`, but the autoconst is `i32`"]
    );
    // A handle of another type.
    let mut p = base();
    p.auto_consts.values_mut().next().unwrap().t = VariableType::Handle("InterpolatedNoiseSampler");
    assert!(one(&p).contains("returns `handle PerlinNoiseSampler`, but the autoconst is `handle InterpolatedNoiseSampler`"));
}

#[test]
fn str_arg_has_no_nul() {
    let mut p = load(&prog("auto v: i64 = make_salt(1L, \"ab\");"));
    let a = p.auto_consts.values_mut().next().unwrap();
    a.args[1] = AutoConstArg::Str("a\0b".into());
    assert_eq!(one(&p), "autoconst `v` › arg 1: string argument 1 contains a NUL character");
}

#[test]
fn dangling_refs() {
    let mut p = base();
    let other = load(&snippet("", "return 1.0;"));
    // Refs of another program's arenas.
    let foreign = other.auto_params.iter().next().unwrap().0;
    p.auto_consts.values_mut().next().unwrap().args[0] = AutoConstArg::Param(foreign);
    let e = errs(&p, &reg());
    assert!(e.iter().any(|m| m.contains("dangling reference to an auto param")), "{e:?}");
    let mut p = base();
    let foreign = other.auto_consts.iter().next().unwrap().0;
    let ComputeUnit::DensityFunction { auto_consts, .. } = p.compute_units.values_mut().next().unwrap();
    auto_consts.push(foreign);
    let e = errs(&p, &reg());
    assert!(e.iter().any(|m| m.contains("dangling reference to an autoconst")), "{e:?}");
}

#[test]
fn all_errors_are_collected() {
    let e = src_errs(&prog(
        "param x: f32; auto a: i64 = make_nothing(); auto b: i64 = make_salt(1L);",
    ));
    assert_eq!(e.len(), 3, "{e:?}");
}

// --- value autoconsts ----------------------------------------------------------------------

fn value_prog(unit_autos: &str) -> String {
    format!(
        "program {{
            param seed: i64;
            auto salt: i64 = make_salt(param(seed), \"caves\");
            auto small: i8 = make_i8(param(seed));
            fn m() -> i64 {{ return (auto(salt) + (auto(small) as i64)); }}
            unit mu {{ main m; autos [{unit_autos}]; }}
            entry mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1); }}"
    )
}

#[test]
fn value_autoconsts_are_hosts_after_the_real_ones() {
    let src = "program {
            host h: f64;
            param seed: i64;
            auto salt: i64 = make_salt(param(seed), \"x\");
            auto t: handle PerlinNoiseSampler = make_perm(param(seed), 1L);
            fn m() -> i64 @hosts(h) { return auto(salt); }
            unit mu { main m; hosts [h]; autos [salt, t]; }
            entry mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1); }";
    let l = ok(&load(src), &reg());
    let p = &l.program;
    assert_eq!(p.hosts.len(), 2);
    assert_eq!(p.tables.len(), 1);
    // The function lists the host it reads; the unit lists all of them.
    assert_eq!(p.functions[0].hosts, [HostId(0), HostId(1)]);
    assert_eq!(p.units[0].hosts, [HostId(0), HostId(1)]);
    assert_eq!(p.units[0].tables.len(), 1);
    assert_eq!(l.map.host_name(HostId(1)), Some("salt"));
    assert_eq!(l.map.auto_consts[0].core, AutoCoreId::Host(HostId(1)));
    assert!(matches!(l.map.auto_consts[1].core, AutoCoreId::Table(_)));
    // `host(name = ..)` bindings never see an autoconst.
    assert!(l.map.host_names[1].is_none());
}

#[test]
fn unlisted_value_read_names_the_autoconst() {
    let l = lower(&load(&value_prog("small")), &reg()).unwrap();
    let e = crate::check_lowered(&l).unwrap_err().to_string();
    assert!(e.contains("autoconst `salt` isn't in the autos of unit `m`"), "{e}");
    assert!(!e.contains("host `"), "{e}");
}

#[test]
fn unread_autoconsts_are_resolved() {
    let l = ok(&load(&value_prog("salt, small")), &reg());
    let params = [("seed".to_string(), V::I64(300))];
    let r = l.map.resolved_auto_consts(&params).unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!((r[0].name, r[0].initializer), ("salt", "make_salt"));
    assert_eq!(r[0].args, [V::I64(300), V::Str("caves".into())]);
    assert_eq!(r[1].args, [V::I64(300)]);
    assert_eq!(r[1].t, &VariableType::I8);
}

/// An autoconst nothing reads or lists is still in `resolved_auto_consts`.
#[test]
fn unlisted_autoconsts_are_resolved() {
    let p = load(&prog("auto never: i64 = make_salt(1L, \"x\");"));
    let l = ok(&p, &reg());
    let r = l.map.resolved_auto_consts(&[]).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].name, "never");
}

fn named(auto_values: Vec<(HostId, Value)>) -> NamedCase {
    NamedCase { origin: [0.0; 3], hosts: vec![], auto_values }
}

#[test]
fn values_reach_the_program() {
    let prep = prepare(&load(&value_prog("salt, small")), &reg()).unwrap();
    let r = prep.map().resolved_auto_consts(&[("seed".into(), V::I64(1))]).unwrap();
    let hosts: Vec<_> = r
        .iter()
        .map(|a| match a.core {
            AutoCoreId::Host(h) => h,
            AutoCoreId::Table(_) => panic!(),
        })
        .collect();
    let case = prep
        .case(&named(vec![(hosts[0], Value::I64(1000)), (hosts[1], Value::I8(-3))]))
        .unwrap();
    let res = prep.eval(&case, &super::eval::Toy(&prep), EvalOptions::default()).unwrap();
    assert_eq!(res.entries[0][0], Ok(Value::I64(997)));
}

#[test]
fn case_binding_failures() {
    let prep = prepare(&load(&value_prog("salt, small")), &reg()).unwrap();
    let (salt, small) = (HostId(0), HostId(1));
    // Missing.
    assert_eq!(
        prep.case(&named(vec![(salt, Value::I64(1))])).unwrap_err(),
        CaseBindError::AutoValueMissing { name: "small".into() }
    );
    // A host that isn't a value autoconst: out of range, and given twice.
    assert_eq!(
        prep.case(&named(vec![(salt, Value::I64(1)), (small, Value::I8(1)), (HostId(2), Value::I8(1))]))
            .unwrap_err(),
        CaseBindError::AutoValueUnknown(HostId(2))
    );
    assert_eq!(
        prep.case(&named(vec![(salt, Value::I64(1)), (salt, Value::I64(1)), (small, Value::I8(1))]))
            .unwrap_err(),
        CaseBindError::AutoValueUnknown(salt)
    );
    // A value of the wrong type is core's error.
    assert!(matches!(
        prep.case(&named(vec![(salt, Value::I32(1)), (small, Value::I8(1))])).unwrap_err(),
        CaseBindError::Invalid(_)
    ));
}

#[test]
fn a_real_host_is_not_an_auto_value() {
    let src = "program {
            host h: i64;
            auto salt: i64 = make_salt(1L, \"x\");
            fn m() -> i64 @hosts(h) { return (host(h) + auto(salt)); }
            unit mu { main m; hosts [h]; autos [salt]; }
            entry mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1); }";
    let prep = prepare(&load(src), &reg()).unwrap();
    let c = NamedCase {
        origin: [0.0; 3],
        hosts: vec![("h".into(), Value::I64(5))],
        auto_values: vec![(HostId(0), Value::I64(1)), (HostId(1), Value::I64(2))],
    };
    assert_eq!(prep.case(&c).unwrap_err(), CaseBindError::AutoValueUnknown(HostId(0)));
}

/// An autoconst and a host may share a name; each is bound its own way.
#[test]
fn autoconst_and_host_names_stay_separate() {
    let src = "program {
            host salt: i64;
            auto s2: i64 @name(\"salt\") = make_salt(1L, \"x\");
            fn m() -> i64 @hosts(salt) { return (host(salt) + auto(s2)); }
            unit mu { main m; hosts [salt]; autos [s2]; }
            entry mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1); }";
    let prep = prepare(&load(src), &reg()).unwrap();
    let c = NamedCase {
        origin: [0.0; 3],
        hosts: vec![("salt".into(), Value::I64(5))],
        auto_values: vec![(HostId(1), Value::I64(2))],
    };
    let case = prep.case(&c).unwrap();
    let res = prep.eval(&case, &super::eval::Toy(&prep), EvalOptions::default()).unwrap();
    assert_eq!(res.entries[0][0], Ok(Value::I64(7)));
}

#[test]
fn param_binding_covers_every_argument_kind() {
    let src = prog("param a: f64; param b: bool; auto v: f64 = make_scale(param(a), param(b));");
    let l = ok(&load(&src), &reg());
    let r = l
        .map
        .resolved_auto_consts(&[("b".into(), V::Bool(true)), ("a".into(), V::F64(0.5))])
        .unwrap();
    assert_eq!(r[0].args, [V::F64(0.5), V::Bool(true)]);
    assert_eq!(
        l.map.resolved_auto_consts(&[("a".into(), V::I64(1)), ("b".into(), V::Bool(true))]).unwrap_err(),
        ParamBindError::WrongType { name: "a".into(), expected: "f64".into(), found: "i64".into() }
    );
}

#[test]
fn a_sampler_is_not_a_permutation_table() {
    // The registry's `sample` takes a PerlinNoiseSampler; `b3` is the other handle type.
    let e = crate::check_lowered(&lower(&load(&snippet("", "return ext sample(auto(b3), p);")), &reg()).unwrap())
        .unwrap_err();
    assert!(e.to_string().contains("handle InterpolatedNoiseSampler"), "{e}");
}

#[test]
fn eval_registry_is_unaffected() {
    // The older registries still work with handle-only programs.
    prepare(&base(), &eval_registry()).unwrap();
}
