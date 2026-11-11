/*
Emission of density functions, helper functions, constants and per-instance kernels.
Every density and helper function takes the uniform parameters first (see `uniform_parameters`).
*/

use std::collections::HashMap;

use super::converter::FunctionConverter;
use super::expression::const_to_expression;
use super::types::convert_type;
use super::{
    AUTO_CONSTS_CLASS, CONFIG_CLASS, HOST_CONSTS_CLASS, INTERNAL_BUFFERS_CLASS, PARAM_AUTO_CONSTS,
    PARAM_BUFFERS, PARAM_CONFIG, PARAM_HOST_CONSTS, PARAM_POS, POSITION_ITERATOR_CLASS, uniform_arguments,
    uniform_parameters,
};
use tungsten_interface::naming::NamingRules;
use crate::pools::Pools;
use tungsten_interface::{Instance, Interface, LerpHelper, RefineRun};
use crate::model as java;
use tungsten_ir::model::VariableType;
use tungsten_ir::model::{Arena, ComputeUnit, Constant, Function, OptimizedProgram, Ref};
use tungsten_rust::expression::const_refs;
use tungsten_rust::types::derive_const_type;

use java::Expression as J;

/// A converted function plus the items it still needs generated.
pub struct Converted {
    pub method: java::Method,
    /// Helpers called by the body that the caller must still generate.
    pub called: Vec<Ref<Function>>,
    /// Module-level constants read by the body that the caller must still generate.
    pub constants: Vec<Ref<Constant>>,
    /// Lerp helpers called by the body that the caller must still generate.
    pub lerps: Vec<LerpHelper>,
}

/// Name of a kernel's own slot offset.
pub const KERNEL_OUT_OFFSET: &str = "out_off";

/// The `int` offset parameter of every input `f` reads, after the uniform parameters.
fn offset_parameters(naming: &Interface, f: Ref<Function>) -> Vec<(java::Type, String)> {
    naming.input_params(f).iter().map(|&d| (java::Type::Int, naming.offset_param(d).to_string())).collect()
}

/// Declares every local bare at the top of the function. Initializing them is the frontend's job
/// (javac reports violations).
fn declare_locals(conv: &mut FunctionConverter<'_>, func: &Function) -> Vec<java::Statement> {
    let mut out = Vec::new();
    for (r, var) in func.variables.iter() {
        if func.parameters.contains(&r) {
            continue;
        }
        let name = conv.declare_var(r);
        out.push(java::Statement::Declare {
            t: convert_type(&var.t),
            name,
            init: None,
            is_final: false,
        });
    }
    out
}

fn finish(conv: FunctionConverter<'_>, method: java::Method) -> Converted {
    Converted { method, called: conv.called, constants: conv.used_constants, lerps: conv.used_lerps }
}

/// Converts a helper function: uniform parameters and offsets, then its own parameters, then the body.
pub fn convert_helper(
    program: &OptimizedProgram,
    naming: &Interface,
    pools: &Pools,
    rules: &NamingRules,
    class_name: &str,
    f: Ref<Function>,
) -> Converted {
    let func = &program.functions[f];
    let mut conv = FunctionConverter::new(program, naming, pools, rules, class_name, func).with_offsets(f);
    let mut params = uniform_parameters();
    params.extend(offset_parameters(naming, f));
    // Helper-specific parameters follow the uniform ones.
    for p in &func.parameters {
        let name = conv.declare_var(*p);
        params.push((conv.java_var_type(*p), name));
    }
    let mut body = declare_locals(&mut conv, func);
    body.extend(conv.convert_block(&func.body));
    let method = java::Method {
        doc: func.canonical_name.clone(),
        modifiers: "static".into(),
        return_type: Some(convert_type(&func.return_type)),
        name: naming.helper(f).to_string(),
        params,
        body,
    };
    finish(conv, method)
}

