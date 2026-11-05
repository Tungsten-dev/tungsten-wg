//! Entries, units, cells, input buffers and constants (§7.3).

use vstd::prelude::*;

use crate::exec::*;
use crate::exec::expr::eval_expr;
use crate::exec::stmt::eval_call;
use crate::ir::*;
use crate::layout;
use crate::proofs::value::*;
use crate::spec::eval::*;
use crate::spec::value::*;
use crate::util::*;
use crate::value::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

/// Whether a built buffer went to the memo or is handed back (memo off).
pub enum InputBuild {
    Memoized,
    Fresh(Result<Vec<Value>, (usize, EvalError)>),
}

fn stuck(r: StuckReason) -> (e: EvalError)
    ensures
        e == crate::spec::eval::stuck(r),
{
    EvalError::Stuck(r)
}

/// The target's rank, or `current` (which fails the caller's `< current` check) if it has none.
/// Callers compare the result with `current` themselves, so Verus sees the decrease.
pub fn rank_or(target: Option<usize>, current: usize) -> (r: usize)
    ensures
        r == spec_rank_or(target, current),
{
    match target {
        Some(r) => r,
        None => current,
    }
}

pub fn eval_entry<E: Externs>(env: &Env<E>, e: usize, memo: &mut Memo) -> (r: Vec<Result<Value, EvalError>>)
    requires
        env_inv(env),
        memo_inv(senv(env), *old(memo)),
        e < env.p.entries@.len(),
    ensures
        memo_inv(senv(env), *final(memo)),
        view_results(r@) == spec_eval_entry(senv(env), e as int),
{
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
        // The layout comes first, as in `eval_unit_ctx` and the failing-const path of
        // `eval_program`, so a cell's result can't change kind with fuel.
        None => match layout::len(&entry.dims) {
            Some(n) => fill(n, stuck(StuckReason::Rank)),
            None => {
                let out = fill(1, stuck(StuckReason::Layout));
                proof {
                    assert(spec_fill(1, EvalError::Stuck(StuckReason::Layout)) =~= seq![Err(EvalError::Stuck(StuckReason::Layout))]);
                }
                out
            },
        },
    }
}

/// Builds every input `U` lists, then evaluates every cell of `base.dims`. If an input fails,
/// every cell is the error of the lowest failing `InputId`.
pub fn eval_unit_ctx<E: Externs>(env: &Env<E>, rank: usize, u: UnitId, base: &Ctx, memo: &mut Memo) -> (r: Vec<Result<Value, EvalError>>)
    requires
        env_inv(env),
        memo_inv(senv(env), *old(memo)),
    ensures
        memo_inv(senv(env), *final(memo)),
        view_results(r@) == spec_eval_unit_ctx(senv(env), rank, u, *base),
    decreases rank, 5int, base,
{
    let n = match layout::len(&base.dims) {
        Some(n) => n,
        None => {
            let out = fill(1, stuck(StuckReason::Layout));
            proof {
                assert(spec_fill(1, EvalError::Stuck(StuckReason::Layout)) =~= seq![Err(EvalError::Stuck(StuckReason::Layout))]);
            }
            return out;
        },
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
            env_inv(env),
            memo_inv(senv(env), *memo),
            spec_best_input(senv(env), rank, unit.inputs@, 0, None)
                == spec_best_input(senv(env), rank, unit.inputs@, j as int, best),
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
        proof {
            assert(res == spec_input_error(senv(env), rank, i));
        }
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
    eval_cells(env, rank, u, base, n, memo)
}

/// Every cell of `base.dims`, in flat order.
fn eval_cells<E: Externs>(env: &Env<E>, rank: usize, u: UnitId, base: &Ctx, n: usize, memo: &mut Memo) -> (r: Vec<Result<Value, EvalError>>)
    requires
        env_inv(env),
        memo_inv(senv(env), *old(memo)),
    ensures
        memo_inv(senv(env), *final(memo)),
        view_results(r@) == Seq::new(n as nat, |k: int| spec_unit_cell(senv(env), rank, u, *base, k)),
    decreases rank, 4int, base,
{
    let mut out: Vec<Result<Value, EvalError>> = Vec::new();
    let mut k: usize = 0;
    while k < n
        invariant
            k <= n,
            env_inv(env),
            memo_inv(senv(env), *memo),
            out@.len() == k,
            forall|j: int| 0 <= j < k ==> view_result(#[trigger] out@[j]) == spec_unit_cell(senv(env), rank, u, *base, j),
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
    proof {
        assert(view_results(out@) =~= Seq::new(n as nat, |k: int| spec_unit_cell(senv(env), rank, u, *base, k)));
    }
    out
}

/// `eval_call(U.main)` with no arguments.
pub fn eval_cell<E: Externs>(env: &Env<E>, rank: usize, u: UnitId, ctx: &Ctx, memo: &mut Memo) -> (r: Result<Value, EvalError>)
    requires
        env_inv(env),
        memo_inv(senv(env), *old(memo)),
    ensures
        memo_inv(senv(env), *final(memo)),
        view_result(r) == spec_eval_cell(senv(env), rank, u, *ctx),
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
        let args: Vec<Value> = Vec::new();
        proof {
            lemma_view_values_empty();
            assert(args@ =~= Seq::<Value>::empty());
        }
        eval_call(env, rm, unit.main, args, ctx, memo)
    } else {
        Err(stuck(StuckReason::Rank))
    }
}

