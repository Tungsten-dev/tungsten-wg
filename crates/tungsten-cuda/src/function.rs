/*
Emission of density functions, helper functions, constants and per-instance kernels (SPEC.md §9).
Every density and helper function takes the uniform parameters first (see `uniform_parameters`).
Each kernel is a `__device__` body holding all per-instance logic plus a thin `extern "C"`
wrapper, so a future fused kernel can call several bodies unchanged.
*/

use std::collections::{HashMap, HashSet};

use tungsten_interface::{Instance, Interface};
use tungsten_ir::model::{Arena, ComputeUnit, Constant, Function, Name, OptimizedProgram, Ref, VariableType};
use tungsten_rust::expression::const_refs;
use tungsten_rust::types::derive_const_type;

use super::converter::FunctionConverter;
use super::expression::{buffer_pointer, const_to_expression};
use super::launch::launch_shape;
use super::types::{device_type, is_pointer, is_scalar, unit_return_type};
use super::{
    AUTO_CONSTS_STRUCT, CONFIG_TYPE, CTX_STRUCT, DEVICE_HOST_CONSTS_STRUCT, PARAM_CTX, PARAM_POS,
    POSITION_ITERATOR_TYPE, kernel_body_name, kernel_symbol, uniform_parameters,
};
use crate::backend::{CudaConfig, InlinePolicy};
use crate::model as cuda;

use cuda::Expression as C;
use cuda::Statement as S;

/// A converted function plus the items it still needs generated.
pub struct Converted {
    pub function: cuda::Function,
    /// Helpers called by the body that the caller must still generate.
    pub called: Vec<Ref<Function>>,
    /// Namespace-level constants read by the body that the caller must still generate.
    pub constants: Vec<Ref<Constant>>,
    pub warnings: Vec<String>,
}

/// Qualifiers of density and helper functions.
fn function_qualifiers(config: &CudaConfig) -> String {
    match config.inline_policy {
        InlinePolicy::Auto => "__device__",
        InlinePolicy::Force => "__device__ __forceinline__",
        InlinePolicy::Never => "__device__ __noinline__",
    }
    .to_string()
}

/// Variables the prologue binds from `p` and `ctx.config` instead of value-initializing.
fn default_binding(name: &Name) -> Option<cuda::Expression> {
    let Name::Named(n) = name else { return None };
    let config_field = |f: &str| C::field(C::field(C::name(PARAM_CTX), "config"), f);
    match n.as_str() {
        "origin" => Some(config_field("origin")),
        "origin_scale" => Some(config_field("origin_scale")),
        "position_scale" => Some(config_field("position_scale")),
        "pos3" => Some(C::field(C::name(PARAM_POS), "pos")),
        _ => None,
    }
}

/// Declares every local at the top of the function: default bindings get their value, everything
/// else is value-initialized (`T name{};`). The IR guarantees definite assignment; this only keeps
/// the code free of UB and warnings.
fn declare_locals(conv: &mut FunctionConverter<'_>, func: &Function) -> Vec<cuda::Statement> {
    let mut out = Vec::new();
    for (r, var) in func.variables.iter() {
        if func.parameters.contains(&r) || matches!(var.t, VariableType::DensityInput) {
            continue;
        }
        let name = conv.declare_var(r);
        out.push(S::Declare { t: device_type(&var.t), name, init: default_binding(&var.name), is_const: false });
    }
    out
}

fn finish(conv: FunctionConverter<'_>, function: cuda::Function) -> Converted {
    Converted { function, called: conv.called, constants: conv.used_constants, warnings: conv.warnings }
}

/// Converts a helper function: uniform parameters, then its own parameters, then the body.
pub fn convert_helper<'a>(
    program: &'a OptimizedProgram,
    naming: &'a Interface,
    config: &'a CudaConfig,
    used_names: &HashSet<String>,
    f: Ref<Function>,
) -> Converted {
    let func = &program.functions[f];
    let name = naming.helper(f).to_string();
    let mut conv = FunctionConverter::new(program, naming, config, func, &name, used_names.clone());
    let mut params = uniform_parameters();
    // Helper-specific parameters follow the uniform ones.
    for p in &func.parameters {
        let param = conv.declare_var(*p);
        params.push((conv.cuda_var_type(*p), param));
    }
    let mut body = declare_locals(&mut conv, func);
    body.extend(conv.convert_block(&func.body));
    let function = cuda::Function {
        doc: func.canonical_name.clone(),
        qualifiers: function_qualifiers(config),
        return_type: device_type(&func.return_type),
        attributes: String::new(),
        name,
        params,
        body: Some(body),
    };
    finish(conv, function)
}

