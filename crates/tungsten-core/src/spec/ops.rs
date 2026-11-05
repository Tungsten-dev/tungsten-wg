//! What each operator computes on values that are already evaluated (SEMANTICS.md §6).
//! Anything else is `Stuck(Shape)`.

use vstd::prelude::*;

use crate::ir::*;
use crate::spec::prim::*;
use crate::spec::value::*;
use crate::value::*;

verus! {

pub open spec fn shape() -> EvalError {
    EvalError::Stuck(StuckReason::Shape)
}

/// SEMANTICS.md §6.1.

pub open spec fn spec_i32_op(op: BinOp, a: i32, b: i32) -> Result<SValue, EvalError> {
    match op {
        BinOp::Add => Ok(SValue::I32(spec_i32_add(a, b))),
        BinOp::Subtract => Ok(SValue::I32(spec_i32_sub(a, b))),
        BinOp::Multiply => Ok(SValue::I32(spec_i32_mul(a, b))),
        BinOp::Divide => if b == 0 { Err(EvalError::DivByZero) } else { Ok(SValue::I32(spec_i32_div(a, b))) },
        BinOp::Equal => Ok(SValue::Bool(a == b)),
        BinOp::NotEqual => Ok(SValue::Bool(a != b)),
        BinOp::Less => Ok(SValue::Bool(a < b)),
        BinOp::LessEqual => Ok(SValue::Bool(a <= b)),
        BinOp::Greater => Ok(SValue::Bool(a > b)),
        BinOp::GreaterEqual => Ok(SValue::Bool(a >= b)),
        BinOp::BitAnd => Ok(SValue::I32(spec_i32_and(a, b))),
        BinOp::BitOr => Ok(SValue::I32(spec_i32_or(a, b))),
        BinOp::BitXor => Ok(SValue::I32(spec_i32_xor(a, b))),
        _ => Err(shape()),
    }
}

pub open spec fn spec_i64_op(op: BinOp, a: i64, b: i64) -> Result<SValue, EvalError> {
    match op {
        BinOp::Add => Ok(SValue::I64(spec_i64_add(a, b))),
        BinOp::Subtract => Ok(SValue::I64(spec_i64_sub(a, b))),
        BinOp::Multiply => Ok(SValue::I64(spec_i64_mul(a, b))),
        BinOp::Divide => if b == 0 { Err(EvalError::DivByZero) } else { Ok(SValue::I64(spec_i64_div(a, b))) },
        BinOp::Equal => Ok(SValue::Bool(a == b)),
        BinOp::NotEqual => Ok(SValue::Bool(a != b)),
        BinOp::Less => Ok(SValue::Bool(a < b)),
        BinOp::LessEqual => Ok(SValue::Bool(a <= b)),
        BinOp::Greater => Ok(SValue::Bool(a > b)),
        BinOp::GreaterEqual => Ok(SValue::Bool(a >= b)),
        BinOp::BitAnd => Ok(SValue::I64(spec_i64_and(a, b))),
        BinOp::BitOr => Ok(SValue::I64(spec_i64_or(a, b))),
        BinOp::BitXor => Ok(SValue::I64(spec_i64_xor(a, b))),
        _ => Err(shape()),
    }
}

pub open spec fn spec_i8_op(op: BinOp, a: i8, b: i8) -> Result<SValue, EvalError> {
    match op {
        BinOp::Add => Ok(SValue::I8(spec_i8_add(a, b))),
        BinOp::Subtract => Ok(SValue::I8(spec_i8_sub(a, b))),
        BinOp::Multiply => Ok(SValue::I8(spec_i8_mul(a, b))),
        BinOp::Divide => if b == 0 { Err(EvalError::DivByZero) } else { Ok(SValue::I8(spec_i8_div(a, b))) },
        BinOp::Equal => Ok(SValue::Bool(a == b)),
        BinOp::NotEqual => Ok(SValue::Bool(a != b)),
        BinOp::Less => Ok(SValue::Bool(a < b)),
        BinOp::LessEqual => Ok(SValue::Bool(a <= b)),
        BinOp::Greater => Ok(SValue::Bool(a > b)),
        BinOp::GreaterEqual => Ok(SValue::Bool(a >= b)),
        BinOp::BitAnd => Ok(SValue::I8(spec_i8_and(a, b))),
        BinOp::BitOr => Ok(SValue::I8(spec_i8_or(a, b))),
        BinOp::BitXor => Ok(SValue::I8(spec_i8_xor(a, b))),
        _ => Err(shape()),
    }
}

pub open spec fn spec_is_shift(op: BinOp) -> bool {
    match op {
        BinOp::ShiftLeft | BinOp::ShiftRight | BinOp::LogicalShiftRight => true,
        _ => false,
    }
}

/// An integer left operand shifted by an `I32` (SEMANTICS.md §6.1).
pub open spec fn spec_shift(op: BinOp, l: SValue, s: i32) -> Result<SValue, EvalError> {
    match l {
        SValue::I8(a) => match op {
            BinOp::ShiftLeft => Ok(SValue::I8(spec_i8_shl(a, s))),
            BinOp::ShiftRight => Ok(SValue::I8(spec_i8_shr(a, s))),
            BinOp::LogicalShiftRight => Ok(SValue::I8(spec_i8_ushr(a, s))),
            _ => Err(shape()),
        },
        SValue::I32(a) => match op {
            BinOp::ShiftLeft => Ok(SValue::I32(spec_i32_shl(a, s))),
            BinOp::ShiftRight => Ok(SValue::I32(spec_i32_shr(a, s))),
            BinOp::LogicalShiftRight => Ok(SValue::I32(spec_i32_ushr(a, s))),
            _ => Err(shape()),
        },
        SValue::I64(a) => match op {
            BinOp::ShiftLeft => Ok(SValue::I64(spec_i64_shl(a, s))),
            BinOp::ShiftRight => Ok(SValue::I64(spec_i64_shr(a, s))),
            BinOp::LogicalShiftRight => Ok(SValue::I64(spec_i64_ushr(a, s))),
            _ => Err(shape()),
        },
        _ => Err(shape()),
    }
}

/// SEMANTICS.md §6.2. `>` and `>=` swap the operands, `!=` negates `==`: exactly IEEE, NaN included.

pub open spec fn spec_f32_op(op: BinOp, a: u32, b: u32) -> Result<SValue, EvalError> {
    match op {
        BinOp::Add => Ok(SValue::F32(spec_f32_add(a, b))),
        BinOp::Subtract => Ok(SValue::F32(spec_f32_sub(a, b))),
        BinOp::Multiply => Ok(SValue::F32(spec_f32_mul(a, b))),
        BinOp::Divide => Ok(SValue::F32(spec_f32_div(a, b))),
        BinOp::Equal => Ok(SValue::Bool(spec_f32_eq(a, b))),
        BinOp::NotEqual => Ok(SValue::Bool(!spec_f32_eq(a, b))),
        BinOp::Less => Ok(SValue::Bool(spec_f32_lt(a, b))),
        BinOp::LessEqual => Ok(SValue::Bool(spec_f32_le(a, b))),
        BinOp::Greater => Ok(SValue::Bool(spec_f32_lt(b, a))),
        BinOp::GreaterEqual => Ok(SValue::Bool(spec_f32_le(b, a))),
        _ => Err(shape()),
    }
}

pub open spec fn spec_f64_op(op: BinOp, a: u64, b: u64) -> Result<SValue, EvalError> {
    match op {
        BinOp::Add => Ok(SValue::F64(spec_f64_add(a, b))),
        BinOp::Subtract => Ok(SValue::F64(spec_f64_sub(a, b))),
        BinOp::Multiply => Ok(SValue::F64(spec_f64_mul(a, b))),
        BinOp::Divide => Ok(SValue::F64(spec_f64_div(a, b))),
        BinOp::Equal => Ok(SValue::Bool(spec_f64_eq(a, b))),
        BinOp::NotEqual => Ok(SValue::Bool(!spec_f64_eq(a, b))),
        BinOp::Less => Ok(SValue::Bool(spec_f64_lt(a, b))),
        BinOp::LessEqual => Ok(SValue::Bool(spec_f64_le(a, b))),
        BinOp::Greater => Ok(SValue::Bool(spec_f64_lt(b, a))),
        BinOp::GreaterEqual => Ok(SValue::Bool(spec_f64_le(b, a))),
        _ => Err(shape()),
    }
}

/// SEMANTICS.md §6.3. `&&` and `||` short-circuit, so they are handled in `spec_eval_expr`.
pub open spec fn spec_bool_op(op: BinOp, a: bool, b: bool) -> Result<SValue, EvalError> {
    match op {
        BinOp::Equal => Ok(SValue::Bool(a == b)),
        BinOp::NotEqual => Ok(SValue::Bool(a != b)),
        BinOp::BitAnd => Ok(SValue::Bool(a && b)),
        BinOp::BitOr => Ok(SValue::Bool(a || b)),
        BinOp::BitXor => Ok(SValue::Bool(a != b)),
        _ => Err(shape()),
    }
}

pub open spec fn spec_f64_arith(op: BinOp, a: u64, b: u64) -> Result<u64, EvalError> {
    match op {
        BinOp::Add => Ok(spec_f64_add(a, b)),
        BinOp::Subtract => Ok(spec_f64_sub(a, b)),
        BinOp::Multiply => Ok(spec_f64_mul(a, b)),
        BinOp::Divide => Ok(spec_f64_div(a, b)),
        _ => Err(shape()),
    }
}

pub open spec fn spec_i32_arith(op: BinOp, a: i32, b: i32) -> Result<i32, EvalError> {
    match op {
        BinOp::Add => Ok(spec_i32_add(a, b)),
        BinOp::Subtract => Ok(spec_i32_sub(a, b)),
        BinOp::Multiply => Ok(spec_i32_mul(a, b)),
        BinOp::Divide => if b == 0 { Err(EvalError::DivByZero) } else { Ok(spec_i32_div(a, b)) },
        _ => Err(shape()),
    }
}

pub open spec fn spec_i2d(x: i32) -> u64 {
    spec_i64_to_f64(spec_i32_to_i64(x))
}

pub open spec fn spec_pos_to_vec(a: I32x3) -> F64x3 {
    F64x3 { x: spec_i2d(a.x), y: spec_i2d(a.y), z: spec_i2d(a.z) }
}

/// Componentwise, x then y then z (SEMANTICS.md §6.4).
pub open spec fn spec_vec3_op(op: BinOp, a: F64x3, b: F64x3) -> Result<SValue, EvalError> {
    match (spec_f64_arith(op, a.x, b.x), spec_f64_arith(op, a.y, b.y), spec_f64_arith(op, a.z, b.z)) {
        (Ok(x), Ok(y), Ok(z)) => Ok(SValue::Vec3(F64x3 { x, y, z })),
        (Err(e), _, _) => Err(e),
        (_, Err(e), _) => Err(e),
        (_, _, Err(e)) => Err(e),
    }
}

/// Componentwise; for `/` the first zero divisor in x, y, z order is the error (SEMANTICS.md §6.4).
pub open spec fn spec_pos3_op(op: BinOp, a: I32x3, b: I32x3) -> Result<SValue, EvalError> {
    match (spec_i32_arith(op, a.x, b.x), spec_i32_arith(op, a.y, b.y), spec_i32_arith(op, a.z, b.z)) {
        (Ok(x), Ok(y), Ok(z)) => Ok(SValue::Pos3(I32x3 { x, y, z })),
        (Err(e), _, _) => Err(e),
        (_, Err(e), _) => Err(e),
        (_, _, Err(e)) => Err(e),
    }
}

pub open spec fn spec_binop(op: BinOp, l: SValue, r: SValue) -> Result<SValue, EvalError> {
    if spec_is_shift(op) {
        match r {
            SValue::I32(s) => spec_shift(op, l, s),
            _ => Err(shape()),
        }
    } else {
        match (l, r) {
            (SValue::I8(a), SValue::I8(b)) => spec_i8_op(op, a, b),
            (SValue::I32(a), SValue::I32(b)) => spec_i32_op(op, a, b),
            (SValue::I64(a), SValue::I64(b)) => spec_i64_op(op, a, b),
            (SValue::F32(a), SValue::F32(b)) => spec_f32_op(op, a, b),
            (SValue::F64(a), SValue::F64(b)) => spec_f64_op(op, a, b),
            (SValue::Bool(a), SValue::Bool(b)) => spec_bool_op(op, a, b),
            (SValue::Vec3(a), SValue::Vec3(b)) => spec_vec3_op(op, a, b),
            (SValue::Vec3(a), SValue::Pos3(b)) => spec_vec3_op(op, a, spec_pos_to_vec(b)),
            (SValue::Pos3(a), SValue::Vec3(b)) => spec_vec3_op(op, spec_pos_to_vec(a), b),
            (SValue::Pos3(a), SValue::Pos3(b)) => spec_pos3_op(op, a, b),
            _ => Err(shape()),
        }
    }
}

pub open spec fn spec_unop(op: UnOp, v: SValue) -> Result<SValue, EvalError> {
    match op {
        UnOp::Not => match v {
            SValue::Bool(b) => Ok(SValue::Bool(!b)),
            SValue::I8(a) => Ok(SValue::I8(spec_i8_not(a))),
            SValue::I32(a) => Ok(SValue::I32(spec_i32_not(a))),
            SValue::I64(a) => Ok(SValue::I64(spec_i64_not(a))),
            _ => Err(shape()),
        },
        UnOp::Negate => match v {
            SValue::I8(a) => Ok(SValue::I8(spec_i8_neg(a))),
            SValue::I32(a) => Ok(SValue::I32(spec_i32_neg(a))),
            SValue::I64(a) => Ok(SValue::I64(spec_i64_neg(a))),
            SValue::F32(a) => Ok(SValue::F32(spec_f32_neg(a))),
            SValue::F64(a) => Ok(SValue::F64(spec_f64_neg(a))),
            SValue::Vec3(a) => Ok(SValue::Vec3(F64x3 { x: spec_f64_neg(a.x), y: spec_f64_neg(a.y), z: spec_f64_neg(a.z) })),
            SValue::Pos3(a) => Ok(SValue::Pos3(I32x3 { x: spec_i32_neg(a.x), y: spec_i32_neg(a.y), z: spec_i32_neg(a.z) })),
            _ => Err(shape()),
        },
    }
}

pub open spec fn spec_axis_f64(a: Axis, v: F64x3) -> u64 {
    match a {
        Axis::X => v.x,
        Axis::Y => v.y,
        Axis::Z => v.z,
    }
}

pub open spec fn spec_axis_i32(a: Axis, v: I32x3) -> i32 {
    match a {
        Axis::X => v.x,
        Axis::Y => v.y,
        Axis::Z => v.z,
    }
}

pub open spec fn spec_field(sel: FieldSel, b: SValue) -> Result<SValue, EvalError> {
    match (sel, b) {
        (FieldSel::Vec3(a), SValue::Vec3(v)) => Ok(SValue::F64(spec_axis_f64(a, v))),
        (FieldSel::Pos3(a), SValue::Pos3(v)) => Ok(SValue::I32(spec_axis_i32(a, v))),
        (FieldSel::Record(r, i), SValue::Record(s, vs)) => {
            if r.0 != s.0 {
                Err(shape())
            } else if i < vs.len() {
                Ok(vs[i as int])
            } else {
                Err(EvalError::Stuck(StuckReason::BadIndex))
            }
        },
        _ => Err(shape()),
    }
}

/// An integer, carried as `i64`, to an integer or float type (SEMANTICS.md §6.5).
pub open spec fn spec_from_int(to: Type, x: i64) -> Result<SValue, EvalError> {
    match to {
        Type::I8 => Ok(SValue::I8(spec_i64_to_i8(x))),
        Type::I32 => Ok(SValue::I32(spec_i64_to_i32(x))),
        Type::I64 => Ok(SValue::I64(x)),
        Type::F32 => Ok(SValue::F32(spec_i64_to_f32(x))),
        Type::F64 => Ok(SValue::F64(spec_i64_to_f64(x))),
        _ => Err(shape()),
    }
}

/// SEMANTICS.md §6.5.
pub open spec fn spec_cast(to: Type, v: SValue) -> Result<SValue, EvalError> {
    match v {
        SValue::I8(a) => spec_from_int(to, spec_i8_to_i64(a)),
        SValue::I32(a) => spec_from_int(to, spec_i32_to_i64(a)),
        SValue::I64(a) => spec_from_int(to, a),
        SValue::Bool(b) => match to {
            Type::I8 | Type::I32 | Type::I64 => spec_from_int(to, if b { 1 } else { 0 }),
            _ => Err(shape()),
        },
        SValue::F32(a) => match to {
            Type::I8 => Ok(SValue::I8(spec_f32_to_i8(a))),
            Type::I32 => Ok(SValue::I32(spec_f32_to_i32(a))),
            Type::I64 => Ok(SValue::I64(spec_f32_to_i64(a))),
            Type::F32 => Ok(SValue::F32(a)),
            Type::F64 => Ok(SValue::F64(spec_f32_to_f64(a))),
            _ => Err(shape()),
        },
        SValue::F64(a) => match to {
            Type::I8 => Ok(SValue::I8(spec_f64_to_i8(a))),
            Type::I32 => Ok(SValue::I32(spec_f64_to_i32(a))),
            Type::I64 => Ok(SValue::I64(spec_f64_to_i64(a))),
            Type::F32 => Ok(SValue::F32(spec_f64_to_f32(a))),
            Type::F64 => Ok(SValue::F64(a)),
            _ => Err(shape()),
        },
        SValue::Pos3(a) => match to {
            Type::Vec3 => Ok(SValue::Vec3(spec_pos_to_vec(a))),
            _ => Err(shape()),
        },
        _ => Err(shape()),
    }
}

/// CHECK.md §9: the shape matches, an array has its declared length, a record its decl's field
/// count, and a table handle exists with the right kind.
pub open spec fn spec_value_has_type(p: Program, v: SValue, t: Type) -> bool
    decreases v,
{
    match v {
        SValue::Bool(_) => t is Bool,
        SValue::I8(_) => t is I8,
        SValue::I32(_) => t is I32,
        SValue::I64(_) => t is I64,
        SValue::F32(_) => t is F32,
        SValue::F64(_) => t is F64,
        SValue::Vec3(_) => t is Vec3,
        SValue::Pos3(_) => t is Pos3,
        SValue::Array(vs) => match t {
            Type::Array(inner, n) => vs.len() == n
                && forall|i: int| #![trigger vs[i]] 0 <= i < vs.len() ==> spec_value_has_type(p, vs[i], *inner),
            _ => false,
        },
        SValue::Record(r, vs) => match t {
            Type::Record(s) => r.0 == s.0
                && r.0 < p.records.len()
                && vs.len() == p.records[r.0 as int].fields.len()
                && forall|i: int| #![trigger vs[i]] 0 <= i < vs.len()
                    ==> spec_value_has_type(p, vs[i], p.records[r.0 as int].fields[i]),
            _ => false,
        },
        SValue::PermTable(h) => t is PermTable && h.0 < p.tables.len() && p.tables[h.0 as int].kind is Perm,
        SValue::Sampler(h) => t is Sampler && h.0 < p.tables.len() && p.tables[h.0 as int].kind is Sampler,
    }
}

} // verus!
