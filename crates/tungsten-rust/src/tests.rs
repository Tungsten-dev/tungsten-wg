//! Smoke tests: small programs, written as `.tw` text, exercising the Rust lowerings.

use tungsten_interface::{Backend, Interface};
use tungsten_parse::{FileId, NoEnv, load};

use crate::backend::{RustBackend, RustConfig, RustOutput};

/// A 4x4x4 unit returning `I8`: byte constants, wrapping arithmetic, bit operations and casts.
const BYTES: &str = r#"
program {
    const `table` = [1b, -2b, 127b];
    fn `minecraft:bytes`() -> i8 {
        vars { a: i8, wide: i32, d: f64 }
        // 127 + 127 wraps
        a = (const(`table`)[2] + 127b);
        a = (-a & (a << 1));
        wide = ((a as i32) & 255);
        d = 300.5;
        a = (d as i8);
        return (a + (wide as i8));
    }
    unit bytes { main `minecraft:bytes`; consts [`table`]; }
    entry bytes @dims(4, 4, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

/// A unit reading all four ambients.
const AMBIENTS: &str = r#"
program {
    fn `minecraft:ambients`() -> f64 {
        return ((ambient(origin).x * ambient(origin_scale).x)
            + ((ambient(pos3).x as f64) * ambient(position_scale).x));
    }
    unit ambients { main `minecraft:ambients`; }
    entry ambients @dims(4, 4, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

fn generate(src: &str) -> RustOutput {
    let program = load(src, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}")).program;
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
    let output = generate(BYTES);
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

#[test]
fn ambients() {
    let output = generate(AMBIENTS);
    assert_contains(&output.full_source, &[
        "config.origin.x",
        "config.origin_scale.x",
        "config.position_scale.x",
        "p.pos.x",
    ]);
    assert!(!output.full_source.contains("let mut origin"), "{}", output.full_source);
}