/// Converts a unit's density function for one set of dimensions (named `<name>_d{X}x{Y}x{Z}`).
///
/// It returns the value for the single position `p`; the kernel does the looping.
pub fn convert_density(
    program: &OptimizedProgram,
    naming: &Interface,
    pools: &Pools,
    rules: &NamingRules,
    class_name: &str,
    cu: Ref<ComputeUnit>,
    dimensions: (i32, i32, i32),
) -> Converted {
    let ComputeUnit::DensityFunction { function, source_hash, .. } = &program.compute_units[cu];
    let func = &program.functions[*function];
    let mut conv = FunctionConverter::new(program, naming, pools, rules, class_name, func).with_offsets(*function);
    let mut body = declare_locals(&mut conv, func);
    body.extend(conv.convert_block(&func.body));
    let doc = format!(
        "{} (source hash {:x})",
        func.canonical_name.as_deref().unwrap_or("density function"),
        source_hash
    );
    let method = java::Method {
        doc: Some(doc),
        modifiers: "static".into(),
        return_type: Some(convert_type(&func.return_type)),
        name: naming.density_fn(cu, dimensions).to_string(),
        params: uniform_parameters().into_iter().chain(offset_parameters(naming, *function)).collect(),
        body,
    };
    finish(conv, method)
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

/// `static final <T> <name> = <init>;` for constant `c`.
pub fn convert_constant(
    program: &OptimizedProgram,
    naming: &Interface,
    pools: &Pools,
    rules: &NamingRules,
    class_name: &str,
    c: Ref<Constant>,
) -> java::Field {
    let constant = &program.constants[c];
    let t = derive_const_type(program, &constant.value)
        .unwrap_or_else(|| panic!("cannot derive the type of constant '{}'", constant.name));
    let function = empty_function();
    let mut conv = FunctionConverter::new(program, naming, pools, rules, class_name, &function);
    let init = conv.convert_to_type(&const_to_expression(&constant.value), &t);
    java::Field {
        modifiers: "static final".into(),
        t: convert_type(&t),
        name: naming.constant(c).to_string(),
        init: Some(init),
    }
}

/// `constants` reordered so every constant follows the ones its initializer reads; Java rejects
/// forward references between static fields.
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
    let mut state = HashMap::new();
    let mut out = Vec::new();
    for c in constants {
        visit(program, *c, &mut state, &mut out);
    }
    out
}

fn vec3(x: java::Expression, y: java::Expression, z: java::Expression) -> java::Expression {
    J::New { class: "Vec3".into(), args: vec![x, y, z], comment: None }
}

fn vec3_literal((x, y, z): (f64, f64, f64)) -> java::Expression {
    vec3(J::DoubleLit(x), J::DoubleLit(y), J::DoubleLit(z))
}

/// `static void <kernel>(origin_x, origin_y, origin_z, buffers, out_off, off_.., auto_constants,
/// host_consts)`: builds the `Config`, loops over every position (x minor, y middle, z major,
/// matching RCL's `iter_3d`) and stores the density function's result at `out_off + idx` of the
/// unit's pool. The offsets are those of the unit's main function.
pub fn build_kernel(program: &OptimizedProgram, naming: &Interface, pools: &Pools, inst: &Instance) -> java::Method {
    kernel(program, naming, pools, inst, inst.cu, naming.kernel(inst), false)
}

/// The kernel of one refine run (`specs/refinement.md` §6.2): `build_kernel` with `ox, oy, oz`
/// after the origin. It runs `run.unit` over the target's grid and adds the offset to the
/// iterator's `pos` only, so the store index stays in the buffer.
pub fn build_refine_kernel(
    program: &OptimizedProgram,
    naming: &Interface,
    pools: &Pools,
    run: &RefineRun,
) -> java::Method {
    kernel(program, naming, pools, &run.target, run.unit, naming.refine_kernel(run), true)
}

