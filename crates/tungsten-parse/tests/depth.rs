//! P1 at the nesting limits (SPEC §6.2): a program nested exactly `MAX_DEPTH` deep, in any of the
//! counted kinds, prints, loads and round-trips in every build profile (these tests run in debug).
//! One level deeper is a diagnostic.

use tungsten_ir::*;
use tungsten_parse::ast::MAX_DEPTH;
use tungsten_parse::*;

const N: usize = MAX_DEPTH as usize;

fn program(body: Vec<Statement>, param: VariableType) -> OptimizedProgram {
    let mut p = OptimizedProgram {
        compute_units: Arena::new(),
        functions: Arena::new(),
        entry_points: vec![],
        constants: Arena::new(),
        host_inputs: Arena::new(),
        permutation_tables: Arena::new(),
    };
    let mut variables = Arena::new();
    let x = variables.insert(Var {
        name: Name::Named("x".into()),
        t: param,
    });
    let f = p.functions.insert(Function {
        canonical_name: Some("f".into()),
        host_inputs: vec![],
        body,
        variables,
        parameters: vec![x],
        return_type: VariableType::F64,
        source_hash: 0,
    });
    p.compute_units.insert(ComputeUnit::DensityFunction {
        function: f,
        density_inputs: Arena::new(),
        permutation_table_inputs: vec![],
        host_inputs: vec![],
        helper_functions: vec![],
        constants: vec![],
        source_hash: 0,
    });
    p
}

/// `x`, the only var of the program's only fn.
fn x(p: &OptimizedProgram) -> Expression {
    let f = p.functions.values().next().unwrap();
    Expression::Variable(f.variables.iter().next().unwrap().0)
}

/// `return e;` where `e` is `shape` applied to `x` until the expression is `depth` deep.
fn with_expr(
    depth: usize,
    shape: impl Fn(Expression, &OptimizedProgram) -> Expression,
) -> OptimizedProgram {
    let mut p = program(vec![], VariableType::F64);
    let mut e = x(&p);
    for _ in 1..depth {
        e = shape(e, &p);
    }
    let f = p.functions.iter().next().unwrap().0;
    p.functions[f].body = vec![Statement::Return(e)];
    p
}

