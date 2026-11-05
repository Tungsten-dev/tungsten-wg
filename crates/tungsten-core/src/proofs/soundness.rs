//! T4 over the spec: a well-typed program and a fitting case never get `Stuck`, except through an
//! extern that breaks its own declaration, and every `Ok` has its static type.
//!
//! `ext_ok` selects the theorem or its corollary: with `ext_ok == false` the extern's two `Stuck`
//! reasons are allowed and nothing is assumed about it; with `ext_ok == true` the extern respects
//! its declarations, and no `Stuck` is allowed at all.

use vstd::prelude::*;

use crate::exec::*;
use crate::ir::*;
use crate::proofs::layout::*;
use crate::spec::eval::*;
use crate::spec::layout::*;
use crate::spec::ops::*;
use crate::spec::ranks::*;
use crate::spec::typing::*;
use crate::spec::value::*;
use crate::value::*;
use crate::wf;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

/// An error a program may legitimately end with.
pub open spec fn allowed(ext_ok: bool, e: EvalError) -> bool {
    match e {
        EvalError::Stuck(r) => !ext_ok && (r is ExternResult || r is ExternDomainUndeclared),
        _ => true,
    }
}

pub open spec fn result_ok(ext_ok: bool, p: Program, r: Result<SValue, EvalError>, t: Type) -> bool {
    match r {
        Ok(v) => spec_value_has_type(p, v, t),
        Err(e) => allowed(ext_ok, e),
    }
}

pub open spec fn args_typed(p: Program, vs: Seq<SValue>, ts: Seq<Type>) -> bool {
    vs.len() == ts.len() && forall|k: int| 0 <= k < vs.len() ==> spec_value_has_type(p, #[trigger] vs[k], ts[k])
}

/// The extern does what its declaration says: on arguments of the declared types it returns a value
/// of the declared type, and a domain error only if it declares one.
pub open spec fn extern_respects<E: Externs>(p: Program, ext: E) -> bool {
    forall|id: int, args: Seq<SValue>| #![trigger ext.spec_call(ExternId(id as usize), args)]
        0 <= id < p.externs@.len() && args_typed(p, args, p.externs@[id].params@) ==> match ext.spec_call(ExternId(id as usize), args) {
            Ok(v) => spec_value_has_type(p, v, p.externs@[id].ret),
            Err(ExternFail::Domain) => p.externs@[id].domain_error,
        }
}

/// What the induction assumes about the environment. Opaque, with one accessor lemma per use, to
/// keep `well_typed`'s quantifiers out of the big proofs.
#[verifier::opaque]
pub open spec fn env_ok<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool) -> bool {
    &&& spec_well_typed_with(env.p, ctys)
    &&& env.ranks == spec_ranks(env.p)
    &&& spec_case_fits(env.p, env.hosts)
    &&& ext_ok ==> extern_respects(env.p, env.ext)
}

proof fn env_host<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, h: int)
    requires
        env_ok(env, ctys, ext_ok),
        0 <= h < env.p.hosts@.len(),
    ensures
        h < env.hosts.len(),
        spec_value_has_type(env.p, env.hosts[h], env.p.hosts@[h].ty),
{
    reveal(env_ok);
}

proof fn env_const<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, c: int)
    requires
        env_ok(env, ctys, ext_ok),
        0 <= c < env.p.consts@.len(),
    ensures
        ctys.len() == env.p.consts@.len(),
        ctys[c] is Some,
        spec_type_of(env.p, Seq::empty(), ctys, env.p.consts@[c].init) == ctys[c],
        env.ranks.consts[c] is Some,
        spec_expr_ranked(env.ranks, env.ranks.consts[c]->0, env.p.consts@[c].init),
        spec_rank(env.ranks.consts, c as usize) == env.ranks.consts[c],
{
    reveal(env_ok);
    assert(env.ranks.consts.len() == env.p.consts@.len());
    assert(env.ranks.consts[c] is Some);
}

proof fn env_fn<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, g: int)
    requires
        env_ok(env, ctys, ext_ok),
        0 <= g < env.p.functions@.len(),
    ensures
        env.p.functions@[g].params <= env.p.functions@[g].locals@.len(),
        spec_function_typed(env.p, ctys, env.p.functions@[g]),
        spec_flow_ok(env.p.functions@[g]),
        env.ranks.fns[g] is Some,
        spec_rank(env.ranks.fns, g as usize) == env.ranks.fns[g],
        forall|j: int| #![trigger env.p.functions@[g].body@[j]] 0 <= j < env.p.functions@[g].body@.len()
            ==> spec_stmt_ranked(env.ranks, env.ranks.fns[g]->0, env.p.functions@[g].body@[j]),
{
    reveal(env_ok);
    assert(wf::wf_function(&env.p, &env.p.functions[g]));
    assert(env.ranks.fns.len() == env.p.functions@.len());
    assert(env.ranks.fns[g] is Some);
    assert(spec_function_typed(env.p, ctys, env.p.functions@[g]) && spec_flow_ok(env.p.functions@[g]));
}

proof fn env_input<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, i: int)
    requires
        env_ok(env, ctys, ext_ok),
        0 <= i < env.p.inputs@.len(),
    ensures
        spec_dims_ok(env.p.inputs@[i].dims),
        env.p.inputs@[i].source.0 < env.p.units@.len(),
        env.ranks.inputs[i] is Some,
        spec_rank(env.ranks.inputs, i as usize) == env.ranks.inputs[i],
        spec_below(env.ranks.units, env.p.inputs@[i].source.0, env.ranks.inputs[i]->0),
{
    reveal(env_ok);
    assert(wf::wf_input(&env.p, &env.p.inputs[i]));
    assert(env.ranks.inputs.len() == env.p.inputs@.len());
    assert(env.ranks.inputs[i] is Some);
    assert(spec_dims_ok(env.p.inputs@[i].dims));
}

proof fn env_unit<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, u: int)
    requires
        env_ok(env, ctys, ext_ok),
        0 <= u < env.p.units@.len(),
    ensures
        env.p.units@[u].main.0 < env.p.functions@.len(),
        env.p.functions@[env.p.units@[u].main.0 as int].params == 0,
        env.ranks.units[u] is Some,
        spec_rank(env.ranks.units, u as usize) == env.ranks.units[u],
        spec_below(env.ranks.fns, env.p.units@[u].main.0, env.ranks.units[u]->0),
        forall|j: int| #![trigger env.p.units@[u].inputs@[j]] 0 <= j < env.p.units@[u].inputs@.len() ==> {
            &&& spec_below(env.ranks.inputs, env.p.units@[u].inputs@[j].0, env.ranks.units[u]->0)
            &&& env.p.units@[u].inputs@[j].0 < env.p.inputs@.len()
        },
{
    reveal(env_ok);
    assert(wf::wf_unit(&env.p, &env.p.units[u]));
    assert(env.ranks.units.len() == env.p.units@.len());
    assert(env.ranks.units[u] is Some);
    assert(env.p.functions@[env.p.units@[u].main.0 as int].params == 0);
}

proof fn env_rank_len<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool)
    requires
        env_ok(env, ctys, ext_ok),
    ensures
        env.ranks.fns.len() == env.p.functions@.len(),
        env.ranks.consts.len() == env.p.consts@.len(),
        env.ranks.inputs.len() == env.p.inputs@.len(),
        env.ranks.units.len() == env.p.units@.len(),
{
    reveal(env_ok);
}

proof fn env_extern<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, id: ExternId, args: Seq<SValue>)
    requires
        env_ok(env, ctys, ext_ok),
        ext_ok,
        id.0 < env.p.externs@.len(),
        args_typed(env.p, args, env.p.externs@[id.0 as int].params@),
    ensures
        match env.ext.spec_call(id, args) {
            Ok(v) => spec_value_has_type(env.p, v, env.p.externs@[id.0 as int].ret),
            Err(ExternFail::Domain) => env.p.externs@[id.0 as int].domain_error,
        },
{
    reveal(env_ok);
    assert(extern_respects(env.p, env.ext));
    assert(ExternId(id.0 as int as usize) == id);
    let ghost _t = env.ext.spec_call(ExternId(id.0 as int as usize), args);
}

// ---------------------------------------------------------------------------------------------
// Operators on typed values
// ---------------------------------------------------------------------------------------------

