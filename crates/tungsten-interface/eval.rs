//! `eval(unit, p)` (`specs/unit-eval.md` §5.2, §5.3): eval keys reached from the scheduled units,
//! their generated functions and helper clones, and the inputs a reader reaches through them.

use std::collections::HashSet;

use tungsten_ir::model::{
    ComputeUnit, DensityInput, EvalKey, Expression, Function, Program, Ref, Statement,
};

/// One generated eval function: an eval key and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalFn {
    pub key: EvalKey,
    /// Fixed in the module namespace, e.g. `eval_ore_noise_s1x1x1_1x1x1`.
    pub name: String,
    /// The helpers the evaluated main reaches through calls, in discovery order, each with the
    /// name of its clone for this key.
    pub helpers: Vec<(Ref<Function>, String)>,
}

/// Calls `f` on every expression of `body`, parents first, with an explicit stack: expressions
/// can nest deeper than the thread's stack allows. An `Eval`'s position is part of the body.
pub(crate) fn each_expr<'a>(body: &'a [Statement], f: &mut dyn FnMut(&'a Expression)) {
    let mut stmts: Vec<&Statement> = body.iter().rev().collect();
    let mut exprs: Vec<&Expression> = vec![];
    while let Some(s) = stmts.pop() {
        match s {
            Statement::Assign { value: e, .. } | Statement::Return(e) => exprs.push(e),
            Statement::If { condition, then_branch, else_branch } => {
                exprs.push(condition);
                stmts.extend(then_branch.iter().chain(else_branch).rev());
            }
            Statement::While { condition, body } => {
                exprs.push(condition);
                stmts.extend(body.iter().rev());
            }
            Statement::Repeat { body, .. } => stmts.extend(body.iter().rev()),
            Statement::Break => {}
        }
        while let Some(e) = exprs.pop() {
            f(e);
            let before = exprs.len();
            match e {
                Expression::DensityVariable(_, index) => exprs.extend(index.as_deref()),
                Expression::FunctionCall { parameters: args, .. }
                | Expression::ExternCall { parameters: args, .. }
                | Expression::Construct { args, .. }
                | Expression::ArrayLiteral(args) => exprs.extend(args),
                Expression::ConstructExtern { args, .. } => exprs.extend(args.iter().map(|(_, a)| a)),
                Expression::BinaryOp { left, right, .. } => exprs.extend([&**left, &**right]),
                Expression::ArrayAccess { array, index } => exprs.extend([&**array, &**index]),
                Expression::UnaryOp { operand: x, .. }
                | Expression::Field { base: x, .. }
                | Expression::ExplicitCast { expr: x, .. }
                | Expression::Eval { position: x, .. } => exprs.push(x),
                Expression::Constant(_)
                | Expression::Variable(_)
                | Expression::Float(_)
                | Expression::Double(_)
                | Expression::Byte(_)
                | Expression::Int(_)
                | Expression::Long(_)
                | Expression::Bool(_)
                | Expression::AutoConst(_)
                | Expression::HostInput(_)
                | Expression::Ambient(_) => {}
            }
            // Children in written order.
            exprs[before..].reverse();
        }
    }
}

/// The main function of `unit`, if both resolve.
pub(crate) fn unit_main(program: &Program, unit: Ref<ComputeUnit>) -> Option<Ref<Function>> {
    let ComputeUnit::DensityFunction { function, .. } = program.compute_units.try_get(unit)?;
    program.functions.try_get(*function).map(|_| *function)
}

/// `main` and every function it reaches through calls, `main` first, then in first-call order.
pub(crate) fn call_reach(program: &Program, main: Ref<Function>) -> Vec<Ref<Function>> {
    let mut out = vec![main];
    let mut i = 0;
    while i < out.len() {
        let Some(f) = program.functions.try_get(out[i]) else { break };
        i += 1;
        each_expr(&f.body, &mut |e| {
            if let Expression::FunctionCall { function, .. } = e
                && program.functions.try_get(*function).is_some()
                && !out.contains(function)
            {
                out.push(*function);
            }
        });
    }
    out
}

fn evals_in(program: &Program, fns: &[Ref<Function>], out: &mut Vec<EvalKey>) {
    for &f in fns {
        let Some(f) = program.functions.try_get(f) else { continue };
        each_expr(&f.body, &mut |e| {
            if let Expression::Eval { unit, origin_scale, position_scale, .. } = e {
                let k = EvalKey::new(*unit, *origin_scale, *position_scale);
                if !out.contains(&k) && unit_main(program, *unit).is_some() {
                    out.push(k);
                }
            }
        });
    }
}

