//! Interpreter tests with hand-computed results (TESTING.md §11 coverage that needs no backend),
//! plus the `prepare` / `Prepared` / `evaluate` API.

use tungsten_core::exec::{CaseResult, Failure};
use tungsten_core::{ConstId, EvalError, ExternFail, ExternId, Externs, F64x3, I32x3, InputId, StuckReason, Value};
use tungsten_ir::VariableType;

use super::*;
use crate::{
    CaseBindError, EvalFailure, EvalOptions, EvaluateError, Invalid, NamedCase, Prepared,
    RegistryBuilder, evaluate, prepare,
};

/// `sin`, `sample`, `sqrt` (domain error below 0), `undeclared` (fails, but declares no domain
/// error) and `bad` (returns the wrong shape), plus the record `Pair`.
pub fn eval_registry() -> Registry {
    super::corpus_initializers(RegistryBuilder::new())
        .extern_fn("sin", &[VariableType::F64], VariableType::F64)
        .extern_fn("sample", &[VariableType::Handle("PerlinNoiseSampler"), VariableType::F64], VariableType::F64)
        .extern_fn_with_domain_error("sqrt", &[VariableType::F64], VariableType::F64)
        .extern_fn("undeclared", &[VariableType::F64], VariableType::F64)
        .extern_fn("bad", &[VariableType::F64], VariableType::F64)
        .auto_initializer("make_perm0", &[], VariableType::Handle("PerlinNoiseSampler"))
        .record("Pair", &[("a", VariableType::F64), ("b", VariableType::I32)])
        .build()
        .unwrap()
}

/// Toy externs that resolve names through the `Prepared` map.
pub struct Toy<'a>(pub &'a Prepared);

impl Externs for Toy<'_> {
    fn call(&self, ext: ExternId, args: &[Value]) -> Result<Value, ExternFail> {
        let x = match args.last() {
            Some(Value::F64(b)) => f64::from_bits(*b),
            _ => 0.0,
        };
        match self.0.map().extern_name(ext) {
            Some("sin") => Ok(Value::F64(x.sin().to_bits())),
            Some("sample") => Ok(Value::F64((x * 0.5).to_bits())),
            Some("sqrt") | Some("undeclared") => {
                if x < 0.0 {
                    Err(ExternFail::Domain)
                } else {
                    Ok(Value::F64(x.sqrt().to_bits()))
                }
            }
            _ => Ok(Value::I32(0)),
        }
    }
}

pub fn f64v(x: f64) -> Value {
    Value::F64(x.to_bits())
}

fn named(hosts: &[(&str, Value)]) -> NamedCase {
    NamedCase {
        origin: [0.0, 0.0, 0.0],
        hosts: hosts.iter().map(|(n, v)| (n.to_string(), v.clone())).collect(),
        auto_values: vec![],
    }
}

pub fn run_with(src: &str, hosts: &[(&str, Value)], opts: EvalOptions) -> CaseResult {
    let prep = prepare(&load(src), &eval_registry()).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let case = prep.case(&named(hosts)).unwrap();
    prep.eval(&case, &Toy(&prep), opts).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

pub fn run(src: &str) -> CaseResult {
    run_with(src, &[], EvalOptions::default())
}

/// `fn m() -> <ty> { <body> }` as the only unit, one 1×1×1 entry.
fn single(ty: &str, decls: &str, body: &str) -> String {
    format!(
        "program {{ {decls}
            fn m() -> {ty} {{ {body} }}
            unit mu {{ main m; }}
            entry mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1); }}"
    )
}

/// The value of `return <e>;` in a `<ty>` main.
#[track_caller]
fn val(ty: &str, e: &str) -> Result<Value, EvalError> {
    let r = run(&single(ty, "", &format!("return {e};")));
    r.entries[0][0].clone()
}

#[track_caller]
fn is(ty: &str, e: &str, want: Value) {
    assert_eq!(val(ty, e), Ok(want), "{e}");
}

// --- operators × types ---------------------------------------------------------------------

