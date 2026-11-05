//! How the views from exec values to spec values behave under the operations exec code performs.

use vstd::prelude::*;

use crate::ir::*;
use crate::spec::value::*;
use crate::value::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

pub proof fn lemma_view_array(vs: Vec<Value>)
    ensures
        view_value(Value::Array(vs)) == SValue::Array(view_values(vs@)),
{
    assert(view_value(Value::Array(vs))->Array_0 =~= view_values(vs@));
}

pub proof fn lemma_view_record(r: RecordId, vs: Vec<Value>)
    ensures
        view_value(Value::Record(r, vs)) == SValue::Record(r, view_values(vs@)),
{
    assert(view_value(Value::Record(r, vs))->Record_1 =~= view_values(vs@));
}

pub proof fn lemma_view_values_push(vs: Seq<Value>, v: Value)
    ensures
        view_values(vs.push(v)) == view_values(vs).push(view_value(v)),
{
    assert(view_values(vs.push(v)) =~= view_values(vs).push(view_value(v)));
}

pub proof fn lemma_view_values_empty()
    ensures
        view_values(Seq::<Value>::empty()) == Seq::<SValue>::empty(),
{
    assert(view_values(Seq::<Value>::empty()) =~= Seq::<SValue>::empty());
}

pub proof fn lemma_view_slots_update(s: Seq<Option<Value>>, i: int, x: Option<Value>)
    requires
        0 <= i < s.len(),
    ensures
        view_slots(s.update(i, x)) == view_slots(s).update(i, view_slot(x)),
{
    assert(view_slots(s.update(i, x)) =~= view_slots(s).update(i, view_slot(x)));
}

} // verus!
