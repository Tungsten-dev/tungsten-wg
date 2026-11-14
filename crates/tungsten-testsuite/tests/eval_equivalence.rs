//! The main property of `specs/unit-eval.md` (§7): on a grid, `eval(u, pos3)` with the grid's
//! scales gives the same bits as reading `u` buffered on that grid through an Identity input.
//!
//! Every corpus file is turned into its eval form: each un-indexed Identity read of an input whose
//! readers all run on the input's own grid becomes `eval(source, pos3)` with the input's scales.
//! The interpreter must give the same cells for every case before and after. Files that expect a
//! rejection, or have refinements (a marched `pos3` reads another cell than the buffer index), are
//! skipped, and so are inputs whose source can't be evaluated (un-indexed or `@previous` reads in
//! its reach, §4.4). Cases where the original fails somewhere are skipped: a buffered input fails
//! eagerly, an evaluated one only in the cells that read it.

use std::collections::HashSet;

use tungsten_check::{EvalOptions, prepare};
use tungsten_ir::model::{ComputeUnit, DensityInput, DensityMapping, Expression, Function, Program, Ref, Statement};
use tungsten_testsuite::corpus::load_all;
use tungsten_testsuite::env::TestEnv;
use tungsten_testsuite::oracle::{fuel, run_sequence, same_entries, sequence_case};

fn each_expr_mut(body: &mut [Statement], f: &mut dyn FnMut(&mut Expression)) {
    fn expr(e: &mut Expression, f: &mut dyn FnMut(&mut Expression)) {
        f(e);
        match e {
            Expression::FunctionCall { parameters: a, .. }
            | Expression::ExternCall { parameters: a, .. }
            | Expression::Construct { args: a, .. }
            | Expression::ArrayLiteral(a) => a.iter_mut().for_each(|x| expr(x, f)),
            Expression::ConstructExtern { args, .. } => args.iter_mut().for_each(|(_, x)| expr(x, f)),
            Expression::DensityVariable(_, Some(i)) => expr(i, f),
            Expression::BinaryOp { left, right, .. } | Expression::ArrayAccess { array: left, index: right } => {
                expr(left, f);
                expr(right, f);
            }
            Expression::UnaryOp { operand: x, .. }
            | Expression::Field { base: x, .. }
            | Expression::ExplicitCast { expr: x, .. }
            | Expression::Eval { position: x, .. } => expr(x, f),
            _ => {}
        }
    }
    for s in body {
        match s {
            Statement::Assign { value, .. } | Statement::Return(value) => expr(value, f),
            Statement::If { condition, then_branch, else_branch } => {
                expr(condition, f);
                each_expr_mut(then_branch, f);
                each_expr_mut(else_branch, f);
            }
            Statement::While { condition, body } => {
                expr(condition, f);
                each_expr_mut(body, f);
            }
            Statement::Repeat { body, .. } => each_expr_mut(body, f),
            Statement::Break => {}
        }
    }
}

fn reads(p: &Program, f: Ref<Function>, out: &mut Vec<Expression>) {
    let mut body = p.functions[f].body.clone();
    each_expr_mut(&mut body, &mut |e| out.push(e.clone()));
}

/// The unit's main and everything it calls.
fn reach(p: &Program, unit: Ref<ComputeUnit>) -> Vec<Ref<Function>> {
    let ComputeUnit::DensityFunction { function, .. } = &p.compute_units[unit];
    let mut out = vec![*function];
    let mut i = 0;
    while i < out.len() {
        let mut es = vec![];
        reads(p, out[i], &mut es);
        i += 1;
        for e in es {
            if let Expression::FunctionCall { function, .. } = e
                && !out.contains(&function)
            {
                out.push(function);
            }
        }
    }
    out
}

/// Whether `unit` can be evaluated: nothing in its reach reads an input without an index or
/// `@previous`, and nothing evaluates (keeps the form simple, and cycles out).
fn evaluable(p: &Program, unit: Ref<ComputeUnit>) -> bool {
    reach(p, unit).into_iter().all(|f| {
        let mut es = vec![];
        reads(p, f, &mut es);
        es.iter().all(|e| match e {
            Expression::DensityVariable(r, i) => i.is_some() && !p.density_inputs[*r].previous,
            Expression::Eval { .. } => false,
            _ => true,
        })
    })
}