#[test]
fn integer_ops() {
    is("i32", "(7 + 5)", Value::I32(12));
    is("i32", "(7 - 9)", Value::I32(-2));
    is("i32", "(7 * -3)", Value::I32(-21));
    is("i32", "(-7 / 2)", Value::I32(-3));
    is("i32", "(12 & 10)", Value::I32(8));
    is("i32", "(12 | 10)", Value::I32(14));
    is("i32", "(12 ^ 10)", Value::I32(6));
    is("i32", "!(0)", Value::I32(-1));
    is("i32", "-(5)", Value::I32(-5));
    is("i64", "(5L * 3L)", Value::I64(15));
    is("i64", "(-9L / 4L)", Value::I64(-2));
    is("i64", "(1L << 40)", Value::I64(1 << 40));
    is("i8", "(100b + 100b)", Value::I8(-56));
    is("i8", "(-8b >> 1)", Value::I8(-4));
    is("bool", "(3 < 4)", Value::Bool(true));
    is("bool", "(3 >= 4)", Value::Bool(false));
    is("bool", "(3L != 4L)", Value::Bool(true));
    is("bool", "(5b == 5b)", Value::Bool(true));
    is("bool", "(true ^ true)", Value::Bool(false));
    is("bool", "(true & false)", Value::Bool(false));
    is("bool", "(true | false)", Value::Bool(true));
    is("bool", "!(true)", Value::Bool(false));
    is("bool", "(true == false)", Value::Bool(false));
}

#[test]
fn overflow_and_shift_masks() {
    is("i32", "(2147483647 + 1)", Value::I32(i32::MIN));
    is("i32", "(-2147483648 / -1)", Value::I32(i32::MIN));
    is("i64", "(-9223372036854775808L / -1L)", Value::I64(i64::MIN));
    is("i8", "(-128b / -1b)", Value::I8(-128));
    is("i32", "(1 << 33)", Value::I32(2));
    is("i32", "(-8 >>> 1)", Value::I32(0x7fff_fffc));
    is("i32", "(1 << -1)", Value::I32(i32::MIN));
    is("i64", "(1L << 65)", Value::I64(2));
    is("i8", "(-8b >>> 1)", Value::I8(-4));
    is("i8", "(-1b >>> 28)", Value::I8(15));
    assert_eq!(val("i32", "(1 / 0)"), Err(EvalError::DivByZero));
    assert_eq!(val("i64", "(1L / 0L)"), Err(EvalError::DivByZero));
    assert_eq!(val("i8", "(1b / 0b)"), Err(EvalError::DivByZero));
}

#[test]
fn float_ops() {
    is("f64", "(1.5 + 2.25)", f64v(3.75));
    is("f64", "(1.0 / 0.0)", f64v(f64::INFINITY));
    is("f32", "(0.1f + 0.2f)", Value::F32((0.1f32 + 0.2f32).to_bits()));
    is("f32", "-(1.5f)", Value::F32((-1.5f32).to_bits()));
    is("bool", "(nan == nan)", Value::Bool(false));
    is("bool", "(nan != nan)", Value::Bool(true));
    is("bool", "(nan < 1.0)", Value::Bool(false));
    is("bool", "(1.0f <= 1.0f)", Value::Bool(true));
    is("bool", "(2.0 > 1.0)", Value::Bool(true));
    // -0.0
    is("f64", "(-0.0 + -0.0)", f64v(-0.0));
    is("f64", "(-0.0 + 0.0)", f64v(0.0));
    is("f64", "-(0.0)", f64v(-0.0));
    is("bool", "(0.0 == -0.0)", Value::Bool(true));
}

