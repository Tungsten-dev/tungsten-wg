//! Smoke tests: small hand-built programs exercising the CUDA-specific lowerings.
//! Set `TUNGSTEN_CUDA_OUT=<dir>` to also write the generated files there, e.g. to run nvcc on them.

use bumpalo::Bump;

use tungsten_interface::{Backend, Interface};
use tungsten_legacy::builder::{Builder, SPMTBuilder, floor, sin};
use tungsten_legacy::conversion::convert;
use tungsten_legacy::model::*;

use crate::backend::{CudaBackend, CudaConfig, CudaOutput};

fn var<'m>(v: Var<'m>) -> Expression<'m> {
    Expression::Variable(v)
}

fn build_program<'m>(arena: &'m Bump) -> SPMT<'m> {
    // Helper `int` (a C++ keyword) with a parameter and a loop with a break.
    let mut h = Builder::new(arena).named("int").returns(VariableType::F64);
    let t = h.param("t", VariableType::F64);
    let acc = h.local("acc", VariableType::F64);
    h.set_body(vec![
        Statement::Assign { target: acc, value: 0.0f64.into() },
        Statement::Repeat {
            count: 3,
            body: vec![
                Statement::Assign { target: acc, value: acc + t },
                Statement::If { condition: acc.gt(10.0f64), then_branch: vec![Statement::Break], else_branch: vec![] },
            ],
        },
        Statement::Return(var(acc)),
    ]);
    let helper = h.finish_function();

    // A 1x1x1 unit reading a host input: wrapping arithmetic, masked shifts.
    let mut s = Builder::new(arena).named("minecraft:scalar").returns(VariableType::I32);
    let sea = s.alloc_var(Name::Named("sea_level".into()), VariableType::I32);
    let flag = s.local("flag", VariableType::Bool);
    let k = s.local("k", VariableType::I32);
    s.set_body(vec![
        Statement::Assign { target: flag, value: var(sea).gt(62i32) },
        // literal shift count folded: 33 & 31 == 1
        Statement::Assign { target: k, value: var(sea).shl(33i32) },
        // i32 + bool, `!` on an integer, logical shift by a variable.
        Statement::Return(((var(sea) + var(flag)).bitxor(!var(sea))).ushr(var(k))),
    ]);
    let mut scalar = s.build_compute_unit();
    scalar.host_inputs = vec![HostInput::Static(sea)];
    let scalar: DensityFunctionRef = Interned::new(arena.alloc(scalar));

    // A 4x4x4 noise unit: constants (one depending on another), permutation table, pos3 binding.
    let n = Builder::new(arena).named("minecraft:noise").returns(VariableType::F64);
    let base = n.constant("base", VariableType::F64, 0.25f64);
    let scale = n.constant("scale", VariableType::F64, Expression::Variable(base) * 2.0f64);
    let points = n.constant(
        "points",
        VariableType::Array(Box::new(VariableType::F64), 3),
        Expression::ArrayLiteral(vec![(-1.0f64).into(), 0.0f64.into(), f64::NAN.into()]),
    );
    let pos3 = n.alloc_var(Name::Named("pos3".into()), VariableType::Pos3);
    let v = n.local("v", VariableType::F64);
    let bits = n.local("bits", VariableType::I64);
    let mut n = n;
    let x = Expression::Field {
        base: Box::new(var(pos3)),
        field: "x".into(),
        type_of_field: VariableType::F64,
        known_idnex: Some(0),
    };
    let perm = Expression::PermutationTable(PermutationTableInput::PerlinNoise {
        ident: "minecraft:temperature".into(),
        subident: None,
        subident_index: 0,
    });
    let sample = Expression::ExternCall {
        function_name: "sample".into(),
        parameters: vec![perm, x.clone()],
        parameter_types: vec![VariableType::PermutationTable, VariableType::F64],
        return_type: VariableType::F64,
    };
    n.set_body(vec![
        Statement::Assign { target: v, value: floor(x * var(scale)) + sin(sample) },
        Statement::Assign {
            target: bits,
            value: Expression::ExplicitCast { to: VariableType::I64, expr: Box::new(var(v)) } * (-5i64),
        },
        // `&mut v` on a local and `&(v * 2.0)` on a temporary.
        Statement::Assign {
            target: bits,
            value: Expression::ExternCall {
                function_name: "nudge".into(),
                parameters: vec![
                    v.as_mut_ref(),
                    Expression::UnaryOp {
                        op: UnaryOperator::Ref,
                        operand: Box::new(var(v) * 2.0f64),
                    },
                ],
                parameter_types: vec![VariableType::F64, VariableType::F64],
                return_type: VariableType::I64,
            },
        },
        Statement::Return(
            var(v) + Expression::ArrayAccess { array: Box::new(var(points)), index: Box::new(var(bits)) },
        ),
    ]);
    let noise = n.finish_compute_unit();

    // The 4x4x4 entry: reads both units, calls the helper, compares vectors.
    let f = Builder::new(arena).named("minecraft:final").returns(VariableType::F64);
    let noise_in = f.density(noise, (1.0, 1.0, 1.0), (0.25, 0.25, 0.25), (4, 4, 4));
    let scalar_in = f.density(scalar, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), (1, 1, 1));
    let origin = f.alloc_var(Name::Named("origin".into()), VariableType::Vec3);
    let same = f.local("same", VariableType::Bool);
    let mut f = f;
    f.set_body(vec![
        Statement::Assign {
            target: same,
            value: var(origin).eq_expr(Expression::Construct {
                t: VariableType::Vec3,
                args: vec![0.0f64.into(), 0i32.into(), 0.0f64.into()],
            }),
        },
        Statement::If {
            condition: var(same),
            then_branch: vec![Statement::Return(noise_in.at(0i32))],
            else_branch: vec![],
        },
        Statement::Return(
            helper.call([noise_in.get()])
                + Expression::ExplicitCast { to: VariableType::F64, expr: Box::new(scalar_in.get()) },
        ),
    ]);
    let entry = f.finish_compute_unit();

    SPMT {
        density_functions: vec![scalar, noise, entry],
        functions: vec![helper],
        main_density_functions: vec![MainDensityFunction {
            density_function: entry,
            dimensions: (4, 4, 4),
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (0.25, 0.25, 0.25),
        }],
    }
}

