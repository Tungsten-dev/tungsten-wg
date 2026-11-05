//! What every primitive means (SEMANTICS.md §6).
//!
//! Integers are mathematical definitions, and `crate::prim` is verified against them. Floats are
//! uninterpreted: the only thing the theorems use about them is that each is a function of its
//! input bits, which `crate::prim_float` makes true by canonicalizing NaN results. IEEE-754
//! correctness is tested, not proven.

use vstd::prelude::*;
#[cfg(verus_keep_ghost)]
use vstd::arithmetic::div_mod::rust_div;
use vstd::wrapping::{i32_specs, i64_specs};

verus! {

// ---------------------------------------------------------------------------------------------
// i32 and i64 (SEMANTICS.md §6.1)
// ---------------------------------------------------------------------------------------------

pub open spec fn spec_i32_add(a: i32, b: i32) -> i32 { i32_specs::wrapping_add(a, b) }
pub open spec fn spec_i32_sub(a: i32, b: i32) -> i32 { i32_specs::wrapping_sub(a, b) }
pub open spec fn spec_i32_mul(a: i32, b: i32) -> i32 { i32_specs::wrapping_mul(a, b) }
/// Truncates toward zero, `MIN / -1 == MIN`, and `0` for a zero divisor (the caller reports
/// `DivByZero` first).
pub open spec fn spec_i32_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        0
    } else if a == i32::MIN && b == -1 {
        a
    } else {
        rust_div(a as int, b as int) as i32
    }
}
pub open spec fn spec_i32_neg(a: i32) -> i32 { i32_specs::wrapping_sub(0, a) }
pub open spec fn spec_i32_not(a: i32) -> i32 { !a }
pub open spec fn spec_i32_and(a: i32, b: i32) -> i32 { a & b }
pub open spec fn spec_i32_or(a: i32, b: i32) -> i32 { a | b }
pub open spec fn spec_i32_xor(a: i32, b: i32) -> i32 { a ^ b }
/// The shift amount is masked to `width - 1`.
pub open spec fn spec_i32_shl(a: i32, s: i32) -> i32 { i32_specs::wrapping_shl(a, (s & 31) as u32) }
pub open spec fn spec_i32_shr(a: i32, s: i32) -> i32 { i32_specs::wrapping_shr(a, (s & 31) as u32) }
pub open spec fn spec_i32_ushr(a: i32, s: i32) -> i32 { ((a as u32) >> ((s & 31) as u32)) as i32 }

pub open spec fn spec_i64_add(a: i64, b: i64) -> i64 { i64_specs::wrapping_add(a, b) }
pub open spec fn spec_i64_sub(a: i64, b: i64) -> i64 { i64_specs::wrapping_sub(a, b) }
pub open spec fn spec_i64_mul(a: i64, b: i64) -> i64 { i64_specs::wrapping_mul(a, b) }
/// Truncates toward zero, `MIN / -1 == MIN`, and `0` for a zero divisor (the caller reports
/// `DivByZero` first).
pub open spec fn spec_i64_div(a: i64, b: i64) -> i64 {
    if b == 0 {
        0
    } else if a == i64::MIN && b == -1 {
        a
    } else {
        rust_div(a as int, b as int) as i64
    }
}
pub open spec fn spec_i64_neg(a: i64) -> i64 { i64_specs::wrapping_sub(0, a) }
pub open spec fn spec_i64_not(a: i64) -> i64 { !a }
pub open spec fn spec_i64_and(a: i64, b: i64) -> i64 { a & b }
pub open spec fn spec_i64_or(a: i64, b: i64) -> i64 { a | b }
pub open spec fn spec_i64_xor(a: i64, b: i64) -> i64 { a ^ b }
/// The shift amount is masked to `width - 1`.
pub open spec fn spec_i64_shl(a: i64, s: i32) -> i64 { i64_specs::wrapping_shl(a, (s & 63) as u32) }
pub open spec fn spec_i64_shr(a: i64, s: i32) -> i64 { i64_specs::wrapping_shr(a, (s & 63) as u32) }
pub open spec fn spec_i64_ushr(a: i64, s: i32) -> i64 { ((a as u64) >> ((s & 63) as u32)) as i64 }

// ---------------------------------------------------------------------------------------------
// i8: promote to i32, apply the i32 rule, truncate (Java's `byte`, SEMANTICS.md §6.1)
// ---------------------------------------------------------------------------------------------

