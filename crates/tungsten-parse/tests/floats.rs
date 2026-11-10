//! P6: float literals print and re-parse bit-identically, for f32 and f64.

mod common;

use common::Rng;
use tungsten_ir::{ConstExpr, OptimizedProgram};
use tungsten_parse::*;

fn consts(p: &OptimizedProgram) -> Vec<ConstExpr> {
    p.constants.values().map(|c| c.value.clone()).collect()
}

fn roundtrip(values: Vec<ConstExpr>) {
    let src = format!(
        "program {{\n{}\n}}",
        values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let mut p = OptimizedProgram {
                    compute_units: tungsten_ir::Arena::new(),
                    functions: tungsten_ir::Arena::new(),
                    entry_points: vec![],
                    constants: tungsten_ir::Arena::new(),
                    host_inputs: tungsten_ir::Arena::new(),
                    permutation_tables: tungsten_ir::Arena::new(),
                };
                p.constants.insert(tungsten_ir::Constant {
                    value: v.clone(),
                    name: format!("c{i}"),
                });
                print(&p)
            })
            .map(|s| s
                .lines()
                .filter(|l| l.contains("const "))
                .collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    );
    let t = load(&src, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}\n{src}"));
    for (want, got) in values.iter().zip(consts(&t.program)) {
        let same = match (want, &got) {
            (ConstExpr::Double(a), ConstExpr::Double(b)) => a.to_bits() == b.to_bits(),
            (ConstExpr::Float(a), ConstExpr::Float(b)) => a.to_bits() == b.to_bits(),
            _ => false,
        };
        assert!(same, "{want:?} came back as {got:?}\n{src}");
    }
}

#[test]
fn f64_edge_values() {
    let vals = [
        0.0,
        -0.0,
        f64::MIN_POSITIVE,
        5e-324,
        -5e-324,
        f64::MAX,
        f64::MIN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::EPSILON,
        0.1,
        1e16,
        1e-7,
        123456789.0,
        f64::from_bits(0x7ff8_0000_0000_0000),
        f64::from_bits(0x7ff8_0000_0000_0001),
        f64::from_bits(0xfff8_0000_0000_0000),
        f64::from_bits(0xfff0_0000_0000_0001),
        f64::from_bits(0x7ff0_0000_0000_0001),
    ];
    roundtrip(vals.iter().map(|v| ConstExpr::Double(*v)).collect());
}

#[test]
fn f32_edge_values() {
    let vals = [
        0.0f32,
        -0.0,
        f32::MIN_POSITIVE,
        1e-45,
        f32::MAX,
        f32::MIN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::EPSILON,
        0.1,
        1e16,
        f32::from_bits(0x7fc0_0000),
        f32::from_bits(0x7fc0_0001),
        f32::from_bits(0xffc0_0000),
        f32::from_bits(0xff80_0001),
    ];
    roundtrip(vals.iter().map(|v| ConstExpr::Float(*v)).collect());
}

#[test]
fn point_one_differs_by_type() {
    let t = load(
        "program { const a = 0.1; const b = 0.1f; const c = 0.1_f64; const d = 0.1_f32; }",
        FileId(0),
        &NoEnv,
    )
    .unwrap();
    let v = consts(&t.program);
    assert_eq!(v[0], ConstExpr::Double(0.1));
    assert_eq!(v[1], ConstExpr::Float(0.1));
    assert_eq!(v[2], ConstExpr::Double(0.1));
    assert_eq!(v[3], ConstExpr::Float(0.1));
    let printed = print(&t.program);
    assert!(printed.contains("const a = 0.1;"), "{printed}");
    assert!(printed.contains("const b = 0.1f;"), "{printed}");
}

#[test]
fn random_bit_patterns() {
    let mut rng = Rng::new(99);
    let mut v64 = vec![];
    let mut v32 = vec![];
    for _ in 0..20_000 {
        v64.push(ConstExpr::Double(f64::from_bits(rng.next())));
        v32.push(ConstExpr::Float(f32::from_bits(rng.next() as u32)));
    }
    // a few hundred per program keeps the sources readable when something fails
    for chunk in v64.chunks(500).chain(v32.chunks(500)) {
        roundtrip(chunk.to_vec());
    }
}

#[test]
fn infinite_literals_must_be_spelled_inf() {
    let e = load("program { const a = 1e999; }", FileId(0), &NoEnv)
        .err()
        .unwrap();
    assert!(e.message.contains("inf"), "{e}");
    let e = load("program { const a = 1e50f; }", FileId(0), &NoEnv)
        .err()
        .unwrap();
    assert!(e.message.contains("inf"), "{e}");
}
