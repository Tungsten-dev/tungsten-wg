//! The trusted primitives: float arithmetic, float comparisons and conversions to and from float,
//! on bits. Each of these 22 functions is an `external_body` whose `ensures` equates it to an
//! uninterpreted function in `crate::spec::prim`. This is the only file `verify.sh` lets use
//! `external_body`.
//!
//! The `ensures` claim each operation is a function of its input bits. For a NaN result Rust
//! doesn't promise that (sign and payload are non-deterministic, and const-folding may differ from
//! run time), so arithmetic and float-to-float conversions return one canonical quiet NaN. The
//! bits are the ones Java's `doubleToLongBits` / `floatToIntBits` produce. Comparisons and casts
//! to integers are deterministic as they are.
//!
//! Floats are IEEE-754 with each op rounded to its own type. Float → int rounds toward zero,
//! saturates and maps NaN to 0.

use vstd::prelude::*;

use crate::spec::prim::*;

fn canon32(x: f32) -> u32 {
    if x.is_nan() { 0x7fc0_0000 } else { x.to_bits() }
}

fn canon64(x: f64) -> u64 {
    if x.is_nan() { 0x7ff8_0000_0000_0000 } else { x.to_bits() }
}

verus! {

#[verifier::external_body]
pub fn f32_add(a: u32, b: u32) -> (r: u32) ensures r == spec_f32_add(a, b) { canon32(f32::from_bits(a) + f32::from_bits(b)) }

#[verifier::external_body]
pub fn f32_sub(a: u32, b: u32) -> (r: u32) ensures r == spec_f32_sub(a, b) { canon32(f32::from_bits(a) - f32::from_bits(b)) }

#[verifier::external_body]
pub fn f32_mul(a: u32, b: u32) -> (r: u32) ensures r == spec_f32_mul(a, b) { canon32(f32::from_bits(a) * f32::from_bits(b)) }

#[verifier::external_body]
pub fn f32_div(a: u32, b: u32) -> (r: u32) ensures r == spec_f32_div(a, b) { canon32(f32::from_bits(a) / f32::from_bits(b)) }

#[verifier::external_body]
pub fn f32_eq(a: u32, b: u32) -> (r: bool) ensures r == spec_f32_eq(a, b) { f32::from_bits(a) == f32::from_bits(b) }

#[verifier::external_body]
pub fn f32_lt(a: u32, b: u32) -> (r: bool) ensures r == spec_f32_lt(a, b) { f32::from_bits(a) < f32::from_bits(b) }

#[verifier::external_body]
pub fn f32_le(a: u32, b: u32) -> (r: bool) ensures r == spec_f32_le(a, b) { f32::from_bits(a) <= f32::from_bits(b) }

#[verifier::external_body]
pub fn f64_add(a: u64, b: u64) -> (r: u64) ensures r == spec_f64_add(a, b) { canon64(f64::from_bits(a) + f64::from_bits(b)) }

#[verifier::external_body]
pub fn f64_sub(a: u64, b: u64) -> (r: u64) ensures r == spec_f64_sub(a, b) { canon64(f64::from_bits(a) - f64::from_bits(b)) }

#[verifier::external_body]
pub fn f64_mul(a: u64, b: u64) -> (r: u64) ensures r == spec_f64_mul(a, b) { canon64(f64::from_bits(a) * f64::from_bits(b)) }

#[verifier::external_body]
pub fn f64_div(a: u64, b: u64) -> (r: u64) ensures r == spec_f64_div(a, b) { canon64(f64::from_bits(a) / f64::from_bits(b)) }

#[verifier::external_body]
pub fn f64_eq(a: u64, b: u64) -> (r: bool) ensures r == spec_f64_eq(a, b) { f64::from_bits(a) == f64::from_bits(b) }

#[verifier::external_body]
pub fn f64_lt(a: u64, b: u64) -> (r: bool) ensures r == spec_f64_lt(a, b) { f64::from_bits(a) < f64::from_bits(b) }

#[verifier::external_body]
pub fn f64_le(a: u64, b: u64) -> (r: bool) ensures r == spec_f64_le(a, b) { f64::from_bits(a) <= f64::from_bits(b) }

/// Round to nearest even.
#[verifier::external_body]
pub fn i64_to_f32(a: i64) -> (r: u32) ensures r == spec_i64_to_f32(a) { (a as f32).to_bits() }

#[verifier::external_body]
pub fn i64_to_f64(a: i64) -> (r: u64) ensures r == spec_i64_to_f64(a) { (a as f64).to_bits() }

/// Round toward zero, saturate, NaN → 0.
#[verifier::external_body]
pub fn f32_to_i32(a: u32) -> (r: i32) ensures r == spec_f32_to_i32(a) { f32::from_bits(a) as i32 }

#[verifier::external_body]
pub fn f32_to_i64(a: u32) -> (r: i64) ensures r == spec_f32_to_i64(a) { f32::from_bits(a) as i64 }

#[verifier::external_body]
pub fn f64_to_i32(a: u64) -> (r: i32) ensures r == spec_f64_to_i32(a) { f64::from_bits(a) as i32 }

#[verifier::external_body]
pub fn f64_to_i64(a: u64) -> (r: i64) ensures r == spec_f64_to_i64(a) { f64::from_bits(a) as i64 }

/// Exact, apart from the NaN rule above.
#[verifier::external_body]
pub fn f32_to_f64(a: u32) -> (r: u64) ensures r == spec_f32_to_f64(a) { canon64(f32::from_bits(a) as f64) }

/// Round to nearest even, apart from the NaN rule above.
#[verifier::external_body]
pub fn f64_to_f32(a: u64) -> (r: u32) ensures r == spec_f64_to_f32(a) { canon32(f64::from_bits(a) as f32) }

} // verus!
