//! Programs from `tungsten-legacy`'s builders, after `convert`, lower against a registry without
//! errors. (The program is the one `tungsten-parse/tests/legacy.rs` round-trips.)

use bumpalo::Bump;
use tungsten_legacy::builder::{Builder, SPMTBuilder};
use tungsten_legacy::conversion::convert;
use tungsten_legacy::model::*;

use super::eval::{Toy, eval_registry};
use super::registry;
use crate::{EvalOptions, NamedCase, prepare};
use tungsten_core::Value;

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
    let top = unit(arena, "minecraft:overworld/final_density", |b| {
        let s = b.density(shared, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), (4, 4, 4));
        let n = b.density(noise, (1.0, 1.0, 1.0), (0.5, 0.5, 0.5), (2, 2, 2));
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
                else_branch: vec![],
            },
            Statement::Return(Expression::Variable(x) * s.get()),
        ]
    });
    SPMT {
        density_functions: vec![seed_unit, noise, shared, top],
        functions: vec![helper],
        main_density_functions: vec![MainDensityFunction {
            density_function: top,
            dimensions: (4, 4, 4),
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (1.0, 1.0, 1.0),
        }],
    }
}

#[test]
fn converted_program_lowers() {
    let arena = Bump::new();
    let p = convert(&build(&arena)).expect("conversion");
    let l = super::ok(&p, &registry());
    assert_eq!(l.map.externs, vec!["sin".to_string(), "sample".to_string()]);
    assert_eq!(l.program.entries.len(), 1);
    assert_eq!(l.program.inputs.len(), 3);
}

fn field<'m>(v: Var<'m>, name: &str, t: VariableType) -> Expression<'m> {
    Expression::Field {
        base: Box::new(Expression::Variable(v)),
        field: name.into(),
        type_of_field: t,
        known_idnex: None,
    }
}

/// Runs a converted one-entry program on a 2x1x1 grid with `origin = (10, 20, 30)`.
fn eval_cells(spmt: &SPMT<'_>) -> Vec<f64> {
    let p = convert(spmt).expect("conversion");
    let prep = prepare(&p, &eval_registry()).unwrap_or_else(|e| panic!("{e}"));
    let case = prep
        .case(&NamedCase { origin: [10.0, 20.0, 30.0], hosts: vec![] })
        .unwrap();
    let r = prep.eval(&case, &Toy(&prep), EvalOptions::default()).unwrap();
    r.entries[0]
        .iter()
        .map(|c| match c {
            Ok(Value::F64(b)) => f64::from_bits(*b),
            other => panic!("expected a double, found {other:?}"),
        })
        .collect()
}

fn entry<'m>(unit: DensityFunctionRef<'m>) -> SPMT<'m> {
    SPMT {
        density_functions: vec![unit],
        functions: vec![],
        main_density_functions: vec![MainDensityFunction {
            density_function: unit,
            dimensions: (2, 1, 1),
            scaled_origin: (2.0, 1.0, 1.0),
            scaled_position: (0.5, 1.0, 1.0),
        }],
    }
}

/// The four default variables pass the flow check (they used to be uninitialized locals) and
/// evaluate to the context: `origin.x * origin_scale.x + pos3.x * position_scale.x`.
#[test]
fn default_variables_are_ambients() {
    let arena = Bump::new();
    let mut b = Builder::new(&arena).named("u").returns(VariableType::F64);
    let origin = b.alloc_var(Name::Named("origin".into()), VariableType::Vec3);
    let origin_scale = b.alloc_var(Name::Named("origin_scale".into()), VariableType::Vec3);
    let position_scale = b.alloc_var(Name::Named("position_scale".into()), VariableType::Vec3);
    let pos3 = b.local("pos3", VariableType::Pos3);
    b.set_body(vec![Statement::Return(
        field(origin, "x", VariableType::F64) * field(origin_scale, "x", VariableType::F64)
            + Expression::ExplicitCast {
                to: VariableType::F64,
                expr: Box::new(field(pos3, "x", VariableType::I32)),
            } * field(position_scale, "x", VariableType::F64),
    )]);
    // 10 * 2 + x * 0.5
    assert_eq!(eval_cells(&entry(b.finish_compute_unit())), vec![20.0, 20.5]);
}

/// `aquifer_random` in `tungsten-mc-compile` creates `origin` as a `Pos3`. The converter resolves
/// it to the `Vec3` ambient, so the field read must be typed from the ambient to pass the checker.
#[test]
fn mistyped_origin_passes_the_checker() {
    let arena = Bump::new();
    let mut b = Builder::new(&arena).named("u").returns(VariableType::F64);
    let origin = b.alloc_var(Name::Named("origin".into()), VariableType::Pos3);
    b.set_body(vec![Statement::Return(field(origin, "z", VariableType::F64))]);
    assert_eq!(eval_cells(&entry(b.finish_compute_unit())), vec![30.0, 30.0]);
}

/// `water_levels` in `tungsten-mc-compile` moves `pos3` partway through its body, and other code
/// reads it through a second variable of the same name. Before the assignment it is the cell.
#[test]
fn assigned_pos3_starts_as_the_ambient() {
    let arena = Bump::new();
    let mut b = Builder::new(&arena).named("u").returns(VariableType::F64);
    let written = b.local("pos3", VariableType::Pos3);
    let read = b.alloc_var(Name::Named("pos3".into()), VariableType::Pos3);
    let before = b.local("before", VariableType::I32);
    let x = || field(read, "x", VariableType::I32);
    b.set_body(vec![
        Statement::Assign { target: before, value: x() },
        Statement::Assign {
            target: written,
            value: Expression::Construct {
                t: VariableType::Pos3,
                args: vec![7i32.into(), 0i32.into(), 0i32.into()],
            },
        },
        Statement::Return(Expression::ExplicitCast {
            to: VariableType::F64,
            expr: Box::new(Expression::Variable(before) * 10i32 + x()),
        }),
    ]);
    // cell.x * 10 + 7
    assert_eq!(eval_cells(&entry(b.finish_compute_unit())), vec![7.0, 17.0]);
}
