//! What generated code calls by plain name: the registry externs (`registry.toml`), the toy
//! tables (SPEC §5.3) and `make_boxed`. Re-exported through `mathf64`, so the two hardcoded glob
//! imports of the generated code are enough. Thin wrappers over `externs.rs` and `utilsf64.rs`,
//! which the interpreter shares.
#![allow(dead_code)]

use crate::externs as x;
use crate::mathf64::Vec3;
use crate::utilsf64 as u;

/// Marker in the panic message of a failing extern, mapped to protocol kind 2 by the driver.
pub const DOMAIN_PANIC: &str = "tw:ExternDomain";

#[derive(Clone)]
pub struct PerlinNoiseSampler {
    pub perm: [u8; 256],
}

#[derive(Clone)]
pub struct InterpolatedNoiseSampler {
    pub state: u64,
}

pub fn make_permutation_table(
    seed: i64,
    id_lo: i64,
    id_hi: i64,
    subident_index: i64,
    sub_lo: i64,
    sub_hi: i64,
) -> PerlinNoiseSampler {
    let st = u::perlin_state(seed as u64, id_lo, id_hi, subident_index, sub_lo, sub_hi);
    PerlinNoiseSampler { perm: u::make_perm(st) }
}

pub fn make_base3d_perm_table(seed: i64) -> InterpolatedNoiseSampler {
    InterpolatedNoiseSampler { state: u::base3d_state(seed as u64) }
}

pub fn make_salt(seed: i64, name: &str) -> i64 {
    u::toy_salt(seed, name)
}

pub fn make_bits(x: f64) -> i64 {
    x.to_bits() as i64
}

pub fn make_scale(base: f64, flip: bool) -> f64 {
    if flip { -base } else { base }
}

pub fn make_flag(on: bool, n: i64) -> bool {
    on && n > 0
}

pub fn make_i8(v: i64) -> i8 {
    v as i8
}

pub fn make_i32(v: i64) -> i32 {
    v as i32
}

pub fn make_f32(v: f64) -> f32 {
    v as f32
}

/// Zero-initialised, so buffers are deterministic. Unlike the upstream runtime, which hands out
/// uninitialised memory. Never builds `T` on the stack.
pub fn make_boxed<T>() -> Box<T> {
    use std::alloc::{Layout, alloc_zeroed, handle_alloc_error};
    let layout = Layout::new::<T>();
    unsafe {
        if layout.size() == 0 {
            return Box::new(std::mem::zeroed());
        }
        let p = alloc_zeroed(layout) as *mut T;
        if p.is_null() {
            handle_alloc_error(layout);
        }
        Box::from_raw(p)
    }
}

pub fn perm_get(t: &PerlinNoiseSampler, i: i32) -> i32 {
    u::perm_lookup(&t.perm, i)
}

pub fn sampler_sample(s: &InterpolatedNoiseSampler, v: Vec3) -> f64 {
    u::sample_state(s.state, [v.x, v.y, v.z])
}

pub fn test_domain(x_: i32) -> i32 {
    match x::test_domain(x_) {
        Some(v) => v,
        None => panic!("{}", DOMAIN_PANIC),
    }
}

pub fn sqrt(a: f64) -> f64 { a.sqrt() }
pub fn floor(a: f64) -> f64 { a.floor() }
pub fn ceil(a: f64) -> f64 { a.ceil() }
pub fn trunc(a: f64) -> f64 { a.trunc() }
pub fn abs(a: f64) -> f64 { a.abs() }
pub fn min(a: f64, b: f64) -> f64 { x::min_f64(a, b) }
pub fn max(a: f64, b: f64) -> f64 { x::max_f64(a, b) }
pub fn copysign(a: f64, b: f64) -> f64 { a.copysign(b) }
pub fn fma(a: f64, b: f64, c: f64) -> f64 { a.mul_add(b, c) }
pub fn sqrt_f32(a: f32) -> f32 { a.sqrt() }
pub fn floor_f32(a: f32) -> f32 { a.floor() }
pub fn ceil_f32(a: f32) -> f32 { a.ceil() }
pub fn trunc_f32(a: f32) -> f32 { a.trunc() }
pub fn abs_f32(a: f32) -> f32 { a.abs() }
pub fn min_f32(a: f32, b: f32) -> f32 { x::min_f32(a, b) }
pub fn max_f32(a: f32, b: f32) -> f32 { x::max_f32(a, b) }
pub fn copysign_f32(a: f32, b: f32) -> f32 { a.copysign(b) }
pub fn fma_f32(a: f32, b: f32, c: f32) -> f32 { a.mul_add(b, c) }
pub fn float_to_bits(a: f32) -> i32 { x::float_to_bits(a) }
pub fn double_to_bits(a: f64) -> i64 { x::double_to_bits(a) }
pub fn bits_to_float(a: i32) -> f32 { x::bits_to_float(a) }
pub fn bits_to_double(a: i64) -> f64 { x::bits_to_double(a) }
