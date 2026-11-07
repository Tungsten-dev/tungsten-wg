//! Id assignment (README.md, "build_ids").

use tungsten_core::{Expr, InputId, LocalId, Stmt, Type, UnitId};

use super::*;

/// Params first (in `parameters` order, not slot order), then the other vars in slot order.
#[test]
fn locals() {
    let mut p = empty();
    let f = function(
        "f",
        &[
            ("t", VariableType::I32),
            ("b", VariableType::F32),
            ("a", VariableType::F64),
            ("u", VariableType::I64),
        ],
        &[2, 1],
        VariableType::F64,
        |v| {
            vec![
                Statement::Assign {
                    target: v[3],
                    value: Expression::Long(1),
                },
                Statement::Return(Expression::Variable(v[2])),
            ]
        },
    );
    let fr = p.functions.insert(f);
    p.compute_units.insert(unit(fr));
    let l = ok(&p, &registry());
    let f = &l.program.functions[0];
    assert_eq!(f.params, 2);
    assert_eq!(f.locals, vec![Type::F64, Type::F32, Type::I32, Type::I64]);
    assert_eq!(
        f.body,
        vec![
            Stmt::Assign {
                target: LocalId(3),
                value: Expr::Lit(tungsten_core::Lit::I64(1))
            },
            Stmt::Return(Expr::Local(LocalId(0))),
        ]
    );
    let names: Vec<&str> = l.map.locals[0].iter().map(|(_, _, n)| n).collect();
    assert_eq!(names, ["a", "b", "t", "u"]);
}

/// Inputs: units in order, then each unit's arena in order. A shared helper's density read
/// names exactly the input it refers to, and only the unit that lists it.
#[test]
fn inputs_across_units() {
    let src = "program {
        fn leaf() -> f64 { return 1.0; }
        fn shared() -> f64 { return density(b.y); }
        fn amain() -> f64 { return (density(x) + density(y)); }
        fn bmain() -> f64 { return shared(); }
        unit leaf_u { main leaf; }
        unit a {
            main amain;
            input x = leaf_u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
            input y = leaf_u @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
        }
        unit b {
            main bmain;
            helpers [shared];
            input y = leaf_u @dims(3, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
        }
        entry e = a @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    }";
    let l = ok(&load(src), &registry());
    let pr = &l.program;
    let unit_id = |n: &str| l.map.units.iter().find(|(_, _, m)| *m == n).unwrap().0;
    let (a, b) = (unit_id("amain"), unit_id("bmain"));
    assert!(a < b);
    assert_eq!(pr.units[a.0].inputs, vec![InputId(0), InputId(1)]);
    assert_eq!(pr.units[b.0].inputs, vec![InputId(2)]);
    assert_eq!(
        pr.inputs.iter().map(|i| i.dims.x).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        pr.inputs.iter().map(|i| i.source).collect::<Vec<_>>(),
        vec![UnitId(0); 3]
    );
    let shared = &pr.functions[fn_id(&l, "shared").0];
    assert_eq!(
        shared.body,
        vec![Stmt::Return(Expr::Density {
            input: InputId(2),
            index: None
        })]
    );
    assert!(pr.units.iter().enumerate().all(|(u, unit)| unit.inputs.contains(&InputId(2)) == (u == b.0)));
    let names: Vec<&str> = l.map.inputs.iter().map(|(_, _, n)| n).collect();
    assert_eq!(names, ["x", "y", "y"]);
}

/// Holes vanish: ids stay dense.
#[test]
fn holes() {
    let mut p = empty();
    let gone = p.functions.insert(function("gone", &[], &[], VariableType::F64, |_| vec![]));
    let keep = p.functions.insert(function("keep", &[], &[], VariableType::F64, |_| vec![]));
    p.functions.remove(gone);
    let u = p.compute_units.insert(unit(keep));
    p.entry_points.push(entry(u));
    let l = ok(&p, &registry());
    assert_eq!(l.program.functions.len(), 1);
    assert_eq!(l.program.units[0].main, tungsten_core::FnId(0));
    assert_eq!(l.map.functions.name(tungsten_core::FnId(0)), Some("keep"));
}
