//! Integer primitives and the float operations that need no trust. Semantics are Java's
//! (TESTING.md §5.4).
//!
//! Every function here is verified against its definition in `crate::spec::prim`. The float
//! arithmetic, comparisons and float conversions Verus can't reason about are in
//! `crate::prim_float`, re-exported below so that callers see one module.
//!
//! Integers: `+ - *` wrap, `/` truncates toward zero with `MIN / -1 == MIN` (the caller handles
//! division by zero), shift amounts are masked to `width - 1`, and `i8` ops promote to `i32`,
//! apply the `i32` rule and truncate. Float → `i8` goes through `i32` and then truncates, as
//! Java's `(byte)` cast does.

use vstd::prelude::*;

pub use crate::prim_float::*;
use crate::spec::prim::*;

verus! {

// ---------------------------------------------------------------------------------------------
// i32
// ---------------------------------------------------------------------------------------------

pub fn i32_add(a: i32, b: i32) -> (r: i32) ensures r == spec_i32_add(a, b) { a.wrapping_add(b) }

pub fn i32_sub(a: i32, b: i32) -> (r: i32) ensures r == spec_i32_sub(a, b) { a.wrapping_sub(b) }

pub fn i32_mul(a: i32, b: i32) -> (r: i32) ensures r == spec_i32_mul(a, b) { a.wrapping_mul(b) }

/// `b != 0` is the caller's job; `MIN / -1 == MIN`.
pub fn i32_div(a: i32, b: i32) -> (r: i32) ensures r == spec_i32_div(a, b) {
    match a.checked_div(b) {
        Some(q) => q,
        None => if b == 0 { 0 } else { a },
    }
}

pub fn i32_neg(a: i32) -> (r: i32) ensures r == spec_i32_neg(a) { 0i32.wrapping_sub(a) }

pub fn i32_not(a: i32) -> (r: i32) ensures r == spec_i32_not(a) { !a }

pub fn i32_and(a: i32, b: i32) -> (r: i32) ensures r == spec_i32_and(a, b) { a & b }

pub fn i32_or(a: i32, b: i32) -> (r: i32) ensures r == spec_i32_or(a, b) { a | b }

pub fn i32_xor(a: i32, b: i32) -> (r: i32) ensures r == spec_i32_xor(a, b) { a ^ b }

pub fn i32_shl(a: i32, s: i32) -> (r: i32) ensures r == spec_i32_shl(a, s) { a.wrapping_shl((s & 31) as u32) }

pub fn i32_shr(a: i32, s: i32) -> (r: i32) ensures r == spec_i32_shr(a, s) { a.wrapping_shr((s & 31) as u32) }

pub fn i32_ushr(a: i32, s: i32) -> (r: i32) ensures r == spec_i32_ushr(a, s) {
    assert(0 <= (s & 31) < 32) by (bit_vector);
    ((a as u32) >> ((s & 31) as u32)) as i32
}

// ---------------------------------------------------------------------------------------------
// i64
// ---------------------------------------------------------------------------------------------

pub fn i64_add(a: i64, b: i64) -> (r: i64) ensures r == spec_i64_add(a, b) { a.wrapping_add(b) }

pub fn i64_sub(a: i64, b: i64) -> (r: i64) ensures r == spec_i64_sub(a, b) { a.wrapping_sub(b) }

pub fn i64_mul(a: i64, b: i64) -> (r: i64) ensures r == spec_i64_mul(a, b) { a.wrapping_mul(b) }

/// `b != 0` is the caller's job; `MIN / -1 == MIN`.
pub fn i64_div(a: i64, b: i64) -> (r: i64) ensures r == spec_i64_div(a, b) {
    match a.checked_div(b) {
        Some(q) => q,
        None => if b == 0 { 0 } else { a },
    }
}

pub fn i64_neg(a: i64) -> (r: i64) ensures r == spec_i64_neg(a) { 0i64.wrapping_sub(a) }

pub fn i64_not(a: i64) -> (r: i64) ensures r == spec_i64_not(a) { !a }

pub fn i64_and(a: i64, b: i64) -> (r: i64) ensures r == spec_i64_and(a, b) { a & b }

pub fn i64_or(a: i64, b: i64) -> (r: i64) ensures r == spec_i64_or(a, b) { a | b }

pub fn i64_xor(a: i64, b: i64) -> (r: i64) ensures r == spec_i64_xor(a, b) { a ^ b }

pub fn i64_shl(a: i64, s: i32) -> (r: i64) ensures r == spec_i64_shl(a, s) { a.wrapping_shl((s & 63) as u32) }

pub fn i64_shr(a: i64, s: i32) -> (r: i64) ensures r == spec_i64_shr(a, s) { a.wrapping_shr((s & 63) as u32) }

pub fn i64_ushr(a: i64, s: i32) -> (r: i64) ensures r == spec_i64_ushr(a, s) {
    assert(0 <= (s & 63) < 64) by (bit_vector);
    ((a as u64) >> ((s & 63) as u32)) as i64
}

// ---------------------------------------------------------------------------------------------
// i8: promote to i32, apply the i32 rule, truncate
// ---------------------------------------------------------------------------------------------

pub fn i8_add(a: i8, b: i8) -> (r: i8) ensures r == spec_i8_add(a, b) { i32_add(a as i32, b as i32) as i8 }

pub fn i8_sub(a: i8, b: i8) -> (r: i8) ensures r == spec_i8_sub(a, b) { i32_sub(a as i32, b as i32) as i8 }

pub fn i8_mul(a: i8, b: i8) -> (r: i8) ensures r == spec_i8_mul(a, b) { i32_mul(a as i32, b as i32) as i8 }

pub fn i8_div(a: i8, b: i8) -> (r: i8) ensures r == spec_i8_div(a, b) { i32_div(a as i32, b as i32) as i8 }

pub fn i8_neg(a: i8) -> (r: i8) ensures r == spec_i8_neg(a) { i32_neg(a as i32) as i8 }

pub fn i8_not(a: i8) -> (r: i8) ensures r == spec_i8_not(a) { !a }

pub fn i8_and(a: i8, b: i8) -> (r: i8) ensures r == spec_i8_and(a, b) { a & b }

pub fn i8_or(a: i8, b: i8) -> (r: i8) ensures r == spec_i8_or(a, b) { a | b }

pub fn i8_xor(a: i8, b: i8) -> (r: i8) ensures r == spec_i8_xor(a, b) { a ^ b }

pub fn i8_shl(a: i8, s: i32) -> (r: i8) ensures r == spec_i8_shl(a, s) { i32_shl(a as i32, s) as i8 }

pub fn i8_shr(a: i8, s: i32) -> (r: i8) ensures r == spec_i8_shr(a, s) { i32_shr(a as i32, s) as i8 }

/// Java's `(byte)(b >>> s)`: the sign-extended `i32` is shifted logically.
pub fn i8_ushr(a: i8, s: i32) -> (r: i8) ensures r == spec_i8_ushr(a, s) { i32_ushr(a as i32, s) as i8 }

// ---------------------------------------------------------------------------------------------
// Casts. Integers travel as i64 (every i8/i32 fits exactly).
// ---------------------------------------------------------------------------------------------

/// Truncating narrowing.
pub fn i64_to_i8(a: i64) -> (r: i8) ensures r == spec_i64_to_i8(a) { a as i8 }

pub fn i64_to_i32(a: i64) -> (r: i32) ensures r == spec_i64_to_i32(a) { a as i32 }

pub fn i32_to_i8(a: i32) -> (r: i8) ensures r == spec_i32_to_i8(a) { a as i8 }

pub fn i8_to_i64(a: i8) -> (r: i64) ensures r == spec_i8_to_i64(a) { a as i64 }

pub fn i32_to_i64(a: i32) -> (r: i64) ensures r == spec_i32_to_i64(a) { a as i64 }

// ---------------------------------------------------------------------------------------------
// Float operations that need no trust
// ---------------------------------------------------------------------------------------------

/// Flips the sign bit, which is exactly what Rust defines float negation to do, NaN included.
pub fn f32_neg(a: u32) -> (r: u32) ensures r == spec_f32_neg(a) { a ^ 0x8000_0000u32 }

pub fn f64_neg(a: u64) -> (r: u64) ensures r == spec_f64_neg(a) { a ^ 0x8000_0000_0000_0000u64 }

/// Java's `(byte) f`: saturate to `i32`, then truncate.
pub fn f32_to_i8(a: u32) -> (r: i8) ensures r == spec_f32_to_i8(a) { i32_to_i8(f32_to_i32(a)) }

pub fn f64_to_i8(a: u64) -> (r: i8) ensures r == spec_f64_to_i8(a) { i32_to_i8(f64_to_i32(a)) }

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    fn f64b(x: f64) -> u64 {
        x.to_bits()
    }

    #[test]
    fn integer_edges() {
        assert_eq!(i32_div(i32::MIN, -1), i32::MIN);
        assert_eq!(i64_div(i64::MIN, -1), i64::MIN);
        assert_eq!(i8_div(i8::MIN, -1), i8::MIN, "(byte)(128) == -128");
        assert_eq!(i32_div(-7, 2), -3, "truncates toward zero");
        assert_eq!(i32_add(i32::MAX, 1), i32::MIN);
        assert_eq!(i8_add(127, 1), -128);
        assert_eq!(i8_mul(16, 16), 0);
        assert_eq!(i64_mul(i64::MAX, 2), -2);
        assert_eq!(i32_neg(i32::MIN), i32::MIN);
    }

    #[test]
    fn shift_masks() {
        assert_eq!(i32_shl(1, 33), 2, "masked to 31");
        assert_eq!(i32_shl(1, -1), i32::MIN, "-1 & 31 == 31");
        assert_eq!(i64_shl(1, 65), 2, "masked to 63");
        assert_eq!(i32_shr(-8, 1), -4);
        assert_eq!(i32_ushr(-8, 1), 0x7fff_fffc);
        assert_eq!(i64_ushr(-1, 60), 15);
        // Java: (byte)((byte)-8 >>> 1) == (byte)0x7ffffffc == -4.
        assert_eq!(i8_ushr(-8, 1), -4);
        // (byte)(-1 >>> 28) == (byte)15 == 15.
        assert_eq!(i8_ushr(-1, 28), 15);
        assert_eq!(i8_shl(1, 7), -128);
        assert_eq!(i8_shl(1, 8), 0, "shifted in i32, then truncated");
    }

    #[test]
    fn cast_edges() {
        assert_eq!(f64_to_i32(f64b(f64::NAN)), 0);
        assert_eq!(f64_to_i32(f64b(f64::INFINITY)), i32::MAX);
        assert_eq!(f64_to_i32(f64b(f64::NEG_INFINITY)), i32::MIN);
        assert_eq!(f64_to_i32(f64b(3e10)), i32::MAX);
        assert_eq!(f64_to_i64(f64b(-1e300)), i64::MIN);
        assert_eq!(f64_to_i32(f64b(-2.9)), -2);
        assert_eq!(f32_to_i64(f32::NAN.to_bits()), 0);
        // Java: (byte) 300.0 == (byte) 300 == 44; (byte) 1e10 == (byte) MAX_INT == -1.
        assert_eq!(f64_to_i8(f64b(300.0)), 44);
        assert_eq!(f64_to_i8(f64b(1e10)), -1);
        assert_eq!(f64_to_i8(f64b(f64::NAN)), 0);
        assert_eq!(f64_to_f32(f64b(-0.0)), (-0.0f32).to_bits());
        assert_eq!(f32_to_f64((-0.0f32).to_bits()), f64b(-0.0));
        assert_eq!(i64_to_f64(i64::MAX), f64b(9.223372036854776e18));
        assert_eq!(i64_to_f32(16_777_217), 16_777_216.0f32.to_bits(), "round to nearest even");
        assert_eq!(i64_to_i8(300), 44);
        assert_eq!(i64_to_i32(1 << 32), 0);
    }

    #[test]
    fn floats() {
        assert!(!f64_eq(f64b(f64::NAN), f64b(f64::NAN)));
        assert!(f64_eq(f64b(0.0), f64b(-0.0)));
        assert!(!f64_lt(f64b(f64::NAN), f64b(1.0)));
        assert_eq!(f64_neg(f64b(0.0)), f64b(-0.0));
        assert_eq!(f64_add(f64b(-0.0), f64b(-0.0)), f64b(-0.0));
        assert_eq!(f64_add(f64b(-0.0), f64b(0.0)), f64b(0.0));
        assert_eq!(f64_div(f64b(1.0), f64b(0.0)), f64b(f64::INFINITY));
        assert_eq!(f32_mul(1.5f32.to_bits(), 2.0f32.to_bits()), 3.0f32.to_bits());
    }

    #[test]
    fn computed_nans_are_canonical() {
        const NAN64: u64 = 0x7ff8_0000_0000_0000;
        const NAN32: u32 = 0x7fc0_0000;
        let payload64 = 0xfff8_0000_0000_0123u64;
        let payload32 = 0xffc0_0123u32;
        assert_eq!(f64_add(payload64, f64b(1.0)), NAN64);
        assert_eq!(f64_sub(f64b(f64::INFINITY), f64b(f64::INFINITY)), NAN64);
        assert_eq!(f64_mul(f64b(0.0), f64b(f64::INFINITY)), NAN64);
        assert_eq!(f64_div(f64b(0.0), f64b(0.0)), NAN64);
        assert_eq!(f32_add(payload32, 1.0f32.to_bits()), NAN32);
        assert_eq!(f32_div(0.0f32.to_bits(), 0.0f32.to_bits()), NAN32);
        assert_eq!(f64_to_f32(payload64), NAN32);
        assert_eq!(f32_to_f64(payload32), NAN64);
        // Negation only flips the sign bit, so a payload survives it.
        assert_eq!(f64_neg(payload64), payload64 ^ (1u64 << 63));
        assert_eq!(f32_neg(payload32), payload32 ^ (1u32 << 31));
        assert_eq!(f64_neg(f64b(-1.5)), f64b(1.5));
    }

    /// The verified integer bodies against the plain-Rust operations they replaced.
    #[test]
    fn verified_bodies_match_std() {
        let xs = [i32::MIN, i32::MIN + 1, -7, -1, 0, 1, 2, 7, i32::MAX];
        for a in xs {
            assert_eq!(i32_neg(a), a.wrapping_neg());
            for b in xs {
                assert_eq!(i32_div(a, b), if b == 0 { 0 } else { a.wrapping_div(b) });
            }
        }
        let ys = [i64::MIN, -7, -1, 0, 1, 7, i64::MAX];
        for a in ys {
            assert_eq!(i64_neg(a), a.wrapping_neg());
            for b in ys {
                assert_eq!(i64_div(a, b), if b == 0 { 0 } else { a.wrapping_div(b) });
            }
        }
        for a in i8::MIN..=i8::MAX {
            assert_eq!(i8_neg(a), (a as i32).wrapping_neg() as i8);
            for b in [i8::MIN, -1, 0, 1, 3, i8::MAX] {
                assert_eq!(i8_div(a, b), if b == 0 { 0 } else { (a as i32).wrapping_div(b as i32) as i8 });
            }
        }
    }
}
