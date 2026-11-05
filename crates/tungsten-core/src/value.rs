//! The value domain, evaluation errors, the case, and the extern hook.

use vstd::prelude::*;

use crate::ir::*;
use crate::proofs::value::*;
use crate::registry::*;
use crate::spec::ops::*;
use crate::spec::value::*;
use crate::util::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct I32x3 {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Floats are bits, like literals, so equality is exact.
#[derive(Debug, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    I8(i8),
    I32(i32),
    I64(i64),
    F32(u32),
    F64(u64),
    Vec3(F64x3),
    Pos3(I32x3),
    Array(Vec<Value>),
    /// Fields in declaration order.
    Record(RecordId, Vec<Value>),
    PermTable(TableId),
    Sampler(TableId),
}

/// Why evaluation got stuck. Unreachable for programs and cases that pass `check` and
/// `check_case` (T4), other than the two an extern breaking its declaration can cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StuckReason {
    /// A `util::get` miss.
    BadIndex,
    /// A callee rank that is `None` or not below the current one.
    Rank,
    /// An operand, field or index value of the wrong shape.
    Shape,
    /// A read of an unassigned local.
    Unassigned,
    /// A function body fell through.
    NoReturn,
    BreakOutsideLoop,
    /// An extern result that fails `value_has_type(decl.ret)`.
    ExternResult,
    /// `ExternDomain` from an extern with `domain_error: false`.
    ExternDomainUndeclared,
    /// A unit main with params.
    MainParams,
    /// Layout overflow, or dims below zero.
    Layout,
    /// The case doesn't fit the program (only reachable without `check_case`).
    Case,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvalError {
    DivByZero,
    OutOfBounds,
    OutOfFuel,
    ExternDomain,
    Stuck(StuckReason),
}

/// One evaluation case: the origin and the host values by `HostId`.
#[derive(Debug, PartialEq, Eq)]
pub struct Case {
    pub origin: F64x3,
    pub hosts: Vec<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternFail {
    Domain,
}

/// The extern hook. Arguments are passed as evaluated; `check` guarantees they match
/// `ExternDecl.params`. The interpreter checks the result against `decl.ret`.
///
/// **Contract, not checked by Verus:** an implementation is a pure function of `ext` and `args`,
/// including the bits of any NaN it returns. The theorems trust this.
pub trait Externs {
    spec fn spec_call(&self, ext: ExternId, args: Seq<SValue>) -> Result<SValue, ExternFail>;

