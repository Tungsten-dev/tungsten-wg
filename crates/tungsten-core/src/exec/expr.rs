//! Expressions (§7.2, §7.4, §7.5). Left to right: call and extern arguments, binary operands
//! (`And` / `Or` short-circuit; everything else is eager, `BitAnd` / `BitOr` on `Bool` too),
//! `Make*` and array-literal elements in written order, `MakeRecord` fields in written order.
//! The first error ends the cell.

use vstd::prelude::*;

use crate::exec::*;
use crate::exec::stmt::eval_call;
use crate::exec::unit::*;
use crate::ir::*;
use crate::layout;
use crate::prim;
use crate::util::*;
use crate::value::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

fn stuck(r: StuckReason) -> EvalError {
    EvalError::Stuck(r)
}

fn shape() -> EvalError {
    EvalError::Stuck(StuckReason::Shape)
}

pub fn lit_value(l: &Lit) -> Value {
    match l {
        Lit::Bool(b) => Value::Bool(*b),
        Lit::I8(x) => Value::I8(*x),
        Lit::I32(x) => Value::I32(*x),
        Lit::I64(x) => Value::I64(*x),
        Lit::F32(x) => Value::F32(*x),
        Lit::F64(x) => Value::F64(*x),
    }
}

#[verifier::loop_isolation(false)]
pub fn eval_args<E: Externs>(env: &Env<E>, rank: usize, ctx: &Ctx, frame: &mut Frame, args: &Vec<Expr>, memo: &mut Memo) -> Result<Vec<Value>, EvalError>
    decreases rank, 0int, args,
{
    let mut out: Vec<Value> = Vec::new();
    let mut i: usize = 0;
    while i < args.len()
        invariant
            i <= args.len(),
        decreases args.len() - i,
    {
        let v = eval_expr(env, rank, ctx, frame, &args[i], memo)?;
        out.push(v);
        i = i + 1;
    }
    Ok(out)
}

fn read_buf(buf: &Vec<Value>, k: usize) -> Result<Value, EvalError> {
    match get(buf, k) {
        Some(v) => Ok(clone_value(v)),
        None => Err(EvalError::OutOfBounds),
    }
}

/// `Density(i, None)` reads `buf[0]` for a `(1,1,1)` input and `buf[ctx.k]` otherwise.
/// `Density(i, Some(e))` reads `buf[e]`. The index is evaluated before the buffer is built.
pub fn read_density<E: Externs>(env: &Env<E>, rank: usize, ctx: &Ctx, frame: &mut Frame, input: InputId, index: &Option<Box<Expr>>, memo: &mut Memo) -> Result<Value, EvalError>
    decreases rank, 0int, index,
{
    let explicit: Option<i32> = match index {
        Some(ix) => match eval_expr(env, rank, ctx, frame, ix, memo)? {
            Value::I32(i) => Some(i),
            _ => { return Err(shape()); },
        },
        None => None,
    };
    let d = match get(&env.p.inputs, input.0) {
        Some(d) => d,
        None => { return Err(stuck(StuckReason::BadIndex)); },
    };
    let k: usize = match explicit {
        Some(i) => {
            if i < 0 {
                return Err(EvalError::OutOfBounds);
            }
            i as usize
        },
        None => if layout::is_unit(&d.dims) { 0 } else { ctx.k },
    };
    let ri = rank_or(get_rank(&env.ranks.inputs, input.0), rank);
    if ri >= rank {
        return Err(stuck(StuckReason::Rank));
    }
    match build_input(env, ri, input, memo) {
        InputBuild::Fresh(Ok(buf)) => read_buf(&buf, k),
        InputBuild::Fresh(Err((_, e))) => Err(e),
        InputBuild::Memoized => match get(&memo.inputs, input.0) {
            Some(Some(Ok(buf))) => read_buf(buf, k),
            Some(Some(Err((_, e)))) => Err(*e),
            _ => Err(stuck(StuckReason::BadIndex)),
        },
    }
}

