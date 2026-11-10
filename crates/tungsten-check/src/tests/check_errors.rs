//! One reject test per `CheckErrorKind` (both hints included), with its rendered `Location`.
//! Text snippets where the text format can express the program, hand-built core programs for
//! the rest (wf failures, forbidden const inits).

use tungsten_core::check::{CheckErrorKind as K, CoreItem, Forbidden, IdTable, check};
use tungsten_core::*;
use tungsten_core::Function;

use super::*;
use crate::{Invalid, prepare};

fn cerrs_src(src: &str) -> Vec<String> {
    match prepare(&load(src), &registry()) {
        Err(Invalid::Check(e)) => e.0.iter().map(|e| e.to_string()).collect(),
        Err(Invalid::Lower(e)) => panic!("lowering failed:\n{e}"),
        Ok(_) => panic!("checker accepted:\n{src}"),
    }
}

fn cerrs(decls: &str, body: &str) -> Vec<String> {
    cerrs_src(&snippet(decls, body))
}

#[track_caller]
fn one(decls: &str, body: &str, want: &str) {
    assert_eq!(cerrs(decls, body), [want]);
}

/// A full program: the items go inside `program { }`.
fn prog(items: &str) -> String {
    format!("program {{ {items} }}")
}

// --- types ---------------------------------------------------------------------------------

#[test]
fn type_mismatch() {
    one("", "return 1;", "fn `top` › stmt 0 › value: expected `f64`, found `i32`");
    one("", "let x: i32 = p; return p;", "fn `top` › stmt 0 › value: expected `i32`, found `f64`");
    one("", "if p { return p; } return p;", "fn `top` › stmt 0 › cond: expected `bool`, found `f64`");
}

#[test]
fn bad_binary() {
    one("", "return (p + 1);", "fn `top` › stmt 0 › value: `+` can't be applied to `f64` and `i32`");
    one(
        "",
        "let b: bool = (true < false); return p;",
        "fn `top` › stmt 0 › value: `<` can't be applied to `bool` and `bool`",
    );
}

#[test]
fn errors_dont_cascade() {
    // The inner error is reported; the outer `+` and the return see `None` and stay quiet.
    one("", "return ((p + 1) + p);", "fn `top` › stmt 0 › value › left: `+` can't be applied to `f64` and `i32`");
}

#[test]
fn bad_unary() {
    one("", "return !p;", "fn `top` › stmt 0 › value: `!` can't be applied to `f64`");
}

#[test]
fn bad_cast() {
    one("", "return (true as f64);", "fn `top` › stmt 0 › value: can't cast `bool` to `f64`");
}

#[test]
fn arity() {
    one(
        "fn g(x: f64) -> f64 { return x; }",
        "return g(p, p);",
        "fn `top` › stmt 0 › value: expected 1 arguments, found 2",
    );
    one(
        "fn g(x: f64) -> f64 { return x; }",
        "return g(1);",
        "fn `top` › stmt 0 › value › arg 0: expected `f64`, found `i32`",
    );
}

#[test]
fn extern_signature() {
    one(
        "",
        "return ext sin::<(f32) -> f64>(p);",
        "fn `top` › stmt 0 › value: the annotation of `sin` doesn't match its registry signature `(f64) -> f64`",
    );
}

#[test]
fn field_annotation() {
    one(
        "",
        "return ambient(origin).x::<f32>;",
        "fn `top` › stmt 0 › value: field annotated as `f32`, but it is `f64`",
    );
}

#[test]
fn bad_field_base() {
    one(
        "",
        "return (ambient(origin).x::<i32; pos3> as f64);",
        "fn `top` › stmt 0 › value › operand: field `x` needs a `pos3` base, found `vec3`",
    );
}

#[test]
fn not_array() {
    one("", "return ambient(origin)[0];", "fn `top` › stmt 0 › value: indexing needs an array, found `vec3`");
}

#[test]
fn record_fields() {
    one(
        "",
        "let q: extern Pair = new extern Pair { a: 1.0 }; return q.a;",
        "fn `top` › stmt 0 › value: field `b` of `Pair` is missing",
    );
    one(
        "",
        "let q: extern Pair = new extern Pair { a: 1.0, b: 2, a: 3.0 }; return q.a;",
        "fn `top` › stmt 0 › value › field `a`: field `a` of `Pair` is given more than once",
    );
}

#[test]
fn empty_array() {
    one("", "let a: [f64; 0] = []; return p;", "fn `top` › stmt 0 › value: empty array literal");
}