proof fn lemma_binop(p: Program, op: BinOp, l: SValue, r: SValue, lt: Type, rt: Type, t: Type, ext_ok: bool)
    requires
        spec_value_has_type(p, l, lt),
        spec_value_has_type(p, r, rt),
        spec_binary_type(op, lt, rt) == Some(t),
        !(op is And),
        !(op is Or),
    ensures
        result_ok(ext_ok, p, spec_binop(op, l, r), t),
{
    match lt {
        Type::Bool => { assert(l is Bool); },
        Type::I8 => { assert(l is I8); },
        Type::I32 => { assert(l is I32); },
        Type::I64 => { assert(l is I64); },
        Type::F32 => { assert(l is F32); },
        Type::F64 => { assert(l is F64); },
        Type::Vec3 => { assert(l is Vec3); },
        Type::Pos3 => { assert(l is Pos3); },
        _ => {},
    }
    match rt {
        Type::Bool => { assert(r is Bool); },
        Type::I8 => { assert(r is I8); },
        Type::I32 => { assert(r is I32); },
        Type::I64 => { assert(r is I64); },
        Type::F32 => { assert(r is F32); },
        Type::F64 => { assert(r is F64); },
        Type::Vec3 => { assert(r is Vec3); },
        Type::Pos3 => { assert(r is Pos3); },
        _ => {},
    }
}

proof fn lemma_unop(p: Program, op: UnOp, v: SValue, vt: Type, t: Type, ext_ok: bool)
    requires
        spec_value_has_type(p, v, vt),
        spec_unary_type(op, vt) == Some(t),
    ensures
        result_ok(ext_ok, p, spec_unop(op, v), t),
{
}

proof fn lemma_cast(p: Program, to: Type, v: SValue, vt: Type, ext_ok: bool)
    requires
        spec_value_has_type(p, v, vt),
        spec_cast_ok(vt, to),
    ensures
        result_ok(ext_ok, p, spec_cast(to, v), to),
{
}

proof fn lemma_field(p: Program, sel: FieldSel, b: SValue, bt: Type, t: Type, ext_ok: bool)
    requires
        spec_value_has_type(p, b, bt),
        spec_field_type(p, sel, bt) == Some(t),
    ensures
        result_ok(ext_ok, p, spec_field(sel, b), t),
{
    match (sel, b) {
        (FieldSel::Record(r, i), SValue::Record(s, vs)) => {
            assert(spec_value_has_type(p, vs[i as int], p.records@[r.0 as int].fields@[i as int]));
        },
        _ => {},
    }
}

proof fn lemma_collect(p: Program, slots: SFrame, k: int, acc: Seq<SValue>, ts: Seq<Type>)
    requires
        0 <= k <= slots.len(),
        slots.len() == ts.len(),
        acc.len() == k,
        forall|j: int| 0 <= j < k ==> spec_value_has_type(p, #[trigger] acc[j], ts[j]),
        forall|j: int| 0 <= j < slots.len() ==> (#[trigger] slots[j]) is Some && spec_value_has_type(p, slots[j]->0, ts[j]),
    ensures
        spec_collect_slots(slots, k, acc) is Ok,
        args_typed(p, spec_collect_slots(slots, k, acc)->Ok_0, ts),
    decreases slots.len() - k,
{
    if k < slots.len() {
        let v = slots[k]->0;
        assert(slots[k] is Some && spec_value_has_type(p, v, ts[k]));
        let a2 = acc.push(v);
        assert forall|j: int| 0 <= j < k + 1 implies spec_value_has_type(p, #[trigger] a2[j], ts[j]) by {
            if j < k {
                assert(a2[j] == acc[j]);
            }
        }
        lemma_collect(p, slots, k + 1, a2, ts);
    } else {
        assert(args_typed(p, acc, ts));
    }
}

proof fn lemma_buffer(p: Program, cells: Seq<Result<SValue, EvalError>>, k: int, acc: Seq<SValue>, t: Type, ext_ok: bool)
    requires
        0 <= k <= cells.len(),
        forall|j: int| 0 <= j < cells.len() ==> result_ok(ext_ok, p, #[trigger] cells[j], t),
        forall|j: int| 0 <= j < acc.len() ==> spec_value_has_type(p, #[trigger] acc[j], t),
    ensures
        match spec_buffer(cells, k, acc) {
            Ok(buf) => forall|j: int| 0 <= j < buf.len() ==> spec_value_has_type(p, #[trigger] buf[j], t),
            Err((_, e)) => allowed(ext_ok, e),
        },
    decreases cells.len() - k,
{
    if k < cells.len() {
        assert(result_ok(ext_ok, p, cells[k], t));
        match cells[k] {
            Ok(v) => {
                let a2 = acc.push(v);
                assert forall|j: int| 0 <= j < a2.len() implies spec_value_has_type(p, #[trigger] a2[j], t) by {
                    if j < acc.len() {
                        assert(a2[j] == acc[j]);
                    }
                }
                lemma_buffer(p, cells, k + 1, a2, t, ext_ok);
            },
            Err(_) => {},
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------------------------

pub open spec fn frame_typed(p: Program, frame: SFrame, locals: Seq<Type>) -> bool {
    &&& frame.len() == locals.len()
    &&& forall|l: int| 0 <= l < frame.len() && (#[trigger] frame[l]) is Some ==> spec_value_has_type(p, frame[l]->0, locals[l])
}

pub open spec fn assigned_some(frame: SFrame, assigned: Seq<bool>) -> bool {
    forall|l: int| 0 <= l < assigned.len() && l < frame.len() && #[trigger] assigned[l] ==> frame[l] is Some
}

pub open spec fn preserves(f1: SFrame, f2: SFrame) -> bool {
    f1.len() == f2.len() && forall|l: int| 0 <= l < f1.len() && (#[trigger] f1[l]) is Some ==> f2[l] is Some
}

/// `e` reads no unassigned local: because it reads none at all (a const), or by flow.
pub open spec fn reads_ok(locals: Seq<Type>, assigned: Seq<bool>, e: Expr) -> bool {
    locals.len() == 0 || spec_reads_assigned(assigned, e)
}

/// The arguments and everything else an expression needs, in one place.
pub open spec fn expr_pre(p: Program, ranks: SRanks, ctys: Seq<Option<Type>>, locals: Seq<Type>, rank: usize, frame: SFrame, assigned: Seq<bool>, e: Expr, t: Type) -> bool {
    &&& frame_typed(p, frame, locals)
    &&& assigned_some(frame, assigned)
    &&& reads_ok(locals, assigned, e)
    &&& spec_expr_ranked(ranks, rank, e)
    &&& spec_type_of(p, locals, ctys, e) == Some(t)
}

// ---------------------------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------------------------

proof fn lemma_args<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, rank: usize, ctx: Ctx, frame: SFrame, assigned: Seq<bool>, args: Vec<Expr>, i: int, acc: Seq<SValue>, ts: Seq<Type>)
    requires
        env_ok(env, ctys, ext_ok),
        0 <= i <= args@.len(),
        args@.len() == ts.len(),
        acc.len() == i,
        forall|k: int| 0 <= k < i ==> spec_value_has_type(env.p, #[trigger] acc[k], ts[k]),
        forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() ==> expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, args@[k], ts[k]),
    ensures
        match spec_args(env, rank, ctx, frame, args, i, acc) {
            Ok(vs) => args_typed(env.p, vs, ts),
            Err(e) => allowed(ext_ok, e),
        },
    decreases rank, 0int, args, 0int, args@.len() - i,
{
    if i < args@.len() {
        assert(expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, args@[i], ts[i]));
        lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, args@[i], ts[i]);
        match spec_eval_expr(env, rank, ctx, frame, args@[i]) {
            Ok(v) => {
                let a2 = acc.push(v);
                assert forall|k: int| 0 <= k < i + 1 implies spec_value_has_type(env.p, #[trigger] a2[k], ts[k]) by {
                    if k < i {
                        assert(a2[k] == acc[k]);
                    }
                }
                lemma_args(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, args, i + 1, a2, ts);
            },
            Err(_) => {},
        }
    } else {
        assert(args_typed(env.p, acc, ts));
    }
}

/// While evaluating `MakeRecord`: a slot is filled exactly when an earlier field wrote it.
pub open spec fn slots_inv(p: Program, slots: SFrame, fields: Seq<FieldInit>, i: int, ts: Seq<Type>) -> bool {
    &&& slots.len() == ts.len()
    &&& forall|j: int| 0 <= j < slots.len() && (#[trigger] slots[j]) is Some ==> spec_value_has_type(p, slots[j]->0, ts[j])
    &&& forall|a: int| 0 <= a < i ==> (#[trigger] slots[fields[a].field as int]) is Some
    &&& forall|j: int| 0 <= j < slots.len() && (#[trigger] slots[j]) is Some ==> exists|a: int| 0 <= a < i && fields[a].field == j
}

proof fn lemma_fields<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, rank: usize, ctx: Ctx, frame: SFrame, assigned: Seq<bool>, fields: Vec<FieldInit>, i: int, slots: SFrame, ts: Seq<Type>)
    requires
        env_ok(env, ctys, ext_ok),
        0 <= i <= fields@.len(),
        slots_inv(env.p, slots, fields@, i, ts),
        forall|k: int| #![trigger fields@[k]] 0 <= k < fields@.len() ==> {
            &&& fields@[k].field < ts.len()
            &&& expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, fields@[k].value, ts[fields@[k].field as int])
        },
        forall|a: int, b: int| #![trigger fields@[a], fields@[b]] 0 <= a < b < fields@.len() ==> fields@[a].field != fields@[b].field,
    ensures
        match spec_fields(env, rank, ctx, frame, fields, i, slots) {
            Ok(s) => slots_inv(env.p, s, fields@, fields@.len() as int, ts),
            Err(e) => allowed(ext_ok, e),
        },
    decreases rank, 0int, fields, 0int, fields@.len() - i,
{
    if i < fields@.len() {
        let f = fields@[i].field;
        assert(f < ts.len());
        assert(expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, fields@[i].value, ts[f as int]));
        lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, fields@[i].value, ts[f as int]);
        match spec_eval_expr(env, rank, ctx, frame, fields@[i].value) {
            Ok(v) => {
                // No earlier field wrote this slot.
                if slots[f as int] is Some {
                    assert(slots[f as int] is Some);
                    let a = choose|a: int| 0 <= a < i && fields@[a].field == f as int;
                    assert(fields@[a] == fields@[a] && fields@[i] == fields@[i]);
                    assert(fields@[a].field != fields@[i].field);
                    assert(false);
                }
                let s2 = slots.update(f as int, Some(v));
                assert forall|a: int| 0 <= a < i + 1 implies (#[trigger] s2[fields@[a].field as int]) is Some by {
                    if a < i {
                        assert(slots[fields@[a].field as int] is Some);
                    }
                }
                assert forall|j: int| 0 <= j < s2.len() && (#[trigger] s2[j]) is Some implies exists|a: int| 0 <= a < i + 1 && fields@[a].field == j by {
                    if j == f as int {
                        assert(fields@[i].field == j);
                    } else {
                        assert(slots[j] is Some);
                        let a = choose|a: int| 0 <= a < i && fields@[a].field == j;
                        assert(0 <= a < i + 1 && fields@[a].field == j);
                    }
                }
                assert forall|j: int| 0 <= j < s2.len() && (#[trigger] s2[j]) is Some implies spec_value_has_type(env.p, s2[j]->0, ts[j]) by {
                    if j != f as int {
                        assert(slots[j] is Some);
                    }
                }
                lemma_fields(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, fields, i + 1, s2, ts);
            },
            Err(_) => {},
        }
    }
}

proof fn lemma_density<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, rank: usize, ctx: Ctx, frame: SFrame, assigned: Seq<bool>, input: InputId, index: Option<Box<Expr>>, t: Type)
    requires
        env_ok(env, ctys, ext_ok),
        spec_density_type(env.p, input) == Some(t),
        spec_below(env.ranks.inputs, input.0, rank),
        match index {
            Some(ix) => expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, *ix, Type::I32),
            None => true,
        },
    ensures
        result_ok(ext_ok, env.p, spec_read_density(env, rank, ctx, frame, input, index), t),
    decreases rank, 0int, index, 0int, 0int,
{
    match index {
        Some(ix) => lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *ix, Type::I32),
        None => {},
    }
    assert(input.0 < env.p.inputs@.len());
    env_input(env, ctys, ext_ok, input.0 as int);
    let ri = spec_rank(env.ranks.inputs, input.0)->0;
    assert(ri < rank);
    lemma_input(env, ctys, ext_ok, ri, input);
    match spec_build_input(env, ri, input) {
        Ok(buf) => {
            match index {
                Some(ix) => match spec_eval_expr(env, rank, ctx, frame, *ix) {
                    Ok(SValue::I32(i)) => {
                        if i >= 0 && (i as usize as int) < buf.len() {
                            assert(spec_value_has_type(env.p, buf[i as usize as int], t));
                        }
                    },
                    _ => {},
                },
                None => {
                    let d = env.p.inputs@[input.0 as int];
                    let k: usize = if spec_is_unit(d.dims) { 0 } else { ctx.k };
                    if (k as int) < buf.len() {
                        assert(spec_value_has_type(env.p, buf[k as int], t));
                    }
                },
            }
        },
        Err(_) => {},
    }
}

/// `spec_call` with typed arguments. Without `ext_ok` the two `Stuck`s are allowed anyway.
proof fn lemma_extern_call<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, ext: ExternId, vals: Seq<SValue>)
    requires
        env_ok(env, ctys, ext_ok),
        ext.0 < env.p.externs@.len(),
        args_typed(env.p, vals, env.p.externs@[ext.0 as int].params@),
    ensures
        ({
            let decl = env.p.externs@[ext.0 as int];
            ext_ok ==> match env.ext.spec_call(ext, vals) {
                Ok(v) => spec_value_has_type(env.p, v, decl.ret),
                Err(ExternFail::Domain) => decl.domain_error,
            }
        }),
{
    if ext_ok {
        env_extern(env, ctys, ext_ok, ext, vals);
    }
}

/// Expression arm with sub-expressions, to keep `lemma_expr` small.
proof fn lemma_expr_call<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, rank: usize, ctx: Ctx, frame: SFrame, assigned: Seq<bool>, e: Expr, t: Type)
    requires
        env_ok(env, ctys, ext_ok),
        e is Call,
        expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, e, t),
    ensures
        result_ok(ext_ok, env.p, spec_eval_expr(env, rank, ctx, frame, e), t),
    decreases rank, 0int, e, 0int, 1int,
{
    match e {
        Expr::Call { func, args } => {
            let f = env.p.functions@[func.0 as int];
            env_fn(env, ctys, ext_ok, func.0 as int);
            let ts = f.locals@.subrange(0, args@.len() as int);
            assert forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() implies expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, args@[k], ts[k]) by {
                assert(ts[k] == f.locals@[k]);
            }
            lemma_args(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, args, 0, Seq::empty(), ts);
            match spec_args(env, rank, ctx, frame, args, 0, Seq::empty()) {
                Ok(vals) => {
                    let rg = spec_rank(env.ranks.fns, func.0)->0;
                    assert(rg < rank);
                    lemma_call(env, ctys, ext_ok, rg, func, vals, ctx);
                },
                Err(_) => {},
            }
        },
        _ => {},
    }
}

