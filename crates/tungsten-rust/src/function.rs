/*
Emission of density functions, helper functions, constants and per-instance kernels.
Every density and helper function takes the uniform parameters first (see `uniform_parameters`).
*/

use std::rc::Rc;

use super::converter::FunctionConverter;
use super::expression::{const_refs, constant_variable, convert_const_expr};
use super::types::convert_type;
use super::{CONFIG_STRUCT_NAME, uniform_kernel_parameters, uniform_parameters};
use crate::model as rcl;
use crate::pools::Pools;
use tungsten_interface::{BufferBounds, Instance, Interface, LerpHelper};
use tungsten_ir::model::{
    ComputeUnit, Constant, Function, OptimizedProgram, Ref,
};

/// Name of a kernel's own slot offset.
pub const KERNEL_OUT_OFFSET: &str = "out_off";

/// A converted function plus the items it still needs generated.
pub struct Converted<'m> {
    pub function: rcl::Function<'m>,
    /// Helpers called by the body that the caller must still generate.
    pub called: Vec<Ref<Function>>,
    /// Module-level constants read by the body that the caller must still generate.
    pub constants: Vec<Ref<Constant>>,
    /// Lerp helpers called by the body that the caller must still generate.
    pub lerps: Vec<LerpHelper>,
}

/// Adds the uniform leading parameters shared by every density and helper function, then one
/// offset per input the function reads.
fn add_uniform_parameters(f: &mut rcl::Function<'_>, conv: &FunctionConverter<'_>) {
    for p in uniform_parameters() {
        f.add_parameter(p.name, p.t);
    }
    for (_, v) in &conv.offsets {
        f.add_parameter(v.name.clone().unwrap(), v.t.clone());
    }
}

/// Converts a helper function: uniform parameters and offsets, then its own parameters, then the
/// body.
pub fn convert_helper<'a, 'm>(
    program: &'a OptimizedProgram,
    naming: &'a Interface,
    pools: &'a Pools,
    f: Ref<Function>,
) -> Converted<'m> {
    let func = &program.functions[f];
    let mut conv = FunctionConverter::new(program, naming, pools, f);
    let mut out = rcl::Function::new(
        Some(naming.helper(f).to_string()),
        Some(convert_type(&func.return_type)),
    );
    add_uniform_parameters(&mut out, &conv);

    // Helper-specific parameters follow the uniform ones.
    for p in &func.parameters {
        let v = conv.declare_var(*p);
        out.add_parameter(v.name.clone().unwrap(), v.t.clone());
    }
    for (r, _) in func.variables.iter() {
        if func.parameters.contains(&r) {
            continue;
        }
        let v = conv.declare_var(r);
        out.add_variable(v);
    }
    out.body = conv.convert_block(&func.body);
    out.inline = true;

    Converted {
        function: out,
        called: conv.called,
        constants: conv.used_constants,
        lerps: conv.used_lerps,
    }
}

/// Converts a unit's density function for one set of dimensions (named `<name>_d{X}x{Y}x{Z}`).
///
/// It returns the value for the single position `p`; the kernel does the looping.
pub fn convert_density<'a, 'm>(
    program: &'a OptimizedProgram,
    naming: &'a Interface,
    pools: &'a Pools,
    cu: Ref<ComputeUnit>,
    dimensions: (i32, i32, i32),
) -> Converted<'m> {
    let ComputeUnit::DensityFunction {
        function,
        ..
    } = &program.compute_units[cu];
    let func = &program.functions[*function];
    let mut conv = FunctionConverter::new(program, naming, pools, *function);
    let mut out = rcl::Function::new(
        Some(naming.density_fn(cu, dimensions).to_string()),
        Some(convert_type(&func.return_type)),
    );
    add_uniform_parameters(&mut out, &conv);

    for (r, _) in func.variables.iter() {
        let v = conv.declare_var(r);
        out.add_variable(v);
    }
    out.body = conv.convert_block(&func.body);
    out.inline = true;

    Converted {
        function: out,
        called: conv.called,
        constants: conv.used_constants,
        lerps: conv.used_lerps,
    }
}

/// Converts a module-level constant; also returns the constants its initializer references.
pub fn convert_constant<'m>(
    program: &OptimizedProgram,
    naming: &Interface,
    c: Ref<Constant>,
) -> (rcl::Constant<'m>, Vec<Ref<Constant>>) {
    let value = &program.constants[c].value;
    let mut deps = Vec::new();
    const_refs(value, &mut deps);
    (
        rcl::Constant {
            var: constant_variable(program, naming, c),
            value: convert_const_expr(program, naming, value),
        },
        deps,
    )
}