#[test]
fn main_params() {
    let src = prog(
        "fn m(x: f64) -> f64 { return x; }
         unit mu { main m; }
         entry mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);",
    );
    assert_eq!(cerrs_src(&src), ["unit `m` › main: a unit's main can't have parameters (it has 1)"]);
}

// --- flow ----------------------------------------------------------------------------------

#[test]
fn unassigned_local() {
    one("", "let x: f64; return x;", "fn `top` › stmt 0 › value: `x` may be read before it is assigned");
    // Each local once.
    one("", "let x: f64; return (x + x);", "fn `top` › stmt 0 › value › left: `x` may be read before it is assigned");
    // One branch isn't enough.
    one(
        "",
        "let x: f64; if (p > 0.0) { x = 1.0; } return x;",
        "fn `top` › stmt 1 › value: `x` may be read before it is assigned",
    );
}

#[test]
fn unassigned_local_hint() {
    one(
        "",
        "let x: f64; repeat 2 { x = 1.0; } return x;",
        "fn `top` › stmt 1 › value: `x` may be read before it is assigned; it is only assigned inside a loop, \
         and loops don't count for definite assignment: assign it before the loop",
    );
}

#[test]
fn definitely_assigned_ok() {
    let r = registry();
    for body in [
        "let x: f64; if (p > 0.0) { x = 1.0; } else { x = 2.0; } return x;",
        "let x: f64; if (p > 0.0) { return p; } else { x = 2.0; } return x;",
        "if (p > 0.0) { return p; } else { return 1.0; }",
        "return p; let y: f64 = 1.0;",
        "while true { break; } return p;",
    ] {
        prepare(&load(&snippet("", body)), &r).unwrap_or_else(|e| panic!("{body}\n{e}"));
    }
}

#[test]
fn missing_return() {
    one("", "let x: f64 = p;", "fn `top`: the end of the body is reachable without a `return`");
}

#[test]
fn missing_return_hint() {
    one(
        "",
        "while true { return p; }",
        "fn `top`: the end of the body is reachable without a `return`; the body ends in a loop that \
         returns, but loops don't count: add a `return` after the loop",
    );
}

#[test]
fn break_outside_loop() {
    one("", "if true { break; } return p;", "fn `top` › stmt 0 › then 0: `break` outside a loop");
}

// --- scoping -------------------------------------------------------------------------------

#[test]
fn helper_not_listed() {
    one(
        "",
        "return (leaf() + leaf());",
        "fn `top` › stmt 0 › value › left: `leaf` is called but isn't in the helpers of unit `top_main`",
    );
}

#[test]
fn host_not_in_fn() {
    one("host h2: f64;", "return host(h2);", "fn `top` › stmt 0 › value: host `h2` is read but isn't in this fn's `@hosts`");
}

#[test]
fn host_not_in_unit() {
    one(
        "host h2: f64; fn g() -> f64 @hosts(h2) { return host(h2); }",
        "return g();",
        "fn `g` › hosts 0: host `h2` isn't in the hosts of unit `top_main`",
    );
}

#[test]
fn table_not_listed() {
    one(
        "auto t2: handle PerlinNoiseSampler @name(\"m\") = make_perm(param(seed), 2L);",
        "return ext sample(auto(t2), p);",
        "fn `top` › stmt 0 › value › arg 0: table `m` isn't in the tables of unit `top_main`",
    );
}

#[test]
fn const_not_listed() {
    one("const k = 1.0;", "return const(k);", "fn `top` › stmt 0 › value: const `k` isn't in the consts of unit `top_main`");
    // Transitive: `k2` is listed and read, but its init reads `k`, which isn't listed.
    let src = prog(
        "const k = 1.0;
         const k2 = (const(k) * 2.0);
         fn m() -> f64 { return const(k2); }
         unit mu { main m; consts [k2]; }
         entry mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);",
    );
    assert_eq!(cerrs_src(&src), ["unit `m` › consts: const `k` isn't in the consts of unit `m`"]);
}

#[test]
fn input_not_listed() {
    let src = prog(
        "fn leaf() -> f64 { return 1.0; }
         fn a_main() -> f64 { return density(d); }
         fn b_main() -> f64 { return (density(d) + density(d)); }
         input d = leaf_u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
         unit leaf_u { main leaf; }
         unit a { main a_main; inputs [d]; }
         unit b { main b_main; }
         entry b @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);",
    );
    // Once per `(unit, input)`, at the first read.
    assert_eq!(
        cerrs_src(&src),
        ["fn `b_main` › stmt 0 › value › left: input `d` isn't in the inputs of unit `b_main`"]
    );
}