proof fn lemma_expr_extern<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, rank: usize, ctx: Ctx, frame: SFrame, assigned: Seq<bool>, e: Expr, t: Type)
    requires
        env_ok(env, ctys, ext_ok),
        e is ExternCall,
        expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, e, t),
    ensures
        result_ok(ext_ok, env.p, spec_eval_expr(env, rank, ctx, frame, e), t),
    decreases rank, 0int, e, 0int, 1int,
{
    match e {
        Expr::ExternCall { ext, args, .. } => {
            let d = env.p.externs@[ext.0 as int];
            let ts = d.params@;
            assert forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() implies expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, args@[k], ts[k]) by {}
            lemma_args(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, args, 0, Seq::empty(), ts);
            match spec_args(env, rank, ctx, frame, args, 0, Seq::empty()) {
                Ok(vals) => lemma_extern_call(env, ctys, ext_ok, ext, vals),
                Err(_) => {},
            }
        },
        _ => {},
    }
}

proof fn lemma_expr_binary<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, rank: usize, ctx: Ctx, frame: SFrame, assigned: Seq<bool>, e: Expr, t: Type)
    requires
        env_ok(env, ctys, ext_ok),
        e is Binary,
        expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, e, t),
    ensures
        result_ok(ext_ok, env.p, spec_eval_expr(env, rank, ctx, frame, e), t),
    decreases rank, 0int, e, 0int, 1int,
{
    match e {
        Expr::Binary { op, left, right } => {
            let lt = spec_type_of(env.p, locals, ctys, *left)->0;
            let rt = spec_type_of(env.p, locals, ctys, *right)->0;
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *left, lt);
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *right, rt);
            match (spec_eval_expr(env, rank, ctx, frame, *left), spec_eval_expr(env, rank, ctx, frame, *right)) {
                (Ok(l), Ok(r)) => {
                    if !(op is And) && !(op is Or) {
                        lemma_binop(env.p, op, l, r, lt, rt, t, ext_ok);
                    }
                },
                _ => {},
            }
        },
        _ => {},
    }
}

