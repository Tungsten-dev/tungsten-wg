//! Extern functions of `registry.toml`, with Java semantics where Rust differs. Self-contained.
#![allow(dead_code)]

pub fn min_f64(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == 0.0 && b == 0.0 {
        if a.is_sign_negative() { a } else { b }
    } else if a < b {
        a
    } else {
        b
    }
}

pub fn max_f64(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == 0.0 && b == 0.0 {
        if a.is_sign_negative() { b } else { a }
    } else if a > b {
        a
    } else {
        b
    }
}

pub fn min_f32(a: f32, b: f32) -> f32 {
    min_f64(a as f64, b as f64) as f32
}

pub fn max_f32(a: f32, b: f32) -> f32 {
    max_f64(a as f64, b as f64) as f32
}

pub fn float_to_bits(x: f32) -> i32 {
    if x.is_nan() { 0x7fc0_0000 } else { x.to_bits() as i32 }
}

pub fn double_to_bits(x: f64) -> i64 {
    if x.is_nan() { 0x7ff8_0000_0000_0000 } else { x.to_bits() as i64 }
}

pub fn bits_to_float(b: i32) -> f32 {
    f32::from_bits(b as u32)
}

pub fn bits_to_double(b: i64) -> f64 {
    f64::from_bits(b as u64)
}

/// `None` is the domain error.
pub fn test_domain(x: i32) -> Option<i32> {
    if x < 0 { None } else { Some(x) }
}
