//! Toy permutation tables and samplers (see the crate README, "Toy tables"). Self-contained: this file is also copied
//! into generated builds, so it must not use `crate::` paths.
#![allow(dead_code)]

const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// splitmix64 as a pure function of its input: `mix(x + GAMMA)`.
pub fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(GAMMA);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `acc = splitmix64(acc ^ a)` over the arguments, starting from the first one.
pub fn fold(first: u64, rest: &[u64]) -> u64 {
    rest.iter().fold(first, |acc, a| splitmix64(acc ^ a))
}

/// Perlin state: fold over `(seed, id_lo, id_hi, subident_index, sub_lo, sub_hi)`.
pub fn perlin_state(seed: u64, id_lo: i64, id_hi: i64, subident_index: i64, sub_lo: i64, sub_hi: i64) -> u64 {
    fold(seed, &[id_lo as u64, id_hi as u64, subident_index as u64, sub_lo as u64, sub_hi as u64])
}

/// Base3D state: `splitmix64(seed)`.
pub fn base3d_state(seed: u64) -> u64 {
    splitmix64(seed)
}

/// Fisher-Yates over `0..256`, drawing from the splitmix64 sequence of `state`.
pub fn make_perm(state: u64) -> [u8; 256] {
    let mut perm = [0u8; 256];
    for (i, p) in perm.iter_mut().enumerate() {
        *p = i as u8;
    }
    let mut s = state;
    for i in (1..256usize).rev() {
        s = s.wrapping_add(GAMMA);
        let r = splitmix64(s.wrapping_sub(GAMMA));
        perm.swap(i, (r % (i as u64 + 1)) as usize);
    }
    perm
}

pub fn perm_lookup(perm: &[u8; 256], i: i32) -> i32 {
    perm[(i & 255) as usize] as i32
}

fn f2i(x: f64) -> u64 {
    (x as i32) as u32 as u64
}

/// `(h >>> 11) as f64 * 2^-53`, `h = fold(state, floor(v) as i32 bits)`.
pub fn sample_state(state: u64, v: [f64; 3]) -> f64 {
    let h = fold(state, &[f2i(v[0].floor()), f2i(v[1].floor()), f2i(v[2].floor())]);
    (h >> 11) as f64 * (1.0 / 9007199254740992.0)
}

// ----- toy autoconst initializers (registry.toml `auto_initializer`) -----

/// FNV-1a over the UTF-8 bytes of `name`, seeded with `seed`.
pub fn toy_salt(seed: i64, name: &str) -> i64 {
    let mut h = (seed as u64) ^ 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h as i64
}