fn generate(program: &SPMT<'_>, config: CudaConfig) -> CudaOutput {
    let program = convert(program).expect("conversion");
    let mut backend = CudaBackend::new(config);
    let interface = Interface::build(&program, backend.naming_rules());
    backend.generate(&program, &interface).unwrap();
    backend.output().expect("cuda output").clone()
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
    let arena = Bump::new();
    let output = generate(&build_program(&arena), CudaConfig::default());
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
        // value-initialized locals and default bindings
        "double acc{};",
        "int3 pos3 = p.pos;",
        // `origin` is reserved (a kernel parameter), so the local is renamed
        "double3 origin_1 = ctx.config.origin;",
        "for (int repeat_counter = 0; repeat_counter < 3; ++repeat_counter) {",
        // Java integer semantics
        "k = (int)((unsigned)ctx.host_consts->sea_level << 1);",
        "(int)((unsigned)ctx.host_consts->sea_level + (unsigned)flag)",
        "~ctx.host_consts->sea_level",
        ">> (k & 31)",
        "(long long)((unsigned long long)tw::f2i64(v) * (unsigned long long)(-5LL))",
        // externs, samplers, refs
        "tw::sample(&ctx.auto_constants->minecraft_temperature_0_, pos3.x)",
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
    let arena = Bump::new();
    let config = CudaConfig {
        namespace: "wg".into(),
        kernel_prefix: Some("wgk_".into()),
        profile: crate::Profile::Tolerance,
        debug_checks: true,
        inline_policy: crate::InlinePolicy::Force,
        ..CudaConfig::default()
    };
    let output = generate(&build_program(&arena), config);
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
    let arena = Bump::new();
    let mut u = Builder::new(&arena).named("minecraft:const").returns(VariableType::F64);
    u.set_body(vec![Statement::Return(1.5f64.into())]);
    let unit = u.finish_compute_unit();
    let spmt = SPMT {
        density_functions: vec![unit],
        functions: vec![],
        main_density_functions: vec![MainDensityFunction {
            density_function: unit,
            dimensions: (2, 1, 1),
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (1.0, 1.0, 1.0),
        }],
    };
    let output = generate(&spmt, CudaConfig::default());
    write_out(&output, "minimal");
    assert_contains("interface.cuh", &output.interface_header, &["struct AutoConsts {\n};", "struct DeviceHostConsts {\n};"]);
    assert_contains("orchestration.cu", &output.orchestration_source, &["(void)seed;", "(void)host_consts;"]);
    assert!(!output.orchestration_source.contains("cudaEventSynchronize"));

    let empty = SPMT { density_functions: vec![], functions: vec![], main_density_functions: vec![] };
    let output = generate(&empty, CudaConfig::default());
    assert_contains("orchestration.h", &output.public_header, &["Status sync();"]);
    assert_contains("interface.cuh", &output.interface_header, &["static constexpr unsigned long long total_bytes = 0;"]);
}

#[test]
#[should_panic(expected = "block_size")]
fn rejects_invalid_block_size() {
    CudaBackend::new(CudaConfig { block_size: 100, ..CudaConfig::default() });
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

#[test]
fn bytes() {
    let arena = Bump::new();
    let output = generate(&build_byte_program(&arena), CudaConfig::default());
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
