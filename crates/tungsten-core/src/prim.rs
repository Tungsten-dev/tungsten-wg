//! Trusted primitives: every program-level integer op, float op, float comparison and cast,
//! as small functions on bits. Semantics are Java's (TESTING.md §5.4).
//!
//! This is the only file with `external_body`, and `scripts/verify.sh` exempts it. In this phase
//! the functions have no `ensures`. The proof phase adds `ensures` equating each one to an
//! uninterpreted `spec fn`.
//!
//! Integers: `+ - *` wrap, `/` truncates toward zero with `MIN / -1 == MIN` (the caller handles
//! division by zero), shift amounts are masked to `width - 1`, and `i8` ops promote to `i32`,
//! apply the `i32` rule and truncate. Floats are IEEE-754 with each op rounded to its own type.
//! Float → int rounds toward zero, saturates and maps NaN to 0. Float → `i8` goes through
//! `i32` and then truncates, as Java's `(byte)` cast does.

use vstd::prelude::*;

verus! {

// ---------------------------------------------------------------------------------------------
// i32
// ---------------------------------------------------------------------------------------------

#[verifier::external_body]
pub fn i32_add(a: i32, b: i32) -> i32 { a.wrapping_add(b) }

#[verifier::external_body]
pub fn i32_sub(a: i32, b: i32) -> i32 { a.wrapping_sub(b) }

#[verifier::external_body]
pub fn i32_mul(a: i32, b: i32) -> i32 { a.wrapping_mul(b) }

/// `b != 0` is the caller's job; `MIN / -1 == MIN`.
#[verifier::external_body]
pub fn i32_div(a: i32, b: i32) -> i32 { if b == 0 { 0 } else { a.wrapping_div(b) } }

#[verifier::external_body]
pub fn i32_neg(a: i32) -> i32 { a.wrapping_neg() }

#[verifier::external_body]
pub fn i32_not(a: i32) -> i32 { !a }

#[verifier::external_body]
pub fn i32_and(a: i32, b: i32) -> i32 { a & b }

#[verifier::external_body]
pub fn i32_or(a: i32, b: i32) -> i32 { a | b }

#[verifier::external_body]
pub fn i32_xor(a: i32, b: i32) -> i32 { a ^ b }

#[verifier::external_body]
pub fn i32_shl(a: i32, s: i32) -> i32 { a.wrapping_shl((s & 31) as u32) }

#[verifier::external_body]
pub fn i32_shr(a: i32, s: i32) -> i32 { a.wrapping_shr((s & 31) as u32) }

#[verifier::external_body]
pub fn i32_ushr(a: i32, s: i32) -> i32 { ((a as u32) >> ((s & 31) as u32)) as i32 }

// ---------------------------------------------------------------------------------------------
// i64
// ---------------------------------------------------------------------------------------------

#[verifier::external_body]
pub fn i64_add(a: i64, b: i64) -> i64 { a.wrapping_add(b) }

#[verifier::external_body]
pub fn i64_sub(a: i64, b: i64) -> i64 { a.wrapping_sub(b) }

#[verifier::external_body]
pub fn i64_mul(a: i64, b: i64) -> i64 { a.wrapping_mul(b) }

#[verifier::external_body]
pub fn i64_div(a: i64, b: i64) -> i64 { if b == 0 { 0 } else { a.wrapping_div(b) } }

#[verifier::external_body]
pub fn i64_neg(a: i64) -> i64 { a.wrapping_neg() }

#[verifier::external_body]
pub fn i64_not(a: i64) -> i64 { !a }

#[verifier::external_body]
pub fn i64_and(a: i64, b: i64) -> i64 { a & b }

#[verifier::external_body]
pub fn i64_or(a: i64, b: i64) -> i64 { a | b }

#[verifier::external_body]
pub fn i64_xor(a: i64, b: i64) -> i64 { a ^ b }

#[verifier::external_body]
pub fn i64_shl(a: i64, s: i32) -> i64 { a.wrapping_shl((s & 63) as u32) }

#[verifier::external_body]
pub fn i64_shr(a: i64, s: i32) -> i64 { a.wrapping_shr((s & 63) as u32) }

#[verifier::external_body]
pub fn i64_ushr(a: i64, s: i32) -> i64 { ((a as u64) >> ((s & 63) as u32)) as i64 }

// ---------------------------------------------------------------------------------------------
// i8: promote to i32, apply the i32 rule, truncate
// ---------------------------------------------------------------------------------------------

#[verifier::external_body]
pub fn i8_add(a: i8, b: i8) -> i8 { (a as i32).wrapping_add(b as i32) as i8 }

#[verifier::external_body]
pub fn i8_sub(a: i8, b: i8) -> i8 { (a as i32).wrapping_sub(b as i32) as i8 }

#[verifier::external_body]
pub fn i8_mul(a: i8, b: i8) -> i8 { (a as i32).wrapping_mul(b as i32) as i8 }

#[verifier::external_body]
pub fn i8_div(a: i8, b: i8) -> i8 { if b == 0 { 0 } else { (a as i32).wrapping_div(b as i32) as i8 } }

#[verifier::external_body]
pub fn i8_neg(a: i8) -> i8 { (a as i32).wrapping_neg() as i8 }

#[verifier::external_body]
pub fn i8_not(a: i8) -> i8 { !a }

#[verifier::external_body]
pub fn i8_and(a: i8, b: i8) -> i8 { a & b }

#[verifier::external_body]
pub fn i8_or(a: i8, b: i8) -> i8 { a | b }

#[verifier::external_body]
pub fn i8_xor(a: i8, b: i8) -> i8 { a ^ b }

#[verifier::external_body]
pub fn i8_shl(a: i8, s: i32) -> i8 { (a as i32).wrapping_shl((s & 31) as u32) as i8 }

#[verifier::external_body]
pub fn i8_shr(a: i8, s: i32) -> i8 { (a as i32).wrapping_shr((s & 31) as u32) as i8 }

/// Java's `(byte)(b >>> s)`: the sign-extended `i32` is shifted logically.
#[verifier::external_body]
pub fn i8_ushr(a: i8, s: i32) -> i8 { (((a as i32) as u32) >> ((s & 31) as u32)) as i8 }

// ---------------------------------------------------------------------------------------------
// f32 / f64 (as bits)
// ---------------------------------------------------------------------------------------------

#[verifier::external_body]
pub fn f32_add(a: u32, b: u32) -> u32 { (f32::from_bits(a) + f32::from_bits(b)).to_bits() }

#[verifier::external_body]
pub fn f32_sub(a: u32, b: u32) -> u32 { (f32::from_bits(a) - f32::from_bits(b)).to_bits() }

#[verifier::external_body]
pub fn f32_mul(a: u32, b: u32) -> u32 { (f32::from_bits(a) * f32::from_bits(b)).to_bits() }

#[verifier::external_body]
pub fn f32_div(a: u32, b: u32) -> u32 { (f32::from_bits(a) / f32::from_bits(b)).to_bits() }

#[verifier::external_body]
pub fn f32_neg(a: u32) -> u32 { (-f32::from_bits(a)).to_bits() }

#[verifier::external_body]
pub fn f32_eq(a: u32, b: u32) -> bool { f32::from_bits(a) == f32::from_bits(b) }

#[verifier::external_body]
pub fn f32_lt(a: u32, b: u32) -> bool { f32::from_bits(a) < f32::from_bits(b) }

#[verifier::external_body]
pub fn f32_le(a: u32, b: u32) -> bool { f32::from_bits(a) <= f32::from_bits(b) }

#[verifier::external_body]
pub fn f64_add(a: u64, b: u64) -> u64 { (f64::from_bits(a) + f64::from_bits(b)).to_bits() }

#[verifier::external_body]
pub fn f64_sub(a: u64, b: u64) -> u64 { (f64::from_bits(a) - f64::from_bits(b)).to_bits() }

#[verifier::external_body]
pub fn f64_mul(a: u64, b: u64) -> u64 { (f64::from_bits(a) * f64::from_bits(b)).to_bits() }

#[verifier::external_body]
pub fn f64_div(a: u64, b: u64) -> u64 { (f64::from_bits(a) / f64::from_bits(b)).to_bits() }

#[verifier::external_body]
pub fn f64_neg(a: u64) -> u64 { (-f64::from_bits(a)).to_bits() }

#[verifier::external_body]
pub fn f64_eq(a: u64, b: u64) -> bool { f64::from_bits(a) == f64::from_bits(b) }

#[verifier::external_body]
pub fn f64_lt(a: u64, b: u64) -> bool { f64::from_bits(a) < f64::from_bits(b) }

#[verifier::external_body]
pub fn f64_le(a: u64, b: u64) -> bool { f64::from_bits(a) <= f64::from_bits(b) }

// ---------------------------------------------------------------------------------------------
// Casts. Integers travel as i64 (every i8/i32 fits exactly).
// ---------------------------------------------------------------------------------------------

/// Truncating narrowing.
#[verifier::external_body]
pub fn i64_to_i8(a: i64) -> i8 { a as i8 }

#[verifier::external_body]
pub fn i64_to_i32(a: i64) -> i32 { a as i32 }

/// Round to nearest even.
#[verifier::external_body]
pub fn i64_to_f32(a: i64) -> u32 { (a as f32).to_bits() }

#[verifier::external_body]
pub fn i64_to_f64(a: i64) -> u64 { (a as f64).to_bits() }

/// Round toward zero, saturate, NaN → 0.
#[verifier::external_body]
pub fn f32_to_i32(a: u32) -> i32 { f32::from_bits(a) as i32 }

#[verifier::external_body]
pub fn f32_to_i64(a: u32) -> i64 { f32::from_bits(a) as i64 }

#[verifier::external_body]
pub fn f64_to_i32(a: u64) -> i32 { f64::from_bits(a) as i32 }

#[verifier::external_body]
pub fn f64_to_i64(a: u64) -> i64 { f64::from_bits(a) as i64 }

/// Java's `(byte) f`: saturate to `i32`, then truncate.
#[verifier::external_body]
pub fn f32_to_i8(a: u32) -> i8 { (f32::from_bits(a) as i32) as i8 }

#[verifier::external_body]
pub fn f64_to_i8(a: u64) -> i8 { (f64::from_bits(a) as i32) as i8 }

/// Exact.
#[verifier::external_body]
pub fn f32_to_f64(a: u32) -> u64 { (f32::from_bits(a) as f64).to_bits() }

/// Round to nearest even.
#[verifier::external_body]
pub fn f64_to_f32(a: u64) -> u32 { (f64::from_bits(a) as f32).to_bits() }

#[verifier::external_body]
pub fn i8_to_i64(a: i8) -> i64 { a as i64 }

#[verifier::external_body]
pub fn i32_to_i64(a: i32) -> i64 { a as i64 }

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
}