#[test]
fn vector_ops() {
    is("vec3", "(vec3(1.0, 2.0, 3.0) * vec3(2.0, 2.0, 2.0))", Value::Vec3(F64x3 { x: 2.0f64.to_bits(), y: 4.0f64.to_bits(), z: 6.0f64.to_bits() }));
    is("vec3", "(vec3(0.5, 0.5, 0.5) + pos3(1, 2, 3))", Value::Vec3(F64x3 { x: 1.5f64.to_bits(), y: 2.5f64.to_bits(), z: 3.5f64.to_bits() }));
    is("vec3", "(pos3(1, 2, 3) - vec3(1.0, 1.0, 1.0))", Value::Vec3(F64x3 { x: 0, y: 1.0f64.to_bits(), z: 2.0f64.to_bits() }));
    is("pos3", "(pos3(7, 8, 9) / pos3(2, 2, 2))", Value::Pos3(I32x3 { x: 3, y: 4, z: 4 }));
    is("pos3", "(pos3(2147483647, 0, 0) + pos3(1, 0, 0))", Value::Pos3(I32x3 { x: i32::MIN, y: 0, z: 0 }));
    is("pos3", "-(pos3(1, -2, -2147483648))", Value::Pos3(I32x3 { x: -1, y: 2, z: i32::MIN }));
    is("vec3", "-(vec3(0.0, 1.0, -2.0))", Value::Vec3(F64x3 { x: (-0.0f64).to_bits(), y: (-1.0f64).to_bits(), z: 2.0f64.to_bits() }));
    assert_eq!(val("pos3", "(pos3(1, 1, 1) / pos3(1, 0, 0))"), Err(EvalError::DivByZero));
    is("f64", "vec3(1.0, 2.0, 3.0).y", f64v(2.0));
    is("i32", "pos3(1, 2, 3).z", Value::I32(3));
}

#[test]
fn casts() {
    is("i32", "(nan as i32)", Value::I32(0));
    is("i32", "(inf as i32)", Value::I32(i32::MAX));
    is("i64", "(-inf as i64)", Value::I64(i64::MIN));
    is("i32", "(3.0e10 as i32)", Value::I32(i32::MAX));
    is("i32", "(-2.9 as i32)", Value::I32(-2));
    is("i8", "(300.0 as i8)", Value::I8(44));
    is("i8", "(300 as i8)", Value::I8(44));
    is("i32", "(4294967297L as i32)", Value::I32(1));
    is("f32", "(-0.0 as f32)", Value::F32((-0.0f32).to_bits()));
    is("f32", "(16777217 as f32)", Value::F32(16_777_216.0f32.to_bits()));
    is("f64", "(0.1f as f64)", f64v(0.1f32 as f64));
    is("i32", "(true as i32)", Value::I32(1));
    is("i64", "(false as i64)", Value::I64(0));
    is("vec3", "(pos3(1, -2, 3) as vec3)", Value::Vec3(F64x3 { x: 1.0f64.to_bits(), y: (-2.0f64).to_bits(), z: 3.0f64.to_bits() }));
    is("f64", "(2.5 as f64)", f64v(2.5));
}

#[test]
fn short_circuit() {
    is("bool", "(false && ((1 / 0) == 1))", Value::Bool(false));
    is("bool", "(true || ((1 / 0) == 1))", Value::Bool(true));
    // BitAnd/BitOr on bool are eager.
    assert_eq!(val("bool", "(false & ((1 / 0) == 1))"), Err(EvalError::DivByZero));
    // Left to right: the left error wins.
    assert_eq!(val("i32", "((1 / 0) + [1][5])"), Err(EvalError::DivByZero));
    assert_eq!(val("i32", "([1][5] + (1 / 0))"), Err(EvalError::OutOfBounds));
}

#[test]
fn records_arrays_index() {
    is("f64", "new extern Pair { b: 3, a: 1.5 }.a", f64v(1.5));
    is("i32", "new extern Pair(1.5, 7).b", Value::I32(7));
    is("i32", "[1, 2, 3][2]", Value::I32(3));
    assert_eq!(val("i32", "[1, 2, 3][3]"), Err(EvalError::OutOfBounds));
    assert_eq!(val("i32", "[1, 2, 3][-1]"), Err(EvalError::OutOfBounds));
}

// --- statements ----------------------------------------------------------------------------

