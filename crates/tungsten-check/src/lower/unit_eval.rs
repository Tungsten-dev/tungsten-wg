//! `eval(unit, p)` (`specs/unit-eval.md` §5.1, §6): the eval keys, their cloned functions, the
//! inputs and lists a reader reaches through them, and the validation.
//!
//! Core has no `eval`. Each eval key gets a clone of the evaluated unit's main function and of
//! every helper that main reaches through calls. A clone takes an extra first parameter, the
//! position, and reads it where the source reads `Pos3`. Its scale ambients are the key's
//! literals, `Origin` stays the run's. An `Eval` lowers to a call of the key's main clone, and
//! a call inside a clone to the clone of the callee for the same key.

use std::collections::{HashMap, HashSet, VecDeque};

use tungsten_core::{Expr, FnId, Lit, LocalId};
use tungsten_ir::{
    ComputeUnit, DensityInput, EvalKey, Expression, Function, Program as IrProgram, Ref, Statement,
};

use super::mapping::listed_inputs;
use super::{Ctx, get};
use crate::error::{Item, LowerErrorKind, Step};
use crate::map::{EvalFnInfo, LowerMap};

/// Calls `f` on every expression of `body`, parents first. An `Eval`'s position is part of the
/// body; the evaluated unit isn't.
pub(super) fn each_expr(body: &[Statement], f: &mut dyn FnMut(&Expression)) {
    for s in body {
        match s {
            Statement::Assign { value, .. } | Statement::Return(value) => expr(value, f),
            Statement::If { condition, then_branch, else_branch } => {
                expr(condition, f);
                each_expr(then_branch, f);
                each_expr(else_branch, f);
            }
            Statement::While { condition, body } => {
                expr(condition, f);
                each_expr(body, f);
            }
            Statement::Repeat { body, .. } => each_expr(body, f),
            Statement::Break => {}
        }
    }
}

