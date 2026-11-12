//! Smoke test: a small program, as `.tw` text, exercising the Java-specific lowerings.
//! Set `TUNGSTEN_JAVA_OUT=<dir>` to also write the generated Java (and RCL) there, e.g. to run javac on it.

use tungsten_interface::naming::java_sanitize;
use tungsten_interface::{Backend, BackendError, BufferBounds, CompileError, Compiler, Interface};
use tungsten_ir::model::Program;
use tungsten_parse::{FileId, NoEnv, load};
use tungsten_rust::backend::{RustBackend, RustConfig};

use crate::backend::{JavaBackend, JavaCompile, JavaConfig};

fn program(src: &str) -> Program {
    load(src, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}")).program
}

fn java(program: &Program) -> String {
    java_with(program, BufferBounds::PerBuffer)
}

fn java_with(program: &Program, bounds: BufferBounds) -> String {
    let backend = JavaBackend::new(JavaConfig { buffer_bounds: bounds, ..JavaConfig::default() });
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
        // a read of an input: its pool, its offset parameter and a check against its own length
        "buffers.f64s[off_noise_in + java.util.Objects.checkIndex(p.index, 64)]",
        // an explicit index of 0 into a non-empty input can't fail
        "buffers.f64s[off_noise_in + 0]",
        "public double[] run(double x, double y, double z, MinecraftFinalHostConsts hc)",
        "public MinecraftFinalWorker minecraftFinalWorker()",
        "public static final int MINECRAFT_FINAL_LEN = 64;",
        "hosts.sea_level = hc.sea_level;",
        "return buffers.f64s;",
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
        "final byte[] i8s;",
        "this.i8s = new byte[len_i8s];",
        "return buffers.i8s;",
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
        .runtime_package("rt")
        .buffer_bounds(BufferBounds::Slab);
    let config = c.backend().config();
    assert_eq!(
        (config.package.as_str(), config.class_name.as_str(), config.runtime_package.as_str()),
        ("p.q", "x y", "rt")
    );
    assert_eq!(config.buffer_bounds, BufferBounds::Slab);
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
        "final Table t;",
        "final double v;",
        "public static Orchestration create(long seed, double scale, boolean buffers_1)",
        "new AutoConsts(seed, scale, buffers_1)",
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

/// Two entries share `mid`, which reads `leaf` through a helper. Entry `a` reads `mid` and the
/// f64 `side`; entry `b` reads only `mid`, so `mid` lands at different offsets in the two workers.
/// Entry `a` reads host `ha`, entry `b` reads `hb` and `big`, a `[f64; 33]` host.
const SHARED: &str = r#"
program {
    host ha: f64;
    host hb: i32;
    host big: [f64; 33];
    fn leaf_main() -> i32 { return ambient(pos3).x; }
    unit leaf { main leaf_main; }
    input leaf_in = leaf @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    fn get_leaf() -> i32 { return density(leaf_in); }
    fn mid_main() -> i32 { return (get_leaf() + 1); }
    unit mid { main mid_main; helpers [get_leaf]; inputs [leaf_in]; }
    input mid_in = mid @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    fn side_main() -> f64 { return host(ha); }
    unit side { main side_main; hosts [ha]; }
    input side_in = side @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    fn a_main() -> f64 { return (density(side_in) + (density(mid_in) as f64)); }
    unit a { main a_main; inputs [side_in, mid_in]; }
    fn b_main() -> i32 { return ((density(mid_in) + host(hb)) + (host(big)[0] as i32)); }
    unit b { main b_main; hosts [hb, big]; inputs [mid_in]; }
    entry a @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    entry b @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

/// Expand and lerp inputs over one leaf, read by one unit (`specs/density-mappings.md` §5.2).
const MAPPED: &str = r#"
program {
    fn leaf() -> f64 { return (ambient(pos3).x as f64); }
    unit leaf_u { main leaf; }
    input e = leaf_u @dims(4, 1, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @expand(4, *, 4);
    input l = leaf_u @dims(5, 49, 5) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @lerp(4, 8, 3);
    input m = leaf_u @dims(3, 1, 3) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @lerp(2, *, 1);
    fn top() -> f64 { return ((density(e) + density(l)) + density(m)); }
    unit t { main top; inputs [e, l, m]; }
    entry t @dims(16, 4, 16) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

/// `specs/refinement.md` §6.2: the worker stores the origin of `run`, a refine call takes a `Pos3`
/// offset, and its kernel moves `pos` and not `idx`.
const ACCUMULATOR: &str = r#"
program {
    fn seed_main() -> i32 { return (ambient(pos3).x * 10); }
    unit seed { main seed_main; }
    input prev = seed @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @previous;
    fn acc_main() -> i32 { return (density(prev) + 1); }
    unit acc { main acc_main; inputs [prev]; }
    entry seed @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    refinement step of seed @march(0, 3, 0) {
        rerun seed @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) with acc;
    }
}
"#;

fn assert_contains(text: &str, expected: &[&str]) {
    for e in expected {
        assert!(text.contains(e), "missing `{e}` in:\n{text}");
    }
}

#[test]
fn orchestration_and_workers() {
    let java = java(&program(SHARED));
    assert_contains(&java, &[
        "static final class HostConsts {\n        double ha;",
        "static final class InternalBuffers {\n        final double[] f64s;\n        final int[] i32s;",
        "InternalBuffers(int len_f64s, int len_i32s) {",
        "this.i32s = new int[len_i32s];",
        // global and immutable, and creating workers is its only job
        "public static final class Orchestration {\n        private final AutoConsts auto_constants;",
        "public static Orchestration create() {",
        // one worker and one host class per entry, each with only its own hosts
        "public static final class AMainHostConsts {\n        public double ha;\n    }",
        "public static final class BMainHostConsts {\n        public int hb;\n        public double[] big;\n    }",
        "public AMainWorker aMainWorker() {\n            return new AMainWorker(this.auto_constants, new InternalBuffers(5, 8));",
        "public BMainWorker bMainWorker() {\n            return new BMainWorker(this.auto_constants, new InternalBuffers(0, 9));",
        "public static final int A_MAIN_LEN = 4;",
        // the pool array, and a single-cell entry's value
        "public double[] run(double x, double y, double z, AMainHostConsts hc) {",
        "hosts.ha = hc.ha;",
        "return buffers.f64s;",
        "public int run(double x, double y, double z, BMainHostConsts hc) {",
        "hosts.big = hc.big;",
        "return buffers.i32s[0];",
        // `mid` is at i32 offset 0 in `a`'s worker and 1 in `b`'s, reading `leaf` after it
        "GeneratedWorldgen.mid_main_d4x1x1_kernel(x, y, z, buffers, 0, 4, auto_constants, hosts);",
        "GeneratedWorldgen.mid_main_d4x1x1_kernel(x, y, z, buffers, 1, 5, auto_constants, hosts);",
    ]);
    assert!(!java.contains("orchestrate_") && !java.contains("AutoConsts create"), "{java}");
    assert!(!java.contains("public static final class HostConsts"));
}

/// Helpers take the offsets of what they read, and callers pass their own through.
#[test]
fn offsets_pass_through_helpers() {
    let java = java(&program(SHARED));
    assert_contains(&java, &[
        "static int get_leaf(PositionIterator p, Config config, InternalBuffers buffers, AutoConsts auto_constants, HostConsts host_consts, int off_leaf_in)",
        "return buffers.i32s[off_leaf_in + java.util.Objects.checkIndex(p.index, 4)];",
        "get_leaf(p, config, buffers, auto_constants, host_consts, off_leaf_in)",
        "static void mid_main_d4x1x1_kernel(double origin_x, double origin_y, double origin_z, InternalBuffers buffers, int out_off, int off_leaf_in, AutoConsts auto_constants, HostConsts host_consts)",
        "out[out_off + idx] = mid_main_d4x1x1(p, config, buffers, auto_constants, host_consts, off_leaf_in);",
        // a (1, 1, 1) input reads cell 0
        "buffers.f64s[off_side_in + 0]",
    ]);
}

#[test]
fn bounds_modes() {
    let p = program(SHARED);
    let per_buffer = java(&p);
    assert!(per_buffer.contains("java.util.Objects.checkIndex(p.index, 4)"));
    // `Slab` indexes the pool, and the JVM checks its end. `Unchecked` is the same on Java.
    for bounds in [BufferBounds::Slab, BufferBounds::Unchecked] {
        let text = java_with(&p, bounds);
        assert!(text.contains("buffers.i32s[off_leaf_in + p.index]"), "{text}");
        assert!(!text.contains("checkIndex"), "{text}");
    }
    assert_eq!(java_with(&p, BufferBounds::Slab), java_with(&p, BufferBounds::Unchecked));
}

/// An expand read is an index folded from the factors: shifts for powers of two, no term for
/// `Repeat`, and no helper.
#[test]
fn expand_is_an_inline_index() {
    let text = java(&program(MAPPED));
    assert_contains(&text, &["buffers.f64s[off_e + java.util.Objects.checkIndex((p.pos.x >> 2) + ((p.pos.z >> 2) * 4), 16)]"]);
    let src = r#"
program {
    fn leaf() -> f64 { return 1.0; }
    unit leaf_u { main leaf; }
    input a = leaf_u @dims(5, 1, 2) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @expand(1, *, 3);
    input b = leaf_u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @expand(*, *, *);
    input c = leaf_u @dims(2, 3, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @expand(1, 1, *);
    fn top() -> f64 { return ((density(a) + density(b)) + density(c)); }
    unit t { main top; inputs [a, b, c]; }
    entry t @dims(5, 3, 6) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;
    assert_contains(&java(&program(src)), &[
        // factor 1 reads the coordinate, factor 3 divides, `Repeat` has no term
        "buffers.f64s[off_a + java.util.Objects.checkIndex(p.pos.x + ((p.pos.z / 3) * 5), 10)]",
        // all `Repeat` reads cell 0
        "buffers.f64s[off_b + 0]",
        "buffers.f64s[off_c + java.util.Objects.checkIndex(p.pos.x + (p.pos.y * 2), 6)]",
    ]);
}

/// One `private static` helper per shape, folded from dims and factors, loading and interpolating
/// in the order of §3.3, with no shortcut.
#[test]
fn lerp_is_a_helper_in_canonical_order() {
    let java = java(&program(MAPPED));
    assert_contains(&java, &[
        "lerp_5x49x5_by_4x8x3_f64(buffers.f64s, off_l, p.pos.x, p.pos.y, p.pos.z)",
        "private static double lerp_5x49x5_by_4x8x3_f64(double[] pool, int off, int px, int py, int pz) {",
        // a power of two shifts and masks, any other factor divides
        "final int x0 = px >> 2;\n        final double tx = ((double) (px & 3)) / 4.0;",
        "final int z0 = pz / 3;\n        final double tz = ((double) (pz % 3)) / 3.0;",
        "final double v000 = pool[off + java.util.Objects.checkIndex((x0 + (y0 * 5)) + (z0 * 245), 1225)];",
        // x first, then y, then z, each as a + t * (b - a)
        "final double lx00 = v000 + (tx * (v100 - v000));",
        "final double ly0 = lx00 + (ty * (lx10 - lx00));",
        "final double lz = ly0 + (tz * (ly1 - ly0));",
        // `Repeat` has no term, and no fraction
        "private static double lerp_3x1x3_by_2xrx1_f64(double[] pool, int off, int px, int py, int pz) {",
    ]);
    assert!(!java.contains("== 0.0"), "no shortcut at t == 0:\n{java}");
}

#[test]
fn lerp_helpers_are_emitted_once() {
    let src = r#"
program {
    fn leaf() -> f32 { return 1.0f; }
    unit leaf_u { main leaf; }
    input a = leaf_u @dims(3, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @lerp(2, *, *);
    input b = leaf_u @dims(3, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @lerp(2, *, *);
    fn top() -> f32 { return (density(a) + density(b)); }
    unit t { main top; inputs [a, b]; }
    entry t @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;
    let java = java(&program(src));
    assert_eq!(java.matches("private static float lerp_3x1x1_by_2xrxr_f32(").count(), 1, "{java}");
    assert_contains(&java, &["final float tx = ((float) (px & 1)) / 2.0f;"]);
}

#[test]
fn lerp_helper_follows_bounds_modes() {
    let p = program(MAPPED);
    let slab = java_with(&p, BufferBounds::Slab);
    assert!(slab.contains("final double v000 = pool[off + ((x0 + (y0 * 5)) + (z0 * 245))];"), "{slab}");
    assert!(!slab.contains("checkIndex"), "{slab}");
}

fn generate_err(src: &str) -> String {
    let p = program(src);
    let backend = JavaBackend::new(JavaConfig::default());
    let interface = Interface::build(&p, &backend.naming_rules());
    match backend.generate(&p, &interface) {
        Err(BackendError::Codegen(m)) => m,
        other => panic!("expected a codegen error, got {other:?}"),
    }
}

/// The low-level path doesn't validate; a backend refuses only what it can't emit (§5.3).
#[test]
fn unemittable_mappings_are_codegen_errors() {
    let source = |leaf_type: &str, ret: &str, mapping: &str| {
        format!(
            r#"
program {{
    fn leaf() -> {leaf_type} {{ return {ret}; }}
    unit leaf_u {{ main leaf; }}
    input d = leaf_u @dims(2, 2, 2) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) {mapping};
    fn top() -> {leaf_type} {{ return density(d); }}
    unit t {{ main top; inputs [d]; }}
    entry t @dims(2, 2, 2) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}}
"#
        )
    };
    let e = generate_err(&source("i32", "1", "@lerp(2, 2, 2)"));
    assert!(e.contains("`d`") && e.contains("not f32 or f64"), "{e}");
    let e = generate_err(&source("f64", "1.0", "@expand(0, 1, 1)"));
    assert!(e.contains("`d`") && e.contains("factor 0"), "{e}");
    let e = generate_err(&source("f64", "1.0", "@lerp(2, 0, 2)"));
    assert!(e.contains("factor 0"), "{e}");
    // The same shapes are fine where they can be emitted.
    java(&program(&source("f64", "1.0", "@lerp(2, 2, 2)")));
    java(&program(&source("i32", "1", "@expand(2, 2, 2)")));
}

/// A function with `n` inputs takes `n` offsets, and its kernel takes them too, after six slots of
/// origin and three of buffers, `out_off` and the two constants structs.
fn many_inputs(n: usize) -> String {
    let mut s = String::from("program {\n    fn leaf() -> f64 { return 1.0; }\n    unit leaf_u { main leaf; }\n");
    for i in 0..n {
        s += &format!("    input in{i} = leaf_u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);\n");
    }
    // One statement per read: a sum of 245 terms would nest too deeply for the test thread's stack.
    s += "    fn top() -> f64 {\n        vars { acc: f64 }\n        acc = 0.0;\n";
    for i in 0..n {
        s += &format!("        acc = (acc + density(in{i}));\n");
    }
    s += "        return acc;\n    }\n";
    let inputs: Vec<String> = (0..n).map(|i| format!("in{i}")).collect();
    s += &format!("    unit t {{ main top; inputs [{}]; }}\n", inputs.join(", "));
    s += "    entry t @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);\n}\n";
    s
}

/// The JVM allows 255 parameter slots (`specs/workers.md` D7). The kernel is the widest method:
/// 6 slots of origin, `buffers`, `out_off`, one per input, and the two constants structs.
#[test]
fn too_many_offsets_are_a_codegen_error() {
    java(&program(&many_inputs(245)));
    let e = generate_err(&many_inputs(246));
    assert!(e.contains("_kernel") && e.contains("256 parameter slots") && e.contains("255"), "{e}");
}

#[test]
fn refine_method_stores_the_origin_and_moves_pos_only() {
    let java = java(&program(ACCUMULATOR));
    assert_contains(&java, &[
        // The worker keeps the origin of its last run.
        "private double origin_x;",
        "this.origin_x = x;",
        // `refine_step` takes the offset, and its kernel too, after the origin.
        "public int[] refine_step(int ox, int oy, int oz, SeedMainHostConsts hc) {",
        "_refine_kernel(origin_x, origin_y, origin_z, ox, oy, oz, buffers, 0, 0, auto_constants, hosts);",
        "_refine_kernel(double origin_x, double origin_y, double origin_z, int ox, int oy, int oz, InternalBuffers buffers, int out_off, int off_prev, AutoConsts auto_constants, HostConsts host_consts)",
        // The offset moves `pos`, not the index the store and unindexed reads use.
        "p.set(ix + ox, iy + oy, iz + oz, idx);",
    ]);
    assert!(java.contains("out[out_off + idx] = acc_main_d2x1x1("), "{java}");
}

#[test]
fn workers_without_refinements_are_unchanged() {
    let java = java(&program(AMBIENTS));
    assert!(!java.contains("origin_x;") && !java.contains("refine_") && !java.contains("ox"), "{java}");
}
