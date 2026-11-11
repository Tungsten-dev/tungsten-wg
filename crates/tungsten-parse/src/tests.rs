//! L0 tests: the pieces of the spec one by one. The properties P1–P7 live in `tests/`.

use tungsten_ir::*;

use crate::*;

struct TestEnv;

impl Env for TestEnv {
    fn extern_fn(&self, name: &str) -> Option<ExternSig> {
        (name == "sin").then(|| ExternSig {
            parameter_types: vec![VariableType::F64],
            return_type: VariableType::F64,
        })
    }
    fn extern_type(&self, name: &str) -> Option<Vec<(&'static str, VariableType)>> {
        (name == "Rec").then(|| vec![("re", VariableType::F64), ("im", VariableType::I32)])
    }
    fn file_name(&self, file: FileId) -> Option<String> {
        (file == FileId(8)).then(|| "lib/syntax.tw".to_string())
    }
    fn library(&self, name: &str) -> Option<(FileId, &str)> {
        match name {
            "prelude" => Some((
                FileId(1),
                "program {
                    const half = 0.5;
                    const quarter = (const(half) * const(half));
                    fn halve(x: f64) -> f64 { return (x * const(half)); }
                    fn quarter_of(x: f64) -> f64 { return (halve(halve(x)) + const(quarter)); }
                    fn unrelated() -> f64 { return 0.0; }
                }",
            )),
            "cycle_a" => Some((
                FileId(2),
                "program { use cycle_b::{b}; fn a() -> f64 { return b(); } }",
            )),
            "cycle_b" => Some((
                FileId(3),
                "program { use cycle_a::{a}; fn b() -> f64 { return a(); } }",
            )),
            "chain" => Some((
                FileId(4),
                "program { use prelude::{halve}; fn third(x: f64) -> f64 { return halve(x); } }",
            )),
            "dirty" => Some((FileId(5), "program { host h: f64; }")),
            "badref" => Some((FileId(6), "program { fn f() -> f64 { return missing(); } }")),
            "hostref" => Some((FileId(7), "program { fn f() -> f64 { return host(h); } }")),
            "syntax" => Some((FileId(8), "program { fn f( }")),
            "hostattr" => Some((
                FileId(9),
                "program { fn f() -> f64 @hosts(h) { return 1.0; } }",
            )),
            "clash" => Some((FileId(0), "program { fn c() -> f64 { return 1.0; } }")),
            _ => None,
        }
    }
}

fn load_with(src: &str, env: &dyn Env) -> Result<TestFile, Diagnostic> {
    load(src, FileId(0), env)
}

fn ok(src: &str) -> TestFile {
    match load_with(src, &TestEnv) {
        Ok(t) => t,
        Err(d) => panic!("{d}\n--- source ---\n{src}"),
    }
}

fn fails(src: &str, needle: &str) -> Diagnostic {
    match load_with(src, &TestEnv) {
        Ok(_) => panic!("expected an error containing {needle:?}\n--- source ---\n{src}"),
        Err(d) => {
            assert!(
                d.message.contains(needle),
                "expected {needle:?} in {:?}\n--- source ---\n{src}",
                d.message
            );
            d
        }
    }
}

/// The constants of `program { <items> }` as `ConstExpr`s.
fn consts(items: &str) -> Vec<ConstExpr> {
    let t = ok(&format!("program {{ {items} }}"));
    t.program
        .constants
        .values()
        .map(|c| c.value.clone())
        .collect()
}

fn last_const(items: &str) -> ConstExpr {
    consts(items).pop().unwrap()
}

fn find_fn<'a>(t: &'a TestFile, name: &str) -> (Ref<Function>, &'a Function) {
    t.program
        .functions
        .iter()
        .find(|(_, f)| f.canonical_name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no fn {name}"))
}

fn int(v: i32) -> Box<ConstExpr> {
    Box::new(ConstExpr::Int(v))
}

// ---------------------------------------------------------------------------------------------
// §4.1 glued minus and literals
// ---------------------------------------------------------------------------------------------

#[test]
fn glued_minus_folds_into_the_literal_in_operand_position() {
    // refs differ between loads, so compare shapes
    let shape = |c: &ConstExpr| -> String {
        match c {
            ConstExpr::BinaryOp { op, left, right } => {
                format!("{op:?}({}, {})", short(left), short(right))
            }
            c => short(c),
        }
    };
    fn short(c: &ConstExpr) -> String {
        match c {
            ConstExpr::Constant(_) => "a".into(),
            ConstExpr::Int(v) => format!("Int({v})"),
            ConstExpr::UnaryOp { op, operand } => format!("{op:?}({})", short(operand)),
            c => format!("{c:?}"),
        }
    }
    assert_eq!(
        shape(&last_const("const a = 5; const c = a -1;")),
        "Subtract(a, Int(1))"
    );
    assert_eq!(
        shape(&last_const("const a = 5; const c = a - -1;")),
        "Subtract(a, Int(-1))"
    );
    assert_eq!(
        shape(&last_const("const a = 5; const c = a-1;")),
        "Subtract(a, Int(1))"
    );
    assert_eq!(shape(&last_const("const c = - 1;")), "Negate(Int(1))");
    assert_eq!(shape(&last_const("const c = -(1);")), "Negate(Int(1))");
    assert_eq!(shape(&last_const("const c = --1;")), "Negate(Int(-1))");
    assert_eq!(shape(&last_const("const c = -1;")), "Int(-1)");
    assert_eq!(
        last_const("const c = -inf;"),
        ConstExpr::Double(f64::NEG_INFINITY)
    );
    assert_eq!(
        last_const("const c = - inf;"),
        ConstExpr::UnaryOp {
            op: UnaryOperator::Negate,
            operand: Box::new(ConstExpr::Double(f64::INFINITY)),
        }
    );
}

#[test]
fn literal_kinds_and_ranges() {
    assert_eq!(last_const("const c = 1;"), ConstExpr::Int(1));
    assert_eq!(last_const("const c = 1b;"), ConstExpr::Byte(1));
    assert_eq!(last_const("const c = 1L;"), ConstExpr::Long(1));
    assert_eq!(last_const("const c = 1_000;"), ConstExpr::Int(1000));
    assert_eq!(last_const("const c = -128b;"), ConstExpr::Byte(-128));
    assert_eq!(
        last_const("const c = -2147483648;"),
        ConstExpr::Int(i32::MIN)
    );
    assert_eq!(last_const("const c = 0xFFFF_FFFF;"), ConstExpr::Int(-1));
    assert_eq!(last_const("const c = 0x80_i8;"), ConstExpr::Byte(-128));
    assert_eq!(
        last_const("const c = 0x8000000000000000_i64;"),
        ConstExpr::Long(i64::MIN)
    );
    assert_eq!(last_const("const c = 0b101;"), ConstExpr::Int(5));
    assert_eq!(last_const("const c = 0x1b;"), ConstExpr::Int(0x1b));
    assert_eq!(last_const("const c = 0b;"), ConstExpr::Byte(0));
    assert_eq!(last_const("const c = 1.5;"), ConstExpr::Double(1.5));
    assert_eq!(last_const("const c = 1e10;"), ConstExpr::Double(1e10));
    assert_eq!(last_const("const c = 0.5f;"), ConstExpr::Float(0.5));
    assert_eq!(
        last_const("const c = inf_f32;"),
        ConstExpr::Float(f32::INFINITY)
    );
    assert_eq!(last_const("const c = true;"), ConstExpr::Bool(true));
    match last_const("const c = nan;") {
        ConstExpr::Double(d) => assert_eq!(d.to_bits(), 0x7ff8_0000_0000_0000),
        c => panic!("{c:?}"),
    }
    match last_const("const c = nan(0xfff8000000000001);") {
        ConstExpr::Double(d) => assert_eq!(d.to_bits(), 0xfff8_0000_0000_0001),
        c => panic!("{c:?}"),
    }
    match last_const("const c = nan_f32(0xffc00001);") {
        ConstExpr::Float(d) => assert_eq!(d.to_bits(), 0xffc0_0001),
        c => panic!("{c:?}"),
    }

    let bad = |lit: &str, needle: &str| {
        fails(&format!("program {{ const c = {lit}; }}"), needle);
    };
    bad("2147483648", "out of range");
    bad("128b", "out of range");
    bad("9223372036854775808L", "out of range");
    bad("0x1_0000_0000", "more significant bits");
    bad("0x100_i8", "more significant bits");
    bad("-0x1", "");
    bad("0x1b_i64_", "");
    bad("0x1L", "suffix");
    bad("1e999", "inf");
    bad("1.", "both sides");
    bad("nan(0x7ff0000000000000)", "NaN");
    bad("nan(0x0)", "NaN");
    bad("nan(0x1ffffffffffffffff)", "at most 16");
    bad("nan_f32(0x7fc000000)", "at most 8");
    bad("nan_f32(0x7ff8000000000001)", "at most 8");
    bad("-nan", "-nan");
    bad("1e-400", "underflows");
    bad("1e-50f", "underflows");
    bad("1__0", "misplaced");
    bad("0X10", "suffix");
}

#[test]
fn attribute_positions_check_literals_against_the_expected_kind() {
    // in bodies a literal is checked against its own kind: 3000000000 is too big for i32
    fails("program { const c = 3000000000; }", "out of range for i32");
    // in attribute positions it is checked as the f64 it becomes
    let t = ok("program {
        fn f() -> f64 { return 1.0; }
        unit u { main f; }
        entry u @dims(1, 1, 1) @origin_scale(3000000000, 1, 1) @position_scale(1, 1, 1);
    }
    @case \"c\" origin(3000000000, 0, 0)");
    assert_eq!(t.program.entry_points[0].scaled_origin.0, 3e9);
    assert_eq!(t.cases[0].origin[0], 3e9);
}

#[test]
fn dims_of_any_sign_load() {
    // the checker judges dims (TESTING.md §5.3)
    let t = ok("program {
        fn f() -> f64 { return 1.0; }
        unit u { main f; }
        entry u @dims(-1, 0, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    }");
    assert_eq!(t.program.entry_points[0].dimensions, (-1, 0, 1));
    fails(
        "program {
            fn f() -> f64 { return 1.0; }
            unit u { main f; }
            entry u @dims(1, 1, 3000000000) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
        }",
        "out of range for i32",
    );
}

// ---------------------------------------------------------------------------------------------
// §6.2 precedence and the mixing restriction
// ---------------------------------------------------------------------------------------------