/// A unit's cells as a buffer: all the values, or the first failing cell in flat order.
fn buffer_of(cells: &Vec<Result<Value, EvalError>>) -> (r: Result<Vec<Value>, (usize, EvalError)>)
    ensures
        view_buffer(r) == spec_buffer(view_results(cells@), 0, Seq::empty()),
{
    let mut vals: Vec<Value> = Vec::new();
    proof {
        lemma_view_values_empty();
        assert(vals@ =~= Seq::<Value>::empty());
    }
    let mut k: usize = 0;
    while k < cells.len()
        invariant
            k <= cells.len(),
            spec_buffer(view_results(cells@), 0, Seq::empty())
                == spec_buffer(view_results(cells@), k as int, view_values(vals@)),
        decreases cells.len() - k,
    {
        proof {
            assert(view_results(cells@)[k as int] == view_result(cells@[k as int]));
        }
        match &cells[k] {
            Ok(v) => {
                let c = clone_value(v);
                proof {
                    lemma_view_values_push(vals@, c);
                }
                vals.push(c);
            },
            Err(e) => { return Err((k, *e)); },
        }
        k = k + 1;
    }
    Ok(vals)
}

/// Evaluates `inputs[i].source` over all of `inputs[i].dims`; the buffer fails with its first
/// failing cell in flat order. Memoized per `InputId`; a failure is listed once.
pub fn build_input<E: Externs>(env: &Env<E>, rank: usize, i: InputId, memo: &mut Memo) -> (r: InputBuild)
    requires
        env_inv(env),
        memo_inv(senv(env), *old(memo)),
        spec_rank(env.ranks.inputs@, i.0) == Some(rank),
    ensures
        memo_inv(senv(env), *final(memo)),
        input_build_is(r, *final(memo), i, spec_build_input(senv(env), rank, i)),
    decreases rank, 6int, env.p,
{
    reveal(memo_inv);
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
                buffer_of(&cells)
            }
        },
    };
    proof {
        assert(view_buffer(result) == spec_build_input(senv(env), rank, i));
    }
    match &result {
        Err((cell, e)) => {
            if !get_bool(&memo.listed, i.0) {
                let ghost old_failures = memo.failures@;
                let ghost old_listed = memo.listed@;
                memo.failures.push(Failure::Input { id: i, cell: *cell, error: *e });
                set(&mut memo.listed, i.0, true);
                proof {
                    let fs = memo.failures@;
                    assert(fs.len() == old_failures.len() + 1);
                    assert forall|j: int| 0 <= j < fs.len() implies input_failure_ok(senv(env), #[trigger] fs[j]) by {
                        if j < old_failures.len() {
                            assert(fs[j] == old_failures[j]);
                        }
                    }
                    assert forall|j: int| 0 <= j < fs.len() implies memo.listed@[#[trigger] failure_input(fs[j]) as int] by {
                        if j < old_failures.len() {
                            assert(fs[j] == old_failures[j]);
                        }
                    }
                    assert forall|x: int, y: int| 0 <= x < y < fs.len() implies #[trigger] failure_input(fs[x]) != #[trigger] failure_input(fs[y]) by {
                        assert(fs[x] == old_failures[x]);
                        if y < old_failures.len() {
                            assert(fs[y] == old_failures[y]);
                        } else {
                            assert(old_listed[failure_input(old_failures[x]) as int]);
                            assert(!old_listed[i.0 as int]);
                        }
                    }
                }
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
pub fn eval_const<E: Externs>(env: &Env<E>, rank: usize, c: ConstId, memo: &mut Memo) -> (r: Result<Value, EvalError>)
    requires
        env_inv(env),
        memo_inv(senv(env), *old(memo)),
        spec_rank(env.ranks.consts@, c.0) == Some(rank),
    ensures
        memo_inv(senv(env), *final(memo)),
        view_result(r) == spec_eval_const(senv(env), rank, c),
    decreases rank, 2int, env.p,
{
    reveal(memo_inv);
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
    let frame = Frame { locals: Vec::new() };
    proof {
        assert(view_frame(frame) =~= Seq::<Option<SValue>>::empty());
    }
    let r = eval_expr(env, rank, &ctx, &frame, &decl.init, memo);
    if memo.enabled {
        set(&mut memo.consts, c.0, Some(clone_result(&r)));
    }
    r
}

} // verus!
