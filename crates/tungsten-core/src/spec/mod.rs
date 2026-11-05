//! The semantics of the core IR, as definitions only. This is what a reader has to audit: every
//! theorem in this crate is stated in terms of it. See `crates/tungsten-core/PROOF.md`.

pub mod eval;
pub mod layout;
pub mod ops;
pub mod prim;
pub mod ranks;
pub mod typing;
pub mod value;