#[test]
fn mixing_restriction() {
    let bad = |e: &str| {
        fails(
            &format!("program {{ const a = 1; const b = 2; const c = {e}; }}"),
            "parentheses",
        );
    };
    let good = |e: &str| {
        ok(&format!(
            "program {{ const a = 1; const b = 2; const c = {e}; }}"
        ));
    };
    bad("a & 1 == 0");
    bad("a << 1 + 2");
    bad("a + b & a");
    bad("a & b | a");
    bad("a | b ^ a");
    bad("a >> 1 << 2");
    bad("a & b + 1");
    bad("a + (b) & 1");
    bad("a == b & a");
    bad("a && b | a");
    bad("a < b < a");
    bad("a == b == a");
    bad("a < b == a");
    good("a | b | a");
    good("a << 1 << 2");
    good("a >> 1 >> 2 >> 3");
    good("a >>> 1");
    good("a & b & a");
    good("a ^ b ^ a");
    good("(a & 1) == 0");
    good("a & (1 == 0)");
    good("a + (b & 1)");
    good("(a << 1) + 2");
    good("a == b && b != a || a < b");
    good("-a & b");
    good("a as i32 | b as i32");
    good("a + b * a - b / a");
    good("(a < b) == (b < a)");
    good("a >= b");
    good("a > b");
}

#[test]
fn precedence_matches_rust() {
    // 1 + 2 * 3 == 7 && 1 << 3 as expected
    let c = last_const("const c = 1 + 2 * 3;");
    assert_eq!(
        c,
        ConstExpr::BinaryOp {
            op: BinaryOperator::Add,
            left: int(1),
            right: Box::new(ConstExpr::BinaryOp {
                op: BinaryOperator::Multiply,
                left: int(2),
                right: int(3)
            }),
        }
    );
    // `as` binds tighter than `*`, unary tighter than `as`
    let c = last_const("const c = -2 as f64 * 3.0;");
    let ConstExpr::BinaryOp {
        op: BinaryOperator::Multiply,
        left,
        ..
    } = c
    else {
        panic!()
    };
    let ConstExpr::ExplicitCast { expr, .. } = *left else {
        panic!()
    };
    assert_eq!(*expr, ConstExpr::Int(-2));
    // left associative
    let c = last_const("const c = 8 - 4 - 2;");
    let ConstExpr::BinaryOp { left, right, .. } = c else {
        panic!()
    };
    assert!(matches!(*left, ConstExpr::BinaryOp { .. }));
    assert_eq!(*right, ConstExpr::Int(2));
    // `>>` and `>>>` are single operators, `>` `>` apart is not a shift
    assert!(matches!(
        last_const("const c = 8 >> 1;"),
        ConstExpr::BinaryOp {
            op: BinaryOperator::ShiftRight,
            ..
        }
    ));
    assert!(matches!(
        last_const("const c = 8 >>> 1;"),
        ConstExpr::BinaryOp {
            op: BinaryOperator::LogicalShiftRight,
            ..
        }
    ));
    fails("program { const c = 8 > > 1; }", "expected an expression");
}

#[test]
fn ampersand_in_operand_position() {
    let t = ok("program { fn f(x: f64) -> f64 { return &&x; } }");
    let (_, f) = find_fn(&t, "f");
    let Statement::Return(Expression::UnaryOp {
        op: UnaryOperator::Ref(false),
        operand,
    }) = &f.body[0]
    else {
        panic!()
    };
    assert!(matches!(
        **operand,
        Expression::UnaryOp {
            op: UnaryOperator::Ref(false),
            ..
        }
    ));
    let t = ok("program { fn f(x: f64) -> f64 { return &mut x; } }");
    let (_, f) = find_fn(&t, "f");
    assert!(matches!(
        &f.body[0],
        Statement::Return(Expression::UnaryOp {
            op: UnaryOperator::Ref(true),
            ..
        })
    ));
    // infix `&&` stays logical-and
    let t = ok("program { fn f(x: bool) -> bool { return x && x; } }");
    let (_, f) = find_fn(&t, "f");
    assert!(matches!(
        &f.body[0],
        Statement::Return(Expression::BinaryOp {
            op: BinaryOperator::And,
            ..
        })
    ));
}

// ---------------------------------------------------------------------------------------------
// §5 lenient forms
// ---------------------------------------------------------------------------------------------

#[test]
fn bare_names_are_vars_then_constants() {
    let t = ok("program {
        const k = 2.0;
        fn f(k2: f64) -> f64 { vars { k: f64 } k = 1.0; return (k + k2); }
        fn g() -> f64 { return k; }
    }");
    let (_, f) = find_fn(&t, "f");
    let Statement::Return(Expression::BinaryOp { left, .. }) = &f.body[1] else {
        panic!()
    };
    assert!(
        matches!(**left, Expression::Variable(_)),
        "a var shadows the constant"
    );
    let (_, g) = find_fn(&t, "g");
    assert!(matches!(
        &g.body[0],
        Statement::Return(Expression::Constant(_))
    ));
    fails(
        "program { fn g() -> f64 { return nothing; } }",
        "unknown name",
    );
}

#[test]
fn lets_declare_in_textual_order_and_split_into_decl_plus_assign() {
    let t = ok("program {
        fn f(a: f64) -> f64 {
            vars { v: f64 }
            let x: f64 = 1.0;
            if true { let y: i32; let z: i32 = 2; } else { let w: bool = false; }
            return x;
        }
    }");
    let (_, f) = find_fn(&t, "f");
    let names: Vec<_> = f
        .variables
        .values()
        .map(|v| match &v.name {
            Name::Named(n) => n.clone(),
            n => format!("{n:?}"),
        })
        .collect();
    assert_eq!(names, ["a", "v", "x", "y", "z", "w"]);
    // `let y: i32;` produced no statement
    assert_eq!(f.body.len(), 3);
    let Statement::If {
        then_branch,
        else_branch,
        ..
    } = &f.body[1]
    else {
        panic!()
    };
    assert_eq!((then_branch.len(), else_branch.len()), (1, 1));
}

#[test]
fn let_needs_a_type_and_untyped_params_need_vars() {
    fails(
        "program { fn f() -> f64 { let x = 1.0; return x; } }",
        "needs a type",
    );
    fails("program { fn f(a) -> f64 { return 1.0; } }", "no type");
    ok("program { fn f(a) -> f64 { vars { a: f64 } return a; } }");
    fails(
        "program { fn f(a: f64, a: f64) -> f64 { return 1.0; } }",
        "duplicate",
    );
    fails(
        "program { fn f(a: f64) -> f64 { let a: f64 = 1.0; return a; } }",
        "duplicate",
    );
    // a param declared after another param's declaration is fine; `parameters` follows the list
    let t = ok("program { fn f(b, a) -> f64 { vars { a: f64, b: f64 } return a; } }");
    let (_, f) = find_fn(&t, "f");
    let slots: Vec<_> = f.variables.iter().map(|(r, _)| r).collect();
    assert_eq!(f.parameters, vec![slots[1], slots[0]]);
}

#[test]
fn forward_references_and_slot_order_are_textual() {
    let t = ok("program {
        entry e = u @dims(1,1,1) @origin_scale(1,1,1) @position_scale(1,1,1);
        unit u { main late; consts [b, a]; }
        fn early() -> f64 { return late(); }
        fn late() -> f64 { return (const(a) + const(b)); }
        const b = 2.0;
        const a = (const(b) * 2.0);
    }");
    let names: Vec<_> = t
        .program
        .functions
        .values()
        .map(|f| f.canonical_name.clone().unwrap())
        .collect();
    assert_eq!(names, ["early", "late"]);
    let cn: Vec<_> = t
        .program
        .constants
        .values()
        .map(|c| c.name.clone())
        .collect();
    assert_eq!(cn, ["b", "a"]);
    let ComputeUnit::DensityFunction { constants, .. } =
        t.program.compute_units.values().next().unwrap();
    // lists are verbatim, in the order written
    assert_eq!(constants[0], t.program.constants.iter().next().unwrap().0);
}

#[test]
fn else_if_chains_and_empty_branches() {
    let t = ok("program { fn f(x: bool) -> f64 {
        if x { } else if x { return 1.0; } else if x { } else { return 2.0; }
        return 3.0;
    } }");
    let (_, f) = find_fn(&t, "f");
    let Statement::If {
        then_branch,
        else_branch,
        ..
    } = &f.body[0]
    else {
        panic!()
    };
    assert!(then_branch.is_empty());
    assert_eq!(else_branch.len(), 1);
    let Statement::If {
        else_branch: e2, ..
    } = &else_branch[0]
    else {
        panic!()
    };
    assert_eq!(e2.len(), 1);
    assert!(matches!(&e2[0], Statement::If { .. }));
}

#[test]
fn comments_whitespace_and_crlf() {
    let t = ok("// header\r\nprogram {\r\n  fn f() -> f64 { // c\r\n return 1.0 ;\r\n }\r\n}\r\n");
    assert_eq!(t.program.functions.iter().count(), 1);
    ok("program{fn f()->f64{return 1.0;}}");
}

// ---------------------------------------------------------------------------------------------
// §6.3 derivation
// ---------------------------------------------------------------------------------------------

#[test]
fn derived_fields_and_externs() {
    let t = ok("program { fn f(v: vec3, p: pos3, r: extern Rec) -> f64 {
        vars { a: f64, b: i32, c: f64, d: i32 }
        a = v.x; b = p.y; c = r.re; d = r.im;
        a = ext sin(a);
        return a;
    } }");
    let (_, f) = find_fn(&t, "f");
    let field = |i: usize| match &f.body[i] {
        Statement::Assign {
            value:
                Expression::Field {
                    type_of_field,
                    base_type,
                    ..
                },
            ..
        } => (type_of_field.clone(), base_type.clone()),
        s => panic!("{s:?}"),
    };
    let rec = VariableType::Extern("Rec");
    assert_eq!(field(0), (VariableType::F64, VariableType::Vec3));
    assert_eq!(field(1), (VariableType::I32, VariableType::Pos3));
    assert_eq!(field(2), (VariableType::F64, rec.clone()));
    assert_eq!(field(3), (VariableType::I32, rec));
    match &f.body[4] {
        Statement::Assign {
            value:
                Expression::ExternCall {
                    parameter_types,
                    return_type,
                    ..
                },
            ..
        } => {
            assert_eq!(parameter_types, &vec![VariableType::F64]);
            assert_eq!(return_type, &VariableType::F64);
        }
        s => panic!("{s:?}"),
    }
}