/// Converts a unit's density function for one set of dimensions (named `<name>_d{X}x{Y}x{Z}`).
///
/// It returns the value for the single position `p`; the kernel picks the position.
pub fn convert_density<'a>(
    program: &'a OptimizedProgram,
    naming: &'a Interface,
    config: &'a CudaConfig,
    used_names: &HashSet<String>,
    cu: Ref<ComputeUnit>,
    dimensions: (i32, i32, i32),
) -> Converted {
    let ComputeUnit::DensityFunction { function, source_hash, .. } = &program.compute_units[cu];
    let func = &program.functions[*function];
    let name = naming.density_fn(cu, dimensions).to_string();
    let mut conv = FunctionConverter::new(program, naming, config, func, &name, used_names.clone());
    let mut body = declare_locals(&mut conv, func);
    body.extend(conv.convert_block(&func.body));
    let doc = format!(
        "{} (source hash {:x})",
        func.canonical_name.as_deref().unwrap_or("density function"),
        source_hash
    );
    let function = cuda::Function {
        doc: Some(doc),
        qualifiers: function_qualifiers(config),
        return_type: device_type(&func.return_type),
        attributes: String::new(),
        name,
        params: uniform_parameters(),
        body: Some(body),
    };
    finish(conv, function)
}

/// A function with no variables, used to convert constant initializers like ordinary expressions.
fn empty_function() -> Function {
    Function {
        canonical_name: None,
        host_inputs: vec![],
        body: vec![],
        variables: Arena::new(),
        parameters: vec![],
        return_type: VariableType::F64,
        source_hash: 0,
    }
}

/// `constexpr T name = init;` for a scalar constant, `__device__ constexpr` for an aggregate (a
/// host `constexpr` aggregate can't be indexed dynamically from device code).
pub fn convert_constant(
    program: &OptimizedProgram,
    naming: &Interface,
    config: &CudaConfig,
    c: Ref<Constant>,
) -> (cuda::Item, Vec<String>) {
    let constant = &program.constants[c];
    let t = derive_const_type(program, &constant.value)
        .unwrap_or_else(|| panic!("cannot derive the type of constant '{}'", constant.name));
    if is_pointer(&t) {
        panic!("constant '{}' has the pointer type {:?}", constant.name, t);
    }
    let name = naming.constant(c).to_string();
    let function = empty_function();
    let mut conv = FunctionConverter::new(program, naming, config, &function, &name, HashSet::new());
    let init = conv.convert_to_type(&const_to_expression(&constant.value), &t);
    let qualifiers = if is_scalar(&t) { "constexpr" } else { "__device__ constexpr" };
    let item = cuda::Item::Variable { qualifiers: qualifiers.into(), t: device_type(&t), name, init };
    (item, conv.warnings)
}

/// `constants` reordered so every constant follows the ones its initializer reads (C++ needs
/// declaration before use). Roots are visited in arena order, so the output is deterministic.
pub fn sort_constants(program: &OptimizedProgram, constants: &[Ref<Constant>]) -> Vec<Ref<Constant>> {
    fn visit(
        program: &OptimizedProgram,
        c: Ref<Constant>,
        state: &mut HashMap<Ref<Constant>, bool>,
        out: &mut Vec<Ref<Constant>>,
    ) {
        match state.get(&c) {
            Some(true) => return,
            Some(false) => panic!("cyclic dependency between constants ('{}')", program.constants[c].name),
            None => {}
        }
        state.insert(c, false);
        let mut deps = Vec::new();
        const_refs(&program.constants[c].value, &mut deps);
        for d in deps {
            visit(program, d, state, out);
        }
        state.insert(c, true);
        out.push(c);
    }
    let wanted: HashSet<_> = constants.iter().copied().collect();
    let mut state = HashMap::new();
    let mut out = Vec::new();
    for (c, _) in program.constants.iter() {
        if wanted.contains(&c) {
            visit(program, c, &mut state, &mut out);
        }
    }
    out
}

fn double3(x: cuda::Expression, y: cuda::Expression, z: cuda::Expression) -> cuda::Expression {
    C::BraceInit { t: cuda::Type::named("double3"), items: vec![x, y, z] }
}

fn double3_literal((x, y, z): (f64, f64, f64)) -> cuda::Expression {
    double3(C::DoubleLit(x), C::DoubleLit(y), C::DoubleLit(z))
}

fn int3(x: cuda::Expression, y: cuda::Expression, z: cuda::Expression) -> cuda::Expression {
    C::BraceInit { t: cuda::Type::named("int3"), items: vec![x, y, z] }
}

/// The parameters every kernel body and wrapper shares after `local`.
fn kernel_parameters() -> Vec<(cuda::Type, String)> {
    let restrict = |n: &str| cuda::Type::const_restrict_ptr(cuda::Type::named(n));
    vec![
        (cuda::Type::named("double3"), "origin".into()),
        (cuda::Type::ptr(cuda::Type::Char), "buffers".into()),
        (restrict(AUTO_CONSTS_STRUCT), "auto_constants".into()),
        (restrict(DEVICE_HOST_CONSTS_STRUCT), "host_consts".into()),
    ]
}

fn kernel_arguments() -> Vec<cuda::Expression> {
    kernel_parameters().into_iter().map(|(_, n)| C::Name(n)).collect()
}

