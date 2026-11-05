//! T2: a cell that isn't `OutOfFuel` at fuel `f` has the same result at every `f2 >= f`.
//!
//! Every lemma has the same shape: run a spec function at `env` and at `with_fuel(env, f2)`; if the
//! first isn't `OutOfFuel`, the second is identical. The proof follows the spec's own recursion.
//! The one case that needs thought is the canonical-error choice in `spec_best_input`.

use vstd::prelude::*;

use crate::exec::Ctx;
use crate::ir::*;
use crate::spec::eval::*;
use crate::spec::layout::*;
use crate::spec::value::*;
use crate::value::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

pub open spec fn with_fuel<E: Externs>(env: SEnv<E>, f2: u64) -> SEnv<E> {
    SEnv { fuel: f2, ..env }
}

/// `r1` isn't `OutOfFuel`, so `r2` is the same.
pub open spec fn mono<T>(r1: Result<T, EvalError>, r2: Result<T, EvalError>) -> bool {
    r1 != Err::<T, EvalError>(EvalError::OutOfFuel) ==> r2 == r1
}

proof fn lemma_args<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, ctx: Ctx, frame: SFrame, args: Vec<Expr>, i: int, acc: Seq<SValue>)
    requires
        env.fuel <= f2,
    ensures
        mono(spec_args(env, rank, ctx, frame, args, i, acc), spec_args(with_fuel(env, f2), rank, ctx, frame, args, i, acc)),
    decreases rank, 0int, args, 0int, args@.len() - i,
{
    if 0 <= i < args@.len() {
        lemma_expr(env, f2, rank, ctx, frame, args@[i]);
        match spec_eval_expr(env, rank, ctx, frame, args@[i]) {
            Ok(v) => lemma_args(env, f2, rank, ctx, frame, args, i + 1, acc.push(v)),
            Err(_) => {},
        }
    }
}

proof fn lemma_fields<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, ctx: Ctx, frame: SFrame, fields: Vec<FieldInit>, i: int, slots: SFrame)
    requires
        env.fuel <= f2,
    ensures
        mono(spec_fields(env, rank, ctx, frame, fields, i, slots), spec_fields(with_fuel(env, f2), rank, ctx, frame, fields, i, slots)),
    decreases rank, 0int, fields, 0int, fields@.len() - i,
{
    if 0 <= i < fields@.len() {
        lemma_expr(env, f2, rank, ctx, frame, fields@[i].value);
        match spec_eval_expr(env, rank, ctx, frame, fields@[i].value) {
            Ok(v) => {
                let f = fields@[i].field;
                if f < slots.len() && !(slots[f as int] is Some) {
                    lemma_fields(env, f2, rank, ctx, frame, fields, i + 1, slots.update(f as int, Some(v)));
                }
            },
            Err(_) => {},
        }
    }
}

proof fn lemma_density<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, ctx: Ctx, frame: SFrame, input: InputId, index: Option<Box<Expr>>)
    requires
        env.fuel <= f2,
    ensures
        mono(spec_read_density(env, rank, ctx, frame, input, index), spec_read_density(with_fuel(env, f2), rank, ctx, frame, input, index)),
    decreases rank, 0int, index, 0int, 0int,
{
    match index {
        Some(ix) => lemma_expr(env, f2, rank, ctx, frame, *ix),
        None => {},
    }
    let ri = spec_rank_or(spec_rank(env.ranks.inputs, input.0), rank);
    if ri < rank {
        lemma_input(env, f2, ri, input);
    }
}