fn expr(e: &Expression, f: &mut dyn FnMut(&Expression)) {
    f(e);
    match e {
        Expression::FunctionCall { parameters, .. } | Expression::ExternCall { parameters, .. } => {
            parameters.iter().for_each(|a| expr(a, f))
        }
        Expression::DensityVariable(_, Some(i)) => expr(i, f),
        Expression::BinaryOp { left, right, .. } => {
            expr(left, f);
            expr(right, f);
        }
        Expression::UnaryOp { operand, .. } => expr(operand, f),
        Expression::Field { base, .. } => expr(base, f),
        Expression::ArrayAccess { array, index } => {
            expr(array, f);
            expr(index, f);
        }
        Expression::Construct { args, .. } | Expression::ArrayLiteral(args) => args.iter().for_each(|a| expr(a, f)),
        Expression::ConstructExtern { args, .. } => args.iter().for_each(|(_, a)| expr(a, f)),
        Expression::ExplicitCast { expr: e, .. } => expr(e, f),
        Expression::Eval { position, .. } => expr(position, f),
        Expression::DensityVariable(_, None)
        | Expression::Constant(_)
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
}

/// A scale.
type Triple = (f64, f64, f64);

fn push<T: PartialEq>(v: &mut Vec<T>, x: T) {
    if !v.contains(&x) {
        v.push(x);
    }
}

/// The main function of `unit`, if both resolve.
pub(super) fn unit_main(p: &IrProgram, unit: Ref<ComputeUnit>) -> Option<Ref<Function>> {
    let ComputeUnit::DensityFunction { function, .. } = get(&p.compute_units, unit)?;
    get(&p.functions, *function).map(|_| *function)
}

/// `main` and every function it reaches through calls, `main` first, then in first-call order.
/// Calls inside an `Eval` position count; the evaluated unit doesn't.
pub(super) fn call_reach(p: &IrProgram, main: Ref<Function>) -> Vec<Ref<Function>> {
    let mut out = vec![main];
    let mut i = 0;
    while i < out.len() {
        let Some(f) = get(&p.functions, out[i]) else { break };
        i += 1;
        each_expr(&f.body, &mut |e| {
            if let Expression::FunctionCall { function, .. } = e
                && get(&p.functions, *function).is_some()
            {
                push(&mut out, *function);
            }
        });
    }
    out
}

/// The eval keys `fns` name, in walk order, whether their unit resolves or not.
fn evals_in(p: &IrProgram, fns: &[Ref<Function>]) -> Vec<EvalKey> {
    let mut out = vec![];
    for &f in fns {
        let Some(f) = get(&p.functions, f) else { continue };
        each_expr(&f.body, &mut |e| {
            if let Expression::Eval { unit, origin_scale, position_scale, .. } = e {
                push(&mut out, EvalKey::new(*unit, *origin_scale, *position_scale));
            }
        });
    }
    out
}

/// A unit's own functions: its main and its listed helpers.
fn unit_fns(p: &IrProgram, unit: Ref<ComputeUnit>) -> Vec<Ref<Function>> {
    let Some(ComputeUnit::DensityFunction { function, helper_functions, .. }) = get(&p.compute_units, unit) else {
        return vec![];
    };
    std::iter::once(*function).chain(helper_functions.iter().copied()).collect()
}

/// Every eval key `unit` reaches: those in its own functions, then through the evaluated bodies,
/// in discovery order. Keys whose unit doesn't resolve are left out.
pub(super) fn reached_keys(p: &IrProgram, unit: Ref<ComputeUnit>) -> Vec<EvalKey> {
    let mut out: Vec<EvalKey> = evals_in(p, &unit_fns(p, unit));
    let mut i = 0;
    while i < out.len() {
        let k = out[i];
        i += 1;
        if let Some(main) = unit_main(p, k.unit) {
            for n in evals_in(p, &call_reach(p, main)) {
                push(&mut out, n);
            }
        }
    }
    out.retain(|k| unit_main(p, k.unit).is_some());
    out
}

/// The inputs read inside the bodies `unit` evaluates, transitively, in discovery order.
pub(super) fn eval_inputs(p: &IrProgram, unit: Ref<ComputeUnit>) -> Vec<(Ref<DensityInput>, &DensityInput)> {
    let mut units: Vec<Ref<ComputeUnit>> = vec![];
    for k in reached_keys(p, unit) {
        push(&mut units, k.unit);
    }
    let mut out: Vec<Ref<DensityInput>> = vec![];
    for u in units {
        let Some(main) = unit_main(p, u) else { continue };
        for f in call_reach(p, main) {
            let Some(f) = get(&p.functions, f) else { continue };
            each_expr(&f.body, &mut |e| {
                if let Expression::DensityVariable(r, _) = e {
                    push(&mut out, *r);
                }
            });
        }
    }
    out.into_iter().filter_map(|r| Some((r, get(&p.density_inputs, r)?))).collect()
}

/// What a unit reaches (§3.2): the inputs it lists, then the ones its evaluated bodies read.
pub(super) fn reached_inputs(p: &IrProgram, unit: Ref<ComputeUnit>) -> Vec<(Ref<DensityInput>, &DensityInput)> {
    let mut out = listed_inputs(p, unit);
    for (r, d) in eval_inputs(p, unit) {
        if !out.iter().any(|(x, _)| *x == r) {
            out.push((r, d));
        }
    }
    out
}

/// One eval key and the functions cloned for it.
pub(super) struct KeyPlan {
    pub key: EvalKey,
    /// Sources and clone ids, the main first.
    pub fns: Vec<(Ref<Function>, FnId)>,
}

/// Every eval key of the program, in discovery order: the program's functions in arena order,
/// each walked in order. Clone ids follow the program's own functions.
#[derive(Default)]
pub(super) struct EvalPlan {
    pub keys: Vec<KeyPlan>,
    index: HashMap<EvalKey, usize>,
}

impl EvalPlan {
    pub fn new(p: &IrProgram, first_fn: usize) -> Self {
        let all: Vec<Ref<Function>> = p.functions.iter().map(|(r, _)| r).collect();
        let mut plan = EvalPlan::default();
        let mut next = first_fn;
        for key in evals_in(p, &all) {
            let Some(main) = unit_main(p, key.unit) else { continue };
            let fns = call_reach(p, main)
                .into_iter()
                .map(|f| {
                    next += 1;
                    (f, FnId(next - 1))
                })
                .collect();
            plan.index.insert(key, plan.keys.len());
            plan.keys.push(KeyPlan { key, fns });
        }
        plan
    }

    /// Records every clone in `map`, under its source's name.
    pub fn register(&self, map: &mut LowerMap) {
        for k in &self.keys {
            for (i, &(source, id)) in k.fns.iter().enumerate() {
                let name = map.functions.id(source).and_then(|s| map.functions.name(s)).unwrap_or("?").to_string();
                let pushed = map.functions.push(source, name);
                debug_assert_eq!(pushed, id);
                map.eval_fns.push(EvalFnInfo { id, source, key: k.key, main: i == 0 });
            }
        }
    }

    pub fn key(&self, k: &EvalKey) -> Option<usize> {
        self.index.get(k).copied()
    }

    /// The clone of `f` for key `k`.
    pub fn clone_of(&self, k: usize, f: Ref<Function>) -> Option<FnId> {
        self.keys[k].fns.iter().find(|(s, _)| *s == f).map(|(_, id)| *id)
    }
}

/// What a reader unit's core lists gain from the bodies it evaluates.
#[derive(Default)]
pub(super) struct Extras {
    pub helpers: Vec<FnId>,
    pub inputs: Vec<Ref<DensityInput>>,
    pub units: Vec<Ref<ComputeUnit>>,
}

impl<'p, 'r> Ctx<'p, 'r> {
    /// `Eval` in any function: a call of the key's main clone with the position.
    pub(super) fn eval_expr(
        &mut self,
        unit: Ref<ComputeUnit>,
        position: &Expression,
        origin_scale: (f64, f64, f64),
        position_scale: (f64, f64, f64),
    ) -> Expr {
        let key = EvalKey::new(unit, origin_scale, position_scale);
        let first = self.eval_reported.insert(key);
        if first && self.map.units.id(unit).is_none() {
            self.err(LowerErrorKind::EvalUnknownUnit);
        }
        if first {
            for (scale, v) in [("origin", origin_scale), ("position", position_scale)] {
                for (axis, c) in [('x', v.0), ('y', v.1), ('z', v.2)] {
                    if !c.is_finite() {
                        self.err(LowerErrorKind::EvalBadScale { scale, axis, value: format!("{c}") });
                    }
                }
            }
        }
        let pos = self.at(Step::Arg(0), |cx| cx.expr(position));
        match self.eval.key(&key) {
            Some(k) => Expr::Call { func: self.eval.keys[k].fns[0].1, args: vec![pos] },
            None => Expr::Lit(Lit::Bool(false)),
        }
    }

    /// The position parameter, while lowering a clone.
    pub(super) fn eval_position(&self) -> Option<Expr> {
        self.eval_clone.map(|_| Expr::Local(LocalId(0)))
    }

    /// The clone to call instead of `f`, and the position to pass, while lowering a clone.
    pub(super) fn eval_call(&self, f: Ref<Function>) -> Option<FnId> {
        self.eval_clone.and_then(|k| self.eval.clone_of(k, f))
    }

    /// The scales of the key being cloned.
    pub(super) fn eval_scales(&self) -> Option<(Triple, Triple)> {
        self.eval_clone.map(|k| {
            let key = self.eval.keys[k].key;
            (key.origin_scale(), key.position_scale())
        })
    }

    /// The clones, inputs and evaluated units `unit` reaches through `eval`.
    pub(super) fn eval_extras(&self, unit: Ref<ComputeUnit>) -> Extras {
        let p = self.p;
        let mut out = Extras::default();
        for key in reached_keys(p, unit) {
            if let Some(k) = self.eval.key(&key) {
                for &(_, id) in &self.eval.keys[k].fns {
                    push(&mut out.helpers, id);
                }
            }
            push(&mut out.units, key.unit);
        }
        out.inputs = eval_inputs(p, unit).into_iter().map(|(r, _)| r).collect();
        out
    }

    /// §6: per eval key, un-indexed and `@previous` reads in the evaluated body; then cycles
    /// through `eval`.
    pub(super) fn eval_checks(&mut self) {
        let p = self.p;
        for k in 0..self.eval.keys.len() {
            let key = self.eval.keys[k].key;
            let Some(uid) = self.map.units.id(key.unit) else { continue };
            let unit = self.map.units.name(uid).unwrap_or("?").to_string();
            self.enter(Item::Unit { id: Some(uid), name: unit.clone() });
            let mut seen: HashSet<Ref<DensityInput>> = HashSet::new();
            let fns: Vec<Ref<Function>> = self.eval.keys[k].fns.iter().map(|(f, _)| *f).collect();
            for f in fns {
                let Some(func) = get(&p.functions, f) else { continue };
                let mut reads = vec![];
                each_expr(&func.body, &mut |e| {
                    if let Expression::DensityVariable(r, i) = e {
                        reads.push((*r, i.is_none()));
                    }
                });
                for (r, unindexed) in reads {
                    let Some(d) = get(&p.density_inputs, r) else { continue };
                    if !(d.previous || unindexed) || !seen.insert(r) {
                        continue;
                    }
                    let input = self.map.inputs.id(r).and_then(|i| self.map.inputs.name(i)).unwrap_or("?").to_string();
                    if d.previous {
                        self.err(LowerErrorKind::EvalPrevious { unit: unit.clone(), input });
                    } else {
                        let function = self.map.functions.id(f).and_then(|i| self.map.functions.name(i));
                        let function = function.unwrap_or("?").to_string();
                        self.err(LowerErrorKind::EvalUnindexedRead { unit: unit.clone(), function, input });
                    }
                }
            }
        }
        self.eval_cycles();
    }

    /// A cycle through units, with `eval` and input reads (not `@previous`) as edges, that
    /// contains at least one `eval` edge. Reported once per cycle found, under the program.
    fn eval_cycles(&mut self) {
        let p = self.p;
        let units: Vec<Ref<ComputeUnit>> = p.compute_units.iter().map(|(r, _)| r).collect();
        let pos = |u: Ref<ComputeUnit>| units.iter().position(|x| *x == u);
        // Per unit: (target, is_eval), in order.
        let mut edges: Vec<Vec<(usize, bool)>> = vec![vec![]; units.len()];
        for (i, &u) in units.iter().enumerate() {
            let mut fns = unit_fns(p, u);
            if let Some(main) = unit_main(p, u) {
                for f in call_reach(p, main) {
                    push(&mut fns, f);
                }
            }
            for k in evals_in(p, &fns) {
                if let Some(j) = pos(k.unit) {
                    push(&mut edges[i], (j, true));
                }
            }
            let mut inputs: Vec<Ref<DensityInput>> = listed_inputs(p, u).into_iter().map(|(r, _)| r).collect();
            for f in &fns {
                let Some(f) = get(&p.functions, *f) else { continue };
                each_expr(&f.body, &mut |e| {
                    if let Expression::DensityVariable(r, _) = e {
                        push(&mut inputs, *r);
                    }
                });
            }
            for r in inputs {
                let Some(d) = get(&p.density_inputs, r) else { continue };
                if d.previous {
                    continue;
                }
                if let Some(j) = pos(d.density_function) {
                    push(&mut edges[i], (j, false));
                }
            }
        }
        let mut reported: HashSet<usize> = HashSet::new();
        for from in 0..units.len() {
            for &(to, is_eval) in &edges[from] {
                if !is_eval || reported.contains(&from) {
                    continue;
                }
                let Some(path) = find_path(&edges, to, from) else { continue };
                if path.iter().any(|n| reported.contains(n)) {
                    continue;
                }
                reported.extend(path.iter().copied());
                let name = |n: usize| {
                    let id = self.map.units.id(units[n]);
                    format!("`{}`", id.and_then(|i| self.map.units.name(i)).unwrap_or("?"))
                };
                // `from`, then the path back to it.
                let mut cycle = vec![name(from)];
                cycle.extend(path.iter().map(|&n| name(n)));
                self.enter(Item::Program);
                self.err(LowerErrorKind::EvalCycle { cycle: cycle.join(" → ") });
            }
        }
    }
}

/// The shortest path from `from` to `to`, both included. `from == to` is the path `[from]`.
fn find_path(edges: &[Vec<(usize, bool)>], from: usize, to: usize) -> Option<Vec<usize>> {
    let mut parent: Vec<Option<usize>> = vec![None; edges.len()];
    let mut seen = vec![false; edges.len()];
    let mut queue = VecDeque::from([from]);
    seen[from] = true;
    while let Some(n) = queue.pop_front() {
        if n == to {
            let mut path = vec![n];
            let mut cur = n;
            while let Some(prev) = parent[cur] {
                path.push(prev);
                cur = prev;
            }
            path.reverse();
            return Some(path);
        }
        for &(m, _) in &edges[n] {
            if !seen[m] {
                seen[m] = true;
                parent[m] = Some(n);
                queue.push_back(m);
            }
        }
    }
    None
}