/// `unit` is the one called per cell, and `target` the instance whose grid and buffer it fills.
fn kernel(
    program: &OptimizedProgram,
    naming: &Interface,
    pools: &Pools,
    inst: &Instance,
    unit: Ref<ComputeUnit>,
    name: &str,
    refine: bool,
) -> java::Method {
    use java::Statement as S;
    let ComputeUnit::DensityFunction { function, .. } = &program.compute_units[unit];
    let ComputeUnit::DensityFunction { function: target_function, .. } = &program.compute_units[inst.cu];
    let target_type = &program.functions[*target_function].return_type;
    let element = convert_type(target_type);
    let (dx, dy, dz) = inst.dimensions;
    let class = |n: &str| java::Type::class(n);
    let offsets = offset_parameters(naming, *function);

    let mut body = vec![
        S::Declare {
            t: class(CONFIG_CLASS),
            name: PARAM_CONFIG.into(),
            init: Some(J::New {
                class: CONFIG_CLASS.into(),
                args: vec![
                    vec3(J::name("origin_x"), J::name("origin_y"), J::name("origin_z")),
                    vec3_literal(inst.scaled_origin()),
                    vec3_literal(inst.scaled_position()),
                    J::IntLit(dx),
                    J::IntLit(dy),
                    J::IntLit(dz),
                ],
                comment: None,
            }),
            is_final: true,
        },
        S::Declare {
            t: class(POSITION_ITERATOR_CLASS),
            name: PARAM_POS.into(),
            init: Some(J::New {
                class: POSITION_ITERATOR_CLASS.into(),
                args: vec![J::name(PARAM_CONFIG)],
                comment: None,
            }),
            is_final: true,
        },
        S::Declare {
            t: java::Type::Array(Box::new(element)),
            name: "out".into(),
            init: Some(J::field(J::name(PARAM_BUFFERS), pools.field(target_type))),
            is_final: true,
        },
        S::Declare { t: java::Type::Int, name: "idx".into(), init: Some(J::IntLit(0)), is_final: false },
    ];

    // The running `idx` replaces a per-element index computation, like RCL's `iter_3d`. A refine
    // call moves `pos` and leaves `idx`, which the store and unindexed reads use.
    let moved = |axis: &str, offset: &str| {
        if refine { J::binary(java::BinaryOperator::Add, J::name(axis), J::name(offset)) } else { J::name(axis) }
    };
    let arguments = uniform_arguments()
        .into_iter()
        .chain(offsets.iter().map(|(_, n)| J::name(n)))
        .collect();
    let inner = vec![
        S::Expr(J::call(
            Some(J::name(PARAM_POS)),
            "set",
            vec![moved("ix", "ox"), moved("iy", "oy"), moved("iz", "oz"), J::name("idx")],
        )),
        // In range by construction, so never the `PerBuffer` form.
        S::Assign {
            target: J::Index {
                base: Box::new(J::name("out")),
                index: Box::new(J::binary(java::BinaryOperator::Add, J::name(KERNEL_OUT_OFFSET), J::name("idx"))),
            },
            value: J::call(None, naming.density_fn(unit, inst.dimensions), arguments),
        },
        S::Assign {
            target: J::name("idx"),
            value: J::binary(java::BinaryOperator::Add, J::name("idx"), J::IntLit(1)),
        },
    ];
    let x_loop = S::ForRange { var: "ix".into(), end: J::IntLit(dx), body: inner };
    let y_loop = S::ForRange { var: "iy".into(), end: J::IntLit(dy), body: vec![x_loop] };
    body.push(S::ForRange { var: "iz".into(), end: J::IntLit(dz), body: vec![y_loop] });

    let mut params = vec![
        (java::Type::Double, "origin_x".into()),
        (java::Type::Double, "origin_y".into()),
        (java::Type::Double, "origin_z".into()),
    ];
    if refine {
        params.extend(["ox", "oy", "oz"].map(|n| (java::Type::Int, n.to_string())));
    }
    params.push((class(INTERNAL_BUFFERS_CLASS), PARAM_BUFFERS.into()));
    params.push((java::Type::Int, KERNEL_OUT_OFFSET.into()));
    params.extend(offsets);
    params.push((class(AUTO_CONSTS_CLASS), PARAM_AUTO_CONSTS.into()));
    params.push((class(HOST_CONSTS_CLASS), PARAM_HOST_CONSTS.into()));
    java::Method {
        doc: None,
        modifiers: "static".into(),
        return_type: Some(java::Type::Void),
        name: name.to_string(),
        params,
        body,
    }
}
