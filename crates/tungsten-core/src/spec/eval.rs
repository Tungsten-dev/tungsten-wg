//! The semantics of a core program: one function per `eval_*` in `crate::exec`, with the same
//! arguments minus `memo`. It is total, so it also describes programs the checker would reject,
//! and it is stateless: a constant or a buffer is simply recomputed where it is read, so
//! memoization is something the refinement proof has to justify.
//!
//! Loops are recursive here (`spec_while`, `spec_repeat`) where `crate::exec` has Rust loops.
//!
//! Termination uses `(rank, layer, structure, loop count, list index)`, the same idea as
//! `crate::exec` plus the two extra components that recursion over loops and lists needs.

use vstd::prelude::*;

use crate::exec::{Ctx, Failure};
use crate::ir::*;
use crate::spec::layout::*;
use crate::spec::ops::*;
use crate::spec::value::*;
use crate::value::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

/// What every evaluation function reads and never changes.
pub struct SEnv<E> {
    pub p: Program,
    pub ranks: SRanks,
    pub origin: F64x3,
    pub hosts: Seq<SValue>,
    pub ext: E,
    pub fuel: u64,
}

pub open spec fn stuck(r: StuckReason) -> EvalError {
    EvalError::Stuck(r)
}

pub open spec fn spec_rank(t: Seq<Option<usize>>, id: usize) -> Option<usize> {
    if id < t.len() { t[id as int] } else { None }
}

/// The target's rank, or `current`, which then fails the caller's `< current` test.
pub open spec fn spec_rank_or(target: Option<usize>, current: usize) -> usize {
    match target {
        Some(r) => r,
        None => current,
    }
}

pub open spec fn spec_fill(n: nat, e: EvalError) -> Seq<Result<SValue, EvalError>> {
    Seq::new(n, |k: int| Err(e))
}

pub open spec fn spec_lit_value(l: Lit) -> SValue {
    match l {
        Lit::Bool(b) => SValue::Bool(b),
        Lit::I8(x) => SValue::I8(x),
        Lit::I32(x) => SValue::I32(x),
        Lit::I64(x) => SValue::I64(x),
        Lit::F32(x) => SValue::F32(x),
        Lit::F64(x) => SValue::F64(x),
    }
}

pub open spec fn spec_cond(v: SValue) -> Result<bool, EvalError> {
    match v {
        SValue::Bool(b) => Ok(b),
        _ => Err(shape()),
    }
}

pub open spec fn spec_read_buf(buf: Seq<SValue>, k: usize) -> Result<SValue, EvalError> {
    if k < buf.len() { Ok(buf[k as int]) } else { Err(EvalError::OutOfBounds) }
}

/// `Some` in every slot, in slot order, or `Stuck(Shape)` for the first empty one (a missing field).
pub open spec fn spec_collect_slots(slots: SFrame, k: int, acc: Seq<SValue>) -> Result<Seq<SValue>, EvalError>
    decreases slots.len() - k,
{
    if 0 <= k < slots.len() {
        match slots[k] {
            Some(v) => spec_collect_slots(slots, k + 1, acc.push(v)),
            None => Err(shape()),
        }
    } else {
        Ok(acc)
    }
}

pub open spec fn spec_empty_slots(n: nat) -> SFrame {
    Seq::new(n, |k: int| None)
}

/// Fresh locals: the arguments in the first slots, the rest unassigned (SEMANTICS.md §7.1).
pub open spec fn spec_init_frame(args: Seq<SValue>, n: nat) -> SFrame {
    Seq::new(n, |k: int| if k < args.len() { Some(args[k]) } else { None })
}

/// A const's placeholder context. A checked const never reads it (CHECK.md §4.5).
pub open spec fn spec_const_ctx() -> Ctx {
    let zero = F64x3 { x: 0, y: 0, z: 0 };
    Ctx {
        origin: zero,
        origin_scale: zero,
        position_scale: zero,
        dims: Dims { x: 1, y: 1, z: 1 },
        k: 0,
        cell: I32x3 { x: 0, y: 0, z: 0 },
    }
}

