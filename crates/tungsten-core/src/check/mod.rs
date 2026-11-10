//! The checker (§6). [`check`] runs [`wf::check_wf`] first and stops if that reports anything.
//! Otherwise it runs typing, flow, scoping, grid and cycle rules and returns every error in walk
//! order: consts (by id), functions (by id), units, inputs, entries, then cycles. Within an item
//! the order is pre-order by path, a node's own error before its children's.

use vstd::prelude::*;

use crate::ir::*;
use crate::util::*;
use crate::value::*;

pub mod acyclic;
pub mod error;
pub mod flow;
pub mod scope;
pub mod types;
pub mod wf;

pub use error::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

// ---------------------------------------------------------------------------------------------
// Walk order
// ---------------------------------------------------------------------------------------------

/// (category, id) of an item, in walk order.
fn item_key(i: CoreItem) -> (usize, usize) {
    match i {
        CoreItem::Const(c) => (0, c.0),
        CoreItem::Function(f) => (1, f.0),
        CoreItem::Unit(u) => (2, u.0),
        CoreItem::Input(x) => (3, x.0),
        CoreItem::Entry(e) => (4, e),
        CoreItem::Program => (5, 0),
        CoreItem::Host(h) => (6, h.0),
        CoreItem::Extern(x) => (7, x.0),
        CoreItem::Record(r) => (8, r.0),
    }
}

/// `base + position of l` (base ≤ 10, so no overflow).
fn list_class(l: ScopeList, base: usize) -> usize
    requires
        base <= 10,
{
    match l {
        ScopeList::Inputs => base,
        ScopeList::Helpers => base + 1,
        ScopeList::Tables => base + 2,
        ScopeList::Hosts => base + 3,
        ScopeList::Consts => base + 4,
    }
}

/// (class, index) of a step: siblings in the order the walk visits them.
fn step_key(s: CoreStep) -> (usize, usize) {
    match s {
        CoreStep::Local(i) => (0, i),
        CoreStep::Param(i) => (0, i),
        CoreStep::Ret => (1, 0),
        CoreStep::Type => (2, 0),
        CoreStep::ParamType(i) => (3, i),
        CoreStep::Main => (4, 0),
        CoreStep::List(l, i) => (list_class(l, 5), i),
        CoreStep::WholeList(l) => (list_class(l, 10), 0),
        CoreStep::Source => (15, 0),
        CoreStep::Unit => (16, 0),
        CoreStep::Dims => (17, 0),
        CoreStep::Cond => (20, 0),
        CoreStep::Value => (20, 0),
        CoreStep::Then(i) => (21, i),
        CoreStep::Else(i) => (22, i),
        CoreStep::Body(i) => (23, i),
        CoreStep::Stmt(i) => (24, i),
        CoreStep::Base => (30, 0),
        CoreStep::Left => (30, 0),
        CoreStep::Operand => (30, 0),
        CoreStep::Right => (31, 0),
        CoreStep::Index => (32, 0),
        CoreStep::Arg(i) => (33, i),
        CoreStep::Element(i) => (33, i),
        CoreStep::Field(_, i) => (33, i),
    }
}

/// -1, 0, 1.
fn cmp_pair(a: (usize, usize), b: (usize, usize)) -> i8 {
    if a.0 < b.0 {
        -1
    } else if a.0 > b.0 {
        1
    } else if a.1 < b.1 {
        -1
    } else if a.1 > b.1 {
        1
    } else {
        0
    }
}

/// Is `a` strictly before `b` in walk order?
fn before(a: &CorePath, b: &CorePath) -> bool {
    let c = cmp_pair(item_key(a.item), item_key(b.item));
    if c != 0 {
        return c < 0;
    }
    let mut i: usize = 0;
    while i < a.steps.len() && i < b.steps.len()
        invariant
            i <= a.steps.len(),
        decreases a.steps.len() - i,
    {
        let c = cmp_pair(step_key(a.steps[i]), step_key(b.steps[i]));
        if c != 0 {
            return c < 0;
        }
        i = i + 1;
    }
    a.steps.len() < b.steps.len()
}