pub proof fn lemma_expr<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, ctx: Ctx, frame: SFrame, e: Expr)
    requires
        env.fuel <= f2,
    ensures
        mono(spec_eval_expr(env, rank, ctx, frame, e), spec_eval_expr(with_fuel(env, f2), rank, ctx, frame, e)),
    decreases rank, 0int, e, 0int, 0int,
{
    match e {
        Expr::Const(c) => {
            let rc = spec_rank_or(spec_rank(env.ranks.consts, c.0), rank);
            if rc < rank {
                lemma_const(env, f2, rc, c);
            }
        },
        Expr::Density { input, index } => lemma_density(env, f2, rank, ctx, frame, input, index),
        Expr::Call { func, args } => {
            lemma_args(env, f2, rank, ctx, frame, args, 0, Seq::empty());
            match spec_args(env, rank, ctx, frame, args, 0, Seq::empty()) {
                Ok(vals) => {
                    let rg = spec_rank_or(spec_rank(env.ranks.fns, func.0), rank);
                    if rg < rank {
                        lemma_call(env, f2, rg, func, vals, ctx);
                    }
                },
                Err(_) => {},
            }
        },
        Expr::ExternCall { args, .. } => lemma_args(env, f2, rank, ctx, frame, args, 0, Seq::empty()),
        Expr::Binary { left, right, .. } => {
            lemma_expr(env, f2, rank, ctx, frame, *left);
            lemma_expr(env, f2, rank, ctx, frame, *right);
        },
        Expr::Unary { operand, .. } => lemma_expr(env, f2, rank, ctx, frame, *operand),
        Expr::Field { base, .. } => lemma_expr(env, f2, rank, ctx, frame, *base),
        Expr::Index { array, index } => {
            lemma_expr(env, f2, rank, ctx, frame, *array);
            lemma_expr(env, f2, rank, ctx, frame, *index);
        },
        Expr::MakeVec3 { x, y, z } | Expr::MakePos3 { x, y, z } => {
            lemma_expr(env, f2, rank, ctx, frame, *x);
            lemma_expr(env, f2, rank, ctx, frame, *y);
            lemma_expr(env, f2, rank, ctx, frame, *z);
        },
        Expr::MakeRecord { record, fields } => {
            if record.0 < env.p.records@.len() {
                let n = env.p.records@[record.0 as int].fields@.len();
                lemma_fields(env, f2, rank, ctx, frame, fields, 0, spec_empty_slots(n));
            }
        },
        Expr::ArrayLit(es) => lemma_args(env, f2, rank, ctx, frame, es, 0, Seq::empty()),
        Expr::Cast { operand, .. } => lemma_expr(env, f2, rank, ctx, frame, *operand),
        _ => {},
    }
}

proof fn lemma_stmts<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, ctx: Ctx, frame: SFrame, ss: Vec<Stmt>, i: int)
    requires
        env.fuel <= f2,
    ensures
        mono(spec_stmts(env, rank, ctx, frame, ss, i), spec_stmts(with_fuel(env, f2), rank, ctx, frame, ss, i)),
    decreases rank, 1int, ss, 0int, ss@.len() - i,
{
    if 0 <= i < ss@.len() {
        lemma_stmt(env, f2, rank, ctx, frame, ss@[i]);
        match spec_eval_stmt(env, rank, ctx, frame, ss@[i]) {
            Ok((SFlow::Next, f)) => lemma_stmts(env, f2, rank, ctx, f, ss, i + 1),
            _ => {},
        }
    }
}

/// The heart of T2: fuel is only read when the cond is true and `iters` has reached it, which is
/// the `OutOfFuel` the hypothesis excludes.
proof fn lemma_while<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, ctx: Ctx, frame: SFrame, cond: Expr, body: Vec<Stmt>, iters: int)
    requires
        env.fuel <= f2,
    ensures
        mono(spec_while(env, rank, ctx, frame, cond, body, iters), spec_while(with_fuel(env, f2), rank, ctx, frame, cond, body, iters)),
    decreases rank, 1int, body, env.fuel - iters + 1, 0int,
{
    lemma_expr(env, f2, rank, ctx, frame, cond);
    if 0 <= iters < env.fuel {
        lemma_stmts(env, f2, rank, ctx, frame, body, 0);
        match spec_stmts(env, rank, ctx, frame, body, 0) {
            Ok((SFlow::Next, f)) => lemma_while(env, f2, rank, ctx, f, cond, body, iters + 1),
            _ => {},
        }
    }
}

proof fn lemma_repeat<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, ctx: Ctx, frame: SFrame, n: int, body: Vec<Stmt>, k: int)
    requires
        env.fuel <= f2,
    ensures
        mono(spec_repeat(env, rank, ctx, frame, n, body, k), spec_repeat(with_fuel(env, f2), rank, ctx, frame, n, body, k)),
    decreases rank, 1int, body, n - k + 1, 0int,
{
    if 0 <= k < n {
        lemma_stmts(env, f2, rank, ctx, frame, body, 0);
        match spec_stmts(env, rank, ctx, frame, body, 0) {
            Ok((SFlow::Next, f)) => lemma_repeat(env, f2, rank, ctx, f, n, body, k + 1),
            _ => {},
        }
    }
}

