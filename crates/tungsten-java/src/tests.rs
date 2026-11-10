//! Smoke test: a small program, as `.tw` text, exercising the Java-specific lowerings.
//! Set `TUNGSTEN_JAVA_OUT=<dir>` to also write the generated Java (and RCL) there, e.g. to run javac on it.

use tungsten_interface::naming::java_sanitize;
use tungsten_interface::{Backend, CompileError, Compiler, Interface};
use tungsten_ir::model::OptimizedProgram;
use tungsten_parse::{FileId, NoEnv, load};
use tungsten_rust::backend::{RustBackend, RustConfig};

use crate::backend::{JavaBackend, JavaCompile, JavaConfig};

fn program(src: &str) -> OptimizedProgram {
    load(src, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}")).program
}

fn java(program: &OptimizedProgram) -> String {
    let backend = JavaBackend::new(JavaConfig::default());
    let interface = Interface::build(program, &backend.naming_rules());
    backend.generate(program, &interface).unwrap().source
}

const SMALL: &str = r#"
program {
    host sea_level: i32;
    param seed: i64;
    auto temperature: handle PerlinNoiseSampler @name("minecraft:temperature_0_") = make_permutation_table(param(seed), 6664882324328353151L, -587597586455377528L, 0L, 0L, 0L);
    const base = 0.25;
    const scale = (const(base) * 2.0);
    const points = [-1.0, 0.0, nan];

    // Helper `int` (a Java keyword) with a parameter and a loop with a break.
    fn `int`(t: f64) -> f64 {
        vars { acc: f64 }
        acc = 0.0;
        repeat 3 {
            acc = (acc + t);
            if (acc > 10.0) {
                break;
            }
        }
        return acc;
    }

    // A 1x1x1 unit reading a host input.
    fn `minecraft:scalar`() -> i32 @hosts(sea_level) {
        vars { flag: bool }
        flag = (host(sea_level) > 62);
        // i32 + bool, `!` on an integer, logical shift.
        return (((host(sea_level) + flag) ^ !host(sea_level)) >>> 1);
    }

    // A 4x4x4 noise unit: constants (one depending on another), permutation table, an ambient read.
    fn `minecraft:noise`() -> f64 {
        vars { v: f64, bits: i64 }
        v = (ext floor::<(f64) -> f64>(ambient(pos3).x::<f64> * const(scale))
            + ext sin::<(f64) -> f64>(ext sample::<(handle PerlinNoiseSampler, f64) -> f64>(auto(temperature), ambient(pos3).x::<f64>)));
        bits = ((v as i64) * -5L);
        // `&mut v` on a primitive becomes a DoubleRef with a write-back.
        bits = ext nudge::<(f64) -> i64>(&mut v);
        return (v + const(points)[bits]);
    }

    // The 4x4x4 entry: reads both units, calls the helper, compares vectors.
    fn `minecraft:final`() -> f64 {
        vars { same: bool }
        same = (ambient(origin) == vec3(0.0, 0.0, 0.0));
        if same {
            return density(noise_in, 0);
        }
        return (`int`(density(noise_in)) + (density(scalar_in) as f64));
    }

    unit scalar { main `minecraft:scalar`; hosts [sea_level]; }
    unit noise { main `minecraft:noise`; consts [base, scale, points]; autos [temperature]; }
    input noise_in = noise @dims(4, 4, 4) @origin_scale(1, 1, 1) @position_scale(0.25, 0.25, 0.25);
    input scalar_in = scalar @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    unit final_u {
        main `minecraft:final`;
        helpers [`int`];
        inputs [noise_in, scalar_in];
    }
    entry final_u @dims(4, 4, 4) @origin_scale(1, 1, 1) @position_scale(0.25, 0.25, 0.25);
}
"#;

#[test]
fn generates_java_for_small_program() {
    let program = program(SMALL);
    let java = java(&program);

    if let Ok(dir) = std::env::var("TUNGSTEN_JAVA_OUT") {
        let dir = std::path::Path::new(&dir).join("tungsten/generated");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("GeneratedWorldgen.java"), &java).unwrap();
        // The Rust output for the same program, to compare against.
        let rust = RustBackend::new(RustConfig::default());
        let rust_interface = Interface::build(&program, &rust.naming_rules());
        let rust_out = rust.generate(&program, &rust_interface).unwrap();
        std::fs::write(dir.join("../../rcl_new.rs"), &rust_out.full_source).unwrap();
    }

    for expected in [
        "package tungsten.generated;",
        "import tungsten.runtime.*;",
        "public final class GeneratedWorldgen {",
        // keyword-named helper is renamed
        "static double int_1(",
        // constants are qualified and ordered by dependency
        "static final double base = 0.25;",
        "static final double scale = GeneratedWorldgen.base * 2.0;",
        "Double.NaN",
        // Java-specific lowerings
        ">>>",
        "(~host_consts.sea_level)",
        "(flag ? 1 : 0)",
        "final DoubleRef v_ref = new DoubleRef(v);",
        "v = v_ref.value;",
        "config.origin.equals(new Vec3(0.0, 0.0, 0.0))",
        "Helpers.sample(auto_constants.",
        "host_consts.sea_level",
        "buffers.minecraft_noise_d4x4x4_output[p.index]",
        "public double[] orchestrate_minecraft_final(double x, double y, double z, HostConsts host_consts)",
    ] {
        assert!(java.contains(expected), "missing `{}` in:\n{}", expected, java);
    }
    assert!(
        java.find("static final double base").unwrap() < java.find("static final double scale").unwrap(),
        "constants must be emitted in dependency order"
    );
}

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