/// `__device__ void <kernel>_body(int3 local, ...)`: bounds check, `Ctx`, position, call, store.
pub fn build_kernel_body(program: &OptimizedProgram, naming: &Interface, config: &CudaConfig, inst: &Instance) -> cuda::Function {
    let element = device_type(unit_return_type(program, inst.cu));
    if element.is_pointer() {
        panic!("compute unit {} returns a sampler, which can't be stored in a buffer", naming.density_fn(inst.cu, inst.dimensions));
    }
    let (dx, dy, dz) = inst.dimensions;
    let local = |f: &str| C::field(C::name("local"), f);
    let ge = |a, b| C::binary(cuda::BinaryOperator::GreaterEqual, a, b);
    let or = |a, b| C::binary(cuda::BinaryOperator::Or, a, b);
    let out_of_bounds = or(or(ge(local("x"), C::IntLit(dx)), ge(local("y"), C::IntLit(dy))), ge(local("z"), C::IntLit(dz)));

    let ctx = C::BraceInit {
        t: cuda::Type::named(CTX_STRUCT),
        items: vec![
            C::BraceInit {
                t: cuda::Type::named(CONFIG_TYPE),
                items: vec![
                    C::name("origin"),
                    double3_literal(inst.scaled_origin()),
                    double3_literal(inst.scaled_position()),
                    int3(C::IntLit(dx), C::IntLit(dy), C::IntLit(dz)),
                ],
            },
            C::name("buffers"),
            C::name("auto_constants"),
            C::name("host_consts"),
        ],
    };
    let mut index = C::call("tw::index", vec![C::name(PARAM_POS)]);
    if config.debug_checks {
        let location = C::Raw(format!("\"{}\"", kernel_symbol(config, naming, inst)));
        index = C::call("TW_CHECK_INDEX", vec![index, C::IntLit(dx * dy * dz), location]);
    }
    let body = vec![
        S::If { cond: out_of_bounds, then: vec![S::Return(None)], other: vec![] },
        S::Declare { t: cuda::Type::named(CTX_STRUCT), name: PARAM_CTX.into(), init: Some(ctx), is_const: true },
        S::Declare {
            t: cuda::Type::named(POSITION_ITERATOR_TYPE),
            name: PARAM_POS.into(),
            init: Some(C::call("tw::make_position", vec![C::field(C::name(PARAM_CTX), "config"), C::name("local")])),
            is_const: true,
        },
        S::Declare {
            t: element.clone(),
            name: "r".into(),
            init: Some(C::call(naming.density_fn(inst.cu, inst.dimensions), vec![C::name(PARAM_POS), C::name(PARAM_CTX)])),
            is_const: true,
        },
        S::Assign {
            target: C::index(buffer_pointer(element, C::name("buffers"), naming.buffer_field(inst)), index),
            value: C::name("r"),
        },
    ];
    let mut params = vec![(cuda::Type::named("int3"), "local".to_string())];
    params.extend(kernel_parameters());
    cuda::Function {
        doc: None,
        qualifiers: "__device__".into(),
        return_type: cuda::Type::Void,
        attributes: String::new(),
        name: kernel_body_name(naming, inst),
        params,
        body: Some(body),
    }
}

/// `extern "C" __global__ void __launch_bounds__(N) <symbol>(...)`: 1D launch that decodes the
/// linear thread id into `local` (x fastest) and calls the body. Ids past the element count decode
/// to `z >= DZ`, which the body's bounds check rejects. Without `body` it is the declaration that
/// `interface.cuh` uses.
pub fn build_kernel_wrapper(naming: &Interface, config: &CudaConfig, inst: &Instance, with_body: bool) -> cuda::Function {
    use cuda::BinaryOperator::{Add, Divide, Multiply, Subtract};
    let shape = launch_shape(inst.dimensions, config.block_size);
    let (dx, dy, _) = inst.dimensions;
    let declare = |name: &str, init| S::Declare { t: cuda::Type::Int, name: name.into(), init: Some(init), is_const: true };
    let linear = C::cast(
        cuda::Type::Int,
        C::binary(
            Add,
            C::binary(Multiply, C::field(C::name("blockIdx"), "x"), C::field(C::name("blockDim"), "x")),
            C::field(C::name("threadIdx"), "x"),
        ),
    );
    let local = int3(
        C::binary(Subtract, C::name("i"), C::binary(Multiply, C::name("yz"), C::IntLit(dx))),
        C::binary(Subtract, C::name("yz"), C::binary(Multiply, C::name("z"), C::IntLit(dy))),
        C::name("z"),
    );
    let mut args = vec![local];
    args.extend(kernel_arguments());
    cuda::Function {
        doc: None,
        qualifiers: "extern \"C\" __global__".into(),
        return_type: cuda::Type::Void,
        attributes: format!("__launch_bounds__({})", shape.threads_per_block()),
        name: kernel_symbol(config, naming, inst),
        params: kernel_parameters(),
        body: with_body.then(|| {
            vec![
                declare("i", linear),
                declare("yz", C::binary(Divide, C::name("i"), C::IntLit(dx))),
                declare("z", C::binary(Divide, C::name("yz"), C::IntLit(dy))),
                S::Expr(C::call(&kernel_body_name(naming, inst), args)),
            ]
        }),
    }
}
