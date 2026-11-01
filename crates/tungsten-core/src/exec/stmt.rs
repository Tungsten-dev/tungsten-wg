//! Calls and statements (§7.4).
//!
//! `While` and `Repeat` are Rust loops, not recursion, so the stack doesn't grow with fuel:
//! a `While` runs at most `env.fuel` iterations (`OutOfFuel` if its cond is still true after
//! that), and `Repeat { count }` runs its body `count` times at no fuel cost.

use vstd::prelude::*;

use crate::exec::*;
use crate::exec::expr::eval_expr;
use crate::ir::*;
use crate::util::*;
use crate::value::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

fn stuck(r: StuckReason) -> EvalError {
    EvalError::Stuck(r)
}

/// Calls `g` with already evaluated `args`. Falling off the end is `Stuck(NoReturn)`, and a
/// `Break` reaching the body is `Stuck(BreakOutsideLoop)`.
pub fn eval_call<E: Externs>(env: &Env<E>, rank: usize, g: FnId, args: Vec<Value>, ctx: &Ctx, memo: &mut Memo) -> Result<Value, EvalError>
    decreases rank, 2int, args,
{
    let f = match get(&env.p.functions, g.0) {
        Some(f) => f,
        None => { return Err(stuck(StuckReason::BadIndex)); },
    };
    if args.len() != f.params || f.params > f.locals.len() {
        return Err(stuck(StuckReason::Shape));
    }
    let mut locals: Vec<Option<Value>> = Vec::new();
    let mut k: usize = 0;
    while k < f.locals.len()
        invariant
            k <= f.locals.len(),
        decreases f.locals.len() - k,
    {
        if k < args.len() {
            locals.push(Some(clone_value(&args[k])));
        } else {
            locals.push(None);
        }
        k = k + 1;
    }
    let mut frame = Frame { locals };
    match eval_stmts(env, rank, ctx, &mut frame, &f.body, memo) {
        Ok(Flow::Return(v)) => Ok(v),
        Ok(Flow::Next) => Err(stuck(StuckReason::NoReturn)),
        Ok(Flow::Break) => Err(stuck(StuckReason::BreakOutsideLoop)),
        Err(e) => Err(e),
    }
}

#[verifier::loop_isolation(false)]
pub fn eval_stmts<E: Externs>(env: &Env<E>, rank: usize, ctx: &Ctx, frame: &mut Frame, ss: &Vec<Stmt>, memo: &mut Memo) -> Result<Flow, EvalError>
    decreases rank, 1int, ss,
{
    let mut i: usize = 0;
    while i < ss.len()
        invariant
            i <= ss.len(),
        decreases ss.len() - i,
    {
        match eval_stmt(env, rank, ctx, frame, &ss[i], memo) {
            Ok(Flow::Next) => {},
            other => { return other; },
        }
        i = i + 1;
    }
    Ok(Flow::Next)
}

fn cond_bool(v: Value) -> Result<bool, EvalError> {
    match v {
        Value::Bool(b) => Ok(b),
        _ => Err(stuck(StuckReason::Shape)),
    }
}

#[verifier::loop_isolation(false)]
pub fn eval_stmt<E: Externs>(env: &Env<E>, rank: usize, ctx: &Ctx, frame: &mut Frame, s: &Stmt, memo: &mut Memo) -> Result<Flow, EvalError>
    decreases rank, 1int, s,
{
    match s {
        Stmt::Assign { target, value } => {
            let v = eval_expr(env, rank, ctx, frame, value, memo)?;
            if set(&mut frame.locals, target.0, Some(v)) {
                Ok(Flow::Next)
            } else {
                Err(stuck(StuckReason::BadIndex))
            }
        },
        Stmt::Return(e) => {
            let v = eval_expr(env, rank, ctx, frame, e, memo)?;
            Ok(Flow::Return(v))
        },
        Stmt::If { cond, then_branch, else_branch } => {
            let c = cond_bool(eval_expr(env, rank, ctx, frame, cond, memo)?)?;
            if c {
                eval_stmts(env, rank, ctx, frame, then_branch, memo)
            } else {
                eval_stmts(env, rank, ctx, frame, else_branch, memo)
            }
        },
        Stmt::While { cond, body } => {
            let fuel = env.fuel;
            let mut iters: u64 = 0;
            loop
                invariant
                    iters <= fuel,
                decreases fuel - iters,
            {
                let c = cond_bool(eval_expr(env, rank, ctx, frame, cond, memo)?)?;
                if !c {
                    return Ok(Flow::Next);
                }
                if iters >= fuel {
                    return Err(EvalError::OutOfFuel);
                }
                match eval_stmts(env, rank, ctx, frame, body, memo)? {
                    Flow::Next => {},
                    Flow::Break => { return Ok(Flow::Next); },
                    Flow::Return(v) => { return Ok(Flow::Return(v)); },
                }
                iters = iters + 1;
            }
        },
        Stmt::Repeat { count, body } => {
            let n = *count;
            let mut k: usize = 0;
            while k < n
                invariant
                    k <= n,
                decreases n - k,
            {
                match eval_stmts(env, rank, ctx, frame, body, memo)? {
                    Flow::Next => {},
                    Flow::Break => { return Ok(Flow::Next); },
                    Flow::Return(v) => { return Ok(Flow::Return(v)); },
                }
                k = k + 1;
            }
            Ok(Flow::Next)
        },
        Stmt::Break => Ok(Flow::Break),
    }
}

} // verus!