/// A helper shared by two units may read an input, but only if each of them lists it.
#[test]
fn input_read_by_shared_helper() {
    let src = prog(
        "fn leaf() -> f64 { return 1.0; }
         fn g() -> f64 { return density(d); }
         fn a_main() -> f64 { return g(); }
         fn b_main() -> f64 { return g(); }
         input d = leaf_u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
         unit leaf_u { main leaf; }
         unit a { main a_main; helpers [g]; inputs [d]; }
         unit b { main b_main; helpers [g]; }
         entry b @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);",
    );
    assert_eq!(
        cerrs_src(&src),
        ["fn `g` › stmt 0 › value: input `d` isn't in the inputs of unit `b_main`"]
    );
}

/// The same shape with `b` listing the input too: accepted.
#[test]
fn input_listed_by_two_units() {
    let src = prog(
        "fn leaf() -> f64 { return 1.0; }
         fn g() -> f64 { return density(d); }
         fn a_main() -> f64 { return g(); }
         fn b_main() -> f64 { return g(); }
         input d = leaf_u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
         unit leaf_u { main leaf; }
         unit a { main a_main; helpers [g]; inputs [d]; }
         unit b { main b_main; helpers [g]; inputs [d]; }
         entry a @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
         entry b @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);",
    );
    prepare(&load(&src), &registry()).unwrap();
}

// --- grids and cycles ----------------------------------------------------------------------

fn grid(dims: &str) -> String {
    prog(&format!(
        "fn m() -> f64 {{ return 1.0; }}
         unit mu {{ main m; }}
         entry mu @dims({dims}) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);"
    ))
}

#[test]
fn dims_too_small() {
    assert_eq!(cerrs_src(&grid("0, 1, 1")), ["entry 0 › dims: every dimension must be at least 1"]);
    assert_eq!(cerrs_src(&grid("1, -3, 1")), ["entry 0 › dims: every dimension must be at least 1"]);
}

#[test]
fn grid_too_large() {
    assert_eq!(cerrs_src(&grid("65536, 65536, 1")), ["entry 0 › dims: the grid has more than i32::MAX cells"]);
    assert_eq!(cerrs_src(&grid("2147483647, 2147483647, 2147483647")), [
        "entry 0 › dims: the grid has more than i32::MAX cells"
    ]);
    // Exactly i32::MAX cells is fine.
    prepare(&load(&grid("2147483647, 1, 1")), &registry()).unwrap();
}

#[test]
fn cycles() {
    // A const reading itself (the parser rejects that, so it's built on the lowered program):
    // one cycle, and no cascading type errors from the const's `None` type.
    let mut l = ok(&load(&snippet("", "return p;")), &registry());
    let c = ConstId(0);
    l.program.consts[0].init = Expr::Binary {
        op: BinOp::Add,
        left: Box::new(Expr::Const(c)),
        right: Box::new(Expr::Lit(Lit::F64(1.0f64.to_bits()))),
    };
    let e = crate::check_lowered(&l).unwrap_err();
    assert_eq!(e.to_string(), "program: dependency cycle through const `c`");
    // A unit reading its own output through an input.
    let src = prog(
        "fn m() -> f64 { return density(d); }
         input d = mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
         unit mu { main m; inputs [d]; }
         entry mu @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);",
    );
    assert_eq!(cerrs_src(&src), ["program: dependency cycle through fn `m`, input `d`, unit `m`"]);
    // Recursion.
    one(
        "fn g(x: f64) -> f64 { return g(x); }",
        "return g(p);",
        "program: dependency cycle through fn `g`",
    );
}

#[test]
fn walk_order() {
    // consts, then functions, then units, then entries, then cycles.
    let src = prog(
        "const k = (1.0 + 1);
         fn m(x: f64) -> f64 { return 1; }
         fn r() -> f64 { return r(); }
         unit mu { main m; helpers [r]; }
         entry mu @dims(0, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);",
    );
    assert_eq!(
        cerrs_src(&src),
        [
            "const `k` › value: `+` can't be applied to `f64` and `i32`",
            "fn `m` › stmt 0 › value: expected `f64`, found `i32`",
            "unit `m` › main: a unit's main can't have parameters (it has 1)",
            "entry 0 › dims: every dimension must be at least 1",
            "program: dependency cycle through fn `r`",
        ]
    );
}

// --- hand-built core programs --------------------------------------------------------------

fn core_prog(functions: Vec<Function>, consts: Vec<ConstDecl>) -> Program {
    Program {
        units: vec![],
        functions,
        consts,
        hosts: vec![HostDecl { ty: Type::F64 }],
        tables: vec![TableDecl { ty: HandleTypeId(0) }],
        inputs: vec![],
        entries: vec![],
        externs: vec![ExternDecl { params: vec![], ret: Type::F64, domain_error: false }],
        records: vec![],
    }
}

