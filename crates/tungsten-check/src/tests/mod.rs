//! Lowering tests. Inputs are `.tw` snippets through `tungsten_parse::load`, plus
//! hand-built programs for what text can't express. Every successful lowering goes through
//! [`ok`], which runs `check_wf` and the checker.

mod auto;
mod check_errors;
mod check_rejects;
mod determinism;
mod errors;
mod eval;
mod ids;
mod map;
mod memo;
mod no_panic;
mod normalization;
mod registry;
mod stuck_mutation;
mod wf_mirror;

use tungsten_core::{Expr, FnId, Stmt};
use tungsten_ir::*;
use tungsten_parse::{Env, ExternSig, FileId};

use crate::{AutoArgType, Lowered, Registry, RegistryBuilder, lower};

/// Knows `sin`, `sample` and the record `Pair { a: f64, b: i32 }`.
pub struct TestEnv;

impl Env for TestEnv {
    fn extern_fn(&self, name: &str) -> Option<ExternSig> {
        match name {
            "sin" => Some(ExternSig {
                parameter_types: vec![VariableType::F64],
                return_type: VariableType::F64,
            }),
            "sample" => Some(ExternSig {
                parameter_types: vec![VariableType::Handle("PerlinNoiseSampler"), VariableType::F64],
                return_type: VariableType::F64,
            }),
            _ => None,
        }
    }
    fn extern_type(&self, name: &str) -> Option<Vec<(&'static str, VariableType)>> {
        match name {
            "Pair" => Some(vec![("a", VariableType::F64), ("b", VariableType::I32)]),
            _ => None,
        }
    }
    fn library(&self, _: &str) -> Option<(FileId, &str)> {
        None
    }
}

/// The initializers of the shared corpus programs.
pub fn corpus_initializers(b: RegistryBuilder) -> RegistryBuilder {
    use AutoArgType::I64;
    b.auto_initializer(
        "make_permutation_table",
        &[I64, I64, I64, I64, I64, I64],
        VariableType::Handle("PerlinNoiseSampler"),
    )
    .auto_initializer("make_base3d_perm_table", &[I64], VariableType::Handle("InterpolatedNoiseSampler"))
    .auto_initializer("make_perm", &[I64, I64], VariableType::Handle("PerlinNoiseSampler"))
    .auto_initializer("make_base3d", &[I64], VariableType::Handle("InterpolatedNoiseSampler"))
}

/// The registry matching [`TestEnv`].
pub fn registry() -> Registry {
    corpus_initializers(RegistryBuilder::new())
        .extern_fn("sin", &[VariableType::F64], VariableType::F64)
        .extern_fn(
            "sample",
            &[VariableType::Handle("PerlinNoiseSampler"), VariableType::F64],
            VariableType::F64,
        )
        .record("Pair", &[("a", VariableType::F64), ("b", VariableType::I32)])
        .build()
        .unwrap()
}

pub fn load(src: &str) -> OptimizedProgram {
    tungsten_parse::load(src, FileId(0), &TestEnv)
        .unwrap_or_else(|d| panic!("{d}\n{src}"))
        .program
}

/// A program with a host `h`, an i64 param `seed`, autoconsts `t` (named `n`) and `b3`, a const `c`, a `leaf` unit, and a
/// unit `u` whose 0-param main `top_main` returns `top(1.0)`, where
/// `fn top(p: f64) -> f64 @hosts(h) { <body> }`, with input `d` reading `leaf_u`. `decls` go
/// before `top`; every fn they declare is listed in `u`'s helpers, next to `top`.
pub fn snippet(decls: &str, body: &str) -> String {
    let mut helpers = vec!["top".to_string()];
    let mut rest = decls;
    while let Some(i) = rest.find("fn ") {
        let after = &rest[i + 3..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let boundary = i == 0 || !rest.as_bytes()[i - 1].is_ascii_alphanumeric();
        if boundary && !name.is_empty() {
            helpers.push(name);
        }
        rest = after;
    }
    let helpers = helpers.join(", ");
    format!(
        "program {{
    host h: f64;
    param seed: i64;
    auto t: handle PerlinNoiseSampler @name(\"n\") = make_perm(param(seed), 1L);
    auto b3: handle InterpolatedNoiseSampler = make_base3d(param(seed));
    const c = 2.0;
    {decls}
    fn leaf() -> f64 {{ return 1.0; }}
    fn top(p: f64) -> f64 @hosts(h) {{ {body} }}
    fn top_main() -> f64 {{ return top(1.0); }}
    input d = leaf_u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    unit leaf_u {{ main leaf; }}
    unit u {{
        main top_main;
        helpers [{helpers}];
        hosts [h];
        autos [t];
        consts [c];
        inputs [d];
    }}
    entry e = u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}}"
    )
}

