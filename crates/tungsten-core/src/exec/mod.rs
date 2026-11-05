//! The interpreter (§7). [`eval_program`] evaluates every const once (in `ConstId` order), then
//! every entry cell in flat order. A case's result is a pure function of `(program, case, fuel)`:
//! ranks come from the program alone and fuel is per `While` execution, so the memo never
//! changes a result.
//!
//! Termination: every call of `eval_call`, `eval_const`, `build_input` and `eval_unit_ctx` checks
//! that the target's rank is `Some` and strictly below the caller's (`Stuck(Rank)` otherwise).
//! The measure is `(rank, layer, structure)`: the layer orders the fns that run at one rank
//! (expr 0 < stmt 1 < call/const 2 < cell 3 < cells 4 < unit 5 < input 6), and the structure is the
//! expression or statement being evaluated. (Verus can't compare `Stmt` and `Expr` heights
//! inside one recursion group, so a statement → expression step drops a layer instead.)

use vstd::prelude::*;

use crate::check::acyclic::{Ranks, ranks};
use crate::ir::*;
use crate::layout;
use crate::spec::eval::*;
use crate::spec::layout::*;
use crate::spec::ranks::*;
use crate::spec::value::*;
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

// ---------------------------------------------------------------------------------------------
// What the refinement proof says about these types
// ---------------------------------------------------------------------------------------------

pub open spec fn senv<E: Externs>(env: &Env<E>) -> SEnv<E> {
    SEnv {
        p: *env.p,
        ranks: view_ranks(*env.ranks),
        origin: env.case.origin,
        hosts: view_values(env.case.hosts@),
        ext: *env.ext,
        fuel: env.fuel,
    }
}

/// The rank tables have one slot per item.
pub open spec fn env_inv<E: Externs>(env: &Env<E>) -> bool {
    &&& env.ranks.fns@.len() == env.p.functions@.len()
    &&& env.ranks.consts@.len() == env.p.consts@.len()
    &&& env.ranks.inputs@.len() == env.p.inputs@.len()
    &&& env.ranks.units@.len() == env.p.units@.len()
}

pub open spec fn view_frame(f: Frame) -> SFrame {
    view_slots(f.locals@)
}

pub open spec fn view_flow(f: Flow) -> SFlow {
    match f {
        Flow::Next => SFlow::Next,
        Flow::Break => SFlow::Break,
        Flow::Return(v) => SFlow::Return(view_value(v)),
    }
}

pub open spec fn view_list(r: Result<Vec<Value>, EvalError>) -> Result<Seq<SValue>, EvalError> {
    match r {
        Ok(vs) => Ok(view_values(vs@)),
        Err(e) => Err(e),
    }
}

pub open spec fn view_slots_result(r: Result<Vec<Option<Value>>, EvalError>) -> Result<SFrame, EvalError> {
    match r {
        Ok(s) => Ok(view_slots(s@)),
        Err(e) => Err(e),
    }
}

/// A statement's exec result and frame agree with its spec result. On an error the frame can't be
/// observed, so it isn't compared.
pub open spec fn stmt_result_is(r: Result<Flow, EvalError>, frame: Frame, s: Result<(SFlow, SFrame), EvalError>) -> bool {
    match (r, s) {
        (Ok(fl), Ok((sf, sfr))) => view_flow(fl) == sf && view_frame(frame) == sfr,
        (Err(a), Err(b)) => a == b,
        _ => false,
    }
}

pub open spec fn input_build_is(r: unit::InputBuild, memo: Memo, i: InputId, s: Result<Seq<SValue>, (usize, EvalError)>) -> bool {
    match r {
        unit::InputBuild::Memoized => {
            &&& i.0 < memo.inputs@.len()
            &&& memo.inputs@[i.0 as int] is Some
            &&& view_buffer(memo.inputs@[i.0 as int]->0) == s
        },
        unit::InputBuild::Fresh(x) => view_buffer(x) == s,
    }
}

/// Which input a diagnostic is about.
pub open spec fn failure_input(f: Failure) -> usize {
    match f {
        Failure::Input { id, .. } => id.0,
        Failure::Const { id, .. } => id.0,
    }
}

/// A listed input failure is an input in range whose spec buffer fails exactly so.
pub open spec fn input_failure_ok<E: Externs>(env: SEnv<E>, f: Failure) -> bool {
    match f {
        Failure::Input { id, cell, error } => match spec_rank(env.ranks.inputs, id.0) {
            Some(r) => spec_build_input(env, r, id) == Err::<Seq<SValue>, (usize, EvalError)>((cell, error)),
            None => false,
        },
        Failure::Const { .. } => false,
    }
}