/// The buffer of a unit's cells: all values, or the first failing cell (SEMANTICS.md §9.1).
pub open spec fn spec_buffer(cells: Seq<Result<SValue, EvalError>>, k: int, acc: Seq<SValue>) -> Result<Seq<SValue>, (usize, EvalError)>
    decreases cells.len() - k,
{
    if 0 <= k < cells.len() {
        match cells[k] {
            Ok(v) => spec_buffer(cells, k + 1, acc.push(v)),
            Err(e) => Err((k as usize, e)),
        }
    } else {
        Ok(acc)
    }
}

/// The evaluation context of cell `k` of a grid.
pub open spec fn spec_cell_ctx(base: Ctx, k: usize, cell: I32x3) -> Ctx {
    Ctx {
        origin: base.origin,
        origin_scale: base.origin_scale,
        position_scale: base.position_scale,
        dims: base.dims,
        k,
        cell,
    }
}

pub open spec fn spec_base_ctx(origin: F64x3, origin_scale: F64x3, position_scale: F64x3, dims: Dims) -> Ctx {
    Ctx { origin, origin_scale, position_scale, dims, k: 0, cell: I32x3 { x: 0, y: 0, z: 0 } }
}

// ---------------------------------------------------------------------------------------------
// Expressions (SEMANTICS.md §5). Left to right; the first error wins.
// ---------------------------------------------------------------------------------------------

/// Evaluates `args[i..]` left to right, appending to `acc`.
pub open spec fn spec_args<E: Externs>(env: SEnv<E>, rank: usize, ctx: Ctx, frame: SFrame, args: Vec<Expr>, i: int, acc: Seq<SValue>) -> Result<Seq<SValue>, EvalError>
    decreases rank, 0int, args, 0int, args@.len() - i,
{
    if 0 <= i < args@.len() {
        match spec_eval_expr(env, rank, ctx, frame, args@[i]) {
            Ok(v) => spec_args(env, rank, ctx, frame, args, i + 1, acc.push(v)),
            Err(e) => Err(e),
        }
    } else {
        Ok(acc)
    }
}

/// `MakeRecord`: evaluates the field values in written order and stores each in its declared slot.
pub open spec fn spec_fields<E: Externs>(env: SEnv<E>, rank: usize, ctx: Ctx, frame: SFrame, fields: Vec<FieldInit>, i: int, slots: SFrame) -> Result<SFrame, EvalError>
    decreases rank, 0int, fields, 0int, fields@.len() - i,
{
    if 0 <= i < fields@.len() {
        match spec_eval_expr(env, rank, ctx, frame, fields@[i].value) {
            Ok(v) => {
                let f = fields@[i].field;
                if f >= slots.len() {
                    Err(stuck(StuckReason::BadIndex))
                } else if slots[f as int] is Some {
                    Err(shape())
                } else {
                    spec_fields(env, rank, ctx, frame, fields, i + 1, slots.update(f as int, Some(v)))
                }
            },
            Err(e) => Err(e),
        }
    } else {
        Ok(slots)
    }
}

/// `Density`: SEMANTICS.md §9.3. The index is evaluated before the buffer is built.
pub open spec fn spec_read_density<E: Externs>(env: SEnv<E>, rank: usize, ctx: Ctx, frame: SFrame, input: InputId, index: Option<Box<Expr>>) -> Result<SValue, EvalError>
    decreases rank, 0int, index, 0int, 0int,
{
    let explicit: Result<Option<i32>, EvalError> = match index {
        Some(ix) => match spec_eval_expr(env, rank, ctx, frame, *ix) {
            Ok(SValue::I32(i)) => Ok(Some(i)),
            Ok(_) => Err(shape()),
            Err(e) => Err(e),
        },
        None => Ok(None),
    };
    match explicit {
        Err(e) => Err(e),
        Ok(ex) => {
            if input.0 >= env.p.inputs@.len() {
                Err(stuck(StuckReason::BadIndex))
            } else if ex is Some && ex->0 < 0 {
                Err(EvalError::OutOfBounds)
            } else {
                let d = env.p.inputs@[input.0 as int];
                let k: usize = match ex {
                    Some(i) => i as usize,
                    None => if spec_is_unit(d.dims) { 0 } else { ctx.k },
                };
                let ri = spec_rank_or(spec_rank(env.ranks.inputs, input.0), rank);
                if ri >= rank {
                    Err(stuck(StuckReason::Rank))
                } else {
                    match spec_build_input(env, ri, input) {
                        Ok(buf) => spec_read_buf(buf, k),
                        Err((_, e)) => Err(e),
                    }
                }
            }
        },
    }
}

