//! Smoke tests: small programs, written as `.tw` text, exercising the Rust lowerings.

use tungsten_interface::{Backend, BufferBounds, Compiler, Interface};
use tungsten_parse::{FileId, NoEnv, load};

use crate::backend::{RustBackend, RustCompile, RustConfig, RustOutput};

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
    generate_with(src, BufferBounds::PerBuffer)
}

fn generate_with(src: &str, bounds: BufferBounds) -> RustOutput {
    let program = load(src, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}")).program;
    let backend = RustBackend::new(RustConfig { buffer_bounds: bounds, ..RustConfig::default() });
    let interface = Interface::build(&program, &backend.naming_rules());
    backend.generate(&program, &interface).unwrap()
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
        "pub i8s: Box<[i8]>,",
        "buffers.i8s[(out_off + index(p)) as usize] = r;",
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

fn empty_registry() -> tungsten_check::Registry {
    tungsten_check::RegistryBuilder::new().build().unwrap()
}

/// `Compiler` with default passes generates exactly what the low-level path does.
#[test]
fn compiler_matches_low_level_path() {
    let program = load(BYTES, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}")).program;
    let registry = empty_registry();
    let out = Compiler::new(program, registry, RustBackend::new(RustConfig::default()))
        .compile()
        .unwrap();
    assert_eq!(out, generate(BYTES));
}

/// Calls every setter. This also catches a setter that an inherent `Compiler` method shadows.
#[test]
fn every_setter_sets_its_field() {
    let program = load(BYTES, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}")).program;
    let registry = empty_registry();
    let c = Compiler::new(program, registry, RustBackend::new(RustConfig::default()))
        .density_module_name("d")
        .orchestration_module_name("o")
        .buffer_bounds(BufferBounds::Slab);
    assert_eq!(c.backend().config().density_module_name, "d");
    assert_eq!(c.backend().config().orchestration_module_name, "o");
    assert_eq!(c.backend().config().buffer_bounds, BufferBounds::Slab);
    let out = c.compile().unwrap();
    assert!(!out.orchestration_module.is_empty());
}

/// Every argument kind, a handle and value autoconsts of several types.
const AUTO_KINDS: &str = r#"
program {
    param seed: i64;
    param scale: f64;
    param buffers: bool;
    auto t: handle Table = make_table(param(seed), -9223372036854775808L, 1.5, -0.0, true, "q\"\\ \u{e9} \u{1d11e}\n");
    auto v: f64 = make_v(param(scale), param(buffers), nan, inf, -inf);
    auto i: i64 = make_i(7L, false);
    auto b: bool = make_b(param(buffers));
    auto s: i8 = make_s(1L);
    fn run() -> f64 {
        return (ext sample::<(handle Table, f64) -> f64>(auto(t), auto(v)) + (auto(i) as f64));
    }
    unit u { main run; autos [t, v, i]; }
    entry u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

#[test]
fn autoconst_arguments_and_values() {
    let output = generate(AUTO_KINDS);
    assert_contains(&output.full_source, &[
        // fields: a handle by value, scalars by value
        "pub t: Table,",
        "pub v: f64,",
        "pub s: i8,",
        // reads: a handle by reference, a value by copy
        "sample((&auto_constants.t), auto_constants.v)",
        "(auto_constants.i as f64)",
        // every argument kind, as one struct literal in arena order; a param named like
        // generated code is renamed
        "pub fn new(seed: i64, scale: f64, buffers_1: bool) -> Self",
        "AutoConsts { t: make_table(seed, -9223372036854775808_i64, 1.5_f64, -0_f64, true, \"q\\\"\\\\ \\u{e9} \\u{1d11e}\\n\"), \
         v: make_v(scale, buffers_1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY), i: make_i(7_i64, false), \
         b: make_b(buffers_1), s: make_s(1_i64) }",
        // implementation types stay off the public surface
        "#[repr(C)]\n    #[derive(Clone)]\n    pub(super) struct AutoConsts {",
    ]);
    assert!(!output.full_source.contains("make_auto_consts"));
    assert!(!output.full_source.contains("make_boxed"));
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

#[test]
fn orchestration_and_workers() {
    let output = generate(SHARED);
    assert_contains(&output.orchestration_module, &[
        "#[derive(Clone)]\n    pub struct Orchestration {\n        shared: std::sync::Arc<Shared>,\n    }",
        "struct Shared {",
        "pub(super) struct HostConsts {",
        "pub(super) struct InternalBuffers {\n        pub f64s: Box<[f64]>,\n        pub i32s: Box<[i32]>,\n    }",
        // one worker and one host struct per entry, each with only its own hosts
        "pub struct AMainHostConsts {\n        pub ha: f64,\n    }",
        "pub struct BMainHostConsts {\n        pub hb: i32,\n        pub big: [f64; 33],\n    }",
        "pub struct AMainWorker {\n        orch: Orchestration,\n        buffers: InternalBuffers,\n        hosts: HostConsts,\n    }",
        "pub fn a_main_worker(&self) -> AMainWorker",
        "pub fn b_main_worker(&self) -> BMainWorker",
        // pools sized per worker: `a` has f64 [a, side] and i32 [mid, leaf]; `b` has no f64s
        "AMainWorker { orch: self.clone(), buffers: InternalBuffers { f64s: vec![0_f64; 5_usize].into_boxed_slice(), i32s: vec![0_i32; 8_usize].into_boxed_slice() }",
        "BMainWorker { orch: self.clone(), buffers: InternalBuffers { f64s: vec![0_f64; 0_usize].into_boxed_slice(), i32s: vec![0_i32; 9_usize].into_boxed_slice() }",
        "pub const LEN: usize = 4_usize;",
        "pub const DIMS: (i32, i32, i32) = (4_i32, 1_i32, 1_i32);",
        "pub fn run(&mut self, origin: Vec3, host_consts: &AMainHostConsts) -> &[f64; 4] {",
        "self.hosts.ha = host_consts.ha;",
        "return <&[f64; 4]>::try_from(&self.buffers.f64s[0..4]).unwrap();",
        // a single-cell entry returns its value
        "pub fn run(&mut self, origin: Vec3, host_consts: &BMainHostConsts) -> i32 {",
        "self.hosts.big = host_consts.big;",
        "return self.buffers.i32s[0_usize as usize];",
        // `mid` is at i32 offset 0 in `a`'s worker and 1 in `b`'s, reading `leaf` after it
        "mid_main_d4x1x1_kernel(origin, (&mut self.buffers), 0_usize, 4_usize, (&self.orch.shared.auto_constants), (&self.hosts));",
        "mid_main_d4x1x1_kernel(origin, (&mut self.buffers), 1_usize, 5_usize, (&self.orch.shared.auto_constants), (&self.hosts));",
        "const _: () = { fn s<T: Send + Sync>() {} fn w<T: Send>() {} fn a() { s::<Orchestration>(); w::<AMainWorker>(); w::<BMainWorker>(); } };",
        // generated zero values, also past 32 elements
        "impl Default for BMainHostConsts {\n        fn default() -> Self {\n            return Self { hb: 0_i32, big: [0_f64; 33] };",
    ]);
    assert!(!output.orchestration_module.contains("orchestrate_"), "{}", output.orchestration_module);
    assert!(!output.orchestration_module.contains("pub struct HostConsts"));
    assert!(!output.full_source.contains("unsafe"));
}

/// Helpers take the offsets of what they read, and callers pass their own through.
#[test]
fn offsets_pass_through_helpers() {
    let output = generate(SHARED);
    assert_contains(&output.density_module, &[
        "fn get_leaf(p: PositionIterator, config: &Config, buffers: &InternalBuffers, auto_constants: &AutoConsts, host_consts: &HostConsts, off_leaf_in: usize) -> i32",
        "return rd((&buffers.i32s), off_leaf_in, 4_usize, index(p));",
        "fn mid_main_d4x1x1(p: PositionIterator, config: &Config, buffers: &InternalBuffers, auto_constants: &AutoConsts, host_consts: &HostConsts, off_leaf_in: usize) -> i32",
        "get_leaf(p, config, buffers, auto_constants, host_consts, off_leaf_in)",
        "pub fn mid_main_d4x1x1_kernel(origin: Vec3, buffers: &mut InternalBuffers, out_off: usize, off_leaf_in: usize, auto_constants: &AutoConsts, host_consts: &HostConsts)",
        "mid_main_d4x1x1(p, (&config), (&(*buffers)), auto_constants, host_consts, off_leaf_in)",
        // a (1, 1, 1) input reads cell 0
        "rd((&buffers.f64s), off_side_in, 1_usize, 0_usize)",
        "fn rd<T: Copy>(pool: &[T], offset: usize, len: usize, i: usize) -> T {",
    ]);
}

#[test]
fn bounds_modes() {
    let slab = generate_with(SHARED, BufferBounds::Slab).density_module;
    assert_contains(&slab, &["buffers.i32s[(off_leaf_in + index(p)) as usize]"]);
    assert!(!slab.contains("rd((&"), "{slab}");
    assert!(!slab.contains("unsafe"), "{slab}");

    let unchecked = generate_with(SHARED, BufferBounds::Unchecked).density_module;
    assert_contains(&unchecked, &[
        "unsafe { (*buffers.i32s.get_unchecked((off_leaf_in + index(p)))) }",
        "(*unsafe { buffers.i32s.get_unchecked_mut((out_off + index(p))) }) = r;",
    ]);
    assert!(!unchecked.contains("rd((&"), "{unchecked}");
}

/// An explicit index is cast, so a negative one wraps and fails the `PerBuffer` length check.
#[test]
fn explicit_index_is_checked_against_the_input() {
    let src = r#"
program {
    fn leaf_main() -> f64 { return 1.0; }
    unit leaf { main leaf_main; }
    input l = leaf @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    fn top_main() -> f64 { return density(l, (ambient(pos3).x - 1)); }
    unit top { main top_main; inputs [l]; }
    entry top @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;
    assert_contains(&generate(src).density_module, &[
        "rd((&buffers.f64s), off_l, 2_usize, ((p.pos.x - 1_i32) as usize))",
    ]);
}

/// A program with no entries still has an `Orchestration`, with no factories.
#[test]
fn no_entries() {
    let src = "program {\n    fn f() -> f64 { return 1.0; }\n    unit u { main f; }\n}\n";
    let out = generate(src).orchestration_module;
    assert_contains(&out, &["pub fn new() -> Self", "s::<Orchestration>(); }"]);
    assert!(!out.contains("Worker"), "{out}");
}