/// Everything the memo holds agrees with the unmemoized spec, per id, at that id's own rank.
/// Opaque: only the functions that read or write the memo reveal it, which keeps its quantifiers
/// out of every other proof.
#[verifier::opaque]
pub open spec fn memo_inv<E: Externs>(env: SEnv<E>, memo: Memo) -> bool {
    &&& memo.consts@.len() == env.p.consts@.len()
    &&& memo.inputs@.len() == env.p.inputs@.len()
    &&& memo.listed@.len() == env.p.inputs@.len()
    &&& forall|c: int| 0 <= c < memo.consts@.len() && (#[trigger] memo.consts@[c]) is Some ==> match spec_rank(env.ranks.consts, c as usize) {
        Some(rc) => view_result(memo.consts@[c]->0) == spec_eval_const(env, rc, ConstId(c as usize)),
        None => false,
    }
    &&& forall|i: int| 0 <= i < memo.inputs@.len() && (#[trigger] memo.inputs@[i]) is Some ==> match spec_rank(env.ranks.inputs, i as usize) {
        Some(ri) => view_buffer(memo.inputs@[i]->0) == spec_build_input(env, ri, InputId(i as usize)),
        None => false,
    }
    &&& forall|j: int| 0 <= j < memo.failures@.len() ==> input_failure_ok(env, #[trigger] memo.failures@[j])
    &&& forall|j: int| 0 <= j < memo.failures@.len() ==> memo.listed@[#[trigger] failure_input(memo.failures@[j]) as int]
    &&& forall|x: int, y: int| 0 <= x < y < memo.failures@.len()
        ==> #[trigger] failure_input(memo.failures@[x]) != #[trigger] failure_input(memo.failures@[y])
}

// ---------------------------------------------------------------------------------------------

pub fn new_memo(p: &Program, enabled: bool) -> (m: Memo)
    ensures
        m.enabled == enabled,
        m.consts@.len() == p.consts@.len(),
        m.inputs@.len() == p.inputs@.len(),
        m.listed@.len() == p.inputs@.len(),
        forall|c: int| 0 <= c < m.consts@.len() ==> (#[trigger] m.consts@[c]) is None,
        forall|i: int| 0 <= i < m.inputs@.len() ==> (#[trigger] m.inputs@[i]) is None,
        m.failures@.len() == 0,
{
    let mut consts: Vec<Option<Result<Value, EvalError>>> = Vec::new();
    let mut k: usize = 0;
    while k < p.consts.len()
        invariant
            k <= p.consts.len(),
            consts@.len() == k,
            forall|c: int| 0 <= c < k ==> (#[trigger] consts@[c]) is None,
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
            inputs@.len() == k,
            forall|i: int| 0 <= i < k ==> (#[trigger] inputs@[i]) is None,
        decreases p.inputs.len() - k,
    {
        inputs.push(None);
        k = k + 1;
    }
    Memo { enabled, consts, inputs, listed: bools(p.inputs.len(), false), failures: Vec::new() }
}

/// `n` copies of `Err(e)`.
pub fn fill(n: usize, e: EvalError) -> (out: Vec<Result<Value, EvalError>>)
    ensures
        view_results(out@) == spec_fill(n as nat, e),
{
    let mut out: Vec<Result<Value, EvalError>> = Vec::new();
    let mut k: usize = 0;
    while k < n
        invariant
            k <= n,
            out@.len() == k,
            forall|j: int| 0 <= j < k ==> out@[j] == Err::<Value, EvalError>(e),
        decreases n - k,
    {
        out.push(Err(e));
        k = k + 1;
    }
    proof {
        assert(view_results(out@) =~= spec_fill(n as nat, e));
    }
    out
}

/// The environment of a whole case: ranks come from the program alone.
pub open spec fn program_env<E: Externs>(p: Program, case: Case, ext: E, fuel: u64) -> SEnv<E> {
    SEnv { p, ranks: spec_ranks(p), origin: case.origin, hosts: view_values(case.hosts@), ext, fuel }
}

pub open spec fn view_entries(es: Seq<Vec<Result<Value, EvalError>>>) -> Seq<Seq<Result<SValue, EvalError>>> {
    Seq::new(es.len(), |e: int| view_results(es[e]@))
}

#[verifier::loop_isolation(false)]
pub fn eval_program<E: Externs>(p: &Program, case: &Case, ext: &E, fuel: u64, memo_enabled: bool) -> (out: CaseResult)
    ensures
        view_entries(out.entries@) == spec_eval_program(program_env(*p, *case, *ext, fuel)),
        diagnostics_sound(out.failures@, program_env(*p, *case, *ext, fuel)),
{
    let mut memo_store = new_memo(p, memo_enabled);
    let memo = &mut memo_store;
    let r = ranks(p);
    let env = Env { p, ranks: &r, case, ext, fuel };
    let ghost se = senv(&env);
    proof {
        reveal(memo_inv);
    }
    // Eager, global constants.
    let mut first: Option<EvalError> = None;
    let mut const_failures: Vec<Failure> = Vec::new();
    let mut c: usize = 0;
    proof {
        assert(const_failures@ =~= spec_const_failures(se, 0));
    }
    while c < p.consts.len()
        invariant
            c <= p.consts.len(),
            env_inv(&env),
            se == senv(&env),
            memo_inv(se, *memo),
            first == spec_first_const_error(se, c as int),
            const_failures@ == spec_const_failures(se, c as int),
        decreases p.consts.len() - c,
    {
        let res = match get_rank(&r.consts, c) {
            Some(rc) => unit::eval_const(&env, rc, ConstId(c), memo),
            None => Err(EvalError::Stuck(StuckReason::Rank)),
        };
        proof {
            assert(view_result(res) == spec_const_result(se, c as int));
        }
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
            env_inv(&env),
            se == senv(&env),
            c == p.consts@.len(),
            memo_inv(se, *memo),
            first == spec_first_const_error(se, c as int),
            entries@.len() == e,
            forall|j: int| 0 <= j < e ==> view_results((#[trigger] entries@[j])@) == spec_entry_cells(se, j),
        decreases p.entries.len() - e,
    {
        let cells = match first {
            Some(err) => match layout::len(&p.entries[e].dims) {
                Some(n) => fill(n, err),
                None => {
                    let out = fill(1, EvalError::Stuck(StuckReason::Layout));
                    proof {
                        assert(spec_fill(1, EvalError::Stuck(StuckReason::Layout)) =~= seq![Err(EvalError::Stuck(StuckReason::Layout))]);
                    }
                    out
                },
            },
            None => unit::eval_entry(&env, e, memo),
        };
        entries.push(cells);
        e = e + 1;
    }
    proof {
        assert(view_entries(entries@) =~= spec_eval_program(se));
    }
    // Consts first, then inputs in discovery order.
    let mut failures = const_failures;
    let ghost nc = failures@.len();
    let n = memo.failures.len();
    let mut k: usize = 0;
    while k < n
        invariant
            k <= n,
            n == memo.failures.len(),
            failures@.len() == nc + k,
            nc == spec_const_failures(se, p.consts@.len() as int).len(),
            forall|j: int| 0 <= j < nc ==> #[trigger] failures@[j] == spec_const_failures(se, p.consts@.len() as int)[j],
            forall|j: int| 0 <= j < k ==> #[trigger] failures@[nc + j] == memo.failures@[j],
            memo_inv(se, *memo),
        decreases n - k,
    {
        failures.push(memo.failures[k]);
        k = k + 1;
    }
    let out = CaseResult { entries, failures };
    proof {
        reveal(memo_inv);
        assert forall|j: int| nc <= j < failures@.len() implies input_failure_ok(se, #[trigger] failures@[j]) by {
            assert(failures@[nc + (j - nc)] == memo.failures@[j - nc]);
            assert(input_failure_ok(se, memo.failures@[j - nc]));
        }
        assert forall|x: int, y: int| nc <= x < y < failures@.len() implies #[trigger] failure_input(failures@[x]) != #[trigger] failure_input(failures@[y]) by {
            assert(failures@[nc + (x - nc)] == memo.failures@[x - nc]);
            assert(failures@[nc + (y - nc)] == memo.failures@[y - nc]);
            assert(failure_input(memo.failures@[x - nc]) != failure_input(memo.failures@[y - nc]));
        }
        assert(diagnostics_sound(out.failures@, se));
    }
    out
}

/// Const failures are exact; each input failure is correct, and no input is listed twice.
pub open spec fn diagnostics_sound<E: Externs>(fs: Seq<Failure>, env: SEnv<E>) -> bool {
    let cs = spec_const_failures(env, env.p.consts@.len() as int);
    &&& fs.len() >= cs.len()
    &&& forall|j: int| 0 <= j < cs.len() ==> #[trigger] fs[j] == cs[j]
    &&& forall|j: int| cs.len() <= j < fs.len() ==> input_failure_ok(env, #[trigger] fs[j])
    &&& forall|x: int, y: int| cs.len() <= x < y < fs.len() ==> #[trigger] failure_input(fs[x]) != #[trigger] failure_input(fs[y])
}

} // verus!