pub open spec fn spec_eval_expr<E: Externs>(env: SEnv<E>, rank: usize, ctx: Ctx, frame: SFrame, e: Expr) -> Result<SValue, EvalError>
    decreases rank, 0int, e, 0int, 0int,
{
    let p = env.p;
    match e {
        Expr::Lit(l) => Ok(spec_lit_value(l)),
        Expr::Local(l) => {
            if l.0 >= frame.len() {
                Err(stuck(StuckReason::BadIndex))
            } else {
                match frame[l.0 as int] {
                    Some(v) => Ok(v),
                    None => Err(stuck(StuckReason::Unassigned)),
                }
            }
        },
        Expr::Const(c) => {
            let rc = spec_rank_or(spec_rank(env.ranks.consts, c.0), rank);
            if rc < rank {
                spec_eval_const(env, rc, c)
            } else {
                Err(stuck(StuckReason::Rank))
            }
        },
        Expr::Host(h) => if h.0 < env.hosts.len() { Ok(env.hosts[h.0 as int]) } else { Err(stuck(StuckReason::Case)) },
        Expr::Table(t) => {
            if t.0 >= p.tables@.len() {
                Err(stuck(StuckReason::BadIndex))
            } else {
                match p.tables@[t.0 as int].kind {
                    TableKind::Perm => Ok(SValue::PermTable(t)),
                    TableKind::Sampler => Ok(SValue::Sampler(t)),
                }
            }
        },
        Expr::Ambient(a) => Ok(match a {
            Ambient::Origin => SValue::Vec3(ctx.origin),
            Ambient::OriginScale => SValue::Vec3(ctx.origin_scale),
            Ambient::PositionScale => SValue::Vec3(ctx.position_scale),
            Ambient::Pos3 => SValue::Pos3(ctx.cell),
        }),
        Expr::Density { input, index } => spec_read_density(env, rank, ctx, frame, input, index),
        Expr::Call { func, args } => match spec_args(env, rank, ctx, frame, args, 0, Seq::empty()) {
            Err(e) => Err(e),
            Ok(vals) => {
                let rg = spec_rank_or(spec_rank(env.ranks.fns, func.0), rank);
                if rg < rank {
                    spec_eval_call(env, rg, func, vals, ctx)
                } else {
                    Err(stuck(StuckReason::Rank))
                }
            },
        },
        Expr::ExternCall { ext, args, .. } => match spec_args(env, rank, ctx, frame, args, 0, Seq::empty()) {
            Err(e) => Err(e),
            Ok(vals) => {
                if ext.0 >= p.externs@.len() {
                    Err(stuck(StuckReason::BadIndex))
                } else {
                    let decl = p.externs@[ext.0 as int];
                    match env.ext.spec_call(ext, vals) {
                        Ok(v) => {
                            if spec_value_has_type(p, v, decl.ret) {
                                Ok(v)
                            } else {
                                Err(stuck(StuckReason::ExternResult))
                            }
                        },
                        Err(ExternFail::Domain) => {
                            if decl.domain_error {
                                Err(EvalError::ExternDomain)
                            } else {
                                Err(stuck(StuckReason::ExternDomainUndeclared))
                            }
                        },
                    }
                }
            },
        },
        Expr::Binary { op, left, right } => match spec_eval_expr(env, rank, ctx, frame, *left) {
            Err(e) => Err(e),
            Ok(l) => match op {
                // Short-circuit (SEMANTICS.md §5).
                BinOp::And => match spec_cond(l) {
                    Err(e) => Err(e),
                    Ok(false) => Ok(SValue::Bool(false)),
                    Ok(true) => match spec_eval_expr(env, rank, ctx, frame, *right) {
                        Err(e) => Err(e),
                        Ok(r) => match spec_cond(r) {
                            Err(e) => Err(e),
                            Ok(b) => Ok(SValue::Bool(b)),
                        },
                    },
                },
                BinOp::Or => match spec_cond(l) {
                    Err(e) => Err(e),
                    Ok(true) => Ok(SValue::Bool(true)),
                    Ok(false) => match spec_eval_expr(env, rank, ctx, frame, *right) {
                        Err(e) => Err(e),
                        Ok(r) => match spec_cond(r) {
                            Err(e) => Err(e),
                            Ok(b) => Ok(SValue::Bool(b)),
                        },
                    },
                },
                _ => match spec_eval_expr(env, rank, ctx, frame, *right) {
                    Err(e) => Err(e),
                    Ok(r) => spec_binop(op, l, r),
                },
            },
        },
        Expr::Unary { op, operand } => match spec_eval_expr(env, rank, ctx, frame, *operand) {
            Err(e) => Err(e),
            Ok(v) => spec_unop(op, v),
        },
        Expr::Field { base, sel, .. } => match spec_eval_expr(env, rank, ctx, frame, *base) {
            Err(e) => Err(e),
            Ok(b) => spec_field(sel, b),
        },
        Expr::Index { array, index } => match spec_eval_expr(env, rank, ctx, frame, *array) {
            Err(e) => Err(e),
            Ok(a) => match spec_eval_expr(env, rank, ctx, frame, *index) {
                Err(e) => Err(e),
                Ok(i) => match (a, i) {
                    (SValue::Array(vs), SValue::I32(i)) => {
                        if i < 0 {
                            Err(EvalError::OutOfBounds)
                        } else {
                            spec_read_buf(vs, i as usize)
                        }
                    },
                    _ => Err(shape()),
                },
            },
        },
        Expr::MakeVec3 { x, y, z } => match spec_eval_expr(env, rank, ctx, frame, *x) {
            Err(e) => Err(e),
            Ok(a) => match spec_eval_expr(env, rank, ctx, frame, *y) {
                Err(e) => Err(e),
                Ok(b) => match spec_eval_expr(env, rank, ctx, frame, *z) {
                    Err(e) => Err(e),
                    Ok(c) => match (a, b, c) {
                        (SValue::F64(a), SValue::F64(b), SValue::F64(c)) => Ok(SValue::Vec3(F64x3 { x: a, y: b, z: c })),
                        _ => Err(shape()),
                    },
                },
            },
        },
        Expr::MakePos3 { x, y, z } => match spec_eval_expr(env, rank, ctx, frame, *x) {
            Err(e) => Err(e),
            Ok(a) => match spec_eval_expr(env, rank, ctx, frame, *y) {
                Err(e) => Err(e),
                Ok(b) => match spec_eval_expr(env, rank, ctx, frame, *z) {
                    Err(e) => Err(e),
                    Ok(c) => match (a, b, c) {
                        (SValue::I32(a), SValue::I32(b), SValue::I32(c)) => Ok(SValue::Pos3(I32x3 { x: a, y: b, z: c })),
                        _ => Err(shape()),
                    },
                },
            },
        },
        Expr::MakeRecord { record, fields } => {
            if record.0 >= p.records@.len() {
                Err(stuck(StuckReason::BadIndex))
            } else {
                let n = p.records@[record.0 as int].fields@.len();
                match spec_fields(env, rank, ctx, frame, fields, 0, spec_empty_slots(n)) {
                    Err(e) => Err(e),
                    Ok(slots) => match spec_collect_slots(slots, 0, Seq::empty()) {
                        Err(e) => Err(e),
                        Ok(vals) => Ok(SValue::Record(record, vals)),
                    },
                }
            }
        },
        Expr::ArrayLit(es) => match spec_args(env, rank, ctx, frame, es, 0, Seq::empty()) {
            Err(e) => Err(e),
            Ok(vals) => Ok(SValue::Array(vals)),
        },
        Expr::Cast { to, operand } => match spec_eval_expr(env, rank, ctx, frame, *operand) {
            Err(e) => Err(e),
            Ok(v) => spec_cast(to, v),
        },
    }
}