#[test]
fn derivation_through_expressions() {
    // type_of flows through arithmetic, calls, density reads, indexing
    let t = ok("program {
        fn g() -> vec3 { return vec3(1.0, 2.0, 3.0); }
        fn h(a: [vec3; 2]) -> f64 {
            vars { r: f64 }
            r = (g() + g()).x;
            r = a[0].y;
            r = (ambient(origin) + ambient(pos3)).z;
            return r;
        }
    }");
    let (_, h) = find_fn(&t, "h");
    assert_eq!(h.body.len(), 4);
}

#[test]
fn underivable_types_are_errors_only_when_needed() {
    // i32 + f64 has no type, so a field on it can't be derived …
    fails(
        "program { fn f() -> f64 { return (1 + 1.0).x; } }",
        "can't derive",
    );
    // … and neither can its base type, so the field type alone isn't enough …
    fails(
        "program { fn f() -> f64 { return (1 + 1.0).x::<f64>; } }",
        "can't derive the type of the base",
    );
    // … but writing both makes it fine, and an underivable operand elsewhere is no problem
    ok("program { fn f() -> f64 { return (1 + 1.0).x::<f64; vec3>; } }");
    ok("program { fn f() -> f64 { return (1 + 1.0); } }");
    fails(
        "program { fn f(a: f64) -> f64 { return a.x; } }",
        "can't derive",
    );
    fails(
        "program { fn f(a: vec3) -> f64 { return a.w; } }",
        "can't derive",
    );
    fails("program { fn f() -> f64 { return [].x; } }", "can't derive");
    fails(
        "program { fn f() -> f64 { return ext unknown(1.0); } }",
        "unknown extern",
    );
    ok("program { fn f() -> f64 { return ext unknown::<(f64) -> f64>(1.0); } }");
    // an explicit base type is kept, even when it disagrees with the derived one
    let t = ok("program { fn f(v: vec3) -> f64 { return v.x::<f64; pos3>; } }");
    let (_, f) = find_fn(&t, "f");
    assert!(matches!(
        &f.body[0],
        Statement::Return(Expression::Field {
            base_type: VariableType::Pos3,
            ..
        })
    ));
    // the old index form is gone
    fails(
        "program { fn f(v: vec3) -> f64 { return v.x::<f64, 2>; } }",
        "expected",
    );
}

#[test]
fn base_type_is_printed_only_when_needed() {
    let src = "program { fn f(v: vec3, a: [pos3; 2]) -> f64 {
        vars { r: f64, s: i32 }
        r = v.x;
        s = a[1].y;
        r = v.x::<f64; pos3>;
        r = (1 + 1.0).z::<f64; vec3>;
        return r;
    } }";
    let t = ok(src);
    let text = print(&t.program);
    assert!(text.contains("v.x::<f64>;"), "{text}");
    assert!(text.contains("a[1].y::<i32>;"), "{text}");
    assert!(text.contains("v.x::<f64; pos3>;"), "{text}");
    assert!(text.contains(".z::<f64; vec3>;"), "{text}");
    let again = load(&text, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}\n{text}"));
    assert!(isomorphic(&again.program, &t.program));
}

#[test]
fn no_env_knows_nothing() {
    let src = "program { fn f(a: extern Rec) -> f64 { return a.re; } }";
    assert!(load(src, FileId(0), &NoEnv).is_err());
    assert!(load(src, FileId(0), &TestEnv).is_ok());
}

// ---------------------------------------------------------------------------------------------
// §5.2 labels
// ---------------------------------------------------------------------------------------------

fn empty_program() -> OptimizedProgram {
    OptimizedProgram {
        compute_units: Arena::new(),
        functions: Arena::new(),
        entry_points: vec![],
        refinements: vec![],
        constants: Arena::new(),
        host_inputs: Arena::new(),
        auto_params: Arena::new(),
        auto_consts: Arena::new(),
        density_inputs: Arena::new(),
    }
}

fn func(name: Option<&str>) -> Function {
    Function {
        canonical_name: name.map(String::from),
        host_inputs: vec![],
        body: vec![],
        variables: Arena::new(),
        parameters: vec![],
        return_type: VariableType::F64,
        source_hash: 0,
    }
}

#[test]
fn weird_and_duplicate_names_get_quoted_and_numbered() {
    let mut p = empty_program();
    let mut f = func(Some("dup"));
    let x1 = f.variables.insert(Var {
        name: Name::Named("x".into()),
        t: VariableType::F64,
    });
    f.variables.insert(Var {
        name: Name::Named("x".into()),
        t: VariableType::F64,
    });
    f.variables.insert(Var {
        name: Name::Prefixed("x".into()),
        t: VariableType::F64,
    });
    f.variables.insert(Var {
        name: Name::Anonymous,
        t: VariableType::F64,
    });
    f.variables.insert(Var {
        name: Name::Named("let".into()),
        t: VariableType::F64,
    });
    f.variables.insert(Var {
        name: Name::Named("a b".into()),
        t: VariableType::F64,
    });
    f.variables.insert(Var {
        name: Name::Named("solo".into()),
        t: VariableType::F64,
    });
    f.body = vec![Statement::Return(Expression::Variable(x1))];
    p.functions.insert(f);
    p.functions.insert(func(Some("dup")));
    p.functions.insert(func(None));
    p.functions.insert(func(Some("minecraft:overworld/x")));
    p.functions.insert(func(Some("fn")));
    p.functions.insert(func(Some("")));
    p.functions.insert(func(Some("fine")));
    p.constants.insert(Constant {
        value: ConstExpr::Int(1),
        name: "c".into(),
    });
    p.constants.insert(Constant {
        value: ConstExpr::Int(2),
        name: "c".into(),
    });
    p.constants.insert(Constant {
        value: ConstExpr::Int(3),
        name: "while".into(),
    });
    p.host_inputs.insert(HostInput::Static {
        name: Name::Named("h".into()),
        t: VariableType::I32,
    });
    p.host_inputs.insert(HostInput::Static {
        name: Name::Named("h".into()),
        t: VariableType::I32,
    });
    p.host_inputs.insert(HostInput::Static {
        name: Name::Prefixed("h".into()),
        t: VariableType::I32,
    });
    p.host_inputs.insert(HostInput::Static {
        name: Name::Anonymous,
        t: VariableType::I32,
    });
    p.host_inputs.insert(HostInput::Dynamic);

    let text = print(&p);
    for want in [
        "fn dup#0(",
        "fn dup#1(",
        "fn %2(",
        "fn `minecraft:overworld/x`(",
        "fn `fn`(",
        "fn ``(",
        "fn fine(",
        "const c#0 =",
        "const c#1 =",
        "const `while` =",
        "host h##0: i32;",
        "host h##1: i32;",
        "host h#2: i32;",
        "host %3: i32;",
        "host %4 dynamic;",
        "x##0: f64,",
        "x##1: f64,",
        "x#2: f64,",
        "%3: f64,",
        "`let`: f64,",
        "`a b`: f64,",
        "solo: f64,",
        "return x##0;",
    ] {
        assert!(text.contains(want), "missing {want:?} in\n{text}");
    }
    // and it all reads back identically
    let t = load(&text, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}\n{text}"));
    assert!(isomorphic(&t.program, &compact(&p)));
    assert_eq!(print(&t.program), text);
}

#[test]
fn label_suffix_meanings() {
    let t = ok("program {
        host a##3: f64;
        host a##9: f64;
        host b#1: f64;
        host %7: f64;
        fn f#2(x#5: f64, %0: f64, `q r`: f64) -> f64 { vars { y##1: f64 } return x#5; }
        fn f#4() -> f64 { return 1.0; }
        fn %5() -> f64 { return 1.0; }
        const k#1 = 1;
        const k#2 = 2;
    }");
    let hosts: Vec<_> = t.program.host_inputs.values().cloned().collect();
    assert_eq!(
        hosts[0],
        HostInput::Static {
            name: Name::Named("a".into()),
            t: VariableType::F64
        }
    );
    assert_eq!(
        hosts[2],
        HostInput::Static {
            name: Name::Prefixed("b".into()),
            t: VariableType::F64
        }
    );
    assert_eq!(
        hosts[3],
        HostInput::Static {
            name: Name::Anonymous,
            t: VariableType::F64
        }
    );
    let names: Vec<_> = t
        .program
        .functions
        .values()
        .map(|f| f.canonical_name.clone())
        .collect();
    assert_eq!(names, [Some("f".into()), Some("f".into()), None]);
    let f = t.program.functions.values().next().unwrap();
    let vn: Vec<_> = f.variables.values().map(|v| v.name.clone()).collect();
    assert_eq!(
        vn,
        [
            Name::Prefixed("x".into()),
            Name::Anonymous,
            Name::Named("q r".into()),
            Name::Named("y".into())
        ]
    );
    let cn: Vec<_> = t
        .program
        .constants
        .values()
        .map(|c| c.name.clone())
        .collect();
    assert_eq!(cn, ["k", "k"]);
}

#[test]
fn labels_must_be_unique_per_namespace() {
    fails("program { host a: f64; host a: f64; }", "duplicate host");
    fails(
        "program { host a: f64; host a#1: f64; host a#1: f64; }",
        "duplicate host",
    );
    fails(
        "program { fn f() -> f64 { return 1.0; } fn f() -> f64 { return 1.0; } }",
        "duplicate function",
    );
    fails("program { const c = 1; const c = 2; }", "duplicate const");
    fails(
        "program { auto t: handle H = make(); auto t: handle H = make(); }",
        "duplicate autoconst",
    );
    fails(
        "program { fn f() -> f64 { return 1.0; } unit u { main f; } unit u { main f; } }",
        "duplicate unit",
    );
    fails(
        "program { fn f(a: f64) -> f64 { vars { a: f64 } return 1.0; } }",
        "duplicate variable",
    );
    // the same name in different namespaces is fine
    ok(
        "program { host a: f64; auto a: handle H = make(); const a = 1; fn a() -> f64 { return 1.0; } unit a { main a; } }",
    );
    // and `x` / `x#1` / `` `x` `` are different or equal as written
    ok("program { host x: f64; host x#1: f64; host x##1: f64; }");
    fails("program { host x: f64; host `x`: f64; }", "duplicate host");
    // suffix kinds that make no sense for a namespace
    fails(
        "program { fn f##1() -> f64 { return 1.0; } }",
        "function label",
    );
    fails("program { const c##1 = 1; }", "constant label");
    fails("program { const %1 = 1; }", "constant label");
}

// ---------------------------------------------------------------------------------------------
// §6.1 density reads, §5.3 units and inputs
// ---------------------------------------------------------------------------------------------

const GRID: &str = "@dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1)";