#[test]
fn bytes() {
    let java = java(&program(BYTES));
    for e in [
        "static final byte[] table = new byte[]{(byte) 1, (byte) (-2), (byte) 127};",
        "final byte[] minecraft_bytes_d4x4x4_output = new byte[64];",
        "static byte minecraft_bytes_d4x4x4(",
        // Java promotes bytes to int; results are narrowed back.
        "a = (byte) (GeneratedWorldgen.table[2] + ((byte) 127));",
        "(((byte) (-a)) & ((byte) (a << 1)))",
        "wide = (int) (((int) a) & 255);",
        // Saturating like Rust `as i8`, not wrapping like `(byte)`.
        "a = (byte) Math.max((-128), Math.min(127, (int) d));",
        "return (byte) (a + ((byte) wide));",
    ] {
        assert!(java.contains(e), "missing `{}` in:\n{}", e, java);
    }
}

/// A unit reading all four ambients.
const AMBIENTS: &str = r#"
program {
    fn `minecraft:ambients`() -> f64 {
        return ((ambient(origin).x * ambient(origin_scale).x)
            + (ambient(pos3).x::<f64> * ambient(position_scale).x));
    }
    unit ambients { main `minecraft:ambients`; }
    entry ambients @dims(4, 4, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

#[test]
fn ambients() {
    let java = java(&program(AMBIENTS));
    for e in ["config.origin.x", "config.origin_scale.x", "config.position_scale.x", "p.pos.x"] {
        assert!(java.contains(e), "missing `{e}` in:\n{java}");
    }
    assert!(!java.contains("Vec3 origin"), "{java}");
}

fn empty_registry() -> tungsten_check::Registry {
    tungsten_check::RegistryBuilder::new().build().unwrap()
}

#[test]
fn compiler_matches_low_level_path() {
    let registry = empty_registry();
    let out = Compiler::new(program(BYTES), registry, JavaBackend::new(JavaConfig::default()))
        .compile()
        .unwrap();
    assert_eq!(out.source, java(&program(BYTES)));
}

#[test]
fn every_setter_sets_its_field() {
    let registry = empty_registry();
    let c = Compiler::new(program(BYTES), registry, JavaBackend::new(JavaConfig::default()))
        .package("p.q")
        .class_name("x y")
        .runtime_package("rt");
    let config = c.backend().config();
    assert_eq!(
        (config.package.as_str(), config.class_name.as_str(), config.runtime_package.as_str()),
        ("p.q", "x y", "rt")
    );
    // The sanitized class name is reserved in the naming rules.
    assert!(c.backend().naming_rules().used_set().contains(&java_sanitize("x y")));
    // Sanitizing is a warning, so it only fails the compile under `deny_warnings`.
    assert!(matches!(c.deny_warnings().compile(), Err(CompileError::Warnings { .. })));
}

#[test]
fn autoconst_arguments_and_values() {
    let src = r#"
program {
    param seed: i64;
    param scale: f64;
    param buffers: bool;
    auto t: handle Table = make_table(param(seed), -9223372036854775808L, 1.5, -0.0, true, "q\"\\ \u{e9} \u{1d11e}\n");
    auto v: f64 = make_v(param(scale), param(buffers), nan, inf, -inf);
    auto i: i64 = make_i(7L, false);
    fn run() -> f64 {
        return (ext sample::<(handle Table, f64) -> f64>(auto(t), auto(v)) + (auto(i) as f64));
    }
    unit u { main run; autos [t, v, i]; }
    entry u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;
    let java = java(&program(src));
    for expected in [
        "public final Table t;",
        "public final double v;",
        "public static AutoConsts create(long seed, double scale, boolean buffers_1)",
        // handles and values are read the same way
        "Helpers.sample(auto_constants.t, auto_constants.v)",
        // quotes, backslashes and line breaks keep their own escapes; the rest are UTF-16 units
        "Helpers.make_table(seed, Long.MIN_VALUE, 1.5, (-0.0), true, \"q\\\"\\\\ \\u00e9 \\ud834\\udd1e\\n\")",
        "Helpers.make_v(scale, buffers_1, Double.NaN, Double.POSITIVE_INFINITY, Double.NEGATIVE_INFINITY)",
        "Helpers.make_i(7L, false)",
    ] {
        assert!(java.contains(expected), "missing `{expected}` in:\n{java}");
    }
}