// ---------------------------------------------------------------------------------------------
// Statements and calls (SEMANTICS.md §7). A statement's result carries the frame after it; a
// failed statement's frame can't be observed, so it isn't described.
// ---------------------------------------------------------------------------------------------

/// `ss[i..]` in order; the first that doesn't end with `Next` ends the list.
pub open spec fn spec_stmts<E: Externs>(env: SEnv<E>, rank: usize, ctx: Ctx, frame: SFrame, ss: Vec<Stmt>, i: int) -> Result<(SFlow, SFrame), EvalError>
    decreases rank, 1int, ss, 0int, ss@.len() - i,
{
    if 0 <= i < ss@.len() {
        match spec_eval_stmt(env, rank, ctx, frame, ss@[i]) {
            Ok((SFlow::Next, f)) => spec_stmts(env, rank, ctx, f, ss, i + 1),
            other => other,
        }
    } else {
        Ok((SFlow::Next, frame))
    }
}

/// `While`, with `iters` iterations already run (SEMANTICS.md §7.3).
pub open spec fn spec_while<E: Externs>(env: SEnv<E>, rank: usize, ctx: Ctx, frame: SFrame, cond: Expr, body: Vec<Stmt>, iters: int) -> Result<(SFlow, SFrame), EvalError>
    decreases rank, 1int, body, env.fuel - iters + 1, 0int,
{
    match spec_eval_expr(env, rank, ctx, frame, cond) {
        Err(e) => Err(e),
        Ok(v) => match spec_cond(v) {
            Err(e) => Err(e),
            Ok(false) => Ok((SFlow::Next, frame)),
            Ok(true) => {
                if iters >= env.fuel || iters < 0 {
                    Err(EvalError::OutOfFuel)
                } else {
                    match spec_stmts(env, rank, ctx, frame, body, 0) {
                        Err(e) => Err(e),
                        Ok((SFlow::Next, f)) => spec_while(env, rank, ctx, f, cond, body, iters + 1),
                        Ok((SFlow::Break, f)) => Ok((SFlow::Next, f)),
                        Ok((SFlow::Return(v), f)) => Ok((SFlow::Return(v), f)),
                    }
                }
            },
        },
    }
}