proof fn lemma_stmt<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, ctx: Ctx, frame: SFrame, s: Stmt)
    requires
        env.fuel <= f2,
    ensures
        mono(spec_eval_stmt(env, rank, ctx, frame, s), spec_eval_stmt(with_fuel(env, f2), rank, ctx, frame, s)),
    decreases rank, 1int, s, 0int, 0int,
{
    match s {
        Stmt::Assign { value, .. } => lemma_expr(env, f2, rank, ctx, frame, value),
        Stmt::Return(e) => lemma_expr(env, f2, rank, ctx, frame, e),
        Stmt::If { cond, then_branch, else_branch } => {
            lemma_expr(env, f2, rank, ctx, frame, cond);
            lemma_stmts(env, f2, rank, ctx, frame, then_branch, 0);
            lemma_stmts(env, f2, rank, ctx, frame, else_branch, 0);
        },
        Stmt::While { cond, body } => lemma_while(env, f2, rank, ctx, frame, cond, body, 0),
        Stmt::Repeat { count, body } => lemma_repeat(env, f2, rank, ctx, frame, count as int, body, 0),
        Stmt::Break => {},
    }
}

proof fn lemma_call<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, g: FnId, args: Seq<SValue>, ctx: Ctx)
    requires
        env.fuel <= f2,
    ensures
        mono(spec_eval_call(env, rank, g, args, ctx), spec_eval_call(with_fuel(env, f2), rank, g, args, ctx)),
    decreases rank, 2int, env.p, 0int, 0int,
{
    if g.0 < env.p.functions@.len() {
        let f = env.p.functions@[g.0 as int];
        if args.len() == f.params && f.params <= f.locals@.len() {
            lemma_stmts(env, f2, rank, ctx, spec_init_frame(args, f.locals@.len()), f.body, 0);
        }
    }
}

proof fn lemma_const<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, c: ConstId)
    requires
        env.fuel <= f2,
    ensures
        mono(spec_eval_const(env, rank, c), spec_eval_const(with_fuel(env, f2), rank, c)),
    decreases rank, 2int, env.p, 0int, 0int,
{
    if c.0 < env.p.consts@.len() {
        lemma_expr(env, f2, rank, spec_const_ctx(), Seq::empty(), env.p.consts@[c.0 as int].init);
    }
}

proof fn lemma_cell<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, u: UnitId, ctx: Ctx)
    requires
        env.fuel <= f2,
    ensures
        mono(spec_eval_cell(env, rank, u, ctx), spec_eval_cell(with_fuel(env, f2), rank, u, ctx)),
    decreases rank, 3int, env.p, 0int, 0int,
{
    if u.0 < env.p.units@.len() {
        let unit = env.p.units@[u.0 as int];
        let rm = spec_rank_or(spec_rank(env.ranks.fns, unit.main.0), rank);
        if rm < rank {
            lemma_call(env, f2, rm, unit.main, Seq::empty(), ctx);
        }
    }
}

proof fn lemma_unit_cell<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, u: UnitId, base: Ctx, k: int)
    requires
        env.fuel <= f2,
    ensures
        mono(spec_unit_cell(env, rank, u, base, k), spec_unit_cell(with_fuel(env, f2), rank, u, base, k)),
    decreases rank, 4int, env.p, 0int, 0int,
{
    match spec_decode(k as usize, base.dims) {
        Some(cell) => lemma_cell(env, f2, rank, u, spec_cell_ctx(base, k as usize, cell)),
        None => {},
    }
}

/// How the two runs' "lowest failing input so far" relate: the same, or the first run's is an
/// `OutOfFuel` and the second's has an id no lower than it.
pub open spec fn best_rel(b1: Option<(usize, EvalError)>, b2: Option<(usize, EvalError)>) -> bool {
    b1 == b2 || match b1 {
        Some((m1, EvalError::OutOfFuel)) => match b2 {
            None => true,
            Some((m2, _)) => m2 >= m1,
        },
        _ => false,
    }
}