#[test]
fn loops() {
    // break leaves only the inner loop; return leaves everything.
    let body = "let n: i32 = 0;
        repeat 3 {
            repeat 5 { n = (n + 1); if (n > 1) { break; } }
            n = (n + 10);
        }
        while true {
            repeat 2 { if (n > 30) { return n; } n = (n + 100); }
        }
        return -1;";
    // Outer iterations: n=1→2 (break)→12, 13 (break)→23, 24 (break)→34. Then the while returns 34.
    assert_eq!(run(&single("i32", "", body)).entries[0][0], Ok(Value::I32(34)));
    // repeat 0 never runs its body.
    let body = "let n: i32 = 1; repeat 0 { n = (1 / 0); } return n;";
    assert_eq!(run(&single("i32", "", body)).entries[0][0], Ok(Value::I32(1)));
    // while with a false cond.
    let body = "let n: i32 = 1; while (n > 5) { n = (1 / 0); } return n;";
    assert_eq!(run(&single("i32", "", body)).entries[0][0], Ok(Value::I32(1)));
}

#[test]
fn out_of_fuel_per_cell() {
    // Cell x=0 loops forever, cell x=1 doesn't.
    let src = "program {
        fn m() -> i32 {
            let n: i32 = 0;
            while (ambient(pos3).x == 0) { n = (n + 1); }
            return n;
        }
        unit mu { main m; }
        entry mu @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    }";
    let r = run_with(src, &[], EvalOptions { fuel: 100, memo: true });
    assert_eq!(r.entries[0], vec![Err(EvalError::OutOfFuel), Ok(Value::I32(0))]);
    // Fuel is per `While` execution: two loops of 60 each fit in fuel 100.
    let body = "let n: i32 = 0;
        while (n < 60) { n = (n + 1); }
        while (n < 120) { n = (n + 1); }
        return n;";
    let r = run_with(&single("i32", "", body), &[], EvalOptions { fuel: 100, memo: true });
    assert_eq!(r.entries[0][0], Ok(Value::I32(120)));
    // Exactly `fuel` iterations is fine; the cond being true after them is not.
    let body = "let n: i32 = 0; while (n < 100) { n = (n + 1); } return n;";
    assert_eq!(run_with(&single("i32", "", body), &[], EvalOptions { fuel: 100, memo: true }).entries[0][0], Ok(Value::I32(100)));
    assert_eq!(run_with(&single("i32", "", body), &[], EvalOptions { fuel: 99, memo: true }).entries[0][0], Err(EvalError::OutOfFuel));
    // Repeat is free.
    let body = "let n: i32 = 0; repeat 1000 { n = (n + 1); } return n;";
    assert_eq!(run_with(&single("i32", "", body), &[], EvalOptions { fuel: 1, memo: true }).entries[0][0], Ok(Value::I32(1000)));
}

#[test]
fn helpers_and_hosts() {
    let src = "program {
        host scale: f64;
        fn twice(x: f64) -> f64 { return (x * 2.0); }
        fn m() -> f64 @hosts(scale) { return twice(host(scale)); }
        unit mu { main m; helpers [twice]; hosts [scale]; }
        entry mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    }";
    let r = run_with(src, &[("scale", f64v(1.25))], EvalOptions::default());
    assert_eq!(r.entries[0][0], Ok(f64v(2.5)));
}

#[test]
fn ambients_and_positions() {
    let src = "program {
        fn m() -> vec3 { return ((ambient(origin) * ambient(origin_scale)) + (ambient(pos3) * ambient(position_scale))); }
        unit mu { main m; }
        entry mu @dims(2, 1, 2) @origin_scale(2, 2, 2) @position_scale(0.5, 1, 1);
    }";
    let prep = prepare(&load(src), &eval_registry()).unwrap();
    let case = prep.case(&NamedCase { origin: [10.0, 0.0, -1.0], hosts: vec![], auto_values: vec![] }).unwrap();
    let r = prep.eval(&case, &Toy(&prep), EvalOptions::default()).unwrap();
    let v = |x: f64, y: f64, z: f64| Ok(Value::Vec3(F64x3 { x: x.to_bits(), y: y.to_bits(), z: z.to_bits() }));
    // Flat order: x fastest, then z.
    assert_eq!(r.entries[0], vec![v(20.0, 0.0, -2.0), v(20.5, 0.0, -2.0), v(20.0, 0.0, -1.0), v(20.5, 0.0, -1.0)]);
}

// --- constants -----------------------------------------------------------------------------