proof fn lemma_expr_record<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, rank: usize, ctx: Ctx, frame: SFrame, assigned: Seq<bool>, e: Expr, t: Type)
    requires
        env_ok(env, ctys, ext_ok),
        e is MakeRecord,
        expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, e, t),
    ensures
        result_ok(ext_ok, env.p, spec_eval_expr(env, rank, ctx, frame, e), t),
    decreases rank, 0int, e, 0int, 1int,
{
    match e {
        Expr::MakeRecord { record, fields } => {
            let d = env.p.records@[record.0 as int];
            let ts = d.fields@;
            let empty = spec_empty_slots(ts.len());
            assert(t == Type::Record(record));
            assert(slots_inv(env.p, empty, fields@, 0, ts));
            assert forall|k: int| #![trigger fields@[k]] 0 <= k < fields@.len() implies {
                &&& fields@[k].field < ts.len()
                &&& expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, fields@[k].value, ts[fields@[k].field as int])
            } by {}
            lemma_fields(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, fields, 0, empty, ts);
            match spec_fields(env, rank, ctx, frame, fields, 0, empty) {
                Ok(slots) => {
                    assert forall|j: int| 0 <= j < slots.len() implies (#[trigger] slots[j]) is Some && spec_value_has_type(env.p, slots[j]->0, ts[j]) by {
                        assert(spec_field_given(fields@, j));
                        let a = choose|a: int| 0 <= a < fields@.len() && #[trigger] fields@[a].field == j;
                        assert(slots[fields@[a].field as int] is Some);
                    }
                    lemma_collect(env.p, slots, 0, Seq::empty(), ts);
                    let vals = spec_collect_slots(slots, 0, Seq::empty())->Ok_0;
                    assert(args_typed(env.p, vals, ts));
                    assert(d == env.p.records[record.0 as int]);
                    assert(vals.len() == env.p.records[record.0 as int].fields.len());
                    assert forall|i: int| #![trigger vals[i]] 0 <= i < vals.len() implies spec_value_has_type(env.p, vals[i], env.p.records[record.0 as int].fields[i]) by {
                        assert(spec_value_has_type(env.p, vals[i], ts[i]));
                    }
                    assert(spec_value_has_type(env.p, SValue::Record(record, vals), t));
                },
                Err(_) => {},
            }
        },
        _ => {},
    }
}

proof fn lemma_expr_array<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, rank: usize, ctx: Ctx, frame: SFrame, assigned: Seq<bool>, e: Expr, t: Type)
    requires
        env_ok(env, ctys, ext_ok),
        e is ArrayLit,
        expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, e, t),
    ensures
        result_ok(ext_ok, env.p, spec_eval_expr(env, rank, ctx, frame, e), t),
    decreases rank, 0int, e, 0int, 1int,
{
    match e {
        Expr::ArrayLit(es) => {
            let f = spec_type_of(env.p, locals, ctys, es@[0])->0;
            let ts = Seq::new(es@.len(), |k: int| f);
            assert(t == Type::Array(Box::new(f), es@.len() as usize));
            assert forall|k: int| #![trigger es@[k]] 0 <= k < es@.len() implies expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, es@[k], ts[k]) by {
                assert(ts[k] == f);
            }
            lemma_args(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, es, 0, Seq::empty(), ts);
            match spec_args(env, rank, ctx, frame, es, 0, Seq::empty()) {
                Ok(vals) => {
                    assert(args_typed(env.p, vals, ts));
                    assert(es@.len() == es.len());
                    assert forall|i: int| #![trigger vals[i]] 0 <= i < vals.len() implies spec_value_has_type(env.p, vals[i], f) by {
                        assert(ts[i] == f);
                        assert(spec_value_has_type(env.p, vals[i], ts[i]));
                    }
                    assert(vals.len() == es@.len());
                    assert(spec_value_has_type(env.p, SValue::Array(vals), t));
                },
                Err(_) => {},
            }
        },
        _ => {},
    }
}

proof fn lemma_expr_leaf<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, rank: usize, ctx: Ctx, frame: SFrame, assigned: Seq<bool>, e: Expr, t: Type)
    requires
        env_ok(env, ctys, ext_ok),
        e is Lit || e is Local || e is Const || e is Host || e is Table || e is Ambient,
        expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, e, t),
    ensures
        result_ok(ext_ok, env.p, spec_eval_expr(env, rank, ctx, frame, e), t),
    decreases rank, 0int, e, 0int, 1int,
{
    match e {
        Expr::Local(l) => {
            assert(l.0 < frame.len());
            assert(frame[l.0 as int] is Some) by {
                assert(spec_reads_assigned(assigned, e));
                assert(assigned[l.0 as int]);
            }
        },
        Expr::Const(c) => {
            env_rank_len(env, ctys, ext_ok);
            assert(spec_below(env.ranks.consts, c.0, rank));
            env_const(env, ctys, ext_ok, c.0 as int);
            let rc = spec_rank(env.ranks.consts, c.0)->0;
            lemma_const(env, ctys, ext_ok, rc, c);
        },
        Expr::Host(h) => {
            env_host(env, ctys, ext_ok, h.0 as int);
        },
        _ => {},
    }
}

proof fn lemma_expr_one<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, rank: usize, ctx: Ctx, frame: SFrame, assigned: Seq<bool>, e: Expr, t: Type)
    requires
        env_ok(env, ctys, ext_ok),
        e is Unary || e is Field || e is Cast,
        expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, e, t),
    ensures
        result_ok(ext_ok, env.p, spec_eval_expr(env, rank, ctx, frame, e), t),
    decreases rank, 0int, e, 0int, 1int,
{
    match e {
        Expr::Unary { op, operand } => {
            let vt = spec_type_of(env.p, locals, ctys, *operand)->0;
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *operand, vt);
            match spec_eval_expr(env, rank, ctx, frame, *operand) {
                Ok(v) => lemma_unop(env.p, op, v, vt, t, ext_ok),
                Err(_) => {},
            }
        },
        Expr::Field { base, sel, .. } => {
            let bt = spec_type_of(env.p, locals, ctys, *base)->0;
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *base, bt);
            match spec_eval_expr(env, rank, ctx, frame, *base) {
                Ok(b) => lemma_field(env.p, sel, b, bt, t, ext_ok),
                Err(_) => {},
            }
        },
        Expr::Cast { to, operand } => {
            let vt = spec_type_of(env.p, locals, ctys, *operand)->0;
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *operand, vt);
            match spec_eval_expr(env, rank, ctx, frame, *operand) {
                Ok(v) => lemma_cast(env.p, to, v, vt, ext_ok),
                Err(_) => {},
            }
        },
        _ => {},
    }
}

proof fn lemma_expr_many<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, rank: usize, ctx: Ctx, frame: SFrame, assigned: Seq<bool>, e: Expr, t: Type)
    requires
        env_ok(env, ctys, ext_ok),
        e is Index || e is MakeVec3 || e is MakePos3,
        expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, e, t),
    ensures
        result_ok(ext_ok, env.p, spec_eval_expr(env, rank, ctx, frame, e), t),
    decreases rank, 0int, e, 0int, 1int,
{
    match e {
        Expr::Index { array, index } => {
            let at = spec_type_of(env.p, locals, ctys, *array)->0;
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *array, at);
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *index, Type::I32);
            match (spec_eval_expr(env, rank, ctx, frame, *array), spec_eval_expr(env, rank, ctx, frame, *index)) {
                (Ok(SValue::Array(vs)), Ok(SValue::I32(i))) => {
                    if i >= 0 && (i as usize as int) < vs.len() {
                        assert(spec_value_has_type(env.p, vs[i as usize as int], t));
                    }
                },
                _ => {},
            }
        },
        Expr::MakeVec3 { x, y, z } => {
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *x, Type::F64);
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *y, Type::F64);
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *z, Type::F64);
        },
        Expr::MakePos3 { x, y, z } => {
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *x, Type::I32);
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *y, Type::I32);
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, *z, Type::I32);
        },
        _ => {},
    }
}

#[verifier::spinoff_prover]
pub proof fn lemma_expr<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, rank: usize, ctx: Ctx, frame: SFrame, assigned: Seq<bool>, e: Expr, t: Type)
    requires
        env_ok(env, ctys, ext_ok),
        expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, e, t),
    ensures
        result_ok(ext_ok, env.p, spec_eval_expr(env, rank, ctx, frame, e), t),
    decreases rank, 0int, e, 1int, 0int,
{
    if e is Lit || e is Local || e is Const || e is Host || e is Table || e is Ambient {
        lemma_expr_leaf(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, e, t);
    } else if e is Density {
        lemma_expr_density(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, e, t);
    } else if e is Call {
        lemma_expr_call(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, e, t);
    } else if e is ExternCall {
        lemma_expr_extern(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, e, t);
    } else if e is Binary {
        lemma_expr_binary(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, e, t);
    } else if e is Unary || e is Field || e is Cast {
        lemma_expr_one(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, e, t);
    } else if e is Index || e is MakeVec3 || e is MakePos3 {
        lemma_expr_many(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, e, t);
    } else if e is MakeRecord {
        lemma_expr_record(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, e, t);
    } else {
        lemma_expr_array(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, e, t);
    }
}