fn binary(left: Expression, right: Expression) -> Expression {
    Expression::BinaryOp {
        op: BinaryOperator::Add,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn round_trips(p: &OptimizedProgram, what: &str) {
    let text = print(p);
    let t = match load(&text, FileId(0), &NoEnv) {
        Ok(t) => t,
        Err(d) => panic!("{what}: {d}"),
    };
    assert!(isomorphic(&t.program, &compact(p)), "P1 failed for {what}");
    assert_eq!(print(&t.program), text, "P2 failed for {what}");
}

fn too_deep(p: &OptimizedProgram, what: &str) {
    let d = load(&print(p), FileId(0), &NoEnv).expect_err(what);
    assert!(d.message.contains("nested too deeply"), "{what}: {d}");
}

type Shape = fn(Expression, &OptimizedProgram) -> Expression;

const SHAPES: &[(&str, Shape)] = &[
    ("left-nested", |e, p| binary(e, x(p))),
    ("right-nested", |e, p| binary(x(p), e)),
    ("negations", |e, _| Expression::UnaryOp {
        op: UnaryOperator::Negate,
        operand: Box::new(e),
    }),
    ("refs", |e, _| Expression::UnaryOp {
        op: UnaryOperator::Ref(true),
        operand: Box::new(e),
    }),
    ("casts", |e, _| Expression::ExplicitCast {
        to: VariableType::F64,
        expr: Box::new(e),
    }),
    ("fields", |e, _| Expression::Field {
        base: Box::new(e),
        field: "x".into(),
        type_of_field: VariableType::F64,
        base_type: VariableType::Vec3,
    }),
    ("calls", |e, p| Expression::FunctionCall {
        function: p.functions.iter().next().unwrap().0,
        parameters: vec![e],
    }),
    ("arrays", |e, _| Expression::ArrayLiteral(vec![e])),
];

#[test]
fn expressions_at_the_limit() {
    for (what, shape) in SHAPES {
        round_trips(&with_expr(N, shape), what);
        too_deep(&with_expr(N + 1, shape), what);
    }
}

fn nested_ifs(depth: usize, else_if: bool, inner: Expression) -> Vec<Statement> {
    let mut body = vec![Statement::Return(inner)];
    for _ in 0..depth {
        let cond = Expression::Bool(true);
        body = vec![if else_if {
            Statement::If {
                condition: cond,
                then_branch: vec![Statement::Break],
                else_branch: body,
            }
        } else {
            Statement::While {
                condition: cond,
                body,
            }
        }];
    }
    body
}

#[test]
fn blocks_at_the_limit() {
    for else_if in [false, true] {
        let what = if else_if {
            "else-if chain"
        } else {
            "nested whiles"
        };
        let mut p = program(vec![], VariableType::F64);
        let f = p.functions.iter().next().unwrap().0;
        let e = x(&p);
        p.functions[f].body = nested_ifs(N, else_if, e.clone());
        round_trips(&p, what);
        p.functions[f].body = nested_ifs(N + 1, else_if, e);
        too_deep(&p, what);
    }
}

#[test]
fn blocks_and_expressions_count_separately() {
    let mut p = with_expr(N, SHAPES[1].1);
    let f = p.functions.iter().next().unwrap().0;
    let Statement::Return(e) = p.functions[f].body.pop().unwrap() else {
        unreachable!()
    };
    p.functions[f].body = nested_ifs(N, false, e);
    round_trips(&p, "deep expression in deep blocks");
}

fn nested_array(depth: usize) -> VariableType {
    (1..depth).fold(VariableType::F64, |t, _| {
        VariableType::Array(Box::new(t), 1)
    })
}

#[test]
fn types_and_values_at_the_limit() {
    round_trips(&program(vec![], nested_array(N)), "nested array type");
    too_deep(&program(vec![], nested_array(N + 1)), "nested array type");

    // a host of that type, bound to a value nested as deeply
    let value = |depth: usize| format!("{}1.0{}", "[".repeat(depth - 1), "]".repeat(depth - 1));
    let file = |depth: usize| {
        let mut p = program(vec![], VariableType::F64);
        p.host_inputs.insert(HostInput::Static {
            name: Name::Named("h".into()),
            t: nested_array(depth.min(N)),
        });
        format!(
            "{}@case \"c\" origin(0.0, 0.0, 0.0) seed(1) host(h = {})\n",
            print(&p),
            value(depth)
        )
    };
    let text = file(N);
    let t = load(&text, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}"));
    assert_eq!(print_test_file(&t), text);
    let d = load(&file(N + 1), FileId(0), &NoEnv).unwrap_err();
    assert!(d.message.contains("nested too deeply"), "{d}");
}

#[test]
fn constants_at_the_limit() {
    let constant = |depth: usize| {
        let mut p = program(vec![], VariableType::F64);
        let mut c = ConstExpr::Int(1);
        for _ in 1..depth {
            c = ConstExpr::UnaryOp {
                op: UnaryOperator::Negate,
                operand: Box::new(c),
            };
        }
        p.constants.insert(Constant {
            value: c,
            name: "c".into(),
        });
        p
    };
    round_trips(&constant(N), "constant");
    too_deep(&constant(N + 1), "constant");
}

#[test]
fn parentheses_alone_hit_only_the_recursion_guard() {
    // redundant parentheses add no AST depth, so far more than `MAX_DEPTH` of them is fine
    let deep = 3 * N;
    let src = format!(
        "program {{ fn f(x: f64) -> f64 {{ return {}x{}; }} }}",
        "(".repeat(deep),
        ")".repeat(deep)
    );
    let t = load(&src, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}"));
    assert!(print(&t.program).contains("return x;"));
    let src = src.replace("return ", &format!("return {}", "(".repeat(5000)));
    let src = src.replace("x;", &format!("x{};", ")".repeat(5000)));
    let d = load(&src, FileId(0), &NoEnv).unwrap_err();
    assert!(d.message.contains("levels of recursion"), "{d}");
}