    fn call(&self, ext: ExternId, args: &[Value]) -> (r: Result<Value, ExternFail>)
        ensures
            view_extern_result(r) == self.spec_call(ext, view_values(args@)),
    ;
}

pub fn copy_i32x3(v: &I32x3) -> (r: I32x3)
    ensures
        r == *v,
{
    I32x3 { x: v.x, y: v.y, z: v.z }
}

#[verifier::loop_isolation(false)]
pub fn clone_value(v: &Value) -> (r: Value)
    ensures
        view_value(r) == view_value(*v),
    decreases v,
{
    match v {
        Value::Bool(b) => Value::Bool(*b),
        Value::I8(x) => Value::I8(*x),
        Value::I32(x) => Value::I32(*x),
        Value::I64(x) => Value::I64(*x),
        Value::F32(x) => Value::F32(*x),
        Value::F64(x) => Value::F64(*x),
        Value::Vec3(x) => Value::Vec3(copy_f64x3(x)),
        Value::Pos3(x) => Value::Pos3(copy_i32x3(x)),
        Value::Array(vs) => {
            let mut out: Vec<Value> = Vec::new();
            let mut i: usize = 0;
            while i < vs.len()
                invariant
                    i <= vs.len(),
                    out@.len() == i,
                    forall|j: int| 0 <= j < i ==> #[trigger] view_value(out@[j]) == view_value(vs@[j]),
                decreases vs.len() - i,
            {
                out.push(clone_value(&vs[i]));
                i = i + 1;
            }
            proof {
                lemma_view_array(out);
                lemma_view_array(*vs);
                assert(view_values(out@) =~= view_values(vs@));
            }
            Value::Array(out)
        },
        Value::Record(r, vs) => {
            let mut out: Vec<Value> = Vec::new();
            let mut i: usize = 0;
            while i < vs.len()
                invariant
                    i <= vs.len(),
                    out@.len() == i,
                    forall|j: int| 0 <= j < i ==> #[trigger] view_value(out@[j]) == view_value(vs@[j]),
                decreases vs.len() - i,
            {
                out.push(clone_value(&vs[i]));
                i = i + 1;
            }
            proof {
                lemma_view_record(*r, out);
                lemma_view_record(*r, *vs);
                assert(view_values(out@) =~= view_values(vs@));
            }
            Value::Record(*r, out)
        },
        Value::PermTable(t) => Value::PermTable(*t),
        Value::Sampler(t) => Value::Sampler(*t),
    }
}

pub fn clone_values(vs: &Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut i: usize = 0;
    while i < vs.len()
        invariant
            i <= vs.len(),
        decreases vs.len() - i,
    {
        out.push(clone_value(&vs[i]));
        i = i + 1;
    }
    out
}

pub fn clone_result(r: &Result<Value, EvalError>) -> (out: Result<Value, EvalError>)
    ensures
        view_result(out) == view_result(*r),
{
    match r {
        Ok(v) => Ok(clone_value(v)),
        Err(e) => Err(*e),
    }
}

pub fn clone_case(c: &Case) -> Case {
    Case { origin: copy_f64x3(&c.origin), hosts: clone_values(&c.hosts) }
}

/// The shapes match, array lengths match, a record has its decl's field count and each field
/// its type, and a table handle is in range with the matching kind.
#[verifier::loop_isolation(false)]
pub fn value_has_type(p: &Program, v: &Value, t: &Type) -> (r: bool)
    ensures
        r == spec_value_has_type(*p, view_value(*v), *t),
    decreases v,
{
    match (v, t) {
        (Value::Bool(_), Type::Bool) => true,
        (Value::I8(_), Type::I8) => true,
        (Value::I32(_), Type::I32) => true,
        (Value::I64(_), Type::I64) => true,
        (Value::F32(_), Type::F32) => true,
        (Value::F64(_), Type::F64) => true,
        (Value::Vec3(_), Type::Vec3) => true,
        (Value::Pos3(_), Type::Pos3) => true,
        (Value::Array(vs), Type::Array(inner, n)) => {
            proof {
                lemma_view_array(*vs);
            }
            if vs.len() != *n {
                return false;
            }
            let mut i: usize = 0;
            while i < vs.len()
                invariant
                    i <= vs.len(),
                    view_value(*v) == SValue::Array(view_values(vs@)),
                    forall|j: int| 0 <= j < i ==> spec_value_has_type(*p, #[trigger] view_values(vs@)[j], **inner),
                decreases vs.len() - i,
            {
                if !value_has_type(p, &vs[i], inner) {
                    proof {
                        assert(view_values(vs@)[i as int] == view_value(vs@[i as int]));
                    }
                    return false;
                }
                proof {
                    assert(view_values(vs@)[i as int] == view_value(vs@[i as int]));
                }
                i = i + 1;
            }
            true
        },
        (Value::Record(r, vs), Type::Record(s)) => {
            proof {
                lemma_view_record(*r, *vs);
            }
            if r.0 != s.0 {
                return false;
            }
            let decl = match get(&p.records, r.0) {
                Some(d) => d,
                None => { return false; },
            };
            if vs.len() != decl.fields.len() {
                return false;
            }
            let mut i: usize = 0;
            while i < vs.len()
                invariant
                    i <= vs.len(),
                    vs.len() == decl.fields.len(),
                    view_value(*v) == SValue::Record(*r, view_values(vs@)),
                    r.0 < p.records@.len(),
                    *decl == p.records@[r.0 as int],
                    forall|j: int| 0 <= j < i ==> spec_value_has_type(*p, #[trigger] view_values(vs@)[j], decl.fields@[j]),
                decreases vs.len() - i,
            {
                if !value_has_type(p, &vs[i], &decl.fields[i]) {
                    proof {
                        assert(view_values(vs@)[i as int] == view_value(vs@[i as int]));
                    }
                    return false;
                }
                proof {
                    assert(view_values(vs@)[i as int] == view_value(vs@[i as int]));
                }
                i = i + 1;
            }
            true
        },
        (Value::PermTable(h), Type::PermTable) => match get(&p.tables, h.0) {
            Some(d) => match d.kind {
                TableKind::Perm => true,
                TableKind::Sampler => false,
            },
            None => false,
        },
        (Value::Sampler(h), Type::Sampler) => match get(&p.tables, h.0) {
            Some(d) => match d.kind {
                TableKind::Perm => false,
                TableKind::Sampler => true,
            },
            None => false,
        },
        _ => false,
    }
}

} // verus!

impl Clone for Value {
    fn clone(&self) -> Self {
        clone_value(self)
    }
}

impl Clone for Case {
    fn clone(&self) -> Self {
        clone_case(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog() -> Program {
        Program {
            units: vec![],
            functions: vec![],
            consts: vec![],
            hosts: vec![],
            tables: vec![TableDecl { kind: TableKind::Perm }, TableDecl { kind: TableKind::Sampler }],
            inputs: vec![],
            entries: vec![],
            externs: vec![],
            records: vec![RecordDecl { fields: vec![Type::F64, Type::Array(Box::new(Type::I32), 2)] }],
        }
    }

    #[test]
    fn tables() {
        let p = prog();
        assert!(value_has_type(&p, &Value::PermTable(TableId(0)), &Type::PermTable));
        assert!(!value_has_type(&p, &Value::PermTable(TableId(1)), &Type::PermTable), "kind must match");
        assert!(value_has_type(&p, &Value::Sampler(TableId(1)), &Type::Sampler));
        assert!(!value_has_type(&p, &Value::Sampler(TableId(2)), &Type::Sampler), "id in range");
        assert!(!value_has_type(&p, &Value::PermTable(TableId(0)), &Type::Sampler));
    }

    #[test]
    fn shapes() {
        let p = prog();
        let arr = |n: usize| Value::Array((0..n).map(|i| Value::I32(i as i32)).collect());
        let rec = Value::Record(RecordId(0), vec![Value::F64(0), arr(2)]);
        assert!(value_has_type(&p, &rec, &Type::Record(RecordId(0))));
        assert!(!value_has_type(&p, &Value::Record(RecordId(0), vec![Value::F64(0), arr(3)]), &Type::Record(RecordId(0))));
        assert!(!value_has_type(&p, &Value::Record(RecordId(0), vec![Value::F64(0)]), &Type::Record(RecordId(0))));
        assert!(!value_has_type(&p, &Value::Record(RecordId(1), vec![]), &Type::Record(RecordId(1))));
        assert!(!value_has_type(&p, &Value::I32(0), &Type::I64));
        assert_eq!(rec.clone(), rec);
    }
}