proof fn lemma_expr_density<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, rank: usize, ctx: Ctx, frame: SFrame, assigned: Seq<bool>, e: Expr, t: Type)
    requires
        env_ok(env, ctys, ext_ok),
        e is Density,
        expr_pre(env.p, env.ranks, ctys, locals, rank, frame, assigned, e, t),
    ensures
        result_ok(ext_ok, env.p, spec_eval_expr(env, rank, ctx, frame, e), t),
    decreases rank, 0int, e, 0int, 1int,
{
    match e {
        Expr::Density { input, index } => lemma_density(env, ctys, ext_ok, locals, rank, ctx, frame, assigned, input, index, t),
        _ => {},
    }
}

// ---------------------------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------------------------

/// Flow never changes whether we're in a loop.
proof fn lemma_flow_stmts_in_loop(st: SFlowState, ss: Vec<Stmt>, i: int)
    ensures
        match spec_flow_stmts(st, ss, i) {
            Some(n) => n.in_loop == st.in_loop,
            None => true,
        },
    decreases ss, 1int, ss@.len() - i,
{
    if 0 <= i < ss@.len() {
        lemma_flow_stmt_in_loop(st, ss@[i]);
        match spec_flow_stmt(st, ss@[i]) {
            Some(next) => lemma_flow_stmts_in_loop(next, ss, i + 1),
            None => {},
        }
    }
}

proof fn lemma_flow_stmt_in_loop(st: SFlowState, s: Stmt)
    ensures
        match spec_flow_stmt(st, s) {
            Some(n) => n.in_loop == st.in_loop,
            None => true,
        },
    decreases s, 0int, 0int,
{
}

/// What a statement's result guarantees. `after` is the flow state after it.
pub open spec fn stmt_post(ext_ok: bool, p: Program, r: Result<(SFlow, SFrame), EvalError>, frame: SFrame, locals: Seq<Type>, ret: Type, in_loop: bool, after: SFlowState) -> bool {
    match r {
        Err(e) => allowed(ext_ok, e),
        Ok((fl, f2)) => {
            &&& frame_typed(p, f2, locals)
            &&& preserves(frame, f2)
            &&& match fl {
                SFlow::Next => after.reach && assigned_some(f2, after.assigned),
                SFlow::Break => in_loop,
                SFlow::Return(v) => spec_value_has_type(p, v, ret),
            }
        },
    }
}

pub open spec fn stmt_pre(p: Program, ranks: SRanks, ctys: Seq<Option<Type>>, locals: Seq<Type>, ret: Type, rank: usize, frame: SFrame, st: SFlowState, s: Stmt) -> bool {
    &&& st.reach
    &&& frame_typed(p, frame, locals)
    &&& assigned_some(frame, st.assigned)
    &&& spec_stmt_typed(p, locals, ctys, ret, s)
    &&& spec_stmt_ranked(ranks, rank, s)
    &&& spec_flow_stmt(st, s) is Some
}

pub open spec fn list_pre(p: Program, ranks: SRanks, ctys: Seq<Option<Type>>, locals: Seq<Type>, ret: Type, rank: usize, ss: Vec<Stmt>) -> bool {
    forall|k: int| #![trigger ss@[k]] 0 <= k < ss@.len() ==> spec_stmt_typed(p, locals, ctys, ret, ss@[k]) && spec_stmt_ranked(ranks, rank, ss@[k])
}

proof fn lemma_stmts<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, ret: Type, rank: usize, ctx: Ctx, frame: SFrame, st: SFlowState, ss: Vec<Stmt>, i: int)
    requires
        env_ok(env, ctys, ext_ok),
        0 <= i,
        st.reach,
        frame_typed(env.p, frame, locals),
        assigned_some(frame, st.assigned),
        list_pre(env.p, env.ranks, ctys, locals, ret, rank, ss),
        spec_flow_stmts(st, ss, i) is Some,
    ensures
        stmt_post(ext_ok, env.p, spec_stmts(env, rank, ctx, frame, ss, i), frame, locals, ret, st.in_loop, spec_flow_stmts(st, ss, i)->0),
    decreases rank, 1int, ss, 0int, ss@.len() - i,
{
    if i < ss@.len() {
        let s = ss@[i];
        assert(spec_stmt_typed(env.p, locals, ctys, ret, s) && spec_stmt_ranked(env.ranks, rank, s));
        lemma_flow_stmt_in_loop(st, s);
        let next = spec_flow_stmt(st, s)->0;
        lemma_stmt(env, ctys, ext_ok, locals, ret, rank, ctx, frame, st, s);
        match spec_eval_stmt(env, rank, ctx, frame, s) {
            Ok((SFlow::Next, f)) => {
                lemma_stmts(env, ctys, ext_ok, locals, ret, rank, ctx, f, next, ss, i + 1);
            },
            Ok((SFlow::Break, f)) => {},
            Ok((SFlow::Return(_), f)) => {},
            Err(_) => {},
        }
    }
}

proof fn lemma_while<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, ret: Type, rank: usize, ctx: Ctx, frame: SFrame, st: SFlowState, cond: Expr, body: Vec<Stmt>, iters: int)
    requires
        env_ok(env, ctys, ext_ok),
        st.reach,
        frame_typed(env.p, frame, locals),
        assigned_some(frame, st.assigned),
        expr_pre(env.p, env.ranks, ctys, locals, rank, frame, st.assigned, cond, Type::Bool),
        list_pre(env.p, env.ranks, ctys, locals, ret, rank, body),
        spec_flow_stmts(SFlowState { in_loop: true, ..st }, body, 0) is Some,
    ensures
        stmt_post(ext_ok, env.p, spec_while(env, rank, ctx, frame, cond, body, iters), frame, locals, ret, st.in_loop, st),
    decreases rank, 1int, body, env.fuel - iters + 1, 0int,
{
    lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, st.assigned, cond, Type::Bool);
    if 0 <= iters < env.fuel {
        let inner = SFlowState { in_loop: true, ..st };
        lemma_stmts(env, ctys, ext_ok, locals, ret, rank, ctx, frame, inner, body, 0);
        match spec_stmts(env, rank, ctx, frame, body, 0) {
            Ok((SFlow::Next, f)) => {
                assert(preserves(frame, f));
                assert forall|l: int| 0 <= l < st.assigned.len() && l < f.len() && #[trigger] st.assigned[l] implies f[l] is Some by {
                    assert(frame[l] is Some);
                }
                lemma_while(env, ctys, ext_ok, locals, ret, rank, ctx, f, st, cond, body, iters + 1);
            },
            Ok((_, f)) => {
                assert(preserves(frame, f));
                assert forall|l: int| 0 <= l < st.assigned.len() && l < f.len() && #[trigger] st.assigned[l] implies f[l] is Some by {
                    assert(frame[l] is Some);
                }
            },
            Err(_) => {},
        }
    }
}

proof fn lemma_repeat<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, ret: Type, rank: usize, ctx: Ctx, frame: SFrame, st: SFlowState, n: int, body: Vec<Stmt>, k: int)
    requires
        env_ok(env, ctys, ext_ok),
        st.reach,
        frame_typed(env.p, frame, locals),
        assigned_some(frame, st.assigned),
        list_pre(env.p, env.ranks, ctys, locals, ret, rank, body),
        spec_flow_stmts(SFlowState { in_loop: true, ..st }, body, 0) is Some,
    ensures
        stmt_post(ext_ok, env.p, spec_repeat(env, rank, ctx, frame, n, body, k), frame, locals, ret, st.in_loop, st),
    decreases rank, 1int, body, n - k + 1, 0int,
{
    if 0 <= k < n {
        let inner = SFlowState { in_loop: true, ..st };
        lemma_stmts(env, ctys, ext_ok, locals, ret, rank, ctx, frame, inner, body, 0);
        match spec_stmts(env, rank, ctx, frame, body, 0) {
            Ok((SFlow::Next, f)) => {
                assert(preserves(frame, f));
                assert forall|l: int| 0 <= l < st.assigned.len() && l < f.len() && #[trigger] st.assigned[l] implies f[l] is Some by {
                    assert(frame[l] is Some);
                }
                lemma_repeat(env, ctys, ext_ok, locals, ret, rank, ctx, f, st, n, body, k + 1);
            },
            Ok((_, f)) => {
                assert(preserves(frame, f));
                assert forall|l: int| 0 <= l < st.assigned.len() && l < f.len() && #[trigger] st.assigned[l] implies f[l] is Some by {
                    assert(frame[l] is Some);
                }
            },
            Err(_) => {},
        }
    }
}