#[test]
fn const_to_const() {
    let src = single("f64", "const a = 2.0; const b = (const(a) * 3.0);", "return const(b);")
        .replace("unit mu { main m; }", "unit mu { main m; consts [a, b]; }");
    assert_eq!(run(&src).entries[0][0], Ok(f64v(6.0)));
}

#[test]
fn eager_const_failure() {
    // `k` is never read, but it fails, so every cell of every entry fails.
    let src = "program {
        const ok = 1;
        const k = (1 / 0);
        const k2 = ([1][5]);
        fn m() -> f64 { return 1.0; }
        unit mu { main m; }
        entry e1 = mu @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
        entry e2 = mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    }";
    let r = run(src);
    // The lowest failing ConstId wins; every failing const is listed.
    assert_eq!(r.entries, vec![vec![Err(EvalError::DivByZero); 2], vec![Err(EvalError::DivByZero)]]);
    assert_eq!(
        r.failures,
        vec![
            Failure::Const { id: ConstId(1), error: EvalError::DivByZero },
            Failure::Const { id: ConstId(2), error: EvalError::OutOfBounds },
        ]
    );
}

// --- density inputs ------------------------------------------------------------------------

/// A leaf unit returning `x + 10*y + 100*z` of its cell, read by `top` through input `d`.
fn density_prog(input_dims: &str, entry_dims: &str, read: &str) -> String {
    mapped_prog(input_dims, "", entry_dims, read)
}

/// [`density_prog`] with `mapping` after the input's grid attributes.
fn mapped_prog(input_dims: &str, mapping: &str, entry_dims: &str, read: &str) -> String {
    format!(
        "program {{
            fn leaf() -> f64 {{
                let c: pos3 = ambient(pos3);
                return (((c.x + (10 * c.y)) + (100 * c.z)) as f64);
            }}
            fn top() -> f64 {{ return {read}; }}
            input d = leaf_u @dims({input_dims}) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) {mapping};
            unit leaf_u {{ main leaf; }}
            unit t {{ main top; inputs [d]; }}
            entry t @dims({entry_dims}) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
        }}"
    )
}

fn oks(xs: &[f64]) -> Vec<Result<Value, EvalError>> {
    xs.iter().map(|x| Ok(f64v(*x))).collect()
}

#[test]
fn density_unit_input() {
    // A (1,1,1) input is read at cell 0 from every reader cell.
    let r = run(&mapped_prog("1, 1, 1", "@expand(*, *, *)", "2, 2, 1", "density(d)"));
    assert_eq!(r.entries[0], oks(&[0.0, 0.0, 0.0, 0.0]));
}

#[test]
fn density_equal_dims() {
    let r = run(&density_prog("2, 1, 2", "2, 1, 2", "density(d)"));
    assert_eq!(r.entries[0], oks(&[0.0, 1.0, 100.0, 101.0]));
}

#[test]
fn density_different_dims() {
    // Strict 1-to-1: an un-indexed Identity read across shapes is a lowering error, for a
    // smaller and for a larger input.
    for (input, reader) in [("1, 2, 1", "2, 1, 1"), ("2, 1, 1", "3, 1, 1"), ("1, 1, 1", "2, 1, 1")] {
        let e = crate::lower(&load(&density_prog(input, reader, "density(d)")), &registry()).unwrap_err();
        assert!(matches!(e.0[..], [crate::LowerError { kind: crate::LowerErrorKind::MappingDimsMismatch { .. }, .. }]), "{e}");
    }
    // An explicit index stays legal across shapes: reader (2,1,1) cell k reads buf[k] of a
    // (1,2,1) input, i.e. its cell (0,k,0).
    let r = run(&density_prog("1, 2, 1", "2, 1, 1", "density(d, ambient(pos3).x)"));
    assert_eq!(r.entries[0], oks(&[0.0, 10.0]));
}