#[test]
fn density_reads_resolve_program_wide() {
    // a shared helper reads an input both of its units list, from any function
    let t = ok(&format!(
        "program {{
            fn leaf() -> f64 {{ return 1.0; }}
            fn shared() -> f64 {{ return (density(d) + density(d, 1)); }}
            fn root_a() -> f64 {{ return shared(); }}
            fn root_b() -> f64 {{ return shared(); }}
            fn orphan() -> f64 {{ return density(d); }}
            input d = leaf_u {GRID};
            unit leaf_u {{ main leaf; }}
            unit a {{ main root_a; helpers [shared]; inputs [d]; }}
            unit b {{ main root_b; helpers [shared]; inputs [d]; }}
        }}"
    ));
    let (_, root) = find_fn(&t, "orphan");
    assert!(matches!(
        &root.body[0],
        Statement::Return(Expression::DensityVariable(_, None))
    ));
    assert_eq!(t.program.density_inputs.iter().count(), 1);
    // a read of an input the unit doesn't list is the checker's business, not an error here
    ok(&format!(
        "program {{
            fn leaf() -> f64 {{ return 1.0; }}
            fn root() -> f64 {{ return density(d); }}
            input d = leaf_u {GRID};
            unit leaf_u {{ main leaf; }}
            unit a {{ main root; }}
        }}"
    ));
    // inputs are declared before lists and reads resolve, so order doesn't matter
    ok(&format!(
        "program {{
            fn leaf() -> f64 {{ return 1.0; }}
            fn root() -> f64 {{ return density(d); }}
            unit a {{ main root; inputs [d]; }}
            unit leaf_u {{ main leaf; }}
            input d = leaf_u {GRID};
        }}"
    ));
    fails(
        &format!(
            "program {{ fn leaf() -> f64 {{ return 1.0; }} fn r() -> f64 {{ return density(zz); }}
              input d = leaf_u {GRID}; unit leaf_u {{ main leaf; }} unit a {{ main r; inputs [d]; }} }}"
        ),
        "unknown input `zz`",
    );
    fails(
        &format!(
            "program {{ fn leaf() -> f64 {{ return 1.0; }} fn r() -> f64 {{ return 1.0; }}
              input d = leaf_u {GRID}; unit leaf_u {{ main leaf; }} unit a {{ main r; inputs [e]; }} }}"
        ),
        "unknown input `e`",
    );
    // the old forms point to the new ones
    fails(
        &format!(
            "program {{ fn leaf() -> f64 {{ return 1.0; }} fn r() -> f64 {{ return density(a.d); }}
              input d = leaf_u {GRID}; unit leaf_u {{ main leaf; }} unit a {{ main r; inputs [d]; }} }}"
        ),
        "write `density(d)`",
    );
    fails(
        &format!(
            "program {{ fn leaf() -> f64 {{ return 1.0; }}
              unit leaf_u {{ main leaf; }} unit a {{ main leaf; input d = leaf_u {GRID}; }} }}"
        ),
        "inputs are program items",
    );
    fails(
        &format!(
            "program {{ fn leaf() -> f64 {{ return 1.0; }} fn r() -> f64 {{ return 1.0; }}
              input d = leaf_u {GRID}; unit leaf_u {{ main leaf; }}
              unit a {{ main r; inputs [d]; inputs [d]; }} }}"
        ),
        "duplicate `inputs` list",
    );
}

#[test]
fn inputs_lists_keep_order_and_duplicates() {
    let t = ok(&format!(
        "program {{
            fn leaf() -> f64 {{ return 1.0; }}
            fn root() -> f64 {{ return density(b); }}
            input a = leaf_u {GRID};
            input b = leaf_u {GRID};
            input unlisted = leaf_u {GRID};
            unit leaf_u {{ main leaf; }}
            unit u {{ main root; inputs [b, a, b]; }}
        }}"
    ));
    let refs: Vec<_> = t.program.density_inputs.iter().map(|(r, _)| r).collect();
    let ComputeUnit::DensityFunction { density_inputs, .. } =
        t.program.compute_units.values().nth(1).unwrap();
    assert_eq!(density_inputs, &vec![refs[1], refs[0], refs[1]]);
    let text = print(&t.program);
    assert!(text.contains("        inputs [b, a, b];\n    }"), "{text}");
    assert!(text.contains("    input unlisted = leaf @dims"), "{text}");
    // inputs print after functions and before units
    let input_at = text.find("    input a = ").unwrap();
    assert!(text.find("    fn root").unwrap() < input_at, "{text}");
    assert!(input_at < text.find("    unit leaf").unwrap(), "{text}");
    let back = load(&text, FileId(0), &NoEnv).unwrap();
    assert!(isomorphic(&back.program, &t.program));
}

#[test]
fn density_read_type_is_the_target_return_type() {
    let t = ok(&format!(
        "program {{
            fn leaf() -> vec3 {{ return vec3(1.0, 2.0, 3.0); }}
            fn root() -> f64 {{ return density(d).x; }}
            input d = leaf_u {GRID};
            unit leaf_u {{ main leaf; }}
            unit a {{ main root; inputs [d]; }}
        }}"
    ));
    let (_, root) = find_fn(&t, "root");
    assert!(matches!(
        &root.body[0],
        Statement::Return(Expression::Field {
            type_of_field: VariableType::F64,
            ..
        })
    ));
}

#[test]
fn input_names() {
    let t = ok(&format!(
        "program {{
            fn leaf() -> f64 {{ return 1.0; }}
            fn root(p: f64) -> f64 {{ vars {{ d: f64 }} let l: f64 = 1.0; return density(d); }}
            input d = leaf_u {GRID};
            input %0 = leaf_u {GRID};
            input e#4 = leaf_u {GRID};
            input f##2 = leaf_u {GRID};
            input f##3 = leaf_u {GRID};
            unit leaf_u {{ main leaf; }}
            unit a {{ main root; inputs [d, %0, e#4, f##2, f##3]; }}
        }}"
    ));
    // no placeholder vars, and a var may share an input's label
    let (_, root) = find_fn(&t, "root");
    let names: Vec<_> = root.variables.values().map(|v| v.name.clone()).collect();
    assert_eq!(
        names,
        [
            Name::Named("p".into()),
            Name::Named("d".into()),
            Name::Named("l".into())
        ]
    );
    // input labels follow the host rule
    let names: Vec<_> = t.program.density_inputs.values().map(|d| d.name.clone()).collect();
    assert_eq!(
        names,
        [
            Name::Named("d".into()),
            Name::Anonymous,
            Name::Prefixed("e".into()),
            Name::Named("f".into()),
            Name::Named("f".into())
        ]
    );
    // and print back as such, with `##N` only for the duplicated `Named`
    let text = print(&t.program);
    for l in ["input d = ", "input %1 = ", "input e#2 = ", "input f##3 = ", "input f##4 = "] {
        assert!(text.contains(l), "{l}\n{text}");
    }
    assert!(!text.contains("@var"), "{text}");

    fails(
        &format!(
            "program {{
                fn leaf() -> f64 {{ return 1.0; }}
                input d = leaf_u {GRID};
                input d = leaf_u {GRID};
                unit leaf_u {{ main leaf; }}
            }}"
        ),
        "duplicate input label `d`",
    );
    // `@var` is gone
    fails(
        &format!(
            "program {{
                fn leaf() -> f64 {{ return 1.0; }}
                input d = leaf_u {GRID} @var(leaf.v);
                unit leaf_u {{ main leaf; }}
            }}"
        ),
        "unknown attribute `@var`",
    );
    // `density` is not a type
    fails(
        "program { fn f(x: density) -> f64 { return 1.0; } }",
        "expected a type, found `density`",
    );
    fails(
        "program { fn f() -> f64 { vars { v: density } return 1.0; } }",
        "expected a type, found `density`",
    );
}

#[test]
fn unit_requirements() {
    fails(
        "program { fn f() -> f64 { return 1.0; } unit u { } }",
        "needs `main",
    );
    fails(
        "program { fn f() -> f64 { return 1.0; } unit u { main f; main f; } }",
        "duplicate `main`",
    );
    fails(
        "program { fn f() -> f64 { return 1.0; } unit u { main f; helpers []; helpers []; } }",
        "duplicate",
    );
    fails("program { unit u { main nothing; } }", "unknown function");
    fails(
        "program { fn f() -> f64 { return 1.0; } unit u { main f; hosts [nope]; } }",
        "unknown host",
    );
    fails(
        "program { fn f() -> f64 { return 1.0; } unit u { main f; } entry e = nope @dims(1,1,1) @origin_scale(1,1,1) @position_scale(1,1,1); }",
        "unknown unit",
    );
    let u = "fn f() -> f64 { return 1.0; } unit u { main f; }";
    fails(
        &format!("program {{ {u} entry u @origin_scale(1,1,1) @position_scale(1,1,1); }}"),
        "missing `@dims",
    );
    fails(
        &format!("program {{ {u} entry u @dims(1,1,1) @position_scale(1,1,1); }}"),
        "missing `@origin_scale",
    );
    fails(
        &format!(
            "program {{ {u} entry u @dims(1,1,1) @dims(1,1,1) @origin_scale(1,1,1) @position_scale(1,1,1); }}"
        ),
        "duplicate `@dims`",
    );
    fails(
        &format!(
            "program {{ {u} entry u @dims(1,1,1.5) @origin_scale(1,1,1) @position_scale(1,1,1); }}"
        ),
        "decimal integer",
    );
    fails(
        &format!(
            "program {{ {u} entry u @dims(1,1,1) @origin_scale(1b,1,1) @position_scale(1,1,1); }}"
        ),
        "expected a f64 literal",
    );
    fails(
        &format!("program {{ {u} entry u {GRID}; entry u {GRID}; }}"),
        "duplicate entry",
    );
    ok(&format!(
        "program {{ {u} entry a = u {GRID}; entry b = u {GRID}; }}"
    ));
    // attributes in any order
    ok(&format!(
        "program {{ {u} entry u @position_scale(1,1,1) @dims(1,1,1) @origin_scale(1,1,1); }}"
    ));
    // ints are accepted for floats in attribute positions, negative too
    let t = ok(&format!(
        "program {{ {u} entry u @dims(1,1,1) @origin_scale(-1,0,2) @position_scale(0.5,1,1); }}"
    ));
    assert_eq!(t.program.entry_points[0].scaled_origin, (-1.0, 0.0, 2.0));
}

