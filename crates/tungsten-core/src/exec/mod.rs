//! The interpreter (§7). [`eval_program`] evaluates every const once (in `ConstId` order), then
//! every entry cell in flat order. A case's result is a pure function of `(program, case, fuel)`:
//! ranks come from the program alone and fuel is per `While` execution, so the memo never
//! changes a result.
//!
//! Termination: every call of `eval_call`, `eval_const`, `build_input` and `eval_unit_ctx` checks
//! that the target's rank is `Some` and strictly below the caller's (`Stuck(Rank)` otherwise).
//! The measure is `(rank, layer, structure)`: the layer orders the fns that run at one rank
//! (expr 0 < stmt 1 < call/const 2 < cell 3 < unit 4 < input 5), and the structure is the
//! expression or statement being evaluated. (Verus can't compare `Stmt` and `Expr` heights
//! inside one recursion group, so a statement → expression step drops a layer instead.)

use vstd::prelude::*;

use crate::check::acyclic::{Ranks, ranks};
use crate::ir::*;
use crate::layout;
use crate::util::*;
use crate::value::*;

pub mod expr;
pub mod stmt;
pub mod unit;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

/// What every evaluation fn reads but never changes.
pub struct Env<'a, E: Externs> {
    pub p: &'a Program,
    pub ranks: &'a Ranks,
    pub case: &'a Case,
    pub ext: &'a E,
    /// The most iterations one `While` execution may run.
    pub fuel: u64,
}

/// The evaluation context of one cell. `cell = decode(k, dims)`.
pub struct Ctx {
    pub origin: F64x3,
    pub origin_scale: F64x3,
    pub position_scale: F64x3,
    pub dims: Dims,
    pub k: usize,
    pub cell: I32x3,
}

pub fn copy_ctx(c: &Ctx) -> Ctx {
    Ctx {
        origin: copy_f64x3(&c.origin),
        origin_scale: copy_f64x3(&c.origin_scale),
        position_scale: copy_f64x3(&c.position_scale),
        dims: copy_dims(&c.dims),
        k: c.k,
        cell: copy_i32x3(&c.cell),
    }
}

/// One function call's locals. `locals[0..params]` start as the arguments.
pub struct Frame {
    pub locals: Vec<Option<Value>>,
}

pub enum Flow {
    Next,
    Break,
    Return(Value),
}

/// A diagnostic: a const or input that failed. Each is listed once per case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Const { id: ConstId, error: EvalError },
    /// `cell` is the first failing cell of that buffer, in flat order.
    Input { id: InputId, cell: usize, error: EvalError },
}

#[derive(Debug, PartialEq, Eq)]
pub struct CaseResult {
    /// Per entry, per cell in flat order.
    pub entries: Vec<Vec<Result<Value, EvalError>>>,
    /// Diagnostics (§7.7): every failing const, then every failing input in discovery order.
    pub failures: Vec<Failure>,
}

/// One per case. With `enabled == false` nothing is stored and every read recomputes; results
/// must be identical either way.
pub struct Memo {
    pub enabled: bool,
    /// By `ConstId`.
    pub consts: Vec<Option<Result<Value, EvalError>>>,
    /// By `InputId`. An error is `(first failing cell, error)`.
    pub inputs: Vec<Option<Result<Vec<Value>, (usize, EvalError)>>>,
    /// By `InputId`: already in `failures`. Doesn't affect results.
    pub listed: Vec<bool>,
    pub failures: Vec<Failure>,
}

pub fn new_memo(p: &Program, enabled: bool) -> Memo {
    let mut consts: Vec<Option<Result<Value, EvalError>>> = Vec::new();
    let mut k: usize = 0;
    while k < p.consts.len()
        invariant
            k <= p.consts.len(),
        decreases p.consts.len() - k,
    {
        consts.push(None);
        k = k + 1;
    }
    let mut inputs: Vec<Option<Result<Vec<Value>, (usize, EvalError)>>> = Vec::new();
    let mut k: usize = 0;
    while k < p.inputs.len()
        invariant
            k <= p.inputs.len(),
        decreases p.inputs.len() - k,
    {
        inputs.push(None);
        k = k + 1;
    }
    Memo { enabled, consts, inputs, listed: bools(p.inputs.len(), false), failures: Vec::new() }
}

/// `n` copies of `Err(e)`.
pub fn fill(n: usize, e: EvalError) -> Vec<Result<Value, EvalError>> {
    let mut out: Vec<Result<Value, EvalError>> = Vec::new();
    let mut k: usize = 0;
    while k < n
        invariant
            k <= n,
        decreases n - k,
    {
        out.push(Err(e));
        k = k + 1;
    }
    out
}

/// The cell count of `d`, or 1 if the layout overflows (that one cell is `Stuck(Layout)`).
pub fn cell_count(d: &Dims) -> usize {
    match layout::len(d) {
        Some(n) => n,
        None => 1,
    }
}

pub fn eval_program<E: Externs>(p: &Program, case: &Case, ext: &E, fuel: u64, memo: &mut Memo) -> CaseResult {
    let r = ranks(p);
    let env = Env { p, ranks: &r, case, ext, fuel };
    // Eager, global constants.
    let mut first: Option<EvalError> = None;
    let mut const_failures: Vec<Failure> = Vec::new();
    let mut c: usize = 0;
    while c < p.consts.len()
        invariant
            c <= p.consts.len(),
        decreases p.consts.len() - c,
    {
        let res = match get_rank(&r.consts, c) {
            Some(rc) => unit::eval_const(&env, rc, ConstId(c), memo),
            None => Err(EvalError::Stuck(StuckReason::Rank)),
        };
        match res {
            Ok(_) => {},
            Err(e) => {
                const_failures.push(Failure::Const { id: ConstId(c), error: e });
                if first.is_none() {
                    first = Some(e);
                }
            },
        }
        c = c + 1;
    }
    let mut entries: Vec<Vec<Result<Value, EvalError>>> = Vec::new();
    let mut e: usize = 0;
    while e < p.entries.len()
        invariant
            e <= p.entries.len(),
        decreases p.entries.len() - e,
    {
        let cells = match first {
            Some(err) => match layout::len(&p.entries[e].dims) {
                Some(n) => fill(n, err),
                None => fill(1, EvalError::Stuck(StuckReason::Layout)),
            },
            None => unit::eval_entry(&env, e, memo),
        };
        entries.push(cells);
        e = e + 1;
    }
    // Consts first, then inputs in discovery order.
    let mut failures = const_failures;
    let n = memo.failures.len();
    let mut k: usize = 0;
    while k < n
        invariant
            k <= n,
            n == memo.failures.len(),
        decreases n - k,
    {
        failures.push(memo.failures[k]);
        k = k + 1;
    }
    CaseResult { entries, failures }
}

} // verus!