#[test]
fn density_explicit_index() {
    // Corner reads at the grid edge: index 7 is the last cell of 2×2×2, 8 is out.
    let r = run(&density_prog("2, 2, 2", "1, 1, 1", "density(d, 7)"));
    assert_eq!(r.entries[0], oks(&[111.0]));
    let r = run(&density_prog("2, 2, 2", "1, 1, 1", "density(d, 8)"));
    assert_eq!(r.entries[0], vec![Err(EvalError::OutOfBounds)]);
    let r = run(&density_prog("2, 2, 2", "1, 1, 1", "density(d, -1)"));
    assert_eq!(r.entries[0], vec![Err(EvalError::OutOfBounds)]);
}

/// Unit `t` declares inputs `good` (fine) and `bad` (cell 2 of 4 divides by zero) and reads
/// neither.
fn eager_input_prog(order: &str) -> String {
    let good = "input good = ok_u @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);";
    let bad = "input bad = bad_u @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);";
    let bad2 = "input bad2 = bad2_u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);";
    let (inputs, list) = match order {
        "good_bad" => (format!("{good} {bad}"), "good, bad"),
        _ => (format!("{bad2} {bad}"), "bad2, bad"),
    };
    format!(
        "program {{
            fn okf() -> f64 {{ return 1.0; }}
            fn badf() -> f64 {{ return ((1 / (ambient(pos3).x - 2)) as f64); }}
            fn bad2f() -> f64 {{ return ([1.0][1]); }}
            fn top() -> f64 {{ return 5.0; }}
            {inputs}
            unit ok_u {{ main okf; }}
            unit bad_u {{ main badf; }}
            unit bad2_u {{ main bad2f; }}
            unit t {{ main top; inputs [{list}]; }}
            entry t @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
        }}"
    )
}

#[test]
fn eager_input_failure() {
    let r = run(&eager_input_prog("good_bad"));
    assert_eq!(r.entries[0], vec![Err(EvalError::DivByZero); 2]);
    assert_eq!(r.failures, vec![Failure::Input { id: InputId(1), cell: 2, error: EvalError::DivByZero }]);
}

/// One failing input listed by two units (`a` reads it through a helper, `b` directly): both
/// fail, and the input is built and reported once, with memo on or off.
#[test]
fn shared_input_failure() {
    let src = "program {
        fn badf() -> f64 { return ((1 / (ambient(pos3).x - 2)) as f64); }
        unit bad_u { main badf; }
        input d = bad_u @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
        fn g() -> f64 { return density(d); }
        fn a_main() -> f64 { return g(); }
        fn b_main() -> f64 { return (density(d) + 1.0); }
        unit a { main a_main; helpers [g]; inputs [d]; }
        unit b { main b_main; inputs [d]; }
        entry a @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
        entry b @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    }";
    for memo in [true, false] {
        let r = run_with(src, &[], EvalOptions { memo, ..EvalOptions::default() });
        assert_eq!(r.entries, vec![vec![Err(EvalError::DivByZero); 4], vec![Err(EvalError::DivByZero); 4]]);
        assert_eq!(r.failures, vec![Failure::Input { id: InputId(0), cell: 2, error: EvalError::DivByZero }]);
    }
}

#[test]
fn canonical_input_error() {
    // Two failing inputs: the lowest InputId's error wins, and both are listed once.
    let r = run(&eager_input_prog("bad2_bad"));
    assert_eq!(r.entries[0], vec![Err(EvalError::OutOfBounds); 2]);
    assert_eq!(
        r.failures,
        vec![
            Failure::Input { id: InputId(0), cell: 0, error: EvalError::OutOfBounds },
            Failure::Input { id: InputId(1), cell: 2, error: EvalError::DivByZero },
        ]
    );
}

// --- externs -------------------------------------------------------------------------------

#[test]
fn externs() {
    is("f64", "ext sqrt::<(f64) -> f64>(2.25)", f64v(1.5));
    let src = "program {
        auto t0: handle PerlinNoiseSampler = make_perm0();
        fn m() -> f64 { return ext sample(auto(t0), 3.0); }
        unit mu { main m; autos [t0]; }
        entry mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    }";
    assert_eq!(run(src).entries[0][0], Ok(f64v(1.5)));
    assert_eq!(val("f64", "ext sqrt::<(f64) -> f64>(-1.0)"), Err(EvalError::ExternDomain));
}