proof fn lemma_input_error<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, i: InputId)
    requires
        env.fuel <= f2,
    ensures
        ({
            let e1 = spec_input_error(env, rank, i);
            e1 != Some(EvalError::OutOfFuel) ==> spec_input_error(with_fuel(env, f2), rank, i) == e1
        }),
    decreases rank, 5int, env.p, 0int, 0int,
{
    let ri = spec_rank_or(spec_rank(env.ranks.inputs, i.0), rank);
    if ri < rank {
        lemma_input(env, f2, ri, i);
    }
}

proof fn lemma_best<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, inputs: Seq<InputId>, j: int, b1: Option<(usize, EvalError)>, b2: Option<(usize, EvalError)>)
    requires
        env.fuel <= f2,
        best_rel(b1, b2),
    ensures
        best_rel(spec_best_input(env, rank, inputs, j, b1), spec_best_input(with_fuel(env, f2), rank, inputs, j, b2)),
    decreases rank, 6int, env.p, 0int, inputs.len() - j,
{
    if 0 <= j < inputs.len() {
        let i = inputs[j];
        lemma_input_error(env, f2, rank, i);
        let env2 = with_fuel(env, f2);
        let n1 = match spec_input_error(env, rank, i) {
            Some(e) => {
                let lower = match b1 {
                    Some((b, _)) => i.0 < b,
                    None => true,
                };
                if lower { Some((i.0, e)) } else { b1 }
            },
            None => b1,
        };
        let n2 = match spec_input_error(env2, rank, i) {
            Some(e) => {
                let lower = match b2 {
                    Some((b, _)) => i.0 < b,
                    None => true,
                };
                if lower { Some((i.0, e)) } else { b2 }
            },
            None => b2,
        };
        assert(best_rel(n1, n2));
        lemma_best(env, f2, rank, inputs, j + 1, n1, n2);
    }
}

/// Two lists of cells related cell by cell.
pub open spec fn cells_mono(c1: Seq<Result<SValue, EvalError>>, c2: Seq<Result<SValue, EvalError>>) -> bool {
    &&& c1.len() == c2.len()
    &&& forall|k: int| 0 <= k < c1.len() ==> mono(#[trigger] c1[k], c2[k])
}

pub proof fn lemma_unit<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, u: UnitId, base: Ctx)
    requires
        env.fuel <= f2,
    ensures
        cells_mono(spec_eval_unit_ctx(env, rank, u, base), spec_eval_unit_ctx(with_fuel(env, f2), rank, u, base)),
    decreases rank, 7int, env.p, 0int, 0int,
{
    let env2 = with_fuel(env, f2);
    match spec_len(base.dims) {
        None => {},
        Some(n) => {
            if u.0 < env.p.units@.len() {
                let inputs = env.p.units@[u.0 as int].inputs@;
                lemma_best(env, f2, rank, inputs, 0, None, None);
                let c1 = spec_eval_unit_ctx(env, rank, u, base);
                let c2 = spec_eval_unit_ctx(env2, rank, u, base);
                assert forall|k: int| 0 <= k < c1.len() implies mono(#[trigger] c1[k], c2[k]) by {
                    match spec_best_input(env, rank, inputs, 0, None) {
                        None => { lemma_unit_cell(env, f2, rank, u, base, k); },
                        Some(_) => {},
                    }
                }
            }
        },
    }
}

proof fn lemma_buffer(c1: Seq<Result<SValue, EvalError>>, c2: Seq<Result<SValue, EvalError>>, k: int, acc: Seq<SValue>)
    requires
        cells_mono(c1, c2),
        k >= 0,
    ensures
        ({
            let b1 = spec_buffer(c1, k, acc);
            !(b1 is Err && b1->Err_0.1 == EvalError::OutOfFuel) ==> spec_buffer(c2, k, acc) == b1
        }),
    decreases c1.len() - k,
{
    if 0 <= k < c1.len() {
        assert(mono(c1[k], c2[k]));
        match c1[k] {
            Ok(v) => lemma_buffer(c1, c2, k + 1, acc.push(v)),
            Err(_) => {},
        }
    }
}

