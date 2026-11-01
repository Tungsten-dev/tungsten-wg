//! P1 and P2 for programs the way tungsten-mc makes them: built as legacy SPMT and lowered with
//! `tungsten_legacy::conversion`, before and after inlining.

use bumpalo::Bump;
use tungsten_ir::{OptimizedProgram, compact, isomorphic};
use tungsten_legacy::builder::{Builder, SPMTBuilder};
use tungsten_legacy::conversion::convert;
use tungsten_legacy::model::*;
use tungsten_parse::{FileId, NoEnv, load, print, print_test_file};

fn unit<'m>(
    arena: &'m Bump,
    name: &str,
    body: impl FnOnce(&mut Builder<'m>) -> Vec<Statement<'m>>,
) -> DensityFunctionRef<'m> {
    let mut b = Builder::new(arena).named(name).returns(VariableType::F64);
    let stmts = body(&mut b);
    b.set_body(stmts);
    b.finish_compute_unit()
}

/// Namespaced names, a helper with a loop, a host input, constants, a permutation table, shared
/// and resampled units and an indexed read.
fn build(arena: &Bump) -> SPMT<'_> {
    let mut h = Builder::new(arena)
        .named("minecraft:accumulate")
        .returns(VariableType::F64);
    let t = h.param("t", VariableType::F64);
    let acc = h.local("acc", VariableType::F64);
    h.set_body(vec![
        Statement::Assign {
            target: acc,
            value: 0.0f64.into(),
        },
        Statement::Repeat {
            count: 3,
            body: vec![
                Statement::Assign {
                    target: acc,
                    value: acc + t,
                },
                Statement::If {
                    condition: acc.gt(10.0f64),
                    then_branch: vec![Statement::Break],
                    else_branch: vec![],
                },
            ],
        },
        Statement::Return(Expression::Variable(acc)),
    ]);
    let helper = h.finish_function();

    let mut s = Builder::new(arena)
        .named("minecraft:seed")
        .returns(VariableType::F64);
    let seed = s.alloc_var(Name::Named("seed".into()), VariableType::I64);
    s.set_body(vec![Statement::Return(Expression::ExplicitCast {
        to: VariableType::F64,
        expr: Box::new(Expression::Variable(seed)),
    })]);
    let mut seed_unit = s.build_compute_unit();
    seed_unit.host_inputs = vec![HostInput::Static(seed)];
    let seed_unit: DensityFunctionRef = Interned::new(arena.alloc(seed_unit));

    let noise = unit(arena, "minecraft:overworld/noise", |n| {
        let base = n.constant("base", VariableType::F64, 0.25f64);
        let scale = n.constant(
            "scale",
            VariableType::F64,
            Expression::Variable(base) * 2.0f64,
        );
        let perm = Expression::PermutationTable(PermutationTableInput::PerlinNoise {
            ident: "minecraft:temperature".into(),
            subident: Some("first".into()),
            subident_index: 0,
        });
        vec![Statement::Return(
            Expression::ExternCall {
                function_name: "sample".into(),
                parameters: vec![perm, 1.0f64.into()],
                parameter_types: vec![VariableType::PermutationTable, VariableType::F64],
                return_type: VariableType::F64,
            } * Expression::Variable(scale),
        )]
    });
    let shared = unit(arena, "minecraft:shared", |b| {
        let seed_in = b.density(seed_unit, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), (1, 1, 1));
        vec![Statement::Return(seed_in.get() + 1.0f64)]
    });
    let leaf = unit(arena, "minecraft:leaf", |_| {
        vec![Statement::Return(2.0f64.into())]
    });
    let top = unit(arena, "minecraft:overworld/final_density", |b| {
        let s = b.density(shared, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), (4, 4, 4));
        let n = b.density(noise, (1.0, 1.0, 1.0), (0.5, 0.5, 0.5), (2, 2, 2));
        let l = b.density(leaf, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), (4, 4, 4));
        let x = b.local("x", VariableType::F64);
        vec![
            Statement::Assign {
                target: x,
                value: s.get() + n.at(0i32),
            },
            Statement::If {
                condition: Expression::Variable(x).gt(0.0f64),
                then_branch: vec![Statement::Assign {
                    target: x,
                    value: helper.call([Expression::Variable(x)]),
                }],
                else_branch: vec![Statement::Assign {
                    target: x,
                    value: l.get(),
                }],
            },
            Statement::Return(Expression::Variable(x) * s.get()),
        ]
    });
    SPMT {
        density_functions: vec![seed_unit, noise, shared, leaf, top],
        functions: vec![helper],
        main_density_functions: vec![MainDensityFunction {
            density_function: top,
            dimensions: (4, 4, 4),
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (1.0, 1.0, 1.0),
        }],
    }
}

fn round_trips(m: &OptimizedProgram, what: &str) {
    let text = print(m);
    let t = load(&text, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{what}: {d}\n{text}"));
    assert!(
        isomorphic(&t.program, &compact(m)),
        "P1 failed for {what}\n{text}"
    );
    assert_eq!(print(&t.program), text, "P2 failed for {what}");
}

#[test]
fn converted_programs_round_trip() {
    let arena = Bump::new();
    let mut m = convert(&build(&arena)).expect("conversion");
    let text = print(&m);
    assert!(
        text.contains("`minecraft:overworld/final_density`"),
        "{text}"
    );
    round_trips(&m, "the converted program");
    let stats = tungsten_optimizer::perform_inline(&mut m);
    assert!(stats.inlined > 0, "nothing was inlined");
    round_trips(&m, "the inlined program");
}