#[test]
fn constants_are_restricted() {
    fails(
        "program { host h: f64; const c = host(h); }",
        "not allowed in a constant",
    );
    fails(
        "program { fn f() -> f64 { return 1.0; } const c = f(); }",
        "not allowed in a constant",
    );
    fails(
        "program { const c = ambient(origin); }",
        "not allowed in a constant",
    );
    fails(
        "program { const c = ext sin(1.0); }",
        "not allowed in a constant",
    );
    fails("program { const a = b; const b = a; }", "cycle");
    fails("program { const a = a; }", "cycle");
    fails("program { const a = nope; }", "unknown name");
    // forward references and bare names work
    let t = ok("program { const a = (b + 1); const b = 2; }");
    let a = t.program.constants.values().next().unwrap();
    assert!(
        matches!(&a.value, ConstExpr::BinaryOp { left, .. } if matches!(**left, ConstExpr::Constant(_)))
    );
    // vec3 construction, array literals and indexing
    ok(
        "program { const v = vec3(1.0, 2.0, 3.0); const x = v.x; const arr = [1, 2]; const e = arr[0]; }",
    );
}

#[test]
fn hosts_autos_and_extras() {
    let t = ok(r#"program {
        host a: [vec3; 2];
        host b dynamic;
        param seed: i64;
        param k: f64;
        auto n: handle Noise @name("minecraft:x") = make_noise(param(seed), 3L, -4L, 0x10_i64);
        auto m: handle Noise = make_noise(param(seed), 0L);
        auto z: handle Other = make_other();
        fn f() -> f64 @hash(0xdeadbeef) @hosts(a) { return 1.0; }
        unit u @hash(18446744073709551615) { main f; autos [n, z]; hosts [a, b]; }
    }"#);
    assert!(matches!(
        t.program.host_inputs.values().nth(1),
        Some(HostInput::Dynamic)
    ));
    let params: Vec<_> = t.program.auto_params.values().cloned().collect();
    assert_eq!(
        params,
        vec![
            AutoParam { name: "seed".into(), t: VariableType::I64 },
            AutoParam { name: "k".into(), t: VariableType::F64 },
        ]
    );
    let seed = t.program.auto_params.iter().next().unwrap().0;
    let autos: Vec<_> = t.program.auto_consts.values().cloned().collect();
    assert_eq!(
        autos[0],
        AutoConst {
            name: "minecraft:x".into(),
            t: VariableType::Handle("Noise"),
            initializer: "make_noise".into(),
            args: vec![
                AutoConstArg::Param(seed),
                AutoConstArg::I64(3),
                AutoConstArg::I64(-4),
                AutoConstArg::I64(16)
            ],
        }
    );
    assert_eq!(autos[1].name, "m", "the name defaults to the label");
    assert_eq!(autos[2].t, VariableType::Handle("Other"));
    assert!(autos[2].args.is_empty());
    let (_, f) = find_fn(&t, "f");
    assert_eq!(f.source_hash, 0xdeadbeef);
    assert_eq!(f.host_inputs.len(), 1);
    let ComputeUnit::DensityFunction {
        source_hash,
        auto_consts,
        host_inputs,
        ..
    } = t.program.compute_units.values().next().unwrap();
    assert_eq!(*source_hash, u64::MAX);
    assert_eq!((auto_consts.len(), host_inputs.len()), (2, 2));
}

#[test]
fn auto_i64_argument_needs_the_suffix() {
    fails("program { auto t: handle H = make(12); }", "write `12L`");
}

#[test]
fn auto_argument_kinds() {
    let t = ok(r#"program {
        auto a: i64 = make(-0.0, 1.5, true, false, "q\"\\ \u{e9} \u{1d11e}", nan, -inf, 7L);
    }"#);
    let a = t.program.auto_consts.values().next().unwrap();
    assert_eq!(a.args.len(), 8);
    assert!(matches!(a.args[0], AutoConstArg::F64(v) if v.to_bits() == (-0.0f64).to_bits()));
    assert_eq!(a.args[1], AutoConstArg::F64(1.5));
    assert_eq!(a.args[2], AutoConstArg::Bool(true));
    assert_eq!(a.args[3], AutoConstArg::Bool(false));
    assert_eq!(a.args[4], AutoConstArg::Str("q\"\\ \u{e9} \u{1d11e}".into()));
    assert!(matches!(a.args[5], AutoConstArg::F64(v) if v.is_nan()));
    assert_eq!(a.args[6], AutoConstArg::F64(f64::NEG_INFINITY));
    assert_eq!(a.args[7], AutoConstArg::I64(7));
    // `0.0` and `-0.0` are different arguments.
    assert_ne!(AutoConstArg::F64(0.0), AutoConstArg::F64(-0.0));
    assert_eq!(AutoConstArg::F64(f64::NAN), AutoConstArg::F64(f64::NAN));
}

#[test]
fn auto_argument_errors() {
    fails("program { auto t: i64 = make(1.5f); }", "an autoconst argument is");
    fails("program { auto t: i64 = make(1_i8); }", "an autoconst argument is");
    fails("program { auto t: i64 = make(param(nope)); }", "nope");
}

#[test]
fn construct_forms() {
    let t = ok("program { fn f(x: f64) -> f64 {
        vars { v: vec3, p: pos3, r: extern Rec, a: [f64; 2], o: extern Opaque }
        v = vec3(x, 1.0, 2.0);
        p = pos3(1, 2, 3);
        r = new Rec { re: x, im: 2 };
        a = [x, 1.0];
        o = new extern Opaque(x);
        o = new Opaque(x);
        return x;
    } }");
    let (_, f) = find_fn(&t, "f");
    let value = |i: usize| match &f.body[i] {
        Statement::Assign { value, .. } => value.clone(),
        s => panic!("{s:?}"),
    };
    assert!(matches!(
        value(0),
        Expression::Construct {
            t: VariableType::Vec3,
            ..
        }
    ));
    assert!(matches!(
        value(1),
        Expression::Construct {
            t: VariableType::Pos3,
            ..
        }
    ));
    let Expression::ConstructExtern { t, args } = value(2) else {
        panic!()
    };
    assert_eq!(t, VariableType::Extern("Rec"));
    assert_eq!(
        args.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        ["re", "im"]
    );
    assert!(matches!(value(3), Expression::ArrayLiteral(v) if v.len() == 2));
    assert_eq!(value(4), value(5));
    // `new T { … }` with a non-extern `T` is the checker's business; it prints and loads
    let t = ok("program { fn f() -> f64 { return new f64 { a: 1 }; } }");
    let text = print(&t.program);
    assert!(text.contains("return new f64 { a: 1 };"), "{text}");
    assert!(isomorphic(&ok(&text).program, &t.program));
    // `if x { … }` is never a struct literal
    ok("program { fn f(x: bool) -> f64 { if x { return 1.0; } return 2.0; } }");
}

// ---------------------------------------------------------------------------------------------
// §8 libraries
// ---------------------------------------------------------------------------------------------

#[test]
fn use_copies_the_closure_to_the_front_in_library_order() {
    let t = ok("program {
        use prelude::{quarter_of};
        fn local() -> f64 { return quarter_of(1.0); }
    }");
    let fns: Vec<_> = t
        .program
        .functions
        .values()
        .map(|f| f.canonical_name.clone().unwrap())
        .collect();
    assert_eq!(
        fns,
        ["halve", "quarter_of", "local"],
        "`unrelated` is not imported"
    );
    let cs: Vec<_> = t
        .program
        .constants
        .values()
        .map(|c| c.name.clone())
        .collect();
    assert_eq!(cs, ["half", "quarter"]);
    // the printed file is self-contained
    let text = print(&t.program);
    assert!(!text.contains("use "));
    assert!(load(&text, FileId(0), &NoEnv).is_ok());
    // only what is named is imported; names can be consts
    let t = ok("program { use prelude::{half, halve}; }");
    assert_eq!(t.program.functions.iter().count(), 1);
    assert_eq!(t.program.constants.iter().count(), 1);
    // importing the same item twice, or through another library, doesn't duplicate it
    let t = ok(
        "program { use prelude::{halve}; use prelude::{halve, quarter_of}; use chain::{third}; }",
    );
    let fns: Vec<_> = t
        .program
        .functions
        .values()
        .map(|f| f.canonical_name.clone().unwrap())
        .collect();
    assert_eq!(fns, ["halve", "quarter_of", "third"]);
}

#[test]
fn use_errors() {
    fails("program { use nowhere::{x}; }", "unknown library");
    fails("program { use prelude::{nope}; }", "has no `nope`");
    fails(
        "program { use prelude::{halve}; fn halve(x: f64) -> f64 { return x; } }",
        "duplicate function",
    );
    fails(
        "program { use prelude::{half}; const half = 1; }",
        "duplicate const",
    );
    let d = fails("program { use cycle_a::{a}; }", "uses itself");
    assert!(d.message.contains("cycle_a"));
    fails("program { use dirty::{x}; }", "may only contain");
    fails("program { use badref::{f}; }", "not defined in library");
    fails("program { use hostref::{f}; }", "can't use host inputs");
    // a syntax error in a library points into the library, by its name from the `Env`
    let d = fails("program { use syntax::{f}; }", "expected");
    assert_eq!(d.span.file, FileId(8));
    assert!(d.snippet().unwrap().text.contains("fn f("));
    assert!(d.to_string().contains("--> lib/syntax.tw:1:"), "{d}");
    // other library files are named after their library
    let d = fails("program { use badref::{f}; }", "not defined");
    assert_eq!(d.file_name(), Some("badref"));
    // library fns can't name hosts either
    fails("program { use hostattr::{f}; }", "can't have `@hosts`");
    // every library needs a FileId of its own
    fails("program { use clash::{c}; }", "already uses");
}

#[test]
fn a_clash_with_an_imported_label_names_the_library() {
    let d = fails(
        "program { use prelude::{halve}; fn halve(x: f64) -> f64 { return x; } }",
        "duplicate function label `halve`",
    );
    let (span, note) = &d.notes[0];
    assert_eq!(span.file, FileId(1));
    assert!(note.contains("imported from library `prelude`"), "{note}");
    assert!(d.to_string().contains("prelude:"), "{d}");
}

#[test]
fn imports_keep_library_order_across_use_items() {
    let t = ok("program { use prelude::{unrelated}; use prelude::{halve}; }");
    let fns: Vec<_> = t
        .program
        .functions
        .values()
        .map(|f| f.canonical_name.clone().unwrap())
        .collect();
    assert_eq!(fns, ["halve", "unrelated"]);
}

// ---------------------------------------------------------------------------------------------
// §9 envelope
// ---------------------------------------------------------------------------------------------

const ENV_PROG: &str = "program {
    host h: f64;
    host v: vec3;
    host dyn dynamic;
    fn f() -> f64 { return 1.0; }
    fn arr() -> [i32; 2] { return [1, 2]; }
    unit u { main f; }
    unit w { main arr; }
    entry top = u @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    entry pair = w @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}";
