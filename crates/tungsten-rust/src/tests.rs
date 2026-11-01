//! Smoke tests: small hand-built programs exercising the Rust lowerings.

use bumpalo::Bump;

use tungsten_interface::{Backend, Interface};
use tungsten_legacy::builder::{Builder, SPMTBuilder};
use tungsten_legacy::conversion::convert;
use tungsten_legacy::model::*;

use crate::backend::{RustBackend, RustConfig, RustOutput};

fn var<'m>(v: Var<'m>) -> Expression<'m> {
    Expression::Variable(v)
}

/// A 4x4x4 unit returning `I8`: byte constants, wrapping arithmetic, bit operations and casts.
fn build_byte_program<'m>(arena: &'m Bump) -> SPMT<'m> {
    let b = Builder::new(arena).named("minecraft:bytes").returns(VariableType::I8);
    let table = b.constant(
        "table",
        VariableType::Array(Box::new(VariableType::I8), 3),
        Expression::ArrayLiteral(vec![1i8.into(), (-2i8).into(), 127i8.into()]),
    );
    let a = b.local("a", VariableType::I8);
    let wide = b.local("wide", VariableType::I32);
    let d = b.local("d", VariableType::F64);
    let mut b = b;
    b.set_body(vec![
        // 127 + 127 wraps
        Statement::Assign { target: a, value: var(table).index(2i32) + 127i8 },
        Statement::Assign { target: a, value: (-var(a)).bitand(var(a).shl(1i32)) },
        Statement::Assign { target: wide, value: var(a).cast(VariableType::I32).bitand(255i32) },
        Statement::Assign { target: d, value: 300.5f64.into() },
        Statement::Assign { target: a, value: var(d).cast(VariableType::I8) },
        Statement::Return(var(a) + var(wide).cast(VariableType::I8)),
    ]);
    let unit = b.finish_compute_unit();
    SPMT {
        density_functions: vec![unit],
        functions: vec![],
        main_density_functions: vec![MainDensityFunction {
            density_function: unit,
            dimensions: (4, 4, 4),
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (1.0, 1.0, 1.0),
        }],
    }
}

fn generate(program: &SPMT<'_>) -> RustOutput {
    let program = convert(program).expect("conversion");
    let mut backend = RustBackend::new(RustConfig::default());
    let interface = Interface::build(&program, backend.naming_rules());
    backend.generate(&program, &interface).unwrap();
    backend.output().expect("rust output").clone()
}

fn assert_contains(text: &str, expected: &[&str]) {
    for e in expected {
        assert!(text.contains(e), "missing `{}` in:\n{}", e, text);
    }
}

#[test]
fn bytes() {
    let arena = Bump::new();
    let output = generate(&build_byte_program(&arena));
    assert_contains(&output.full_source, &[
        "const table: [i8; 3] = [1_i8, -2_i8, 127_i8];",
        "-> i8 {",
        "let mut a: i8;",
        "a = (table[2_i32 as usize] + 127_i8);",
        "a = ((-a) & (a << 1_i32));",
        "wide = ((a as i32) & 255_i32);",
        "a = (d as i8);",
        "return (a + (wide as i8));",
        "pub minecraft_bytes_d4x4x4_output: [i8; 64],",
    ]);
}