/// A stable sort into walk order (selection by removal: the first minimal error goes next).
pub fn sort_errors(errs: Vec<CheckError>) -> Vec<CheckError> {
    let mut rest = errs;
    let mut out: Vec<CheckError> = Vec::new();
    let n = rest.len();
    let mut round: usize = 0;
    while round < n && rest.len() > 0
        invariant
            round <= n,
        decreases n - round,
    {
        let mut best: usize = 0;
        let mut i: usize = 1;
        while i < rest.len()
            invariant
                1 <= i <= rest.len(),
                best < rest.len(),
            decreases rest.len() - i,
        {
            if before(&rest[i].at, &rest[best].at) {
                best = i;
            }
            i = i + 1;
        }
        let e = rest.remove(best);
        out.push(e);
        round = round + 1;
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Grids
// ---------------------------------------------------------------------------------------------

/// Every component ≥ 1 (`DimsTooSmall`), and `x*y*z ≤ i32::MAX` in checked `i64` (`GridTooLarge`).
pub fn check_dims(d: &Dims, item: CoreItem, errs: &mut Vec<CheckError>) {
    let mut path: Vec<CoreStep> = Vec::new();
    path.push(CoreStep::Dims);
    if d.x < 1 || d.y < 1 || d.z < 1 {
        report(errs, CheckErrorKind::DimsTooSmall, item, &path);
        return;
    }
    let ok = match (d.x as i64).checked_mul(d.y as i64) {
        Some(xy) => match xy.checked_mul(d.z as i64) {
            Some(n) => n <= 0x7fff_ffffi64,
            None => false,
        },
        None => false,
    };
    if !ok {
        report(errs, CheckErrorKind::GridTooLarge, item, &path);
    }
}

// ---------------------------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------------------------

pub fn check(p: &Program) -> Result<(), CheckErrors> {
    let wf = wf::check_wf(p);
    if wf.len() > 0 {
        return Err(CheckErrors { errors: wf });
    }
    let ranks = acyclic::ranks(p);
    let mut errs: Vec<CheckError> = Vec::new();
    let ctys = types::const_types(p, &ranks, &mut errs);
    types::check_types(p, &ctys, &mut errs);
    flow::check_flow(p, &mut errs);
    scope::check_scope(p, &mut errs);
    let mut k: usize = 0;
    while k < p.inputs.len()
        invariant
            k <= p.inputs.len(),
        decreases p.inputs.len() - k,
    {
        check_dims(&p.inputs[k].dims, CoreItem::Input(InputId(k)), &mut errs);
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < p.entries.len()
        invariant
            k <= p.entries.len(),
        decreases p.entries.len() - k,
    {
        check_dims(&p.entries[k].dims, CoreItem::Entry(k), &mut errs);
        k = k + 1;
    }
    let mut errs = sort_errors(errs);
    let cycles = acyclic::cycles(p);
    let mut c: usize = 0;
    while c < cycles.len()
        invariant
            c <= cycles.len(),
        decreases cycles.len() - c,
    {
        let mut members: Vec<CycleMember> = Vec::new();
        let m = &cycles[c];
        let mut i: usize = 0;
        while i < m.len()
            invariant
                i <= m.len(),
            decreases m.len() - i,
        {
            members.push(m[i]);
            i = i + 1;
        }
        let path: Vec<CoreStep> = Vec::new();
        report(&mut errs, CheckErrorKind::Cycle { members }, CoreItem::Program, &path);
        c = c + 1;
    }
    if errs.len() > 0 {
        Err(CheckErrors { errors: errs })
    } else {
        Ok(())
    }
}

/// `case.hosts` has one value per host, each of the host's declared type.
pub fn check_case(p: &Program, case: &Case) -> Result<(), CaseErrors> {
    if case.hosts.len() != p.hosts.len() {
        let mut errors: Vec<CaseError> = Vec::new();
        errors.push(CaseError::HostCount { expected: p.hosts.len(), found: case.hosts.len() });
        return Err(CaseErrors { errors });
    }
    let mut errors: Vec<CaseError> = Vec::new();
    let mut h: usize = 0;
    while h < p.hosts.len()
        invariant
            h <= p.hosts.len(),
            case.hosts.len() == p.hosts.len(),
        decreases p.hosts.len() - h,
    {
        if !value_has_type(p, &case.hosts[h], &p.hosts[h].ty) {
            errors.push(CaseError::HostValue { host: HostId(h) });
        }
        h = h + 1;
    }
    if errors.len() > 0 {
        Err(CaseErrors { errors })
    } else {
        Ok(())
    }
}

} // verus!