proof fn lemma_stmt<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, locals: Seq<Type>, ret: Type, rank: usize, ctx: Ctx, frame: SFrame, st: SFlowState, s: Stmt)
    requires
        env_ok(env, ctys, ext_ok),
        stmt_pre(env.p, env.ranks, ctys, locals, ret, rank, frame, st, s),
    ensures
        stmt_post(ext_ok, env.p, spec_eval_stmt(env, rank, ctx, frame, s), frame, locals, ret, st.in_loop, spec_flow_stmt(st, s)->0),
    decreases rank, 1int, s, 0int, 0int,
{
    match s {
        Stmt::Assign { target, value } => {
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, st.assigned, value, locals[target.0 as int]);
            match spec_eval_expr(env, rank, ctx, frame, value) {
                Ok(v) => {
                    let f2 = frame.update(target.0 as int, Some(v));
                    let after = spec_flow_stmt(st, s)->0;
                    assert forall|l: int| 0 <= l < f2.len() && (#[trigger] f2[l]) is Some implies spec_value_has_type(env.p, f2[l]->0, locals[l]) by {
                        if l != target.0 as int {
                            assert(frame[l] is Some);
                        }
                    }
                    assert forall|l: int| 0 <= l < f2.len() && (#[trigger] frame[l]) is Some implies f2[l] is Some by {}
                    assert forall|l: int| 0 <= l < after.assigned.len() && l < f2.len() && #[trigger] after.assigned[l] implies f2[l] is Some by {
                        if l != target.0 as int {
                            assert(st.assigned[l]);
                            assert(frame[l] is Some);
                        }
                    }
                },
                Err(_) => {},
            }
        },
        Stmt::Return(e) => {
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, st.assigned, e, ret);
        },
        Stmt::If { cond, then_branch, else_branch } => {
            lemma_expr(env, ctys, ext_ok, locals, rank, ctx, frame, st.assigned, cond, Type::Bool);
            let t = spec_flow_stmts(st, then_branch, 0)->0;
            let e = spec_flow_stmts(st, else_branch, 0)->0;
            lemma_stmts(env, ctys, ext_ok, locals, ret, rank, ctx, frame, st, then_branch, 0);
            lemma_stmts(env, ctys, ext_ok, locals, ret, rank, ctx, frame, st, else_branch, 0);
            let after = spec_flow_merge(st, t, e);
            assert(spec_flow_stmt(st, s)->0 == after);
            // The join: what either branch's assigned set guarantees covers the merged set.
            assert forall|f: SFrame| #![trigger assigned_some(f, after.assigned)] t.reach && e.reach && (assigned_some(f, t.assigned) || assigned_some(f, e.assigned)) implies assigned_some(f, after.assigned) by {
                assert forall|l: int| 0 <= l < after.assigned.len() && l < f.len() && #[trigger] after.assigned[l] implies f[l] is Some by {
                    assert(t.assigned[l] && l < e.assigned.len() && e.assigned[l]);
                    if assigned_some(f, t.assigned) {
                        assert(0 <= l < t.assigned.len());
                    } else {
                        assert(0 <= l < e.assigned.len());
                    }
                }
            }
            match spec_eval_expr(env, rank, ctx, frame, cond) {
                Ok(v) => {
                    match spec_cond(v) {
                        Ok(true) => {
                            match spec_stmts(env, rank, ctx, frame, then_branch, 0) {
                                Ok((SFlow::Next, f)) => { assert(assigned_some(f, after.assigned)); },
                                _ => {},
                            }
                        },
                        Ok(false) => {
                            match spec_stmts(env, rank, ctx, frame, else_branch, 0) {
                                Ok((SFlow::Next, f)) => { assert(assigned_some(f, after.assigned)); },
                                _ => {},
                            }
                        },
                        Err(_) => {},
                    }
                },
                Err(_) => {},
            }
        },
        Stmt::While { cond, body } => {
            lemma_while(env, ctys, ext_ok, locals, ret, rank, ctx, frame, st, cond, body, 0);
        },
        Stmt::Repeat { count, body } => {
            lemma_repeat(env, ctys, ext_ok, locals, ret, rank, ctx, frame, st, count as int, body, 0);
        },
        Stmt::Break => {},
    }
}

// ---------------------------------------------------------------------------------------------
// Calls, consts, inputs, units
// ---------------------------------------------------------------------------------------------

proof fn lemma_call<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, rank: usize, g: FnId, args: Seq<SValue>, ctx: Ctx)
    requires
        env_ok(env, ctys, ext_ok),
        g.0 < env.p.functions@.len(),
        spec_rank(env.ranks.fns, g.0) == Some(rank),
        args.len() == env.p.functions@[g.0 as int].params,
        forall|k: int| 0 <= k < args.len() ==> spec_value_has_type(env.p, #[trigger] args[k], env.p.functions@[g.0 as int].locals@[k]),
    ensures
        result_ok(ext_ok, env.p, spec_eval_call(env, rank, g, args, ctx), env.p.functions@[g.0 as int].ret),
    decreases rank, 2int, env.p, 0int, 0int,
{
    let f = env.p.functions@[g.0 as int];
    env_fn(env, ctys, ext_ok, g.0 as int);
    let locals = f.locals@;
    let frame = spec_init_frame(args, locals.len());
    let st = spec_flow_start(f);
    assert forall|l: int| 0 <= l < frame.len() && (#[trigger] frame[l]) is Some implies spec_value_has_type(env.p, frame[l]->0, locals[l]) by {
        assert(frame[l] == Some(args[l]));
    }
    assert forall|l: int| 0 <= l < st.assigned.len() && l < frame.len() && #[trigger] st.assigned[l] implies frame[l] is Some by {
        assert(l < f.params);
    }
    assert(list_pre(env.p, env.ranks, ctys, locals, f.ret, rank, f.body)) by {
        assert forall|k: int| #![trigger f.body@[k]] 0 <= k < f.body@.len() implies spec_stmt_typed(env.p, locals, ctys, f.ret, f.body@[k]) && spec_stmt_ranked(env.ranks, rank, f.body@[k]) by {
            assert(spec_stmt_typed(env.p, f.locals@, ctys, f.ret, f.body@[k]));
            assert(spec_stmt_ranked(env.ranks, env.ranks.fns[g.0 as int]->0, env.p.functions@[g.0 as int].body@[k]));
        }
    }
    lemma_stmts(env, ctys, ext_ok, locals, f.ret, rank, ctx, frame, st, f.body, 0);
}

proof fn lemma_const<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, rank: usize, c: ConstId)
    requires
        env_ok(env, ctys, ext_ok),
        c.0 < env.p.consts@.len(),
        spec_rank(env.ranks.consts, c.0) == Some(rank),
    ensures
        ctys[c.0 as int] is Some,
        result_ok(ext_ok, env.p, spec_eval_const(env, rank, c), ctys[c.0 as int]->0),
    decreases rank, 2int, env.p, 0int, 0int,
{
    env_const(env, ctys, ext_ok, c.0 as int);
    let init = env.p.consts@[c.0 as int].init;
    let frame: SFrame = Seq::empty();
    let none: Seq<Type> = Seq::empty();
    assert(frame_typed(env.p, frame, none));
    assert(assigned_some(frame, Seq::empty()));
    lemma_expr(env, ctys, ext_ok, none, rank, spec_const_ctx(), frame, Seq::empty(), init, ctys[c.0 as int]->0);
}

proof fn lemma_cell<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, rank: usize, u: UnitId, ctx: Ctx)
    requires
        env_ok(env, ctys, ext_ok),
        u.0 < env.p.units@.len(),
        spec_rank(env.ranks.units, u.0) == Some(rank),
    ensures
        result_ok(ext_ok, env.p, spec_eval_cell(env, rank, u, ctx), env.p.functions@[env.p.units@[u.0 as int].main.0 as int].ret),
    decreases rank, 3int, env.p, 0int, 0int,
{
    env_unit(env, ctys, ext_ok, u.0 as int);
    let unit = env.p.units@[u.0 as int];
    let rm = spec_rank(env.ranks.fns, unit.main.0)->0;
    assert(rm < rank);
    assert(Seq::<SValue>::empty().len() == 0);
    lemma_call(env, ctys, ext_ok, rm, unit.main, Seq::empty(), ctx);
}

proof fn lemma_unit_cell<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, rank: usize, u: UnitId, base: Ctx, k: int, n: usize)
    requires
        env_ok(env, ctys, ext_ok),
        u.0 < env.p.units@.len(),
        spec_rank(env.ranks.units, u.0) == Some(rank),
        spec_dims_ok(base.dims),
        spec_len(base.dims) == Some(n),
        0 <= k < n,
    ensures
        result_ok(ext_ok, env.p, spec_unit_cell(env, rank, u, base, k), env.p.functions@[env.p.units@[u.0 as int].main.0 as int].ret),
    decreases rank, 4int, env.p, 0int, 0int,
{
    lemma_flat_decode(k as usize, base.dims);
    let cell = spec_decode(k as usize, base.dims)->0;
    lemma_cell(env, ctys, ext_ok, rank, u, spec_cell_ctx(base, k as usize, cell));
}

