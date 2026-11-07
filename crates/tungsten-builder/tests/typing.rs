//! The builder's typing agrees with core's (spec §10): for every binary operator, unary operator
//! and cast over the scalar types, `vec3` and `pos3`, a one-expression `.tw` program goes through
//! `check::prepare`, and core's verdict and result type are compared with the builder's.
//!
//! Exhaustive, so it fails if core's rules change.

mod common;

use tungsten_builder::prelude::*;
use tungsten_builder::{BuildErrorKind, Expr};
use tungsten_check::{CheckErrorKind, Invalid};

const TYPES: [(VariableType, &str); 8] = [
    (Bool, "bool"),
    (I8, "i8"),
    (I32, "i32"),
    (I64, "i64"),
    (F32, "f32"),
    (F64, "f64"),
    (Vec3, "vec3"),
    (Pos3, "pos3"),
];

fn spelling(t: &VariableType) -> &'static str {
    TYPES.iter().find(|(x, _)| x == t).unwrap().1
}

/// What core says about `expr` with parameters of the given types: `Ok(())` if it accepts the
/// expression with return type `ret`, or `Err(is_op_error)`.
fn core_verdict(params: &[&str], expr: &str, ret: &str) -> Result<(), bool> {
    let params: Vec<String> = params
        .iter()
        .enumerate()
        .map(|(i, t)| format!("p{i}: {t}"))
        .collect();
    let src = format!(
        "program {{\n    fn h({}) -> {ret} {{\n        return {expr};\n    }}\n}}\n",
        params.join(", ")
    );
    let p = common::parse(&src);
    match tungsten_check::prepare(&p, &common::empty_registry()) {
        Ok(_) => Ok(()),
        Err(Invalid::Check(errs)) => Err(errs.0.iter().any(|e| {
            matches!(
                e.kind,
                CheckErrorKind::BadBinary { .. }
                    | CheckErrorKind::BadUnary { .. }
                    | CheckErrorKind::BadCast { .. }
            )
        })),
        Err(other) => panic!("{other}\n{src}"),
    }
}

/// The builder's verdict on `make(params)`: its type, or the kind of the single error.
fn builder_verdict(
    params: &[VariableType],
    make: impl FnOnce(Vec<Local>) -> Expr,
) -> Result<&'static VariableType, BuildErrorKind> {
    let mut f = FnBuilder::new("h", F64);
    let locals = params
        .iter()
        .enumerate()
        .map(|(i, t)| f.param(&format!("p{i}"), t.clone()))
        .collect();
    let e = make(locals);
    match e.ty() {
        Some(t) if !e.has_problems() => Ok(t),
        _ => {
            f.ret(e);
            let mut pb = ProgramBuilder::new();
            pb.finish_helper(f);
            let Err(errs) = pb.build() else { panic!("expected an error") };
            // The return type is unknown or wrong; the operation's own error comes first.
            Err(errs.0[0].kind.clone())
        }
    }
}

fn agree(
    what: &str,
    params: &[VariableType],
    expr_src: &str,
    make: impl FnOnce(Vec<Local>) -> Expr,
) {
    let spellings: Vec<&str> = params.iter().map(spelling).collect();
    match builder_verdict(params, make) {
        Ok(t) => {
            let ret = format!("{t:?}");
            assert_eq!(
                core_verdict(&spellings, expr_src, &ret),
                Ok(()),
                "{what}: the builder gives `{ret}`, core disagrees"
            );
        }
        Err(kind) => {
            assert!(
                matches!(
                    kind,
                    BuildErrorKind::BadBinary { .. }
                        | BuildErrorKind::BadUnary { .. }
                        | BuildErrorKind::BadCast { .. }
                ),
                "{what}: unexpected builder error {kind:?}"
            );
            assert_eq!(
                core_verdict(&spellings, expr_src, spellings[0]),
                Err(true),
                "{what}: the builder rejects, core accepts"
            );
        }
    }
}

type BinFn = fn(Expr, Local) -> Expr;

#[test]
fn binary_operators_agree() {
    let ops: [(&str, BinFn); 18] = [
        ("+", |a, b| a + b),
        ("-", |a, b| a - b),
        ("*", |a, b| a * b),
        ("/", |a, b| a / b),
        ("==", |a, b| a.eq_expr(b)),
        ("!=", |a, b| a.ne(b)),
        ("<", |a, b| a.lt(b)),
        ("<=", |a, b| a.le(b)),
        (">", |a, b| a.gt(b)),
        (">=", |a, b| a.ge(b)),
        ("&&", |a, b| a.and(b)),
        ("||", |a, b| a.or(b)),
        ("&", |a, b| a.bitand(b)),
        ("|", |a, b| a.bitor(b)),
        ("^", |a, b| a.bitxor(b)),
        ("<<", |a, b| a.shl(b)),
        (">>", |a, b| a.shr(b)),
        (">>>", |a, b| a.ushr(b)),
    ];
    let mut accepted = 0;
    for (sym, op) in ops {
        for (l, _) in &TYPES {
            for (r, _) in &TYPES {
                let what = format!("{l:?} {sym} {r:?}");
                let src = format!("(p0 {sym} p1)");
                agree(&what, &[l.clone(), r.clone()], &src, |v| {
                    let e = op(v[0].into(), v[1]);
                    if !e.has_problems() {
                        accepted += 1;
                    }
                    e
                });
            }
        }
    }
    // Guards against a vacuous run. §4.2 allows 91 of the 1152 combinations: arithmetic 4 × 9,
    // equality 2 × 6, ordering 4 × 5, logic 2, bitwise 3 × 4, shifts 3 × 3.
    assert_eq!(accepted, 91);
}

#[test]
fn unary_operators_agree() {
    for (t, _) in &TYPES {
        agree(&format!("-{t:?}"), std::slice::from_ref(t), "-(p0)", |v| -v[0]);
        agree(&format!("!{t:?}"), std::slice::from_ref(t), "!(p0)", |v| !v[0]);
    }
}

#[test]
fn casts_agree() {
    for (from, _) in &TYPES {
        for (to, to_s) in &TYPES {
            let to2 = to.clone();
            agree(
                &format!("{from:?} as {to:?}"),
                std::slice::from_ref(from),
                &format!("(p0 as {to_s})"),
                move |v| v[0].cast(to2),
            );
        }
    }
}