/// The dims every grid `unit` runs on: its entries and the inputs that read it.
fn grids(p: &Program, unit: Ref<ComputeUnit>) -> Vec<(i32, i32, i32)> {
    let entries = p.entry_points.iter().filter(|e| e.density_function == unit).map(|e| e.dimensions);
    let inputs = p.density_inputs.values().filter(|d| d.density_function == unit).map(|d| d.dimensions);
    entries.chain(inputs).collect()
}

/// Rewrites `p` in place; returns how many reads became evals.
fn to_eval_form(p: &mut Program) -> usize {
    let mut chosen: HashSet<Ref<DensityInput>> = HashSet::new();
    for (r, d) in p.density_inputs.iter() {
        if d.previous || !d.mapping.is_identity_on(d.dimensions) || d.mapping != DensityMapping::Identity {
            continue;
        }
        if !evaluable(p, d.density_function) {
            continue;
        }
        let readers = p.compute_units.iter().filter(|(_, u)| {
            let ComputeUnit::DensityFunction { density_inputs, .. } = u;
            density_inputs.contains(&r)
        });
        let mut on_grid = true;
        for (u, _) in readers {
            on_grid &= grids(p, u).iter().all(|g| *g == d.dimensions);
        }
        if on_grid {
            chosen.insert(r);
        }
    }
    let inputs: Vec<(Ref<DensityInput>, DensityInput)> =
        p.density_inputs.iter().filter(|(r, _)| chosen.contains(r)).map(|(r, d)| (r, d.clone())).collect();
    let mut n = 0;
    for f in p.functions.values_mut() {
        each_expr_mut(&mut f.body, &mut |e| {
            if let Expression::DensityVariable(r, None) = e
                && let Some((_, d)) = inputs.iter().find(|(x, _)| x == r)
            {
                *e = Expression::Eval {
                    unit: d.density_function,
                    position: Box::new(Expression::Ambient(tungsten_ir::model::AmbientKind::Pos3)),
                    origin_scale: d.scaled_origin,
                    position_scale: d.scaled_position,
                };
                n += 1;
            }
        });
    }
    n
}

#[test]
fn eval_form_matches_buffered_reads_over_the_corpus() {
    let env = TestEnv::new();
    let (mut files, mut cases, mut reads) = (0, 0, 0);
    let mut failures = vec![];
    for loaded in load_all(&env) {
        let Ok(mut tf) = loaded.result else { continue };
        // Large files take minutes in a debug build; they are covered by the corpus trials.
        if tf.reject.is_some() || !tf.program.refinements.is_empty() || tf.tier.is_some() {
            continue;
        }
        let opts = EvalOptions { fuel: fuel(&tf), memo: true };
        let Ok(original) = prepare(&tf.program, &env.registry) else { continue };
        let mut before = vec![];
        for i in 0..tf.cases.len() {
            let Ok((named, params)) = sequence_case(&tf, i, &env) else { break };
            match run_sequence(&original, &named, &params, opts) {
                Ok(r) => before.push(r.run),
                Err(_) => break,
            }
        }
        if before.len() != tf.cases.len() {
            continue;
        }
        let replaced = to_eval_form(&mut tf.program);
        if replaced == 0 {
            continue;
        }
        let name = loaded.file.path.display().to_string();
        let prepared = match prepare(&tf.program, &env.registry) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{name}: eval form rejected:\n{e}"));
                continue;
            }
        };
        files += 1;
        reads += replaced;
        for (i, c) in tf.cases.iter().enumerate() {
            if !before[i].failures.is_empty() || before[i].entries.iter().flatten().any(|c| c.is_err()) {
                continue;
            }
            let (named, params) = sequence_case(&tf, i, &env).unwrap();
            match run_sequence(&prepared, &named, &params, opts) {
                Ok(r) => {
                    cases += 1;
                    if let Err(m) = same_entries(&before[i], &r.run) {
                        failures.push(format!("{name}: case `{}`: eval form differs: {m}", c.name));
                    }
                }
                Err(e) => failures.push(format!("{name}: case `{}`: {e}", c.name)),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // The corpus has many eligible reads; a regression in the transform shouldn't pass silently.
    assert!(files >= 10 && cases >= 10, "only {files} files, {cases} cases, {reads} reads were checked");
    eprintln!("eval form: {files} files, {reads} reads, {cases} cases");
}