proof fn lemma_input_error<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, rank: usize, i: InputId)
    requires
        env_ok(env, ctys, ext_ok),
        i.0 < env.p.inputs@.len(),
        spec_below(env.ranks.inputs, i.0, rank),
    ensures
        match spec_input_error(env, rank, i) {
            Some(e) => allowed(ext_ok, e),
            None => true,
        },
    decreases rank, 5int, env.p, 0int, 0int,
{
    let ri = spec_rank(env.ranks.inputs, i.0)->0;
    lemma_input(env, ctys, ext_ok, ri, i);
}

proof fn lemma_best<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, rank: usize, inputs: Seq<InputId>, j: int, best: Option<(usize, EvalError)>)
    requires
        env_ok(env, ctys, ext_ok),
        0 <= j,
        forall|k: int| #![trigger inputs[k]] 0 <= k < inputs.len() ==> spec_below(env.ranks.inputs, inputs[k].0, rank) && inputs[k].0 < env.p.inputs@.len(),
        match best {
            Some((_, e)) => allowed(ext_ok, e),
            None => true,
        },
    ensures
        match spec_best_input(env, rank, inputs, j, best) {
            Some((_, e)) => allowed(ext_ok, e),
            None => true,
        },
    decreases rank, 6int, env.p, 0int, inputs.len() - j,
{
    if j < inputs.len() {
        let i = inputs[j];
        assert(spec_below(env.ranks.inputs, i.0, rank) && i.0 < env.p.inputs@.len());
        lemma_input_error(env, ctys, ext_ok, rank, i);
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
        lemma_best(env, ctys, ext_ok, rank, inputs, j + 1, next);
    }
}

/// A checked grid has a layout.
proof fn lemma_dims_len(d: Dims)
    requires
        spec_dims_ok(d),
    ensures
        spec_len(d) is Some,
{
    assert(d.x * d.y <= d.x * d.y * d.z) by (nonlinear_arith)
        requires d.x >= 1, d.y >= 1, d.z >= 1;
    assert(d.x * d.y >= 1) by (nonlinear_arith)
        requires d.x >= 1, d.y >= 1;
}

pub proof fn lemma_unit<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, rank: usize, u: UnitId, base: Ctx)
    requires
        env_ok(env, ctys, ext_ok),
        u.0 < env.p.units@.len(),
        spec_rank(env.ranks.units, u.0) == Some(rank),
        spec_dims_ok(base.dims),
    ensures
        spec_len(base.dims) is Some,
        spec_eval_unit_ctx(env, rank, u, base).len() == spec_len(base.dims)->0,
        forall|k: int| 0 <= k < spec_eval_unit_ctx(env, rank, u, base).len() ==>
            result_ok(ext_ok, env.p, #[trigger] spec_eval_unit_ctx(env, rank, u, base)[k], env.p.functions@[env.p.units@[u.0 as int].main.0 as int].ret),
    decreases rank, 7int, env.p, 0int, 0int,
{
    lemma_dims_len(base.dims);
    let n = spec_len(base.dims)->0;
    env_unit(env, ctys, ext_ok, u.0 as int);
    let unit = env.p.units@[u.0 as int];
    let t = env.p.functions@[unit.main.0 as int].ret;
    assert forall|k: int| #![trigger unit.inputs@[k]] 0 <= k < unit.inputs@.len() implies spec_below(env.ranks.inputs, unit.inputs@[k].0, rank) && unit.inputs@[k].0 < env.p.inputs@.len() by {
        assert(env.p.units@[u.0 as int].inputs@[k] == unit.inputs@[k]);
    }
    lemma_best(env, ctys, ext_ok, rank, unit.inputs@, 0, None);
    let cells = spec_eval_unit_ctx(env, rank, u, base);
    assert forall|k: int| 0 <= k < cells.len() implies result_ok(ext_ok, env.p, #[trigger] cells[k], t) by {
        match spec_best_input(env, rank, unit.inputs@, 0, None) {
            Some(_) => {},
            None => { lemma_unit_cell(env, ctys, ext_ok, rank, u, base, k, n); },
        }
    }
}

proof fn lemma_input<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, rank: usize, i: InputId)
    requires
        env_ok(env, ctys, ext_ok),
        i.0 < env.p.inputs@.len(),
        spec_rank(env.ranks.inputs, i.0) == Some(rank),
    ensures
        match spec_build_input(env, rank, i) {
            Ok(buf) => match spec_density_type(env.p, i) {
                Some(t) => forall|j: int| 0 <= j < buf.len() ==> spec_value_has_type(env.p, #[trigger] buf[j], t),
                None => false,
            },
            Err((_, e)) => allowed(ext_ok, e),
        },
    decreases rank, 8int, env.p, 0int, 0int,
{
    env_input(env, ctys, ext_ok, i.0 as int);
    let d = env.p.inputs@[i.0 as int];
    env_unit(env, ctys, ext_ok, d.source.0 as int);
    let rs = spec_rank(env.ranks.units, d.source.0)->0;
    assert(rs < rank);
    let base = spec_base_ctx(env.origin, d.origin_scale, d.position_scale, d.dims);
    lemma_unit(env, ctys, ext_ok, rs, d.source, base);
    let t = env.p.functions@[env.p.units@[d.source.0 as int].main.0 as int].ret;
    let cells = spec_eval_unit_ctx(env, rs, d.source, base);
    assert(spec_density_type(env.p, i) == Some(t));
    assert forall|j: int| 0 <= j < cells.len() implies result_ok(ext_ok, env.p, #[trigger] cells[j], t) by {}
    lemma_buffer(env.p, cells, 0, Seq::empty(), t, ext_ok);
}

// ---------------------------------------------------------------------------------------------
// The program
// ---------------------------------------------------------------------------------------------

proof fn lemma_first_const<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, n: int)
    requires
        env_ok(env, ctys, ext_ok),
        n <= env.p.consts@.len(),
    ensures
        match spec_first_const_error(env, n) {
            Some(e) => allowed(ext_ok, e),
            None => true,
        },
    decreases n,
{
    if n > 0 {
        lemma_first_const(env, ctys, ext_ok, n - 1);
        assert(env.p.consts.len() == env.p.consts@.len());
        lemma_const_result(env, ctys, ext_ok, n - 1);
    }
}

proof fn lemma_const_result<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, c: int)
    requires
        env_ok(env, ctys, ext_ok),
        0 <= c < env.p.consts@.len(),
        env.p.consts.len() == env.p.consts@.len(),
    ensures
        match spec_const_result(env, c) {
            Err(e) => allowed(ext_ok, e),
            Ok(_) => true,
        },
{
    env_const(env, ctys, ext_ok, c);
    assert(env.p.consts@.len() <= usize::MAX);
    assert(c as usize as int == c);
    let rc = spec_rank(env.ranks.consts, c as usize)->0;
    lemma_const(env, ctys, ext_ok, rc, ConstId(c as usize));
}