#[test]
fn stuck_externs_are_errors() {
    for (call, reason) in [
        ("ext bad::<(f64) -> f64>(1.0)", StuckReason::ExternResult),
        ("ext undeclared::<(f64) -> f64>(-1.0)", StuckReason::ExternDomainUndeclared),
    ] {
        let src = single("f64", "", &format!("return {call};"));
        let prep = prepare(&load(&src), &eval_registry()).unwrap();
        let case = prep.case(&named(&[])).unwrap();
        match prep.eval(&case, &Toy(&prep), EvalOptions::default()) {
            Err(EvalFailure::Stuck { reasons, .. }) => assert_eq!(reasons, vec![(0, 0, reason)]),
            other => panic!("{call}: {other:?}"),
        }
    }
}

// --- API -----------------------------------------------------------------------------------

fn host_prog() -> String {
    "program {
        host a: f64;
        host b: i32;
        fn m() -> f64 @hosts(a, b) { return (host(a) + (host(b) as f64)); }
        unit mu { main m; hosts [a, b]; }
        entry mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    }"
    .to_string()
}

#[test]
fn case_binding() {
    let prep = prepare(&load(&host_prog()), &eval_registry()).unwrap();
    assert_eq!(prep.host_id("b").map(|h| h.0), Some(1));
    assert_eq!(prep.extern_id("sqrt").map(|e| e.0), Some(2));
    let ok = prep.case(&named(&[("b", Value::I32(2)), ("a", f64v(0.5))])).unwrap();
    assert_eq!(prep.eval(&ok, &Toy(&prep), EvalOptions::default()).unwrap().entries[0][0], Ok(f64v(2.5)));
    assert_eq!(prep.case(&named(&[("a", f64v(0.5))])), Err(CaseBindError::Missing("b".into())));
    assert_eq!(
        prep.case(&named(&[("a", f64v(0.5)), ("b", Value::I32(2)), ("a", f64v(1.0))])),
        Err(CaseBindError::Duplicate("a".into()))
    );
    assert_eq!(
        prep.case(&named(&[("a", f64v(0.5)), ("b", Value::I32(2)), ("zz", f64v(1.0))])),
        Err(CaseBindError::Unknown("zz".into()))
    );
    assert!(matches!(prep.case(&named(&[("a", Value::I32(1)), ("b", Value::I32(2))])), Err(CaseBindError::Invalid(_))));
}

#[test]
fn check_case_failures() {
    let prep = prepare(&load(&host_prog()), &eval_registry()).unwrap();
    let mut case = prep.case(&named(&[("a", f64v(0.5)), ("b", Value::I32(2))])).unwrap();
    case.hosts.pop();
    assert!(matches!(prep.eval(&case, &Toy(&prep), EvalOptions::default()), Err(EvalFailure::Case(_))));
    case.hosts.push(f64v(1.0));
    assert!(matches!(prep.eval(&case, &Toy(&prep), EvalOptions::default()), Err(EvalFailure::Case(_))));
}

#[test]
fn evaluate_one_shot() {
    let r = evaluate(
        &load(&host_prog()),
        &eval_registry(),
        &named(&[("a", f64v(1.0)), ("b", Value::I32(1))]),
        &Toy(&prepare(&load(&host_prog()), &eval_registry()).unwrap()),
        EvalOptions::default(),
    )
    .unwrap();
    assert_eq!(r.entries, vec![vec![Ok(f64v(2.0))]]);
    let bad = single("f64", "", "return 1;");
    assert!(matches!(
        evaluate(&load(&bad), &eval_registry(), &named(&[]), &Toy(&prepare(&load(&host_prog()), &eval_registry()).unwrap()), EvalOptions::default()),
        Err(EvaluateError::Invalid(Invalid::Check(_)))
    ));
}

#[test]
fn empty_program() {
    let prep = prepare(&empty(), &eval_registry()).unwrap();
    let case = prep.case(&named(&[])).unwrap();
    let r = prep.eval(&case, &Toy(&prep), EvalOptions::default()).unwrap();
    assert_eq!(r, CaseResult { entries: vec![], failures: vec![] });
}