/// `pub fn <kernel>(origin, buffers, out_off, off_.., auto_constants, host_consts)`: builds the
/// `Config`, loops over every position and stores the density function's result at
/// `out_off + index(p)` of the unit's pool. The offsets are those of the unit's main function.
pub fn build_kernel<'m>(
    program: &OptimizedProgram,
    naming: &Interface,
    pools: &Pools,
    inst: &Instance,
) -> rcl::Function<'m> {
    use rcl::Expression as E;
    use rcl::Statement as S;
    let ComputeUnit::DensityFunction { function, .. } = &program.compute_units[inst.cu];
    let return_type = convert_type(&program.functions[*function].return_type);

    let var = |name: &str, t: rcl::Type| {
        Rc::new(rcl::Variable {
            name: Some(name.to_string()),
            t,
            mutable: false,
        })
    };
    let uniform = uniform_kernel_parameters();
    let (buffers_p, auto_p, host_p) = (&uniform[2], &uniform[3], &uniform[4]);
    let vec3 = || rcl::Type::Struct("Vec3".to_string());

    let origin = var("origin", vec3());
    let buffers = var(&buffers_p.name, buffers_p.t.clone());
    let auto_consts = var(&auto_p.name, auto_p.t.clone());
    let host_consts = var(&host_p.name, host_p.t.clone());
    let config = var("config", rcl::Type::Struct(CONFIG_STRUCT_NAME.to_string()));
    let it = var("p", rcl::Type::Struct("PositionIterator".to_string()));
    let result = var("r", return_type.clone());

    let out_off = var(KERNEL_OUT_OFFSET, rcl::Type::Usize);
    let offsets: Vec<_> = naming
        .input_params(*function)
        .iter()
        .map(|&d| var(naming.offset_param(d), rcl::Type::Usize))
        .collect();

    let mut kernel = rcl::Function::new(Some(naming.kernel(inst).to_string()), None);
    kernel.add_parameter("origin".to_string(), vec3());
    for v in [&buffers, &out_off].into_iter().chain(&offsets).chain([&auto_consts, &host_consts]) {
        kernel.add_parameter(v.name.clone().unwrap(), v.t.clone());
    }

    let vec3_new = |(x, y, z): (f64, f64, f64)| E::LateBoundCall {
        function_name: "Vec3::new".to_string(),
        argument_types: vec![rcl::Type::F64; 3],
        return_type: vec3(),
        arguments: vec![E::F64Literal(x), E::F64Literal(y), E::F64Literal(z)],
    };
    let (dx, dy, dz) = inst.dimensions;
    kernel.add_statement(S::Declare {
        variable: config.clone(),
        init: Some(E::StructInit {
            struct_name: CONFIG_STRUCT_NAME.to_string(),
            fields: vec![
                ("origin".to_string(), E::Variable(origin.clone())),
                ("origin_scale".to_string(), vec3_new(inst.scaled_origin())),
                (
                    "position_scale".to_string(),
                    vec3_new(inst.scaled_position()),
                ),
                (
                    "dimensions".to_string(),
                    E::TupleInit(vec![
                        E::I32Literal(dx),
                        E::I32Literal(dy),
                        E::I32Literal(dz),
                    ]),
                ),
            ],
        }),
        mutable: false,
    });

    let call = E::LateBoundCall {
        function_name: naming.density_fn(inst.cu, inst.dimensions).to_string(),
        argument_types: uniform.iter().map(|p| p.t.clone()).collect(),
        return_type: return_type.clone(),
        arguments: [
            E::Variable(it.clone()),
            E::Ref(Box::new(E::Variable(config.clone()))),
            E::Ref(Box::new(E::Deref(Box::new(E::Variable(buffers.clone()))))),
            E::Variable(auto_consts.clone()),
            E::Variable(host_consts.clone()),
        ]
        .into_iter()
        .chain(offsets.iter().map(|v| E::Variable(v.clone())))
        .collect(),
    };
    let index = E::LateBoundCall {
        function_name: "index".to_string(),
        argument_types: vec![it.t.clone()],
        return_type: rcl::Type::Usize,
        arguments: vec![E::Variable(it.clone())],
    };
    // In range by construction, so never the `PerBuffer` form.
    let pool = E::Field {
        base: Box::new(E::Variable(buffers.clone())),
        field: pools.field(&program.functions[*function].return_type).to_string(),
    };
    let at = E::BinaryOp {
        op: rcl::BinaryOperator::Add,
        left: Box::new(E::Variable(out_off.clone())),
        right: Box::new(index),
    };
    let target = match pools.bounds {
        BufferBounds::Unchecked => E::Deref(Box::new(E::Unsafe(Box::new(E::MethodCall {
            receiver: Box::new(pool),
            method: "get_unchecked_mut".into(),
            arguments: vec![at],
        })))),
        _ => E::Index { base: Box::new(pool), index: Box::new(at) },
    };
    let store = S::AssignPlace { target, value: E::Variable(result.clone()) };
    kernel.add_statement(S::ForIn {
        variable: it.clone(),
        iterable: E::LateBoundCall {
            function_name: "iter_3d".to_string(),
            argument_types: vec![rcl::Type::Ref(Box::new(config.t.clone()))],
            return_type: rcl::Type::Struct("PositionIter".to_string()),
            arguments: vec![E::Ref(Box::new(E::Variable(config.clone())))],
        },
        body: vec![
            // S::Declare {
            //     variable: pos3.clone(),
            //     init: Some(E::Field {
            //         base: Box::new(E::Variable(it.clone())),
            //         field: "pos".to_string(),
            //     }),
            //     mutable: false,
            // },
            S::Declare {
                variable: result,
                init: Some(call),
                mutable: false,
            },
            store,
        ],
    });
    kernel
}