pub open spec fn spec_i8_add(a: i8, b: i8) -> i8 { spec_i32_add(a as i32, b as i32) as i8 }
pub open spec fn spec_i8_sub(a: i8, b: i8) -> i8 { spec_i32_sub(a as i32, b as i32) as i8 }
pub open spec fn spec_i8_mul(a: i8, b: i8) -> i8 { spec_i32_mul(a as i32, b as i32) as i8 }
pub open spec fn spec_i8_div(a: i8, b: i8) -> i8 { spec_i32_div(a as i32, b as i32) as i8 }
pub open spec fn spec_i8_neg(a: i8) -> i8 { spec_i32_neg(a as i32) as i8 }
pub open spec fn spec_i8_not(a: i8) -> i8 { !a }
pub open spec fn spec_i8_and(a: i8, b: i8) -> i8 { a & b }
pub open spec fn spec_i8_or(a: i8, b: i8) -> i8 { a | b }
pub open spec fn spec_i8_xor(a: i8, b: i8) -> i8 { a ^ b }
pub open spec fn spec_i8_shl(a: i8, s: i32) -> i8 { spec_i32_shl(a as i32, s) as i8 }
pub open spec fn spec_i8_shr(a: i8, s: i32) -> i8 { spec_i32_shr(a as i32, s) as i8 }
/// The sign-extended 32-bit value is what gets shifted.
pub open spec fn spec_i8_ushr(a: i8, s: i32) -> i8 { spec_i32_ushr(a as i32, s) as i8 }

// ---------------------------------------------------------------------------------------------
// Integer casts (SEMANTICS.md §6.5)
// ---------------------------------------------------------------------------------------------

pub open spec fn spec_i64_to_i8(a: i64) -> i8 { a as i8 }
pub open spec fn spec_i64_to_i32(a: i64) -> i32 { a as i32 }
pub open spec fn spec_i32_to_i8(a: i32) -> i8 { a as i8 }
pub open spec fn spec_i8_to_i64(a: i8) -> i64 { a as i64 }
pub open spec fn spec_i32_to_i64(a: i32) -> i64 { a as i64 }

// ---------------------------------------------------------------------------------------------
// Float operations with no trust: negation is a sign-bit flip (Rust defines it so), and `(byte) f`
// is a saturating conversion to i32 followed by a truncation.
// ---------------------------------------------------------------------------------------------

pub open spec fn spec_f32_neg(a: u32) -> u32 { a ^ 0x8000_0000u32 }
pub open spec fn spec_f64_neg(a: u64) -> u64 { a ^ 0x8000_0000_0000_0000u64 }
pub open spec fn spec_f32_to_i8(a: u32) -> i8 { spec_i32_to_i8(spec_f32_to_i32(a)) }
pub open spec fn spec_f64_to_i8(a: u64) -> i8 { spec_i32_to_i8(spec_f64_to_i32(a)) }

// ---------------------------------------------------------------------------------------------
// Uninterpreted floats. Exactly the 22 functions `crate::prim_float` implements.
// ---------------------------------------------------------------------------------------------

pub uninterp spec fn spec_f32_add(a: u32, b: u32) -> u32;
pub uninterp spec fn spec_f32_sub(a: u32, b: u32) -> u32;
pub uninterp spec fn spec_f32_mul(a: u32, b: u32) -> u32;
pub uninterp spec fn spec_f32_div(a: u32, b: u32) -> u32;
pub uninterp spec fn spec_f32_eq(a: u32, b: u32) -> bool;
pub uninterp spec fn spec_f32_lt(a: u32, b: u32) -> bool;
pub uninterp spec fn spec_f32_le(a: u32, b: u32) -> bool;
pub uninterp spec fn spec_f32_to_i32(a: u32) -> i32;
pub uninterp spec fn spec_f32_to_i64(a: u32) -> i64;
pub uninterp spec fn spec_f64_add(a: u64, b: u64) -> u64;
pub uninterp spec fn spec_f64_sub(a: u64, b: u64) -> u64;
pub uninterp spec fn spec_f64_mul(a: u64, b: u64) -> u64;
pub uninterp spec fn spec_f64_div(a: u64, b: u64) -> u64;
pub uninterp spec fn spec_f64_eq(a: u64, b: u64) -> bool;
pub uninterp spec fn spec_f64_lt(a: u64, b: u64) -> bool;
pub uninterp spec fn spec_f64_le(a: u64, b: u64) -> bool;
pub uninterp spec fn spec_f64_to_i32(a: u64) -> i32;
pub uninterp spec fn spec_f64_to_i64(a: u64) -> i64;
pub uninterp spec fn spec_i64_to_f32(a: i64) -> u32;
pub uninterp spec fn spec_i64_to_f64(a: i64) -> u64;
pub uninterp spec fn spec_f32_to_f64(a: u32) -> u64;
pub uninterp spec fn spec_f64_to_f32(a: u64) -> u32;

} // verus!
