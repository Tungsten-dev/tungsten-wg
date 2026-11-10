//! Entries, units, cells, input buffers and constants (§7.3).

use vstd::prelude::*;

use crate::exec::*;
use crate::exec::expr::eval_expr;
use crate::exec::stmt::eval_call;
use crate::ir::*;
use crate::layout;
use crate::util::*;
use crate::value::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

/// Whether a built buffer went to the memo or is handed back (memo off).
pub enum InputBuild {
    Memoized,
    Fresh(Result<Vec<Value>, (usize, EvalError)>),
}

fn stuck(r: StuckReason) -> EvalError {
    EvalError::Stuck(r)
}

/// The target's rank, or `current` (which fails the caller's `< current` check) if it has none.
/// Callers compare the result with `current` themselves, so Verus sees the decrease.
pub fn rank_or(target: Option<usize>, current: usize) -> usize {
    match target {
        Some(r) => r,
        None => current,
    }
}

pub fn eval_entry<E: Externs>(env: &Env<E>, e: usize, memo: &mut Memo) -> Vec<Result<Value, EvalError>> {
    let entry = match get(&env.p.entries, e) {
        Some(x) => x,
        None => { return fill(1, stuck(StuckReason::BadIndex)); },
    };
    let base = Ctx {
        origin: copy_f64x3(&env.case.origin),
        origin_scale: copy_f64x3(&entry.origin_scale),
        position_scale: copy_f64x3(&entry.position_scale),
        dims: copy_dims(&entry.dims),
        k: 0,
        cell: I32x3 { x: 0, y: 0, z: 0 },
    };
    match get_rank(&env.ranks.units, entry.unit.0) {
        Some(ru) => eval_unit_ctx(env, ru, entry.unit, &base, memo),
        None => fill(cell_count(&entry.dims), stuck(StuckReason::Rank)),
    }
}

/// Builds every input `U` lists, then evaluates every cell of `base.dims`. If an input fails,
/// every cell is the error of the lowest failing `InputId`.
pub fn eval_unit_ctx<E: Externs>(env: &Env<E>, rank: usize, u: UnitId, base: &Ctx, memo: &mut Memo) -> Vec<Result<Value, EvalError>>
    decreases rank, 4int, base,
{
    let n = match layout::len(&base.dims) {
        Some(n) => n,
        None => { return fill(1, stuck(StuckReason::Layout)); },
    };
    let unit = match get(&env.p.units, u.0) {
        Some(x) => x,
        None => { return fill(n, stuck(StuckReason::BadIndex)); },
    };
    let mut best: Option<(usize, EvalError)> = None;
    let mut j: usize = 0;
    while j < unit.inputs.len()
        invariant
            j <= unit.inputs.len(),
        decreases unit.inputs.len() - j,
    {
        let i = unit.inputs[j];
        let ri = rank_or(get_rank(&env.ranks.inputs, i.0), rank);
        let res: Option<EvalError> = if ri < rank {
            match build_input(env, ri, i, memo) {
                InputBuild::Fresh(Err((_, e))) => Some(e),
                InputBuild::Fresh(Ok(_)) => None,
                InputBuild::Memoized => match get(&memo.inputs, i.0) {
                    Some(Some(Err((_, e)))) => Some(*e),
                    Some(Some(Ok(_))) => None,
                    _ => Some(stuck(StuckReason::BadIndex)),
                },
            }
        } else {
            Some(stuck(StuckReason::Rank))
        };
        match res {
            Some(e) => {
                let lower = match best {
                    Some((b, _)) => i.0 < b,
                    None => true,
                };
                if lower {
                    best = Some((i.0, e));
                }
            },
            None => {},
        }
        j = j + 1;
    }
    match best {
        Some((_, e)) => { return fill(n, e); },
        None => {},
    }
    let mut out: Vec<Result<Value, EvalError>> = Vec::new();
    let mut k: usize = 0;
    while k < n
        invariant
            k <= n,
        decreases n - k,
    {
        let r = match layout::decode(k, &base.dims) {
            Some(cell) => {
                let ctx = Ctx {
                    origin: copy_f64x3(&base.origin),
                    origin_scale: copy_f64x3(&base.origin_scale),
                    position_scale: copy_f64x3(&base.position_scale),
                    dims: copy_dims(&base.dims),
                    k,
                    cell,
                };
                eval_cell(env, rank, u, &ctx, memo)
            },
            None => Err(stuck(StuckReason::Layout)),
        };
        out.push(r);
        k = k + 1;
    }
    out
}