/// Lowers, expecting success, and checks wf. Then runs the checker: a rejection must be listed
/// in `check_rejects.rs`.
pub fn ok(p: &OptimizedProgram, registry: &Registry) -> Lowered {
    let l = lower(p, registry).unwrap_or_else(|e| panic!("lowering failed:\n{e}"));
    let wf = tungsten_core::check::wf::check_wf(&l.program);
    assert!(wf.is_empty(), "lowered program is not wf: {wf:?}");
    if let Err(e) = crate::check_lowered(&l) {
        check_rejects::allow(&e);
    }
    l
}

/// Lowers, expecting failure; the errors as `Display` lines.
pub fn errs(p: &OptimizedProgram, registry: &Registry) -> Vec<String> {
    match lower(p, registry) {
        Ok(_) => panic!("lowering succeeded"),
        Err(e) => e.0.iter().map(|e| e.to_string()).collect(),
    }
}

pub fn fn_id(l: &Lowered, name: &str) -> FnId {
    l.map
        .functions
        .iter()
        .find(|(_, _, n)| *n == name)
        .unwrap_or_else(|| panic!("no fn `{name}`"))
        .0
}

/// The expression of the last `return` in `top`.
pub fn main_ret(l: &Lowered) -> &Expr {
    let f = &l.program.functions[fn_id(l, "top").0];
    match f.body.last() {
        Some(Stmt::Return(e)) => e,
        other => panic!("top doesn't end in a return: {other:?}"),
    }
}

/// Lowers `snippet("", body)` against the test registry and returns main's return expression.
pub fn ret(body: &str) -> Expr {
    let r = registry();
    let l = ok(&load(&snippet("", body)), &r);
    main_ret(&l).clone()
}

/// An empty program to build on by hand.
pub fn empty() -> OptimizedProgram {
    OptimizedProgram {
        compute_units: Arena::new(),
        functions: Arena::new(),
        entry_points: vec![],
        refinements: vec![],
        constants: Arena::new(),
        host_inputs: Arena::new(),
        auto_params: Arena::new(),
        auto_consts: Arena::new(),
        density_inputs: Arena::new(),
    }
}

/// A hand-built function with the given vars (in slot order), params and body.
pub fn function(
    name: &str,
    vars: &[(&str, VariableType)],
    params: &[usize],
    ret: VariableType,
    body: impl FnOnce(&[Ref<Var>]) -> Vec<Statement>,
) -> Function {
    let mut variables = Arena::new();
    let refs: Vec<Ref<Var>> = vars
        .iter()
        .map(|(n, t)| {
            variables.insert(Var {
                name: Name::Named(n.to_string()),
                t: t.clone(),
            })
        })
        .collect();
    Function {
        canonical_name: Some(name.to_string()),
        host_inputs: vec![],
        body: body(&refs),
        parameters: params.iter().map(|&i| refs[i]).collect(),
        variables,
        return_type: ret,
        source_hash: 0,
    }
}

/// A unit with just a main function and no inputs or lists.
pub fn unit(main: Ref<Function>) -> ComputeUnit {
    ComputeUnit::DensityFunction {
        function: main,
        density_inputs: vec![],
        auto_consts: vec![],
        host_inputs: vec![],
        helper_functions: vec![],
        constants: vec![],
        source_hash: 0,
    }
}

pub fn entry(u: Ref<ComputeUnit>) -> EntryPoint {
    EntryPoint {
        density_function: u,
        dimensions: (1, 1, 1),
        scaled_origin: (1.0, 1.0, 1.0),
        scaled_position: (1.0, 1.0, 1.0),
    }
}
