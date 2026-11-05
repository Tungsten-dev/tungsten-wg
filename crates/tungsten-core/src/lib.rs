//! The core IR: the form of a tungsten program that the verified checker, the spec semantics and
//! the proofs work on. See `README.md`.
//!
//! Everything here sits inside `verus!{}` and depends only on `vstd`. A plain `cargo build`
//! erases the ghost code, so using this crate never needs Verus. Verification runs through
//! `scripts/verify.sh`.
//!
//! The IR holds plain data: dense `usize` ids, resolved types, no names, floats as bits. Names
//! live in `tungsten-check`'s side tables. [`wf`] defines index-bounds well-formedness as
//! `spec fn`s.

#![allow(unused_imports)]
// Parameters that only proofs read look unused once the ghost code is erased.
#![cfg_attr(not(verus_keep_ghost), allow(unused_variables))]
// `check/` and `exec/` are written in the style Verus verifies:
// explicit loops and matches, `i = i + 1`, no `Option` combinators or closures.
#![allow(
    clippy::assign_op_pattern,
    clippy::len_zero,
    clippy::manual_map,
    clippy::match_like_matches_macro,
    clippy::collapsible_match,
    clippy::collapsible_if,
    clippy::collapsible_else_if,
    clippy::question_mark,
    clippy::manual_unwrap_or,
    clippy::manual_unwrap_or_default,
    clippy::redundant_field_names,
    clippy::should_implement_trait,
    clippy::needless_lifetimes,
    clippy::extra_unused_lifetimes,
    clippy::vec_init_then_push,
    clippy::implicit_saturating_sub,
    clippy::manual_range_contains,
    clippy::needless_return,
    clippy::bool_comparison,
    clippy::nonminimal_bool,
    clippy::comparison_chain,
    clippy::unnecessary_cast,
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::new_without_default,
    clippy::manual_is_multiple_of,
    clippy::implicit_saturating_add,
    // `&Vec` (not `&[_]`): the termination measures use vstd's `Vec` index axioms.
    clippy::ptr_arg,
    clippy::single_match,
    // The arm functions take the `&Box<Expr>` the `match` on `Expr` binds.
    clippy::borrowed_box
)]
// The derived `Clone`s have no spec; nothing in the spec uses them.
#![cfg_attr(verus_keep_ghost, verifier::allow(autoderive_clone_without_spec))]

use vstd::prelude::*;

pub mod check;
pub mod exec;
pub mod ir;
pub mod layout;
pub mod prim;
pub mod prim_float;
pub mod proofs;
pub mod registry;
pub mod spec;
pub mod util;
pub mod value;
pub mod wf;

pub use ir::*;
pub use registry::*;
pub use value::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_and_compare() {
        let p = Program {
            units: vec![Unit {
                main: FnId(0),
                inputs: vec![],
                tables: vec![TableId(0)],
                hosts: vec![HostId(0)],
                helpers: vec![],
                consts: vec![ConstId(0)],
            }],
            functions: vec![Function {
                params: 1,
                locals: vec![Type::F64, Type::Array(Box::new(Type::Record(RecordId(0))), 2)],
                ret: Type::F64,
                hosts: vec![HostId(0)],
                body: vec![
                    Stmt::If {
                        cond: Expr::Binary {
                            op: BinOp::Less,
                            left: Box::new(Expr::Local(LocalId(0))),
                            right: Box::new(Expr::Lit(Lit::F64(f64::NAN.to_bits()))),
                        },
                        then_branch: vec![Stmt::Break],
                        else_branch: vec![],
                    },
                    Stmt::Return(Expr::Field {
                        base: Box::new(Expr::Ambient(Ambient::Origin)),
                        sel: FieldSel::Vec3(Axis::X),
                        ty: Type::F64,
                    }),
                ],
            }],
            consts: vec![ConstDecl { init: Expr::Lit(Lit::F32((-0.0f32).to_bits())) }],
            hosts: vec![HostDecl { ty: Type::I64 }],
            tables: vec![TableDecl { kind: TableKind::Perm }],
            inputs: vec![],
            entries: vec![Entry {
                unit: UnitId(0),
                dims: Dims { x: 1, y: 1, z: 1 },
                origin_scale: F64x3 { x: 1.0f64.to_bits(), y: 1.0f64.to_bits(), z: 1.0f64.to_bits() },
                position_scale: F64x3 { x: 0, y: 0, z: 0 },
            }],
            externs: vec![ExternDecl { params: vec![Type::F64], ret: Type::F64, domain_error: false }],
            records: vec![RecordDecl { fields: vec![Type::F64, Type::I32] }],
        };
        let q = p.clone();
        assert_eq!(p, q);
        assert_eq!(format!("{p:?}"), format!("{q:?}"));

        let mut r = p.clone();
        r.consts[0].init = Expr::Lit(Lit::F32(0.0f32.to_bits()));
        assert_ne!(p, r, "-0.0 and 0.0 are different literals");
        assert_eq!(LocalId(3), LocalId(3));
        assert!(LocalId(2) < LocalId(3));
    }
}