/// `Repeat`, with `k` of `n` rounds already run. No fuel.
pub open spec fn spec_repeat<E: Externs>(env: SEnv<E>, rank: usize, ctx: Ctx, frame: SFrame, n: int, body: Vec<Stmt>, k: int) -> Result<(SFlow, SFrame), EvalError>
    decreases rank, 1int, body, n - k + 1, 0int,
{
    if 0 <= k < n {
        match spec_stmts(env, rank, ctx, frame, body, 0) {
            Err(e) => Err(e),
            Ok((SFlow::Next, f)) => spec_repeat(env, rank, ctx, f, n, body, k + 1),
            Ok((SFlow::Break, f)) => Ok((SFlow::Next, f)),
            Ok((SFlow::Return(v), f)) => Ok((SFlow::Return(v), f)),
        }
    } else {
        Ok((SFlow::Next, frame))
    }
}

pub open spec fn spec_eval_stmt<E: Externs>(env: SEnv<E>, rank: usize, ctx: Ctx, frame: SFrame, s: Stmt) -> Result<(SFlow, SFrame), EvalError>
    decreases rank, 1int, s, 0int, 0int,
{
    match s {
        Stmt::Assign { target, value } => match spec_eval_expr(env, rank, ctx, frame, value) {
            Err(e) => Err(e),
            Ok(v) => {
                if target.0 < frame.len() {
                    Ok((SFlow::Next, frame.update(target.0 as int, Some(v))))
                } else {
                    Err(stuck(StuckReason::BadIndex))
                }
            },
        },
        Stmt::Return(e) => match spec_eval_expr(env, rank, ctx, frame, e) {
            Err(e) => Err(e),
            Ok(v) => Ok((SFlow::Return(v), frame)),
        },
        Stmt::If { cond, then_branch, else_branch } => match spec_eval_expr(env, rank, ctx, frame, cond) {
            Err(e) => Err(e),
            Ok(v) => match spec_cond(v) {
                Err(e) => Err(e),
                Ok(true) => spec_stmts(env, rank, ctx, frame, then_branch, 0),
                Ok(false) => spec_stmts(env, rank, ctx, frame, else_branch, 0),
            },
        },
        Stmt::While { cond, body } => spec_while(env, rank, ctx, frame, cond, body, 0),
        Stmt::Repeat { count, body } => spec_repeat(env, rank, ctx, frame, count as int, body, 0),
        Stmt::Break => Ok((SFlow::Break, frame)),
    }
}