fn kinds(p: &Program) -> Vec<K> {
    check(p).unwrap_err().errors.into_iter().map(|e| e.kind).collect()
}

#[test]
fn const_forbidden() {
    let main = Function { params: 0, locals: vec![], ret: Type::F64, hosts: vec![], body: vec![Stmt::Return(Expr::Lit(Lit::F64(0)))] };
    let cases = [
        (Expr::Call { func: FnId(0), args: vec![] }, Forbidden::Call),
        (Expr::Ambient(Ambient::Origin), Forbidden::Ambient),
        (Expr::Host(HostId(0)), Forbidden::Host),
        (Expr::Table(TableId(0)), Forbidden::Table),
        (Expr::ExternCall { ext: ExternId(0), params: vec![], ret: Type::F64, args: vec![] }, Forbidden::ExternCall),
    ];
    for (init, what) in cases {
        let p = core_prog(vec![main.clone()], vec![ConstDecl { init }]);
        assert_eq!(kinds(&p), [K::ConstForbidden { what }]);
    }
    // Density needs an input, which needs units; build it on a lowered snippet.
    let l = ok(&load(&snippet("", "return p;")), &registry());
    let mut p = l.program.clone();
    p.consts[0].init = Expr::Density { input: InputId(0), index: None };
    let errs = check(&p).unwrap_err().errors;
    assert_eq!(errs[0].kind, K::ConstForbidden { what: Forbidden::Density });
    assert_eq!(errs[0].at.item, CoreItem::Const(ConstId(0)));
}

#[test]
fn wf_errors_only() {
    // A wf failure stops the checker: the type error in the same fn isn't reported.
    let f = Function {
        params: 2,
        locals: vec![Type::Record(RecordId(9))],
        ret: Type::F64,
        hosts: vec![HostId(5)],
        body: vec![
            Stmt::Assign { target: LocalId(4), value: Expr::Lit(Lit::Bool(true)) },
            Stmt::Return(Expr::Field {
                base: Box::new(Expr::Lit(Lit::I32(0))),
                sel: FieldSel::Record(RecordId(0), 0),
                ty: Type::F64,
            }),
        ],
    };
    let mut p = core_prog(vec![f], vec![ConstDecl { init: Expr::Local(LocalId(0)) }]);
    p.inputs.push(InputDecl {
        source: UnitId(0),
        dims: Dims { x: 1, y: 1, z: 1 },
        origin_scale: F64x3 { x: 0, y: 0, z: 0 },
        position_scale: F64x3 { x: 0, y: 0, z: 0 },
    });
    assert_eq!(
        kinds(&p),
        [
            K::WfParams { params: 2, locals: 1 },
            K::WfBadId { table: IdTable::Record, id: 9 },
            K::WfBadId { table: IdTable::Host, id: 5 },
            K::WfBadId { table: IdTable::Local, id: 4 },
            K::WfBadId { table: IdTable::Record, id: 0 },
            K::WfBadId { table: IdTable::Local, id: 0 },
            K::WfBadId { table: IdTable::Unit, id: 0 },
        ]
    );
    // A record field that doesn't exist.
    let mut q = core_prog(vec![], vec![]);
    q.records.push(RecordDecl { fields: vec![Type::F64] });
    q.consts.push(ConstDecl {
        init: Expr::MakeRecord { record: RecordId(0), fields: vec![FieldInit { field: 3, value: Expr::Lit(Lit::F64(0)) }] },
    });
    assert_eq!(kinds(&q), [K::WfField { record: RecordId(0), field: 3 }]);
}

#[test]
fn wf_rendering() {
    let l = ok(&load(&snippet("", "return p;")), &registry());
    let mut lowered = l.clone();
    lowered.program.functions[0].body.push(Stmt::Return(Expr::Local(LocalId(7))));
    let e = crate::check_lowered(&lowered).unwrap_err();
    assert_eq!(e.to_string(), "fn `leaf` › stmt 1 › value: internal: local id 7 is out of range");
}

#[test]
fn empty_program_is_ok() {
    check(&core_prog(vec![], vec![])).unwrap();
    let p = Program {
        units: vec![],
        functions: vec![],
        consts: vec![],
        hosts: vec![],
        tables: vec![],
        inputs: vec![],
        entries: vec![],
        externs: vec![],
        records: vec![],
    };
    check(&p).unwrap();
}