const CASE: &str = "@case \"c\" origin(0, 0, 0) host(h = 1.5, v = vec3(1.0, 2.0, 3.0))\n";

fn env_ok(directives: &str) -> TestFile {
    ok(&format!("{ENV_PROG}\n{directives}"))
}
fn env_fails(directives: &str, needle: &str) -> Diagnostic {
    fails(&format!("{ENV_PROG}\n{directives}"), needle)
}

#[test]
fn envelope_happy_path() {
    let t = env_ok(&format!(
        "{CASE}@expect \"c\" top[0, 0, 0] = 1.0
         @expect \"c\" top[1, 0, 0] = nan
         @expect \"c\" pair[*] = [1, 2]
         @case \"d\" origin(1.5, -2, 16) host(h = 2, v = vec3(0.0, 0.0, 0.0))
         @expect \"d\" top = [1.0, 2.0]
         @expect \"d\" pair = [[1, 2]]
         @knownfail(java, \"because\")
         @knownfail(cuda, \"x\", case = \"d\", stage = inlined)
         @fuel(5) @tier(nightly) @cells(sample = 4, seed = 9)"
    ));
    assert_eq!(t.cases.len(), 2);
    assert_eq!(t.cases[1].origin, [1.5, -2.0, 16.0]);
    assert_eq!(t.cases[0].hosts[0].1, Value::F64(1.5));
    assert_eq!(
        t.cases[1].hosts[0].1,
        Value::F64(2.0),
        "ints are accepted for floats"
    );
    assert_eq!(t.expects.len(), 5);
    assert_eq!(
        t.expects[1].kind,
        ExpectKind::Cell([1, 0, 0], Value::F64(f64::NAN))
    );
    assert_eq!(
        t.expects[2].kind,
        ExpectKind::All(Value::Array(vec![Value::I32(1), Value::I32(2)]))
    );
    assert_eq!(t.knownfails.len(), 2);
    assert_eq!(t.knownfails[1].stage.as_deref(), Some("inlined"));
    assert_eq!((t.fuel, t.tier.as_deref()), (Some(5), Some("nightly")));
    assert_eq!(t.cells, Some(CellSample { sample: 4, seed: 9 }));
    assert_eq!(t.entry_labels, ["top", "pair"]);
    // directives may come before the program too
    let t = ok(&format!("{CASE}{ENV_PROG}\n@expect \"c\" top[0,0,0] = 1.0"));
    assert_eq!(t.expects.len(), 1);
    // an expectation may name a case declared after it
    let t = ok(&format!(
        "@expect \"c\" top[0,0,0] = 1.0\n{ENV_PROG}\n{CASE}"
    ));
    assert_eq!(t.expects[0].case, 0);
}

#[test]
fn envelope_case_errors() {
    env_fails(&format!("{CASE}{CASE}"), "duplicate case");
    env_fails(
        "@case \"c\" origin(0,0,0) host(h = 1.0)",
        "doesn't bind the host input `v`",
    );
    env_fails(
        &format!(
            "{CASE} @case \"d\" origin(0,0,0) host(h = 1.0, h = 2.0, v = vec3(0.0,0.0,0.0))"
        ),
        "bound twice",
    );
    env_fails(
        "@case \"c\" origin(0,0,0) host(h = 1.0, v = vec3(0.0,0.0,0.0), nope = 1)",
        "unknown host",
    );
    env_fails(
        "@case \"c\" origin(0,0,0) host(h = 1.0, v = vec3(0.0,0.0,0.0), dyn = 1)",
        "dynamic",
    );
    env_fails(
        "@case \"c\" origin(0,0,0) host(h = 1L, v = vec3(0.0,0.0,0.0))",
        "expected a f64 literal",
    );
    env_fails(
        "@case \"c\" origin(0,0,0) host(h = 1.0f, v = vec3(0.0,0.0,0.0))",
        "expected a f64 literal",
    );
    env_fails(
        "@case \"c\" origin(0,0,0) host(h = true, v = vec3(0.0,0.0,0.0))",
        "expected a number",
    );
    env_fails(
        "@case \"c\" origin(0,0,0) host(h = 1.0, v = [1.0, 2.0, 3.0])",
        "vec3",
    );
    env_fails(
        "@case \"c\" origin(0,0,0) param(nope = 1) host(h = 1.0, v = vec3(0.0,0.0,0.0))",
        "unknown param",
    );
    env_fails(
        "@case \"c\" origin(true,0,0) host(h = 1.0, v = vec3(0.0,0.0,0.0))",
        "expected a number",
    );
}

#[test]
fn envelope_expectation_errors() {
    env_fails(
        &format!("{CASE}@expect \"nope\" top[0,0,0] = 1.0"),
        "no case named",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" nope[0,0,0] = 1.0"),
        "unknown entry",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" top[2,0,0] = 1.0"),
        "outside dims",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" top[-1,0,0] = 1.0"),
        "outside dims",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" top[0,1,0] = 1.0"),
        "outside dims",
    );
    env_fails(&format!("{CASE}@expect \"c\" top = [1.0]"), "2 values");
    env_fails(
        &format!("{CASE}@expect \"c\" top = [1.0, 2.0, 3.0]"),
        "2 values",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" pair = [1, 2]"),
        "a grid lists every cell",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" pair = [1]"),
        "expected a value of type [i32; 2]",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" pair[0,0,0] = [1, 2, 3]"),
        "expected 2 elements",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" top[0,0,0] = 1.0 @expect \"c\" top[0,0,0] = 2.0"),
        "same cell",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" top[*] = 1.0 @expect \"c\" top[1,0,0] = 2.0"),
        "same cell",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" top[1,0,0] = 2.0 @expect \"c\" top[*] = 1.0"),
        "same cell",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" top[*] = 1.0 @expect \"c\" top = [1.0, 2.0]"),
        "same cell",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" error(Boom) @expect \"c\" top[0,0,0] = 1.0"),
        "same case",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" top[0,0,0] = 1.0 @expect \"c\" error(Boom)"),
        "same case",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" error(A) @expect \"c\" error(B)"),
        "same case",
    );
    env_fails(
        &format!("{CASE}@expect \"c\" top[0,0,0] = 1L"),
        "expected a f64 literal",
    );
    // same cell in different cases or entries is fine
    env_ok(&format!(
        "{CASE}@case \"d\" origin(0,0,0) host(h = 1.0, v = vec3(0.0,0.0,0.0))
         @expect \"c\" top[0,0,0] = 1.0 @expect \"d\" top[0,0,0] = 1.0 @expect \"c\" pair[0,0,0] = [1, 2]"
    ));
}

#[test]
fn envelope_directive_errors() {
    env_fails("@fuel(1) @fuel(2)", "duplicate `@fuel`");
    env_fails("@tier(a) @tier(b)", "duplicate `@tier`");
    env_fails(
        "@cells(sample = 1, seed = 1) @cells(sample = 1, seed = 1)",
        "duplicate `@cells`",
    );
    env_fails("@knownfail(rust, \"x\", case = \"nope\")", "names no case");
    env_fails(
        &format!("{CASE}@knownfail(rust, \"x\", stage = after)"),
        "stage must be",
    );
    env_fails("@bogus", "unknown directive");
    env_fails("@fuel(-1)", "unsigned");
    env_fails(
        "@expect reject @expect reject",
        "duplicate `@expect reject`",
    );
    env_fails(&format!("@expect reject {CASE}"), "can't have `@case`");
    env_fails(
        &format!("{CASE}@expect reject @expect \"c\" top[0,0,0] = 1.0"),
        "can't have",
    );
    let t = env_ok("@expect reject(MissingHelper)");
    assert_eq!(t.reject, Some(Some("MissingHelper".into())));
    let t = env_ok("@expect reject");
    assert_eq!(t.reject, Some(None));
}

#[test]
fn envelope_record_values() {
    let src = "program {
        host r: extern Rec;
        fn f() -> extern Rec { return new Rec { re: 1.0, im: 2 }; }
        unit u { main f; }
        entry u @dims(1,1,1) @origin_scale(1,1,1) @position_scale(1,1,1);
    }
    @case \"c\" origin(0,0,0) host(r = new Rec { im: 7, re: 1.5 })
    @expect \"c\" u[0,0,0] = new Rec { re: 1.0, im: 2 }";
    let t = ok(src);
    assert_eq!(
        t.cases[0].hosts[0].1,
        Value::Record("Rec", vec![("re", Value::F64(1.5)), ("im", Value::I32(7))]),
        "fields come out in the registry's order"
    );
    let text = print_test_file(&t);
    assert!(text.contains("new extern Rec { re: 1.5, im: 7 }"), "{text}");
    assert!(load_with(&text, &TestEnv).is_ok());
    // without the registry, fields keep the written order and are typed by their literals,
    // which canonical text always spells unambiguously
    let t = load(src, FileId(0), &NoEnv).unwrap();
    assert_eq!(
        t.cases[0].hosts[0].1,
        Value::Record("Rec", vec![("im", Value::I32(7)), ("re", Value::F64(1.5))]),
    );
    let text = print_test_file(&t);
    assert_eq!(
        print_test_file(&load(&text, FileId(0), &NoEnv).unwrap()),
        text
    );
    fails(
        &src.replace("im: 7, re: 1.5", "im: 7, im: 1.5"),
        "given twice",
    );
    let bad = src.replace("im: 7, re: 1.5", "re: 1.5");
    fails(&bad, "missing field `im`");
    let bad = src.replace("im: 7, re: 1.5", "im: 7, re: 1.5, extra: 1");
    fails(&bad, "no field `extra`");
}

// ---------------------------------------------------------------------------------------------
// diagnostics and the source map
// ---------------------------------------------------------------------------------------------

#[test]
fn diagnostics_render_line_column_and_caret() {
    let src = "program {\n    fn f() -> f64 {\n        return nope;\n    }\n}\n";
    let d = load(src, FileId(0), &NoEnv).unwrap_err();
    assert_eq!(d.stage, Stage::Elab);
    let s = d.to_string();
    assert!(s.starts_with("error: unknown name `nope`"), "{s}");
    assert!(s.contains("--> file0:3:16"), "{s}");
    assert!(s.contains("        return nope;"), "{s}");
    assert!(s.contains("^^^^"), "{s}");
    assert_eq!(d.snippet().unwrap().line, 3);

    let d = load("program {\n  fn f( -> f64 {} }", FileId(0), &NoEnv).unwrap_err();
    assert_eq!(d.stage, Stage::Parse);
    assert!(d.to_string().contains("file0:2:"), "{d}");

    let d = load("program { \u{e9} }", FileId(0), &NoEnv).unwrap_err();
    assert_eq!(d.stage, Stage::Lex);
    assert!(d.to_string().contains("unexpected character"), "{d}");
    // columns count scalar values
    let d = load("program { const \u{e9}\u{e9}x = 1; }", FileId(0), &NoEnv).unwrap_err();
    assert!(d.to_string().contains(":1:"), "{d}");
}