/// The functions a unit's code runs: what its main reaches through calls, then its listed
/// helpers.
fn unit_fns(program: &Program, unit: Ref<ComputeUnit>) -> Vec<Ref<Function>> {
    let Some(main) = unit_main(program, unit) else { return vec![] };
    let mut fns = call_reach(program, main);
    let ComputeUnit::DensityFunction { helper_functions, .. } = &program.compute_units[unit];
    for h in helper_functions {
        if program.functions.try_get(*h).is_some() && !fns.contains(h) {
            fns.push(*h);
        }
    }
    fns
}

/// The eval keys `units` reach, through nested evals too, in discovery order.
pub(crate) fn reached_keys(program: &Program, units: impl IntoIterator<Item = Ref<ComputeUnit>>) -> Vec<EvalKey> {
    let mut out = vec![];
    for u in units {
        evals_in(program, &unit_fns(program, u), &mut out);
    }
    let mut i = 0;
    while i < out.len() {
        let k = out[i];
        i += 1;
        if let Some(main) = unit_main(program, k.unit) {
            evals_in(program, &call_reach(program, main), &mut out);
        }
    }
    out
}

/// The inputs read inside the bodies `cu` evaluates, transitively, in discovery order.
pub(crate) fn eval_inputs(program: &Program, cu: Ref<ComputeUnit>) -> Vec<Ref<DensityInput>> {
    let mut seen_units = HashSet::new();
    let mut out: Vec<Ref<DensityInput>> = vec![];
    for k in reached_keys(program, [cu]) {
        if !seen_units.insert(k.unit) {
            continue;
        }
        let Some(main) = unit_main(program, k.unit) else { continue };
        for f in call_reach(program, main) {
            let Some(f) = program.functions.try_get(f) else { continue };
            each_expr(&f.body, &mut |e| {
                if let Expression::DensityVariable(r, _) = e
                    && program.density_inputs.try_get(*r).is_some()
                    && !out.contains(r)
                {
                    out.push(*r);
                }
            });
        }
    }
    out
}

/// One scale component in a name: an integer as is (`m` for a minus), anything else as its bits.
fn scale_part(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 && !(v == 0.0 && v.is_sign_negative()) {
        let i = v as i64;
        if i < 0 { format!("m{}", -i) } else { i.to_string() }
    } else {
        format!("h{:016x}", v.to_bits())
    }
}

/// `s<ox>x<oy>x<oz>_<px>x<py>x<pz>`, stable for the same bits.
pub(crate) fn scale_suffix(k: &EvalKey) -> String {
    let (o, p) = (k.origin_scale(), k.position_scale());
    format!(
        "s{}x{}x{}_{}x{}x{}",
        scale_part(o.0),
        scale_part(o.1),
        scale_part(o.2),
        scale_part(p.0),
        scale_part(p.1),
        scale_part(p.2)
    )
}

/// What a backend can't emit on the unchecked path (`specs/unit-eval.md` §5.4): an evaluated body
/// that reaches its own unit again through `eval` (it would recurse), or reads an input without
/// an index (there is no cell to map from). Validation rejects both first.
pub(crate) fn check_emittable(program: &Program, keys: &[EvalFn]) -> Result<(), String> {
    let name = |u: Ref<ComputeUnit>| {
        unit_main(program, u)
            .and_then(|f| program.functions[f].canonical_name.clone())
            .unwrap_or_else(|| "a unit".to_string())
    };
    for e in keys {
        let Some(main) = unit_main(program, e.key.unit) else { continue };
        let reach = call_reach(program, main);
        let mut unindexed = None;
        for &f in &reach {
            each_expr(&program.functions[f].body, &mut |x| {
                if let Expression::DensityVariable(r, None) = x {
                    unindexed.get_or_insert(*r);
                }
            });
        }
        if unindexed.is_some() {
            return Err(format!("`{}` is evaluated, but reads an input without an index", name(e.key.unit)));
        }
        let mut nested = vec![];
        evals_in(program, &reach, &mut nested);
        let mut i = 0;
        while i < nested.len() {
            let k = nested[i];
            i += 1;
            if k.unit == e.key.unit {
                return Err(format!("`{}` evaluates itself through `eval` (a cycle)", name(e.key.unit)));
            }
            if let Some(m) = unit_main(program, k.unit) {
                evals_in(program, &call_reach(program, m), &mut nested);
            }
        }
    }
    Ok(())
}