proof fn lemma_const_failures<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, n: int)
    requires
        env_ok(env, ctys, ext_ok),
        n <= env.p.consts@.len(),
    ensures
        forall|j: int| 0 <= j < spec_const_failures(env, n).len() ==> failure_allowed(ext_ok, #[trigger] spec_const_failures(env, n)[j]),
    decreases n,
{
    if n > 0 {
        lemma_const_failures(env, ctys, ext_ok, n - 1);
        assert(env.p.consts.len() == env.p.consts@.len());
        lemma_const_result(env, ctys, ext_ok, n - 1);
        let before = spec_const_failures(env, n - 1);
        let all = spec_const_failures(env, n);
        assert forall|j: int| 0 <= j < all.len() implies failure_allowed(ext_ok, #[trigger] all[j]) by {
            if j < before.len() {
                assert(all[j] == before[j]);
                assert(failure_allowed(ext_ok, before[j]));
            }
        }
    }
}

pub open spec fn failure_allowed(ext_ok: bool, f: Failure) -> bool {
    match f {
        Failure::Const { error, .. } => allowed(ext_ok, error),
        Failure::Input { error, .. } => allowed(ext_ok, error),
    }
}

proof fn lemma_entry<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool, e: int)
    requires
        env_ok(env, ctys, ext_ok),
        0 <= e < env.p.entries@.len(),
    ensures
        ({
            let entry = env.p.entries@[e];
            let t = env.p.functions@[env.p.units@[entry.unit.0 as int].main.0 as int].ret;
            let cells = spec_entry_cells(env, e);
            &&& spec_len(entry.dims) is Some
            &&& cells.len() == spec_len(entry.dims)->0
            &&& forall|k: int| 0 <= k < cells.len() ==> result_ok(ext_ok, env.p, #[trigger] cells[k], t)
        }),
{
    reveal(env_ok);
    let entry = env.p.entries@[e];
    assert(wf::wf_entry(&env.p, &env.p.entries[e]));
    assert(spec_dims_ok(env.p.entries@[e].dims));
    lemma_dims_len(entry.dims);
    env_unit(env, ctys, ext_ok, entry.unit.0 as int);
    lemma_first_const(env, ctys, ext_ok, env.p.consts@.len() as int);
    let ru = spec_rank(env.ranks.units, entry.unit.0)->0;
    let base = spec_base_ctx(env.origin, entry.origin_scale, entry.position_scale, entry.dims);
    lemma_unit(env, ctys, ext_ok, ru, entry.unit, base);
    let t = env.p.functions@[env.p.units@[entry.unit.0 as int].main.0 as int].ret;
    let cells = spec_entry_cells(env, e);
    assert forall|k: int| 0 <= k < cells.len() implies result_ok(ext_ok, env.p, #[trigger] cells[k], t) by {
        match spec_first_const_error(env, env.p.consts@.len() as int) {
            Some(_) => {},
            None => {
                assert(cells == spec_eval_unit_ctx(env, ru, entry.unit, base));
            },
        }
    }
}

/// An environment built for a whole checked case satisfies `env_ok`.
pub proof fn lemma_env_ok<E: Externs>(p: Program, ranks: SRanks, origin: F64x3, hosts: Seq<SValue>, ext: E, fuel: u64, ctys: Seq<Option<Type>>, ext_ok: bool)
    requires
        spec_well_typed_with(p, ctys),
        ranks == spec_ranks(p),
        spec_case_fits(p, hosts),
        ext_ok ==> extern_respects(p, ext),
    ensures
        env_ok(SEnv { p, ranks, origin, hosts, ext, fuel }, ctys, ext_ok),
{
    reveal(env_ok);
}

/// **T4 (and, with `ext_ok`, its corollary).** For a well-typed program and a fitting case, every entry
/// has `len(dims)` cells; every `Ok` has its unit's return type; every error is a real outcome, or
/// (with `ext_ok == false`) one of the two `Stuck`s only a misbehaving extern can cause. Failing
/// consts and failing inputs obey the same bound on their errors.
pub proof fn theorem_soundness<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool)
    requires
        spec_well_typed_with(env.p, ctys),
        env.ranks == spec_ranks(env.p),
        spec_case_fits(env.p, env.hosts),
        ext_ok ==> extern_respects(env.p, env.ext),
    ensures
        forall|e: int| 0 <= e < env.p.entries@.len() ==> {
            let entry = (#[trigger] env.p.entries@[e]);
            let cells = spec_eval_program(env)[e];
            &&& spec_len(entry.dims) is Some
            &&& cells.len() == spec_len(entry.dims)->0
            &&& forall|k: int| 0 <= k < cells.len() ==> result_ok(ext_ok, env.p, #[trigger] cells[k], env.p.functions@[env.p.units@[entry.unit.0 as int].main.0 as int].ret)
        },
        forall|j: int| 0 <= j < spec_const_failures(env, env.p.consts@.len() as int).len()
            ==> failure_allowed(ext_ok, #[trigger] spec_const_failures(env, env.p.consts@.len() as int)[j]),
        forall|f: Failure| #[trigger] input_failure_ok(env, f) ==> failure_allowed(ext_ok, f),
{
    assert(env_ok(env, ctys, ext_ok)) by {
        lemma_env_ok(env.p, env.ranks, env.origin, env.hosts, env.ext, env.fuel, ctys, ext_ok);
        assert(env == SEnv { p: env.p, ranks: env.ranks, origin: env.origin, hosts: env.hosts, ext: env.ext, fuel: env.fuel });
    }
    assert forall|e: int| 0 <= e < env.p.entries@.len() implies {
        let entry = (#[trigger] env.p.entries@[e]);
        let cells = spec_eval_program(env)[e];
        &&& spec_len(entry.dims) is Some
        &&& cells.len() == spec_len(entry.dims)->0
        &&& forall|k: int| 0 <= k < cells.len() ==> result_ok(ext_ok, env.p, #[trigger] cells[k], env.p.functions@[env.p.units@[entry.unit.0 as int].main.0 as int].ret)
    } by {
        lemma_entry(env, ctys, ext_ok, e);
    }
    lemma_const_failures(env, ctys, ext_ok, env.p.consts@.len() as int);
    assert forall|f: Failure| #[trigger] input_failure_ok(env, f) implies failure_allowed(ext_ok, f) by {
        match f {
            Failure::Input { id, cell, error } => {
                env_rank_len(env, ctys, ext_ok);
                let r = spec_rank(env.ranks.inputs, id.0)->0;
                assert(id.0 < env.p.inputs@.len());
                lemma_input(env, ctys, ext_ok, r, id);
            },
            Failure::Const { .. } => {},
        }
    }
}

/// **The whole chain**, for one run of the interpreter: if `check` and `check_case` accepted the
/// program and the case (which give the first two requirements), then what `eval_program` returned
/// obeys T4. `ctys` is the witness `check` proves exists.
pub proof fn theorem_end_to_end<E: Externs>(p: Program, case: Case, ext: E, fuel: u64, out: CaseResult, ctys: Seq<Option<Type>>, ext_ok: bool)
    requires
        spec_well_typed_with(p, ctys),
        spec_case_fits(p, view_values(case.hosts@)),
        ext_ok ==> extern_respects(p, ext),
        view_entries(out.entries@) == spec_eval_program(program_env(p, case, ext, fuel)),
        diagnostics_sound(out.failures@, program_env(p, case, ext, fuel)),
    ensures
        out.entries@.len() == p.entries@.len(),
        forall|e: int, k: int| 0 <= e < p.entries@.len() && 0 <= k < out.entries@[e]@.len()
            ==> result_ok(ext_ok, p, #[trigger] view_results(out.entries@[e]@)[k], p.functions@[p.units@[p.entries@[e].unit.0 as int].main.0 as int].ret),
        forall|e: int| 0 <= e < p.entries@.len() ==> (#[trigger] out.entries@[e])@.len() == spec_len(p.entries@[e].dims)->0,
        forall|j: int| 0 <= j < out.failures@.len() ==> failure_allowed(ext_ok, #[trigger] out.failures@[j]),
{
    let env = program_env(p, case, ext, fuel);
    theorem_soundness(env, ctys, ext_ok);
    let es = view_entries(out.entries@);
    assert(es.len() == out.entries@.len());
    assert forall|e: int, k: int| 0 <= e < p.entries@.len() && 0 <= k < out.entries@[e]@.len()
        implies result_ok(ext_ok, p, #[trigger] view_results(out.entries@[e]@)[k], p.functions@[p.units@[p.entries@[e].unit.0 as int].main.0 as int].ret) by {
        assert(es[e] == view_results(out.entries@[e]@));
        assert(spec_eval_program(env)[e] == es[e]);
        assert(p.entries@[e] == p.entries@[e]);
    }
    assert forall|e: int| 0 <= e < p.entries@.len() implies (#[trigger] out.entries@[e])@.len() == spec_len(p.entries@[e].dims)->0 by {
        assert(es[e] == view_results(out.entries@[e]@));
        assert(spec_eval_program(env)[e] == es[e]);
        assert(p.entries@[e] == p.entries@[e]);
    }
    let cs = spec_const_failures(env, p.consts@.len() as int);
    assert forall|j: int| 0 <= j < out.failures@.len() implies failure_allowed(ext_ok, #[trigger] out.failures@[j]) by {
        if j < cs.len() {
            assert(out.failures@[j] == cs[j]);
            assert(failure_allowed(ext_ok, cs[j]));
        } else {
            assert(input_failure_ok(env, out.failures@[j]));
        }
    }
}

/// The same, stated with what `check` proves directly: `spec_well_typed` hides the witness.
pub proof fn theorem_checked_run<E: Externs>(p: Program, case: Case, ext: E, fuel: u64, out: CaseResult, ext_ok: bool)
    requires
        spec_well_typed(p),
        spec_case_fits(p, view_values(case.hosts@)),
        ext_ok ==> extern_respects(p, ext),
        view_entries(out.entries@) == spec_eval_program(program_env(p, case, ext, fuel)),
        diagnostics_sound(out.failures@, program_env(p, case, ext, fuel)),
    ensures
        out.entries@.len() == p.entries@.len(),
        forall|e: int, k: int| 0 <= e < p.entries@.len() && 0 <= k < out.entries@[e]@.len()
            ==> result_ok(ext_ok, p, #[trigger] view_results(out.entries@[e]@)[k], p.functions@[p.units@[p.entries@[e].unit.0 as int].main.0 as int].ret),
        forall|e: int| 0 <= e < p.entries@.len() ==> (#[trigger] out.entries@[e])@.len() == spec_len(p.entries@[e].dims)->0,
        forall|j: int| 0 <= j < out.failures@.len() ==> failure_allowed(ext_ok, #[trigger] out.failures@[j]),
{
    let ctys = choose|ctys: Seq<Option<Type>>| spec_well_typed_with(p, ctys);
    theorem_end_to_end(p, case, ext, fuel, out, ctys, ext_ok);
}

} // verus!