#[test]
fn only_the_first_error_is_reported() {
    let d = load(
        "program { fn f() -> f64 { return a; } fn g() -> f64 { return b; } }",
        FileId(0),
        &NoEnv,
    )
    .unwrap_err();
    assert!(d.message.contains("`a`"));
}

#[test]
fn source_map_covers_entities_statements_and_expressions() {
    let src = "program {
    host h: f64;
    const c = (1 + 2);
    fn f(x: f64) -> f64 {
        vars { y: f64 }
        let t: f64 = x;
        if (x < 1.0) {
            y = ext sin(x);
        } else if (x > 2.0) {
            y = (x * 2.0);
        }
        repeat 2 {
            while true {
                break;
            }
        }
        return y;
    }
    unit u { main f; }
    entry e = u @dims(1,1,1) @origin_scale(1,1,1) @position_scale(1,1,1);
}";
    let t = load(src, FileId(0), &TestEnv).unwrap();
    let line = |span: Span| src[..span.start as usize].matches('\n').count() + 1;
    let text = |span: Span| &src[span.start as usize..span.end as usize];
    let m = &t.source_map;
    let (f, func) = t.program.functions.iter().next().unwrap();
    let (u, _) = t.program.compute_units.iter().next().unwrap();
    let (c, _) = t.program.constants.iter().next().unwrap();
    let (h, _) = t.program.host_inputs.iter().next().unwrap();

    assert_eq!(line(m.functions[&f]), 4);
    assert_eq!(line(m.units[&u]), 19);
    assert_eq!(line(m.constants[&c]), 3);
    assert_eq!(line(m.hosts[&h]), 2);
    assert_eq!(line(m.entries[0]), 20);
    let y = func.variables.iter().nth(1).unwrap().0;
    assert_eq!(text(m.vars[&(f, y)]), "y");

    use ExprStep as E;
    use StmtStep as S;
    let sp = |path: &[S]| m.statement(f, &StmtPath(path.to_vec())).unwrap();
    // `let t: f64 = x;` is statement 0; the if is 1; `else if` is Else, Index(0)
    assert_eq!(line(sp(&[S::Index(0)])), 6);
    assert_eq!(line(sp(&[S::Index(1)])), 7);
    assert_eq!(line(sp(&[S::Index(1), S::Then, S::Index(0)])), 8);
    assert_eq!(line(sp(&[S::Index(1), S::Else, S::Index(0)])), 9);
    assert_eq!(
        line(sp(&[
            S::Index(1),
            S::Else,
            S::Index(0),
            S::Then,
            S::Index(0)
        ])),
        10
    );
    assert_eq!(
        line(sp(&[
            S::Index(2),
            S::Body,
            S::Index(0),
            S::Body,
            S::Index(0)
        ])),
        14
    );
    assert_eq!(line(sp(&[S::Index(3)])), 17);

    let ep = |s: &[S], e: &[E]| {
        m.expression(f, &StmtPath(s.to_vec()), &ExprPath(e.to_vec()))
            .unwrap()
    };
    // y = ext sin(x);  → root is the call, Arg(0) is `x`
    let then = [S::Index(1), S::Then, S::Index(0)];
    assert_eq!(text(ep(&then, &[])), "ext sin(x)");
    assert_eq!(text(ep(&then, &[E::Arg(0)])), "x");
    // the condition is the root of the `if` statement
    assert_eq!(text(ep(&[S::Index(1)], &[])), "(x < 1.0)");
    assert_eq!(text(ep(&[S::Index(1)], &[E::Left])), "x");
    assert_eq!(text(ep(&[S::Index(1)], &[E::Right])), "1.0");
    let inner = [S::Index(1), S::Else, S::Index(0), S::Then, S::Index(0)];
    assert_eq!(text(ep(&inner, &[E::Right])), "2.0");
    assert_eq!(
        text(m.const_expression(c, &ExprPath(vec![E::Left])).unwrap()),
        "1"
    );
    assert_eq!(
        text(m.const_expression(c, &ExprPath(vec![])).unwrap()),
        "(1 + 2)"
    );
    assert!(m.statement(f, &StmtPath(vec![S::Index(9)])).is_none());
}

#[test]
fn source_map_inputs() {
    let t = ok(&format!(
        "program {{
            fn leaf() -> f64 {{ return 1.0; }}
            fn root() -> f64 {{ return density(d); }}
            input d = leaf_u {GRID};
            unit leaf_u {{ main leaf; }}
            unit a {{ main root; inputs [d]; }}
        }}"
    ));
    let (d, _) = t.program.density_inputs.iter().next().unwrap();
    assert!(t.source_map.inputs.contains_key(&d));
}

// ---------------------------------------------------------------------------------------------
// printer layout
// ---------------------------------------------------------------------------------------------

#[test]
fn canonical_layout() {
    let text = print_test_file(&ok(&format!(
        "program {{
            host h: f64;
            fn f(a: f64, b: i32) -> f64 {{
                vars {{ r: f64 }}
                if (a < 1.0) {{ r = a; }} else if (a > 2.0) {{ r = 2.0; }} else {{ r = 3.0; }}
                return (r + ext sin(a));
            }}
            unit u {{ main f; hosts [h]; }}
            entry u {GRID};
        }}"
    )));
    let want = "program {
    host h: f64;

    fn f(a: f64, b: i32) -> f64 @hash(0x0) {
        vars {
            r: f64,
        }
        if (a < 1.0) {
            r = a;
        } else if (a > 2.0) {
            r = 2.0;
        } else {
            r = 3.0;
        }
        return (r + ext sin::<(f64) -> f64>(a));
    }

    unit f @hash(0x0) {
        main f;
        hosts [h];
    }

    entry f = f @dims(1, 1, 1) @origin_scale(1.0, 1.0, 1.0) @position_scale(1.0, 1.0, 1.0);
}
";
    assert_eq!(text, want);
    assert!(text.ends_with('\n') && !text.contains('\r'));
}

#[test]
fn params_print_untyped_when_they_are_not_a_slot_prefix() {
    let mut p = empty_program();
    let mut f = func(Some("f"));
    let a = f.variables.insert(Var {
        name: Name::Named("a".into()),
        t: VariableType::F64,
    });
    let b = f.variables.insert(Var {
        name: Name::Named("b".into()),
        t: VariableType::I32,
    });
    f.parameters = vec![b, a];
    p.functions.insert(f);
    let text = print(&p);
    assert!(text.contains("fn f(b, a) -> f64"), "{text}");
    assert!(
        text.contains("a: f64,") && text.contains("b: i32,"),
        "{text}"
    );
    assert!(isomorphic(
        &load(&text, FileId(0), &NoEnv).unwrap().program,
        &compact(&p)
    ));
}

#[test]
fn empty_program_prints_and_parses() {
    let text = print(&empty_program());
    assert_eq!(text, "program {\n}\n");
    assert!(load(&text, FileId(0), &NoEnv).is_ok());
}

#[test]
fn printing_is_total_for_unusual_programs() {
    // a helper shared between two units that reads a density input of only one of them,
    // an unregistered extern, a dynamic host, a base type that disagrees with the base
    let mut p = empty_program();
    let mut leaf = func(Some("leaf"));
    leaf.body = vec![Statement::Return(Expression::Double(1.0))];
    let leaf_f = p.functions.insert(leaf);
    let mut shared = func(Some("shared"));
    let leaf_u = p.compute_units.insert(ComputeUnit::DensityFunction {
        function: leaf_f,
        density_inputs: vec![],
        auto_consts: vec![],
        host_inputs: vec![],
        helper_functions: vec![],
        constants: vec![],
        source_hash: 0,
    });
    let d = p.density_inputs.insert(DensityInput {
        name: Name::Named("dv".into()),
        density_function: leaf_u,
        scaled_origin: (1.0, 1.0, 1.0),
        scaled_position: (1.0, 1.0, 1.0),
        dimensions: (1, 1, 1),
        mapping: Default::default(),
        previous: false,
    });
    shared.body = vec![Statement::Return(Expression::Field {
        base: Box::new(Expression::DensityVariable(d, None)),
        field: "x".into(),
        type_of_field: VariableType::F64,
        base_type: VariableType::Vec3,
    })];
    let shared_f = p.functions.insert(shared);
    let mut root = func(Some("root"));
    root.body = vec![Statement::Return(Expression::ExternCall {
        function_name: "never registered".into(),
        parameters: vec![Expression::FunctionCall {
            function: shared_f,
            parameters: vec![],
        }],
        parameter_types: vec![VariableType::Extern("Odd")],
        return_type: VariableType::Array(Box::new(VariableType::Vec3), 3),
    })];
    let root_f = p.functions.insert(root);
    for main in [shared_f, root_f] {
        p.compute_units.insert(ComputeUnit::DensityFunction {
            function: main,
            density_inputs: if main == shared_f { vec![d] } else { vec![] },
            auto_consts: vec![],
            host_inputs: vec![],
            helper_functions: vec![shared_f],
            constants: vec![],
            source_hash: 0,
        });
    }
    p.host_inputs.insert(HostInput::Dynamic);
    let text = print(&p);
    assert!(text.contains("density(dv)"), "{text}");
    assert!(
        text.contains("ext `never registered`::<(extern Odd) -> [vec3; 3]>(shared())"),
        "{text}"
    );
    assert!(text.contains(".x::<f64; vec3>"), "{text}");
    assert!(text.contains("host %0 dynamic;"), "{text}");
    let t = load(&text, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}\n{text}"));
    assert!(isomorphic(&t.program, &compact(&p)));
}

#[test]
fn repeated_params_print_and_load() {
    let mut p = empty_program();
    let mut f = func(Some("f"));
    let a = f.variables.insert(Var {
        name: Name::Named("a".into()),
        t: VariableType::F64,
    });
    f.parameters = vec![a, a];
    f.body = vec![Statement::Return(Expression::Variable(a))];
    p.functions.insert(f);
    let text = print(&p);
    assert!(text.contains("fn f(a, a) -> f64"), "{text}");
    let t = load(&text, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}\n{text}"));
    assert!(isomorphic(&t.program, &compact(&p)));
    assert_eq!(print(&t.program), text);
}