#[verifier::loop_isolation(false)]
pub fn eval_expr<E: Externs>(env: &Env<E>, rank: usize, ctx: &Ctx, frame: &mut Frame, e: &Expr, memo: &mut Memo) -> Result<Value, EvalError>
    decreases rank, 0int, e,
{
    let p = env.p;
    match e {
        Expr::Lit(l) => Ok(lit_value(l)),
        Expr::Local(l) => match get(&frame.locals, l.0) {
            Some(Some(v)) => Ok(clone_value(v)),
            Some(None) => Err(stuck(StuckReason::Unassigned)),
            None => Err(stuck(StuckReason::BadIndex)),
        },
        Expr::Const(c) => {
            let rc = rank_or(get_rank(&env.ranks.consts, c.0), rank);
            if rc < rank {
                eval_const(env, rc, *c, memo)
            } else {
                Err(stuck(StuckReason::Rank))
            }
        },
        Expr::Host(h) => match get(&env.case.hosts, h.0) {
            Some(v) => Ok(clone_value(v)),
            None => Err(stuck(StuckReason::Case)),
        },
        Expr::Table(t) => match get(&p.tables, t.0) {
            Some(d) => match d.kind {
                TableKind::Perm => Ok(Value::PermTable(*t)),
                TableKind::Sampler => Ok(Value::Sampler(*t)),
            },
            None => Err(stuck(StuckReason::BadIndex)),
        },
        Expr::Ambient(a) => Ok(match a {
            Ambient::Origin => Value::Vec3(copy_f64x3(&ctx.origin)),
            Ambient::OriginScale => Value::Vec3(copy_f64x3(&ctx.origin_scale)),
            Ambient::PositionScale => Value::Vec3(copy_f64x3(&ctx.position_scale)),
            Ambient::Pos3 => Value::Pos3(copy_i32x3(&ctx.cell)),
        }),
        Expr::Density { input, index } => read_density(env, rank, ctx, frame, *input, index, memo),
        Expr::Call { func, args } => {
            let vals = eval_args(env, rank, ctx, frame, args, memo)?;
            let rg = rank_or(get_rank(&env.ranks.fns, func.0), rank);
            if rg < rank {
                eval_call(env, rg, *func, vals, ctx, memo)
            } else {
                Err(stuck(StuckReason::Rank))
            }
        },
        Expr::ExternCall { ext, args, .. } => {
            let vals = eval_args(env, rank, ctx, frame, args, memo)?;
            let decl = match get(&p.externs, ext.0) {
                Some(d) => d,
                None => { return Err(stuck(StuckReason::BadIndex)); },
            };
            match env.ext.call(*ext, vals.as_slice()) {
                Ok(v) => {
                    if value_has_type(p, &v, &decl.ret) {
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
        },
        Expr::Binary { op, left, right } => {
            match op {
                BinOp::And => {
                    let l = cond(eval_expr(env, rank, ctx, frame, left, memo)?)?;
                    if !l {
                        return Ok(Value::Bool(false));
                    }
                    let r = cond(eval_expr(env, rank, ctx, frame, right, memo)?)?;
                    return Ok(Value::Bool(r));
                },
                BinOp::Or => {
                    let l = cond(eval_expr(env, rank, ctx, frame, left, memo)?)?;
                    if l {
                        return Ok(Value::Bool(true));
                    }
                    let r = cond(eval_expr(env, rank, ctx, frame, right, memo)?)?;
                    return Ok(Value::Bool(r));
                },
                _ => {},
            }
            let l = eval_expr(env, rank, ctx, frame, left, memo)?;
            let r = eval_expr(env, rank, ctx, frame, right, memo)?;
            binop(op, &l, &r)
        },
        Expr::Unary { op, operand } => {
            let v = eval_expr(env, rank, ctx, frame, operand, memo)?;
            unop(op, &v)
        },
        Expr::Field { base, sel, .. } => {
            let b = eval_expr(env, rank, ctx, frame, base, memo)?;
            field(sel, &b)
        },
        Expr::Index { array, index } => {
            let a = eval_expr(env, rank, ctx, frame, array, memo)?;
            let i = eval_expr(env, rank, ctx, frame, index, memo)?;
            match (a, i) {
                (Value::Array(vs), Value::I32(i)) => {
                    if i < 0 {
                        return Err(EvalError::OutOfBounds);
                    }
                    read_buf(&vs, i as usize)
                },
                _ => Err(shape()),
            }
        },
        Expr::MakeVec3 { x, y, z } => {
            let a = eval_expr(env, rank, ctx, frame, x, memo)?;
            let b = eval_expr(env, rank, ctx, frame, y, memo)?;
            let c = eval_expr(env, rank, ctx, frame, z, memo)?;
            match (a, b, c) {
                (Value::F64(a), Value::F64(b), Value::F64(c)) => Ok(Value::Vec3(F64x3 { x: a, y: b, z: c })),
                _ => Err(shape()),
            }
        },
        Expr::MakePos3 { x, y, z } => {
            let a = eval_expr(env, rank, ctx, frame, x, memo)?;
            let b = eval_expr(env, rank, ctx, frame, y, memo)?;
            let c = eval_expr(env, rank, ctx, frame, z, memo)?;
            match (a, b, c) {
                (Value::I32(a), Value::I32(b), Value::I32(c)) => Ok(Value::Pos3(I32x3 { x: a, y: b, z: c })),
                _ => Err(shape()),
            }
        },
        Expr::MakeRecord { record, fields } => {
            let decl = match get(&p.records, record.0) {
                Some(d) => d,
                None => { return Err(stuck(StuckReason::BadIndex)); },
            };
            let n = decl.fields.len();
            let mut slots: Vec<Option<Value>> = Vec::new();
            let mut k: usize = 0;
            while k < n
                invariant
                    k <= n,
                decreases n - k,
            {
                slots.push(None);
                k = k + 1;
            }
            let mut i: usize = 0;
            while i < fields.len()
                invariant
                    i <= fields.len(),
                decreases fields.len() - i,
            {
                let v = eval_expr(env, rank, ctx, frame, &fields[i].value, memo)?;
                let f = fields[i].field;
                match get(&slots, f) {
                    Some(None) => {},
                    Some(Some(_)) => { return Err(shape()); },
                    None => { return Err(stuck(StuckReason::BadIndex)); },
                }
                set(&mut slots, f, Some(v));
                i = i + 1;
            }
            let mut vals: Vec<Value> = Vec::new();
            let mut k: usize = 0;
            while k < slots.len()
                invariant
                    k <= slots.len(),
                decreases slots.len() - k,
            {
                match &slots[k] {
                    Some(v) => vals.push(clone_value(v)),
                    None => { return Err(shape()); },
                }
                k = k + 1;
            }
            Ok(Value::Record(*record, vals))
        },
        Expr::ArrayLit(es) => {
            let mut vals: Vec<Value> = Vec::new();
            let mut i: usize = 0;
            while i < es.len()
                invariant
                    i <= es.len(),
                decreases es.len() - i,
            {
                let v = eval_expr(env, rank, ctx, frame, &es[i], memo)?;
                vals.push(v);
                i = i + 1;
            }
            Ok(Value::Array(vals))
        },
        Expr::Cast { to, operand } => {
            let v = eval_expr(env, rank, ctx, frame, operand, memo)?;
            cast(to, &v)
        },
    }
}

fn cond(v: Value) -> Result<bool, EvalError> {
    match v {
        Value::Bool(b) => Ok(b),
        _ => Err(shape()),
    }
}

fn axis_f64(a: &Axis, v: &F64x3) -> u64 {
    match a {
        Axis::X => v.x,
        Axis::Y => v.y,
        Axis::Z => v.z,
    }
}

fn axis_i32(a: &Axis, v: &I32x3) -> i32 {
    match a {
        Axis::X => v.x,
        Axis::Y => v.y,
        Axis::Z => v.z,
    }
}

pub fn field(sel: &FieldSel, b: &Value) -> Result<Value, EvalError> {
    match (sel, b) {
        (FieldSel::Vec3(a), Value::Vec3(v)) => Ok(Value::F64(axis_f64(a, v))),
        (FieldSel::Pos3(a), Value::Pos3(v)) => Ok(Value::I32(axis_i32(a, v))),
        (FieldSel::Record(r, i), Value::Record(s, vs)) => {
            if r.0 != s.0 {
                return Err(shape());
            }
            match get(vs, *i) {
                Some(v) => Ok(clone_value(v)),
                None => Err(stuck(StuckReason::BadIndex)),
            }
        },
        _ => Err(shape()),
    }
}

// ---------------------------------------------------------------------------------------------
// Operators
// ---------------------------------------------------------------------------------------------

fn i32_op(op: &BinOp, a: i32, b: i32) -> Result<Value, EvalError> {
    match op {
        BinOp::Add => Ok(Value::I32(prim::i32_add(a, b))),
        BinOp::Subtract => Ok(Value::I32(prim::i32_sub(a, b))),
        BinOp::Multiply => Ok(Value::I32(prim::i32_mul(a, b))),
        BinOp::Divide => if b == 0 { Err(EvalError::DivByZero) } else { Ok(Value::I32(prim::i32_div(a, b))) },
        BinOp::Equal => Ok(Value::Bool(a == b)),
        BinOp::NotEqual => Ok(Value::Bool(a != b)),
        BinOp::Less => Ok(Value::Bool(a < b)),
        BinOp::LessEqual => Ok(Value::Bool(a <= b)),
        BinOp::Greater => Ok(Value::Bool(a > b)),
        BinOp::GreaterEqual => Ok(Value::Bool(a >= b)),
        BinOp::BitAnd => Ok(Value::I32(prim::i32_and(a, b))),
        BinOp::BitOr => Ok(Value::I32(prim::i32_or(a, b))),
        BinOp::BitXor => Ok(Value::I32(prim::i32_xor(a, b))),
        BinOp::ShiftLeft => Ok(Value::I32(prim::i32_shl(a, b))),
        BinOp::ShiftRight => Ok(Value::I32(prim::i32_shr(a, b))),
        BinOp::LogicalShiftRight => Ok(Value::I32(prim::i32_ushr(a, b))),
        BinOp::And | BinOp::Or => Err(shape()),
    }
}

fn i64_op(op: &BinOp, a: i64, b: i64) -> Result<Value, EvalError> {
    match op {
        BinOp::Add => Ok(Value::I64(prim::i64_add(a, b))),
        BinOp::Subtract => Ok(Value::I64(prim::i64_sub(a, b))),
        BinOp::Multiply => Ok(Value::I64(prim::i64_mul(a, b))),
        BinOp::Divide => if b == 0 { Err(EvalError::DivByZero) } else { Ok(Value::I64(prim::i64_div(a, b))) },
        BinOp::Equal => Ok(Value::Bool(a == b)),
        BinOp::NotEqual => Ok(Value::Bool(a != b)),
        BinOp::Less => Ok(Value::Bool(a < b)),
        BinOp::LessEqual => Ok(Value::Bool(a <= b)),
        BinOp::Greater => Ok(Value::Bool(a > b)),
        BinOp::GreaterEqual => Ok(Value::Bool(a >= b)),
        BinOp::BitAnd => Ok(Value::I64(prim::i64_and(a, b))),
        BinOp::BitOr => Ok(Value::I64(prim::i64_or(a, b))),
        BinOp::BitXor => Ok(Value::I64(prim::i64_xor(a, b))),
        // Shifts take an `I32` right operand; see `shift`.
        _ => Err(shape()),
    }
}

fn i8_op(op: &BinOp, a: i8, b: i8) -> Result<Value, EvalError> {
    match op {
        BinOp::Add => Ok(Value::I8(prim::i8_add(a, b))),
        BinOp::Subtract => Ok(Value::I8(prim::i8_sub(a, b))),
        BinOp::Multiply => Ok(Value::I8(prim::i8_mul(a, b))),
        BinOp::Divide => if b == 0 { Err(EvalError::DivByZero) } else { Ok(Value::I8(prim::i8_div(a, b))) },
        BinOp::Equal => Ok(Value::Bool(a == b)),
        BinOp::NotEqual => Ok(Value::Bool(a != b)),
        BinOp::Less => Ok(Value::Bool(a < b)),
        BinOp::LessEqual => Ok(Value::Bool(a <= b)),
        BinOp::Greater => Ok(Value::Bool(a > b)),
        BinOp::GreaterEqual => Ok(Value::Bool(a >= b)),
        BinOp::BitAnd => Ok(Value::I8(prim::i8_and(a, b))),
        BinOp::BitOr => Ok(Value::I8(prim::i8_or(a, b))),
        BinOp::BitXor => Ok(Value::I8(prim::i8_xor(a, b))),
        _ => Err(shape()),
    }
}

fn is_shift(op: &BinOp) -> bool {
    match op {
        BinOp::ShiftLeft | BinOp::ShiftRight | BinOp::LogicalShiftRight => true,
        _ => false,
    }
}

/// An integer left operand shifted by an `I32`.
fn shift(op: &BinOp, l: &Value, s: i32) -> Result<Value, EvalError> {
    match (op, l) {
        (BinOp::ShiftLeft, Value::I8(a)) => Ok(Value::I8(prim::i8_shl(*a, s))),
        (BinOp::ShiftRight, Value::I8(a)) => Ok(Value::I8(prim::i8_shr(*a, s))),
        (BinOp::LogicalShiftRight, Value::I8(a)) => Ok(Value::I8(prim::i8_ushr(*a, s))),
        (BinOp::ShiftLeft, Value::I32(a)) => Ok(Value::I32(prim::i32_shl(*a, s))),
        (BinOp::ShiftRight, Value::I32(a)) => Ok(Value::I32(prim::i32_shr(*a, s))),
        (BinOp::LogicalShiftRight, Value::I32(a)) => Ok(Value::I32(prim::i32_ushr(*a, s))),
        (BinOp::ShiftLeft, Value::I64(a)) => Ok(Value::I64(prim::i64_shl(*a, s))),
        (BinOp::ShiftRight, Value::I64(a)) => Ok(Value::I64(prim::i64_shr(*a, s))),
        (BinOp::LogicalShiftRight, Value::I64(a)) => Ok(Value::I64(prim::i64_ushr(*a, s))),
        _ => Err(shape()),
    }
}

fn f32_op(op: &BinOp, a: u32, b: u32) -> Result<Value, EvalError> {
    match op {
        BinOp::Add => Ok(Value::F32(prim::f32_add(a, b))),
        BinOp::Subtract => Ok(Value::F32(prim::f32_sub(a, b))),
        BinOp::Multiply => Ok(Value::F32(prim::f32_mul(a, b))),
        BinOp::Divide => Ok(Value::F32(prim::f32_div(a, b))),
        BinOp::Equal => Ok(Value::Bool(prim::f32_eq(a, b))),
        BinOp::NotEqual => Ok(Value::Bool(!prim::f32_eq(a, b))),
        BinOp::Less => Ok(Value::Bool(prim::f32_lt(a, b))),
        BinOp::LessEqual => Ok(Value::Bool(prim::f32_le(a, b))),
        BinOp::Greater => Ok(Value::Bool(prim::f32_lt(b, a))),
        BinOp::GreaterEqual => Ok(Value::Bool(prim::f32_le(b, a))),
        _ => Err(shape()),
    }
}

fn f64_op(op: &BinOp, a: u64, b: u64) -> Result<Value, EvalError> {
    match op {
        BinOp::Add => Ok(Value::F64(prim::f64_add(a, b))),
        BinOp::Subtract => Ok(Value::F64(prim::f64_sub(a, b))),
        BinOp::Multiply => Ok(Value::F64(prim::f64_mul(a, b))),
        BinOp::Divide => Ok(Value::F64(prim::f64_div(a, b))),
        BinOp::Equal => Ok(Value::Bool(prim::f64_eq(a, b))),
        BinOp::NotEqual => Ok(Value::Bool(!prim::f64_eq(a, b))),
        BinOp::Less => Ok(Value::Bool(prim::f64_lt(a, b))),
        BinOp::LessEqual => Ok(Value::Bool(prim::f64_le(a, b))),
        BinOp::Greater => Ok(Value::Bool(prim::f64_lt(b, a))),
        BinOp::GreaterEqual => Ok(Value::Bool(prim::f64_le(b, a))),
        _ => Err(shape()),
    }
}

fn bool_op(op: &BinOp, a: bool, b: bool) -> Result<Value, EvalError> {
    match op {
        BinOp::Equal => Ok(Value::Bool(a == b)),
        BinOp::NotEqual => Ok(Value::Bool(a != b)),
        BinOp::BitAnd => Ok(Value::Bool(a && b)),
        BinOp::BitOr => Ok(Value::Bool(a || b)),
        BinOp::BitXor => Ok(Value::Bool(a != b)),
        _ => Err(shape()),
    }
}

/// `+ - * /` on one f64 component (vector ops).
fn f64_arith(op: &BinOp, a: u64, b: u64) -> Result<u64, EvalError> {
    match op {
        BinOp::Add => Ok(prim::f64_add(a, b)),
        BinOp::Subtract => Ok(prim::f64_sub(a, b)),
        BinOp::Multiply => Ok(prim::f64_mul(a, b)),
        BinOp::Divide => Ok(prim::f64_div(a, b)),
        _ => Err(shape()),
    }
}

fn i32_arith(op: &BinOp, a: i32, b: i32) -> Result<i32, EvalError> {
    match op {
        BinOp::Add => Ok(prim::i32_add(a, b)),
        BinOp::Subtract => Ok(prim::i32_sub(a, b)),
        BinOp::Multiply => Ok(prim::i32_mul(a, b)),
        BinOp::Divide => if b == 0 { Err(EvalError::DivByZero) } else { Ok(prim::i32_div(a, b)) },
        _ => Err(shape()),
    }
}

/// An `i32` component converted to f64 exactly.
fn i2d(x: i32) -> u64 {
    prim::i64_to_f64(prim::i32_to_i64(x))
}

fn vec3_op(op: &BinOp, a: &F64x3, b: &F64x3) -> Result<Value, EvalError> {
    let x = f64_arith(op, a.x, b.x)?;
    let y = f64_arith(op, a.y, b.y)?;
    let z = f64_arith(op, a.z, b.z)?;
    Ok(Value::Vec3(F64x3 { x, y, z }))
}

fn pos_to_vec(a: &I32x3) -> F64x3 {
    F64x3 { x: i2d(a.x), y: i2d(a.y), z: i2d(a.z) }
}

pub fn binop(op: &BinOp, l: &Value, r: &Value) -> Result<Value, EvalError> {
    if is_shift(op) {
        return match r {
            Value::I32(s) => shift(op, l, *s),
            _ => Err(shape()),
        };
    }
    match (l, r) {
        (Value::I8(a), Value::I8(b)) => i8_op(op, *a, *b),
        (Value::I32(a), Value::I32(b)) => i32_op(op, *a, *b),
        (Value::I64(a), Value::I64(b)) => i64_op(op, *a, *b),
        (Value::F32(a), Value::F32(b)) => f32_op(op, *a, *b),
        (Value::F64(a), Value::F64(b)) => f64_op(op, *a, *b),
        (Value::Bool(a), Value::Bool(b)) => bool_op(op, *a, *b),
        (Value::Vec3(a), Value::Vec3(b)) => vec3_op(op, a, b),
        (Value::Vec3(a), Value::Pos3(b)) => vec3_op(op, a, &pos_to_vec(b)),
        (Value::Pos3(a), Value::Vec3(b)) => vec3_op(op, &pos_to_vec(a), b),
        (Value::Pos3(a), Value::Pos3(b)) => {
            // Componentwise; for `/` the first zero divisor in x, y, z order is the error.
            let x = i32_arith(op, a.x, b.x)?;
            let y = i32_arith(op, a.y, b.y)?;
            let z = i32_arith(op, a.z, b.z)?;
            Ok(Value::Pos3(I32x3 { x, y, z }))
        },
        _ => Err(shape()),
    }
}

pub fn unop(op: &UnOp, v: &Value) -> Result<Value, EvalError> {
    match (op, v) {
        (UnOp::Not, Value::Bool(b)) => Ok(Value::Bool(!*b)),
        (UnOp::Not, Value::I8(a)) => Ok(Value::I8(prim::i8_not(*a))),
        (UnOp::Not, Value::I32(a)) => Ok(Value::I32(prim::i32_not(*a))),
        (UnOp::Not, Value::I64(a)) => Ok(Value::I64(prim::i64_not(*a))),
        (UnOp::Negate, Value::I8(a)) => Ok(Value::I8(prim::i8_neg(*a))),
        (UnOp::Negate, Value::I32(a)) => Ok(Value::I32(prim::i32_neg(*a))),
        (UnOp::Negate, Value::I64(a)) => Ok(Value::I64(prim::i64_neg(*a))),
        (UnOp::Negate, Value::F32(a)) => Ok(Value::F32(prim::f32_neg(*a))),
        (UnOp::Negate, Value::F64(a)) => Ok(Value::F64(prim::f64_neg(*a))),
        (UnOp::Negate, Value::Vec3(a)) => Ok(Value::Vec3(F64x3 { x: prim::f64_neg(a.x), y: prim::f64_neg(a.y), z: prim::f64_neg(a.z) })),
        (UnOp::Negate, Value::Pos3(a)) => Ok(Value::Pos3(I32x3 { x: prim::i32_neg(a.x), y: prim::i32_neg(a.y), z: prim::i32_neg(a.z) })),
        _ => Err(shape()),
    }
}

/// An integer, carried as i64, to an integer or float type.
fn from_int(to: &Type, x: i64) -> Result<Value, EvalError> {
    match to {
        Type::I8 => Ok(Value::I8(prim::i64_to_i8(x))),
        Type::I32 => Ok(Value::I32(prim::i64_to_i32(x))),
        Type::I64 => Ok(Value::I64(x)),
        Type::F32 => Ok(Value::F32(prim::i64_to_f32(x))),
        Type::F64 => Ok(Value::F64(prim::i64_to_f64(x))),
        _ => Err(shape()),
    }
}

pub fn cast(to: &Type, v: &Value) -> Result<Value, EvalError> {
    match v {
        Value::I8(a) => from_int(to, prim::i8_to_i64(*a)),
        Value::I32(a) => from_int(to, prim::i32_to_i64(*a)),
        Value::I64(a) => from_int(to, *a),
        Value::Bool(b) => match to {
            Type::I8 | Type::I32 | Type::I64 => from_int(to, if *b { 1 } else { 0 }),
            _ => Err(shape()),
        },
        Value::F32(a) => match to {
            Type::I8 => Ok(Value::I8(prim::f32_to_i8(*a))),
            Type::I32 => Ok(Value::I32(prim::f32_to_i32(*a))),
            Type::I64 => Ok(Value::I64(prim::f32_to_i64(*a))),
            Type::F32 => Ok(Value::F32(*a)),
            Type::F64 => Ok(Value::F64(prim::f32_to_f64(*a))),
            _ => Err(shape()),
        },
        Value::F64(a) => match to {
            Type::I8 => Ok(Value::I8(prim::f64_to_i8(*a))),
            Type::I32 => Ok(Value::I32(prim::f64_to_i32(*a))),
            Type::I64 => Ok(Value::I64(prim::f64_to_i64(*a))),
            Type::F32 => Ok(Value::F32(prim::f64_to_f32(*a))),
            Type::F64 => Ok(Value::F64(*a)),
            _ => Err(shape()),
        },
        Value::Pos3(a) => match to {
            Type::Vec3 => Ok(Value::Vec3(pos_to_vec(a))),
            _ => Err(shape()),
        },
        _ => Err(shape()),
    }
}

} // verus!
