//! Spec-side data. A `spec fn` can't build a `Vec` (vstd only exposes its view), so anything the
//! semantics constructs has a `Seq`-based twin here, with a view from the exec type.

use vstd::prelude::*;

use crate::check::acyclic::Ranks;
use crate::ir::*;
use crate::value::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

/// [`Value`] with `Seq`s for `Vec`s. Floats stay bits, so equality is exact.
pub enum SValue {
    Bool(bool),
    I8(i8),
    I32(i32),
    I64(i64),
    F32(u32),
    F64(u64),
    Vec3(F64x3),
    Pos3(I32x3),
    Array(Seq<SValue>),
    Record(RecordId, Seq<SValue>),
    PermTable(TableId),
    Sampler(TableId),
}

pub open spec fn view_value(v: Value) -> SValue
    decreases v,
{
    match v {
        Value::Bool(b) => SValue::Bool(b),
        Value::I8(x) => SValue::I8(x),
        Value::I32(x) => SValue::I32(x),
        Value::I64(x) => SValue::I64(x),
        Value::F32(x) => SValue::F32(x),
        Value::F64(x) => SValue::F64(x),
        Value::Vec3(x) => SValue::Vec3(x),
        Value::Pos3(x) => SValue::Pos3(x),
        Value::Array(vs) => SValue::Array(
            Seq::new(vs@.len(), |i: int| if 0 <= i < vs@.len() { view_value(vs@[i]) } else { SValue::Bool(false) }),
        ),
        Value::Record(r, vs) => SValue::Record(
            r,
            Seq::new(vs@.len(), |i: int| if 0 <= i < vs@.len() { view_value(vs@[i]) } else { SValue::Bool(false) }),
        ),
        Value::PermTable(t) => SValue::PermTable(t),
        Value::Sampler(t) => SValue::Sampler(t),
    }
}

pub open spec fn view_values(vs: Seq<Value>) -> Seq<SValue> {
    Seq::new(vs.len(), |i: int| view_value(vs[i]))
}

pub open spec fn view_result(r: Result<Value, EvalError>) -> Result<SValue, EvalError> {
    match r {
        Ok(v) => Ok(view_value(v)),
        Err(e) => Err(e),
    }
}

pub open spec fn view_extern_result(r: Result<Value, ExternFail>) -> Result<SValue, ExternFail> {
    match r {
        Ok(v) => Ok(view_value(v)),
        Err(e) => Err(e),
    }
}

pub open spec fn view_results(rs: Seq<Result<Value, EvalError>>) -> Seq<Result<SValue, EvalError>> {
    Seq::new(rs.len(), |i: int| view_result(rs[i]))
}

/// A buffer: all the values, or the first failing cell and its error.
pub open spec fn view_buffer(r: Result<Vec<Value>, (usize, EvalError)>) -> Result<Seq<SValue>, (usize, EvalError)> {
    match r {
        Ok(vs) => Ok(view_values(vs@)),
        Err(e) => Err(e),
    }
}

/// [`crate::exec::Flow`].
pub enum SFlow {
    Next,
    Break,
    Return(SValue),
}

/// [`crate::exec::Frame`]: one slot per local, `None` until assigned.
pub type SFrame = Seq<Option<SValue>>;

pub open spec fn view_slot(s: Option<Value>) -> Option<SValue> {
    match s {
        Some(v) => Some(view_value(v)),
        None => None,
    }
}

pub open spec fn view_slots(s: Seq<Option<Value>>) -> SFrame {
    Seq::new(s.len(), |i: int| view_slot(s[i]))
}

/// [`Ranks`].
pub struct SRanks {
    pub fns: Seq<Option<usize>>,
    pub consts: Seq<Option<usize>>,
    pub inputs: Seq<Option<usize>>,
    pub units: Seq<Option<usize>>,
}

pub open spec fn view_ranks(r: Ranks) -> SRanks {
    SRanks { fns: r.fns@, consts: r.consts@, inputs: r.inputs@, units: r.units@ }
}

} // verus!
