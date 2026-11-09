//! Smoke tests: small programs, as `.tw` text, exercising the CUDA-specific lowerings.
//! Set `TUNGSTEN_CUDA_OUT=<dir>` to also write the generated files there, e.g. to run nvcc on them.

use tungsten_interface::{Backend, BackendError, CompileError, Compiler, Interface};
use tungsten_parse::{FileId, NoEnv, load};

use crate::backend::{CudaBackend, CudaCompile, CudaConfig, CudaOutput, InlinePolicy, Profile};

const SMALL: &str = r#"
program {
    host sea_level: i32;
    param seed: i64;
    auto temperature: handle PerlinNoiseSampler @name("minecraft:temperature_0_") = make_permutation_table(param(seed), 6664882324328353151L, -587597586455377528L, 0L, 0L, 0L);
    const base = 0.25;
    const scale = (const(base) * 2.0);
    const points = [-1.0, 0.0, nan];

    // Helper `int` (a C++ keyword) with a parameter and a loop with a break.
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

    // A 1x1x1 unit reading a host input: wrapping arithmetic, masked shifts.
    fn `minecraft:scalar`() -> i32 @hosts(sea_level) {
        vars { flag: bool, k: i32 }
        flag = (host(sea_level) > 62);
        // literal shift count folded: 33 & 31 == 1
        k = (host(sea_level) << 33);
        // i32 + bool, `!` on an integer, logical shift by a variable.
        return (((host(sea_level) + flag) ^ !host(sea_level)) >>> k);
    }

    // A 4x4x4 noise unit: constants (one depending on another), permutation table, an ambient read.
    fn `minecraft:noise`() -> f64 {
        vars { v: f64, bits: i64 }
        v = (ext floor::<(f64) -> f64>(ambient(pos3).x::<f64> * const(scale))
            + ext sin::<(f64) -> f64>(ext sample::<(handle PerlinNoiseSampler, f64) -> f64>(auto(temperature), ambient(pos3).x::<f64>)));
        bits = ((v as i64) * -5L);
        // `&mut v` on a local and `&(v * 2.0)` on a temporary.
        bits = ext nudge::<(f64, f64) -> i64>(&mut v, &(v * 2.0));
        return (v + const(points)[bits]);
    }

    // The 4x4x4 entry: reads both units, calls the helper, compares vectors.
    fn `minecraft:final`() -> f64 {
        vars { same: bool }
        same = (ambient(origin) == vec3(0.0, 0, 0.0));
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

const MINIMAL: &str = r#"
program {
    fn `minecraft:const`() -> f64 { return 1.5; }
    unit constant { main `minecraft:const`; }
    entry constant @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

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
            + (ambient(pos3).x::<f64> * ambient(position_scale).x));
    }
    unit ambients { main `minecraft:ambients`; }
    entry ambients @dims(4, 4, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

fn generate(src: &str, config: CudaConfig) -> CudaOutput {
    let program = load(src, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}")).program;
    let backend = CudaBackend::new(config);
    let interface = Interface::build(&program, &backend.naming_rules());
    backend.generate(&program, &interface).unwrap()
}

fn write_out(output: &CudaOutput, subdir: &str) {
    if let Ok(dir) = std::env::var("TUNGSTEN_CUDA_OUT") {
        let dir = std::path::Path::new(&dir).join(subdir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, contents) in output.files() {
            std::fs::write(dir.join(name), contents).unwrap();
        }
    }
}

fn assert_contains(file: &str, text: &str, expected: &[&str]) {
    for e in expected {
        assert!(text.contains(e), "missing `{}` in {}:\n{}", e, file, text);
    }
}

#[test]
fn generates_cuda_for_small_program() {
    let output = generate(SMALL, CudaConfig::default());
    write_out(&output, "small");

    assert_contains("density.cu", &output.density_source, &[
        "#define TUNGSTEN_PROFILE_EXACT 1",
        "#include \"tw/profile_exact.cuh\"",
        "#include \"tw/tungsten_runtime.cuh\"",
        "#include \"interface.cuh\"",
        "namespace tungsten_gen::dev {",
        // keyword-named helper is renamed
        "__device__ double int_1(tw::PositionIterator p, const Ctx& ctx, double t)",
        // constants in dependency order; aggregates are __device__
        "constexpr double base = 0.25;",
        "constexpr double scale = base * 2.0;",
        "__device__ constexpr tw::Array<double, 3> points = tw::Array<double, 3>{{-1.0, 0.0, TW_NAN_F64}};",
        // value-initialized locals
        "double acc{};",
        "for (int repeat_counter = 0; repeat_counter < 3; ++repeat_counter) {",
        // Java integer semantics
        "k = (int)((unsigned)ctx.host_consts->sea_level << 1);",
        "(int)((unsigned)ctx.host_consts->sea_level + (unsigned)flag)",
        "~ctx.host_consts->sea_level",
        ">> (k & 31)",
        "(long long)((unsigned long long)tw::f2i64(v) * (unsigned long long)(-5LL))",
        // externs, samplers, refs
        "tw::sample(&ctx.auto_constants->minecraft_temperature_0_, p.pos.x)",
        "tw::nudge(&v, &tw::lval(v * 2.0))",
        // buffers and array access
        "tw::buffer<double>(ctx.buffers, BufferLayout::minecraft_noise_d4x4x4_output)[tw::index(p)]",
        "tw::buffer<int>(ctx.buffers, BufferLayout::minecraft_scalar_d1x1x1_output)[0]",
        "points[(int)bits]",
        "double3{0.0, (double)0, 0.0}",
        // kernels
        "__device__ void minecraft_final_d4x4x4_kernel_body(int3 local, double3 origin, char* buffers,",
        "if ((local.x >= 4) || (local.y >= 4) || (local.z >= 4)) {",
        "const tw::PositionIterator p = tw::make_position(ctx.config, local);",
        "extern \"C\" __global__ void __launch_bounds__(64) tungsten_gen_minecraft_final_d4x4x4_kernel(",
        "const int i = (int)(blockIdx.x * blockDim.x + threadIdx.x);",
        "const int yz = i / 4;",
        "const int z = yz / 4;",
        "minecraft_final_d4x4x4_kernel_body(int3{i - yz * 4, yz - z * 4, z}, origin,",
    ]);
    assert!(
        output.density_source.find("constexpr double base").unwrap()
            < output.density_source.find("constexpr double scale").unwrap(),
        "constants must be emitted in dependency order"
    );

    assert_contains("interface.cuh", &output.interface_header, &[
        "#pragma once",
        "struct DeviceHostConsts {\n    int sea_level;\n};",
        "tw::PerlinNoiseSampler minecraft_temperature_0_;",
        "static constexpr unsigned long long total_bytes = tw::align_up(",
        "const AutoConsts* __restrict__ auto_constants;",
        "extern \"C\" __global__ void __launch_bounds__(1) tungsten_gen_minecraft_scalar_d1x1x1_kernel(",
    ]);

    assert_contains("orchestration.h", &output.public_header, &[
        "#include <memory>",
        "struct HostConsts {\n    int sea_level;\n};",
        "static constexpr int minecraft_final_len = 64;",
        "static constexpr int minecraft_final_dims[3] = {4, 4, 4};",
        "Status enqueue_minecraft_final(Vec3 origin, const HostConsts& host_consts);",
        "const double* result_minecraft_final() const;",
        "static constexpr const char* required_flags = \"--fmad=false",
    ]);
    for cuda_only in ["#include <cuda", "cudaStream_t", "double3", "int3", "__device__"] {
        assert!(!output.public_header.contains(cuda_only), "orchestration.h must not use `{}`", cuda_only);
    }

    assert_contains("orchestration.cu", &output.orchestration_source, &[
        "#define TUNGSTEN_CUDA_CHECK(expr)",
        "return ::tungsten_gen::Status{(int)tw_e_};",
        "static_assert(offsetof(HostConsts, sea_level) == offsetof(dev::DeviceHostConsts, sea_level)",
        "auto_constants->minecraft_temperature_0_ = tw::make_permutation_table(seed, (long long)0x",
        "dev::tungsten_gen_minecraft_scalar_d1x1x1_kernel<<<1, 1, 0, s.stream>>>",
        "dev::tungsten_gen_minecraft_final_d4x4x4_kernel<<<1, 64, 0, s.stream>>>",
        "cudaMemcpyAsync(s.result_minecraft_final, s.slab + dev::BufferLayout::minecraft_final_d4x4x4_output",
    ]);
    // The entry's dependencies run in an earlier wave than the entry itself.
    let src = &output.orchestration_source;
    assert!(src.find("minecraft_noise_d4x4x4_kernel<<<").unwrap() < src.find("minecraft_final_d4x4x4_kernel<<<").unwrap());

    assert!(output.warnings.iter().any(|w| w.contains("partial warp")), "{:?}", output.warnings);
}

#[test]
fn debug_checks_and_config() {
    let config = CudaConfig {
        namespace: "wg".into(),
        kernel_prefix: Some("wgk_".into()),
        profile: crate::Profile::Tolerance,
        debug_checks: true,
        inline_policy: crate::InlinePolicy::Force,
        ..CudaConfig::default()
    };
    let output = generate(SMALL, config);
    write_out(&output, "debug");
    assert_contains("density.cu", &output.density_source, &[
        "#define TUNGSTEN_PROFILE_TOLERANCE 1",
        "#include \"tw/profile_fast.cuh\"",
        "namespace wg::dev {",
        "__device__ __forceinline__ double int_1(",
        "points[TW_CHECK_INDEX((int)bits, 3, \"minecraft_noise_d4x4x4\")]",
        "[TW_CHECK_INDEX(tw::index(p), 64, \"wgk_minecraft_final_d4x4x4_kernel\")]",
        "extern \"C\" __global__ void __launch_bounds__(64) wgk_minecraft_final_d4x4x4_kernel(",
    ]);
    assert_contains("orchestration.cu", &output.orchestration_source, &["return ::wg::Status{(int)tw_e_};"]);
}

#[test]
fn program_without_inputs_or_entries() {
    let output = generate(MINIMAL, CudaConfig::default());
    write_out(&output, "minimal");
    assert_contains("interface.cuh", &output.interface_header, &["struct AutoConsts {\n};", "struct DeviceHostConsts {\n};"]);
    assert_contains("orchestration.cu", &output.orchestration_source, &["Orchestration::create(std::unique_ptr<Orchestration>& out)", "(void)host_consts;"]);
    assert!(!output.orchestration_source.contains("cudaEventSynchronize"));

    let output = generate("program { }", CudaConfig::default());
    assert_contains("orchestration.h", &output.public_header, &["Status sync();"]);
    assert_contains("interface.cuh", &output.interface_header, &["static constexpr unsigned long long total_bytes = 0;"]);
}

#[test]
fn rejects_invalid_config() {
    let err = |config| CudaBackend::new(config).validate().unwrap_err().to_string();
    assert!(err(CudaConfig { block_size: 100, ..CudaConfig::default() }).contains("block_size"));
    assert!(err(CudaConfig { namespace: "1x".into(), ..CudaConfig::default() }).contains("namespace"));
    assert!(err(CudaConfig { host_consts_ring: 0, ..CudaConfig::default() }).contains("host_consts_ring"));
    assert!(CudaBackend::new(CudaConfig::default()).validate().is_ok());
}

#[test]
fn bytes() {
    let output = generate(BYTES, CudaConfig::default());
    write_out(&output, "bytes");
    assert_contains("density.cu", &output.density_source, &[
        "__device__ constexpr tw::Array<signed char, 3> table = tw::Array<signed char, 3>{{(signed char)1, (signed char)(-2), (signed char)127}};",
        "__device__ signed char minecraft_bytes_d4x4x4(tw::PositionIterator p, const Ctx& ctx) {",
        "signed char a{};",
        // Computed in 32 bits, narrowed back.
        "a = (signed char)((unsigned)table[2] + (unsigned)(signed char)127);",
        "a = (signed char)((signed char)(0 - (unsigned)a) & (signed char)((unsigned)a << 1));",
        "wide = (int)a & 255;",
        "a = tw::f2i8(d);",
        "return (signed char)((unsigned)a + (unsigned)(signed char)wide);",
        "tw::buffer<signed char>(buffers, BufferLayout::minecraft_bytes_d4x4x4_output)",
    ]);
    assert_contains("orchestration.h", &output.public_header, &["const signed char* result_minecraft_bytes() const;"]);
}

#[test]
fn ambients() {
    let output = generate(AMBIENTS, CudaConfig::default());
    assert_contains("density.cu", &output.density_source, &[
        "ctx.config.origin.x",
        "ctx.config.origin_scale.x",
        "ctx.config.position_scale.x",
        "p.pos.x",
    ]);
    assert!(!output.density_source.contains("double3 origin_1"), "{}", output.density_source);
}

fn empty_registry() -> tungsten_check::Registry {
    tungsten_check::RegistryBuilder::new().build().unwrap()
}

fn compiler(r: tungsten_check::Registry) -> Compiler<CudaBackend> {
    let program = load(BYTES, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}")).program;
    Compiler::new(program, r, CudaBackend::new(CudaConfig::default()))
}

#[test]
fn compiler_matches_low_level_path() {
    let registry = empty_registry();
    let out = compiler(registry).compile().unwrap();
    assert_eq!(out, generate(BYTES, CudaConfig::default()));
}

#[test]
fn every_setter_sets_its_field() {
    let registry = empty_registry();
    let c = compiler(registry)
        .profile(Profile::Tolerance)
        .block_size(256)
        .host_consts_ring(4)
        .debug_checks(true)
        .inline_policy(InlinePolicy::Force)
        .namespace("ns");
    let config = c.backend().config();
    assert_eq!(config.profile, Profile::Tolerance);
    assert_eq!((config.block_size, config.host_consts_ring), (256, 4));
    assert!(config.debug_checks);
    assert_eq!(config.inline_policy, InlinePolicy::Force);
    assert_eq!(config.namespace, "ns");
    // The namespace is picked up by the naming rules, which are computed at compile time.
    assert!(c.backend().naming_rules().used_set().contains("ns"));
    c.compile().unwrap();
}

#[test]
fn invalid_setter_value_is_an_error_not_a_panic() {
    let registry = empty_registry();
    let r = compiler(registry).block_size(7).compile();
    assert!(matches!(
        r,
        Err(CompileError::Backend { source: BackendError::InvalidConfig(_), .. })
    ));
}

#[test]
fn autoconst_arguments_and_values() {
    let src = r#"
program {
    param seed: i64;
    param scale: f64;
    param buffers: bool;
    auto t: handle Table = make_table(param(seed), -9223372036854775808L, 1.5, -0.0, true, "q\"\\? \u{e9} \u{1d11e}\n");
    auto v: f64 = make_v(param(scale), param(buffers), nan, inf, -inf);
    auto i: i64 = make_i(7L, false);
    fn run() -> f64 {
        return (ext sample::<(handle Table, f64) -> f64>(auto(t), auto(v)) + (auto(i) as f64));
    }
    unit u { main run; autos [t, v, i]; }
    entry u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;
    let output = generate(src, CudaConfig::default());
    assert_contains("interface.cuh", &output.interface_header, &[
        "tw::Table t;",
        "double v;",
        "long long i;",
    ]);
    // handles are read by address, values by copy
    assert_contains("density.cu", &output.density_source, &[
        "tw::sample(&ctx.auto_constants->t, ctx.auto_constants->v)",
        "(double)ctx.auto_constants->i",
    ]);
    assert_contains("orchestration.cu", &output.orchestration_source, &[
        // the copy to the device is a byte copy
        "static_assert(std::is_trivially_copyable<dev::AutoConsts>::value",
        // i64 is always hex; strings are UTF-8 with octal escapes and `?` escaped
        "tw::make_table(seed, (long long)0x8000000000000000ULL, 1.5, -0.0, true, \"q\\\"\\\\\\? \\303\\251 \\360\\235\\204\\236\\012\")",
        "tw::make_v(scale, buffers_1, TW_NAN_F64, TW_INF_F64, (-TW_INF_F64))",
        "tw::make_i((long long)0x0000000000000007ULL, false)",
    ]);
}