proof fn lemma_input<E: Externs>(env: SEnv<E>, f2: u64, rank: usize, i: InputId)
    requires
        env.fuel <= f2,
    ensures
        ({
            let b1 = spec_build_input(env, rank, i);
            !(b1 is Err && b1->Err_0.1 == EvalError::OutOfFuel) ==> spec_build_input(with_fuel(env, f2), rank, i) == b1
        }),
    decreases rank, 8int, env.p, 0int, 0int,
{
    if i.0 < env.p.inputs@.len() {
        let d = env.p.inputs@[i.0 as int];
        let rs = spec_rank_or(spec_rank(env.ranks.units, d.source.0), rank);
        if rs < rank {
            let base = spec_base_ctx(env.origin, d.origin_scale, d.position_scale, d.dims);
            lemma_unit(env, f2, rs, d.source, base);
            lemma_buffer(spec_eval_unit_ctx(env, rs, d.source, base), spec_eval_unit_ctx(with_fuel(env, f2), rs, d.source, base), 0, Seq::empty());
        }
    }
}

proof fn lemma_first_const<E: Externs>(env: SEnv<E>, f2: u64, n: int)
    requires
        env.fuel <= f2,
    ensures
        ({
            let r1 = spec_first_const_error(env, n);
            r1 != Some(EvalError::OutOfFuel) ==> spec_first_const_error(with_fuel(env, f2), n) == r1
        }),
    decreases n,
{
    if n > 0 {
        lemma_first_const(env, f2, n - 1);
        match spec_rank(env.ranks.consts, (n - 1) as usize) {
            Some(rc) => lemma_const(env, f2, rc, ConstId((n - 1) as usize)),
            None => {},
        }
    }
}

proof fn lemma_entry<E: Externs>(env: SEnv<E>, f2: u64, e: int)
    requires
        env.fuel <= f2,
        0 <= e < env.p.entries@.len(),
    ensures
        cells_mono(spec_entry_cells(env, e), spec_entry_cells(with_fuel(env, f2), e)),
{
    let env2 = with_fuel(env, f2);
    let nc = env.p.consts@.len() as int;
    lemma_first_const(env, f2, nc);
    let entry = env.p.entries@[e];
    let base = spec_base_ctx(env.origin, entry.origin_scale, entry.position_scale, entry.dims);
    match spec_rank(env.ranks.units, entry.unit.0) {
        Some(ru) => lemma_unit(env, f2, ru, entry.unit, base),
        None => {},
    }
    // Whatever the second run does, a grid has the same number of cells.
    lemma_entry_len(env, e);
    lemma_entry_len(env2, e);
}

/// With a layout, an entry has that many cells; without one, one `Stuck(Layout)` cell.
proof fn lemma_entry_len<E: Externs>(env: SEnv<E>, e: int)
    requires
        0 <= e < env.p.entries@.len(),
    ensures
        match spec_len(env.p.entries@[e].dims) {
            Some(n) => spec_entry_cells(env, e).len() == n,
            None => spec_entry_cells(env, e) == seq![Err::<SValue, EvalError>(stuck(StuckReason::Layout))],
        },
{
    let entry = env.p.entries@[e];
    let base = spec_base_ctx(env.origin, entry.origin_scale, entry.position_scale, entry.dims);
    match spec_rank(env.ranks.units, entry.unit.0) {
        Some(ru) => {
            match spec_len(base.dims) {
                Some(n) => {
                    if entry.unit.0 < env.p.units@.len() {
                        match spec_best_input(env, ru, env.p.units@[entry.unit.0 as int].inputs@, 0, None) {
                            Some(_) => {},
                            None => {},
                        }
                    }
                },
                None => {},
            }
        },
        None => {},
    }
}

/// **T2.** Per entry, per cell: a result that isn't `OutOfFuel` at `env.fuel` is the same at every
/// `f2 >= env.fuel`. For every program, checked or not.
pub proof fn theorem_fuel_monotone<E: Externs>(env: SEnv<E>, f2: u64, e: int, k: int)
    requires
        env.fuel <= f2,
        0 <= e < env.p.entries@.len(),
        0 <= k < spec_eval_program(env)[e].len(),
        spec_eval_program(env)[e][k] != Err::<SValue, EvalError>(EvalError::OutOfFuel),
    ensures
        spec_eval_program(with_fuel(env, f2))[e].len() == spec_eval_program(env)[e].len(),
        spec_eval_program(with_fuel(env, f2))[e][k] == spec_eval_program(env)[e][k],
{
    lemma_entry(env, f2, e);
    let c1 = spec_entry_cells(env, e);
    let c2 = spec_entry_cells(with_fuel(env, f2), e);
    assert(mono(c1[k], c2[k]));
}

} // verus!