/// `eval_call(U.main)` with no arguments.
pub fn eval_cell<E: Externs>(env: &Env<E>, rank: usize, u: UnitId, ctx: &Ctx, memo: &mut Memo) -> Result<Value, EvalError>
    decreases rank, 3int, ctx,
{
    let unit = match get(&env.p.units, u.0) {
        Some(x) => x,
        None => { return Err(stuck(StuckReason::BadIndex)); },
    };
    let f = match get(&env.p.functions, unit.main.0) {
        Some(f) => f,
        None => { return Err(stuck(StuckReason::BadIndex)); },
    };
    if f.params > 0 {
        return Err(stuck(StuckReason::MainParams));
    }
    let rm = rank_or(get_rank(&env.ranks.fns, unit.main.0), rank);
    if rm < rank {
        eval_call(env, rm, unit.main, Vec::new(), ctx, memo)
    } else {
        Err(stuck(StuckReason::Rank))
    }
}

/// Evaluates `inputs[i].source` over all of `inputs[i].dims`; the buffer fails with its first
/// failing cell in flat order. Memoized per `InputId`; a failure is listed once.
pub fn build_input<E: Externs>(env: &Env<E>, rank: usize, i: InputId, memo: &mut Memo) -> InputBuild
    decreases rank, 5int, env.p,
{
    if memo.enabled {
        match get(&memo.inputs, i.0) {
            Some(Some(_)) => { return InputBuild::Memoized; },
            _ => {},
        }
    }
    let result: Result<Vec<Value>, (usize, EvalError)> = match get(&env.p.inputs, i.0) {
        None => Err((0, stuck(StuckReason::BadIndex))),
        Some(d) => {
            let rs = rank_or(get_rank(&env.ranks.units, d.source.0), rank);
            if rs >= rank {
                Err((0, stuck(StuckReason::Rank)))
            } else {
                let base = Ctx {
                    origin: copy_f64x3(&env.case.origin),
                    origin_scale: copy_f64x3(&d.origin_scale),
                    position_scale: copy_f64x3(&d.position_scale),
                    dims: copy_dims(&d.dims),
                    k: 0,
                    cell: I32x3 { x: 0, y: 0, z: 0 },
                };
                let cells = eval_unit_ctx(env, rs, d.source, &base, memo);
                let mut vals: Vec<Value> = Vec::new();
                let mut err: Option<(usize, EvalError)> = None;
                let mut k: usize = 0;
                while k < cells.len()
                    invariant
                        k <= cells.len(),
                    decreases cells.len() - k,
                {
                    match &cells[k] {
                        Ok(v) => vals.push(clone_value(v)),
                        Err(e) => {
                            err = Some((k, *e));
                            break;
                        },
                    }
                    k = k + 1;
                }
                match err {
                    Some(x) => Err(x),
                    None => Ok(vals),
                }
            }
        },
    };
    match &result {
        Err((cell, e)) => {
            if !get_bool(&memo.listed, i.0) {
                memo.failures.push(Failure::Input { id: i, cell: *cell, error: *e });
                set(&mut memo.listed, i.0, true);
            }
        },
        Ok(_) => {},
    }
    if memo.enabled && i.0 < memo.inputs.len() {
        set(&mut memo.inputs, i.0, Some(result));
        InputBuild::Memoized
    } else {
        InputBuild::Fresh(result)
    }
}

/// A const's init, evaluated with no locals and a placeholder context (const inits can't read
/// ambients; the checker forbids it).
pub fn eval_const<E: Externs>(env: &Env<E>, rank: usize, c: ConstId, memo: &mut Memo) -> Result<Value, EvalError>
    decreases rank, 2int, env.p,
{
    if memo.enabled {
        match get(&memo.consts, c.0) {
            Some(Some(r)) => { return clone_result(r); },
            _ => {},
        }
    }
    let decl = match get(&env.p.consts, c.0) {
        Some(d) => d,
        None => { return Err(stuck(StuckReason::BadIndex)); },
    };
    let zero = F64x3 { x: 0, y: 0, z: 0 };
    let ctx = Ctx {
        origin: copy_f64x3(&zero),
        origin_scale: copy_f64x3(&zero),
        position_scale: copy_f64x3(&zero),
        dims: Dims { x: 1, y: 1, z: 1 },
        k: 0,
        cell: I32x3 { x: 0, y: 0, z: 0 },
    };
    let mut frame = Frame { locals: Vec::new() };
    let r = eval_expr(env, rank, &ctx, &mut frame, &decl.init, memo);
    if memo.enabled {
        set(&mut memo.consts, c.0, Some(clone_result(&r)));
    }
    r
}

} // verus!