#[test]
#[should_panic(expected = "dangling Ref")]
fn dangling_refs_panic_with_a_name() {
    let mut p = empty_program();
    let mut f = func(Some("f"));
    let stale = p.constants.insert(Constant {
        value: ConstExpr::Int(1),
        name: "c".into(),
    });
    p.constants.remove(stale);
    f.body = vec![Statement::Return(Expression::Constant(stale))];
    p.functions.insert(f);
    print(&p);
}

#[test]
fn labels_api_matches_printed_text() {
    let t = ok(&format!(
        "program {{
            fn leaf() -> f64 {{ return 1.0; }}
            fn root(a: f64) -> f64 {{ return density(d); }}
            input d = leaf_u {GRID};
            unit leaf_u {{ main leaf; }}
            unit a {{ main root; inputs [d]; }}
            entry a {GRID};
        }}"
    ));
    let labels = Labels::new(&t.program);
    let (f, _) = t.program.functions.iter().nth(1).unwrap();
    assert_eq!(labels.function(f), "root");
    let (u, _) = t.program.compute_units.iter().nth(1).unwrap();
    assert_eq!(labels.unit(u), "root");
    assert_eq!(labels.entry(0), "root");
    let text = print_function(&t.program, &labels, f);
    assert!(
        text.starts_with("fn root(a: f64) -> f64 @hash(0x0) {\n"),
        "{text}"
    );
    assert!(text.contains("density(d)"), "{text}");
    assert!(text.ends_with("}\n"));
}

#[test]
fn case_params() {
    let prog = "program { param seed: i64; param k: f64; param on: bool; fn f() -> f64 { return 1.0; } unit u { main f; } entry u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1); }\n";
    let case = |bindings: &str| format!("{prog}@case \"c\" origin(0, 0, 0) {bindings}");
    let t = ok(&case("param(seed = 1, k = 2.5, on = true)"));
    let values: Vec<_> = t.cases[0].params.iter().map(|(_, v)| v.clone()).collect();
    assert_eq!(values, [Value::I64(1), Value::F64(2.5), Value::Bool(true)]);
    // The whole u64 range is accepted for an i64, as bits. Hex too.
    for (text, want) in [
        ("18446744073709551615", -1),
        ("9223372036854775808", i64::MIN),
        ("-5", -5),
        ("0xFFFFFFFFFFFFFFFF", -1),
        ("7L", 7),
    ] {
        let t = ok(&case(&format!("param(seed = {text}, k = 0.0, on = false)")));
        assert_eq!(t.cases[0].params[0].1, Value::I64(want), "{text}");
    }
    fails(&case("param(seed = 1, k = 2.5)"), "doesn't bind the param `on`");
    fails(&case("param(seed = 1, seed = 2, k = 2.5, on = true)"), "bound twice");
    fails(&case("param(seed = 1.5, k = 2.5, on = true)", ), "i64");
    fails(&case("param(seed = 1, k = true, on = true)"), "expected a number");
    fails(&case("param(seed = 18446744073709551616, k = 0.0, on = true)"), "out of range");
    // The printer writes params by label and reads them back.
    let t = ok(&case("param(seed = 18446744073709551615, k = 2.5, on = true)"));
    let text = crate::print_test_file(&t);
    assert!(text.contains("param(seed = -1, k = 2.5, on = true)"), "{text}");
}

#[test]
fn removed_table_syntax_fails() {
    fails("program { table t = base3d; }", "expected an item");
    fails("program { fn f() -> f64 { return 1.0; } unit u { main f; tables [t]; } }", "main` or a list");
    fails("program { param seed: i64; fn f() -> f64 { return 1.0; } unit u { main f; } }\n@case \"c\" origin(0, 0, 0) seed(1)", "seed");
}

// ---------------------------------------------------------------------------------------------
// Mappings and refinements
// ---------------------------------------------------------------------------------------------

/// `base` and `other` are units, with entry `e` on `other`; `items` goes after them.
fn with_units(items: &str) -> String {
    format!(
        "program {{
            fn base() -> f64 {{ return 1.0; }}
            fn other() -> f64 {{ return 2.0; }}
            unit base {{ main base; }}
            unit other {{ main other; }}
            entry e = other @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
            {items}
        }}"
    )
}

const GRID4: &str = "@dims(4, 1, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1)";

fn load_ok(src: &str) -> TestFile {
    load(src, FileId(0), &TestEnv).unwrap_or_else(|d| panic!("{d:?}"))
}

fn load_err(src: &str) -> String {
    format!("{:?}", load(src, FileId(0), &TestEnv).err().expect("should not load"))
}

fn round_trips(tf: &TestFile) {
    let text = print(&tf.program);
    let again = load_ok(&text);
    assert!(isomorphic(&compact(&tf.program), &again.program), "{text}");
    assert_eq!(print(&again.program), text);
}

#[test]
fn input_mappings_parse_and_print() {
    use AxisFactor::{Factor, Repeat};
    let tf = load_ok(&with_units(&format!(
        "input a = base {GRID4};
         input b = base @expand(4, *, 1) {GRID4};
         input c = base {GRID4} @lerp(4, 8, 4);
         input d = base @dims(4, 1, 4) @lerp(*, *, 3) @origin_scale(1, 1, 1) @previous @position_scale(1, 1, 1);
         input e = base {GRID4} @previous;"
    )));
    let got: Vec<_> = tf.program.density_inputs.values().map(|d| (d.mapping, d.previous)).collect();
    assert_eq!(
        got,
        [
            (DensityMapping::Identity, false),
            (DensityMapping::Expand([Factor(4), Repeat, Factor(1)]), false),
            (DensityMapping::Lerp([Factor(4), Factor(8), Factor(4)]), false),
            (DensityMapping::Lerp([Repeat, Repeat, Factor(3)]), true),
            (DensityMapping::Identity, true),
        ]
    );
    let text = print(&tf.program);
    assert!(text.contains("@position_scale(1.0, 1.0, 1.0) @expand(4, *, 1);"), "{text}");
    assert!(text.contains("@lerp(*, *, 3) @previous;"), "{text}");
    assert!(!text.contains("input a = base @dims(4, 1, 4) @origin_scale(1.0, 1.0, 1.0) @position_scale(1.0, 1.0, 1.0) @"));
    round_trips(&tf);
}

#[test]
fn mapping_errors() {
    let bad = [
        ("@expand(1, 1, 1) @lerp(1, 1, 1)", "at most one mapping"),
        ("@expand(1, 1, 1) @expand(1, 1, 1)", "at most one mapping"),
        ("@previous @previous", "duplicate `@previous`"),
        ("@expand(1, 1)", "expected `,`"),
        ("@expand(-1, 1, 1)", "an unsigned integer"),
        ("@lerp(1.5, 1, 1)", "an unsigned integer"),
        ("@expand(2147483648, 1, 1)", "out of range for i32"),
    ];
    for (attrs, want) in bad {
        let e = load_err(&with_units(&format!("input a = base {GRID4} {attrs};")));
        assert!(e.contains(want), "{attrs}: {e}");
    }
    // Factor 0 is the validator's to reject (`MappingBadFactor`), so it must load.
    load_ok(&with_units(&format!("input a = base {GRID4} @expand(0, 1, 1);")));
    // Mappings and `@previous` belong to inputs only.
    for attr in ["@expand(1, 1, 1)", "@lerp(1, 1, 1)", "@previous"] {
        let e = load_err(&with_units(&format!("entry f = base {GRID4} {attr};")));
        assert!(e.contains("unknown attribute"), "{attr}: {e}");
        let e = load_err(&with_units(&format!("refinement r of e {{ rerun base {GRID4} {attr}; }}")));
        assert!(e.contains("unknown attribute"), "{attr}: {e}");
    }
}

#[test]
fn refinements_parse_and_print() {
    let tf = load_ok(&with_units(&format!(
        "refinement step of e @march(0, 383, 0) {{
            rerun other {GRID4} with base;
            rerun base {GRID4};
         }}
         refinement `odd name` of e {{ }}"
    )));
    let r = &tf.program.refinements;
    assert_eq!((r[0].name.as_str(), r[0].entry, r[0].march), ("step", 0, (0, 383, 0)));
    assert_eq!(r[0].reruns.len(), 2);
    assert!(r[0].reruns[0].with.is_some() && r[0].reruns[1].with.is_none());
    assert_eq!((r[1].name.as_str(), r[1].march), ("odd name", (0, 0, 0)));
    let text = print(&tf.program);
    assert!(text.contains("refinement step of other @march(0, 383, 0) {"), "{text}");
    assert!(text.contains("refinement `odd name` of other {"), "{text}");
    assert_eq!(tf.source_map.refinements.len(), 2);
    round_trips(&tf);
}

/// Every contextual keyword still works as a label, including in its own construct.
#[test]
fn refinement_words_are_contextual() {
    let src = format!(
        "program {{
            fn refinement() -> f64 {{ let of: f64 = 1.0; return of; }}
            fn with() -> f64 {{ return 2.0; }}
            unit rerun {{ main refinement; }}
            unit with {{ main with; }}
            input of = rerun {GRID4};
            entry of = rerun {GRID4};
            refinement of of of {{ rerun rerun {GRID4} with with; }}
        }}"
    );
    let tf = load_ok(&src);
    assert_eq!(tf.program.refinements[0].name, "of");
    round_trips(&tf);
}

#[test]
fn refinement_errors() {
    let bad = [
        (format!("refinement r of nope {{ }}"), "unknown entry `nope`"),
        (format!("refinement r of e {{ rerun nope {GRID4}; }}"), "unknown unit `nope`"),
        (format!("refinement r of e {{ rerun base {GRID4} with nope; }}"), "unknown unit `nope`"),
        (format!("refinement r of e {{ rerun base @dims(1, 1, 1); }}"), "missing `@origin_scale"),
        (format!("refinement r of e @march(-1, 0, 0) {{ }}"), "an unsigned integer"),
        (format!("refinement r of e @march(0, 0, 3000000000) {{ }}"), "out of range for i32"),
        (format!("refinement r of e @hash(1) {{ }}"), "unknown refinement attribute"),
        (format!("refinement r e {{ }}"), "expected `of`"),
        (format!("refinement r of e {{ base; }}"), "expected `rerun`"),
    ];
    for (src, want) in bad {
        let e = load_err(&with_units(&src));
        assert!(e.contains(want), "{src}: {e}");
    }
    // Duplicate names and empty rerun lists are the validator's to reject, so they must load.
    load_ok(&with_units("refinement r of e { } refinement r of e { }"));
}