/// SEMANTICS.md §7.1.
pub open spec fn spec_eval_call<E: Externs>(env: SEnv<E>, rank: usize, g: FnId, args: Seq<SValue>, ctx: Ctx) -> Result<SValue, EvalError>
    decreases rank, 2int, env.p, 0int, 0int,
{
    if g.0 >= env.p.functions@.len() {
        Err(stuck(StuckReason::BadIndex))
    } else {
        let f = env.p.functions@[g.0 as int];
        if args.len() != f.params || f.params > f.locals@.len() {
            Err(shape())
        } else {
            match spec_stmts(env, rank, ctx, spec_init_frame(args, f.locals@.len()), f.body, 0) {
                Ok((SFlow::Return(v), _)) => Ok(v),
                Ok((SFlow::Next, _)) => Err(stuck(StuckReason::NoReturn)),
                Ok((SFlow::Break, _)) => Err(stuck(StuckReason::BreakOutsideLoop)),
                Err(e) => Err(e),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Constants, inputs, units, cells (SEMANTICS.md §4, §8, §9)
// ---------------------------------------------------------------------------------------------

pub open spec fn spec_eval_const<E: Externs>(env: SEnv<E>, rank: usize, c: ConstId) -> Result<SValue, EvalError>
    decreases rank, 2int, env.p, 0int, 0int,
{
    if c.0 >= env.p.consts@.len() {
        Err(stuck(StuckReason::BadIndex))
    } else {
        spec_eval_expr(env, rank, spec_const_ctx(), Seq::empty(), env.p.consts@[c.0 as int].init)
    }
}

pub open spec fn spec_eval_cell<E: Externs>(env: SEnv<E>, rank: usize, u: UnitId, ctx: Ctx) -> Result<SValue, EvalError>
    decreases rank, 3int, env.p, 0int, 0int,
{
    if u.0 >= env.p.units@.len() {
        Err(stuck(StuckReason::BadIndex))
    } else {
        let unit = env.p.units@[u.0 as int];
        if unit.main.0 >= env.p.functions@.len() {
            Err(stuck(StuckReason::BadIndex))
        } else if env.p.functions@[unit.main.0 as int].params > 0 {
            Err(stuck(StuckReason::MainParams))
        } else {
            let rm = spec_rank_or(spec_rank(env.ranks.fns, unit.main.0), rank);
            if rm < rank {
                spec_eval_call(env, rm, unit.main, Seq::empty(), ctx)
            } else {
                Err(stuck(StuckReason::Rank))
            }
        }
    }
}

/// Cell `k` of `u` over `base.dims`.
pub open spec fn spec_unit_cell<E: Externs>(env: SEnv<E>, rank: usize, u: UnitId, base: Ctx, k: int) -> Result<SValue, EvalError>
    decreases rank, 4int, env.p, 0int, 0int,
{
    match spec_decode(k as usize, base.dims) {
        Some(cell) => spec_eval_cell(env, rank, u, spec_cell_ctx(base, k as usize, cell)),
        None => Err(stuck(StuckReason::Layout)),
    }
}

/// The result of building one input a unit lists: its error, if it fails.
pub open spec fn spec_input_error<E: Externs>(env: SEnv<E>, rank: usize, i: InputId) -> Option<EvalError>
    decreases rank, 5int, env.p, 0int, 0int,
{
    let ri = spec_rank_or(spec_rank(env.ranks.inputs, i.0), rank);
    if ri < rank {
        match spec_build_input(env, ri, i) {
            Err((_, e)) => Some(e),
            Ok(_) => None,
        }
    } else {
        Some(stuck(StuckReason::Rank))
    }
}

/// Over `inputs[j..]`: the failing input with the lowest `InputId` (SEMANTICS.md §9.2).
pub open spec fn spec_best_input<E: Externs>(env: SEnv<E>, rank: usize, inputs: Seq<InputId>, j: int, best: Option<(usize, EvalError)>) -> Option<(usize, EvalError)>
    decreases rank, 6int, env.p, 0int, inputs.len() - j,
{
    if 0 <= j < inputs.len() {
        let i = inputs[j];
        let next = match spec_input_error(env, rank, i) {
            Some(e) => {
                let lower = match best {
                    Some((b, _)) => i.0 < b,
                    None => true,
                };
                if lower { Some((i.0, e)) } else { best }
            },
            None => best,
        };
        spec_best_input(env, rank, inputs, j + 1, next)
    } else {
        best
    }
}

/// Unit `u` over every cell of `base.dims`: every listed input first, then the cells.
pub open spec fn spec_eval_unit_ctx<E: Externs>(env: SEnv<E>, rank: usize, u: UnitId, base: Ctx) -> Seq<Result<SValue, EvalError>>
    decreases rank, 7int, env.p, 0int, 0int,
{
    match spec_len(base.dims) {
        None => seq![Err(stuck(StuckReason::Layout))],
        Some(n) => {
            if u.0 >= env.p.units@.len() {
                spec_fill(n as nat, stuck(StuckReason::BadIndex))
            } else {
                match spec_best_input(env, rank, env.p.units@[u.0 as int].inputs@, 0, None) {
                    Some((_, e)) => spec_fill(n as nat, e),
                    None => Seq::new(n as nat, |k: int| spec_unit_cell(env, rank, u, base, k)),
                }
            }
        },
    }
}

/// SEMANTICS.md §9.1. Depends only on the input, never on who reads it.
pub open spec fn spec_build_input<E: Externs>(env: SEnv<E>, rank: usize, i: InputId) -> Result<Seq<SValue>, (usize, EvalError)>
    decreases rank, 8int, env.p, 0int, 0int,
{
    if i.0 >= env.p.inputs@.len() {
        Err((0, stuck(StuckReason::BadIndex)))
    } else {
        let d = env.p.inputs@[i.0 as int];
        let rs = spec_rank_or(spec_rank(env.ranks.units, d.source.0), rank);
        if rs >= rank {
            Err((0, stuck(StuckReason::Rank)))
        } else {
            let base = spec_base_ctx(env.origin, d.origin_scale, d.position_scale, d.dims);
            spec_buffer(spec_eval_unit_ctx(env, rs, d.source, base), 0, Seq::empty())
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The program (SEMANTICS.md §4)
// ---------------------------------------------------------------------------------------------

pub open spec fn spec_eval_entry<E: Externs>(env: SEnv<E>, e: int) -> Seq<Result<SValue, EvalError>> {
    let entry = env.p.entries@[e];
    let base = spec_base_ctx(env.origin, entry.origin_scale, entry.position_scale, entry.dims);
    match spec_rank(env.ranks.units, entry.unit.0) {
        Some(ru) => spec_eval_unit_ctx(env, ru, entry.unit, base),
        None => match spec_len(entry.dims) {
            Some(n) => spec_fill(n as nat, stuck(StuckReason::Rank)),
            None => seq![Err(stuck(StuckReason::Layout))],
        },
    }
}

/// One constant at its own rank.
pub open spec fn spec_const_result<E: Externs>(env: SEnv<E>, c: int) -> Result<SValue, EvalError> {
    match spec_rank(env.ranks.consts, c as usize) {
        Some(rc) => spec_eval_const(env, rc, ConstId(c as usize)),
        None => Err(stuck(StuckReason::Rank)),
    }
}

/// The error of the lowest failing const among the first `n`.
pub open spec fn spec_first_const_error<E: Externs>(env: SEnv<E>, n: int) -> Option<EvalError>
    decreases n,
{
    if n <= 0 {
        None
    } else {
        match spec_first_const_error(env, n - 1) {
            Some(e) => Some(e),
            None => match spec_const_result(env, n - 1) {
                Err(e) => Some(e),
                Ok(_) => None,
            },
        }
    }
}

/// The failing consts among the first `n`, in id order.
pub open spec fn spec_const_failures<E: Externs>(env: SEnv<E>, n: int) -> Seq<Failure>
    decreases n,
{
    if n <= 0 {
        Seq::empty()
    } else {
        let before = spec_const_failures(env, n - 1);
        match spec_const_result(env, n - 1) {
            Err(e) => before.push(Failure::Const { id: ConstId((n - 1) as usize), error: e }),
            Ok(_) => before,
        }
    }
}

/// A failing const fails every cell of every entry with the lowest failing const's error.
pub open spec fn spec_entry_cells<E: Externs>(env: SEnv<E>, e: int) -> Seq<Result<SValue, EvalError>> {
    match spec_first_const_error(env, env.p.consts@.len() as int) {
        Some(err) => match spec_len(env.p.entries@[e].dims) {
            Some(n) => spec_fill(n as nat, err),
            None => seq![Err(stuck(StuckReason::Layout))],
        },
        None => spec_eval_entry(env, e),
    }
}

/// Per entry, per cell in flat order. This is what T1 says `eval_program` computes.
pub open spec fn spec_eval_program<E: Externs>(env: SEnv<E>) -> Seq<Seq<Result<SValue, EvalError>>> {
    Seq::new(env.p.entries@.len(), |e: int| spec_entry_cells(env, e))
}

} // verus!
