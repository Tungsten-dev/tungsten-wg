//! The declaration tables a [`Program`](crate::ir::Program) holds: externs and records.
//! Names are not here; they live in `tungsten-check`'s `Registry` and `LowerMap`.

use vstd::prelude::*;

use crate::ir::Type;

verus! {

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternDecl {
    pub params: Vec<Type>,
    pub ret: Type,
    /// The extern can fail with a domain error (e.g. `sqrt` of a negative number).
    pub domain_error: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordDecl {
    /// Field types, in order.
    pub fields: Vec<Type>,
}

} // verus!
