//! Density mappings through lowering and the interpreter (`specs/density-mappings.md` §5.1, §8).
//! The corpus has the hand-computed cases; this checks the lowered lerp against the frontend's
//! formula, bit for bit, on edge values.

use tungsten_core::Value;

use super::eval::run_with;
use crate::EvalOptions;

/// Copied from `tungsten-frontends/tungsten-mc/src/mathf64.rs` (commit 88b88ae): `lerp`,
/// `interpolate` and the fractions of the `corner*_8` reads.
mod mc {
    fn lerp(a: f64, b: f64, t: f64) -> f64 {
        a + t * (b - a)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn interpolate(
        v000: f64,
        v100: f64,
        v010: f64,
        v110: f64,
        v001: f64,
        v101: f64,
        v011: f64,
        v111: f64,
        fx: f64,
        fy: f64,
        fz: f64,
    ) -> f64 {
        // Interpolate along X
        let x00 = lerp(v000, v100, fx);
        let x10 = lerp(v010, v110, fx);
        let x01 = lerp(v001, v101, fx);
        let x11 = lerp(v011, v111, fx);

        // Interpolate along Y
        let y0 = lerp(x00, x10, fy);
        let y1 = lerp(x01, x11, fy);

        // Interpolate along Z
        lerp(y0, y1, fz)
    }

    pub fn xfract4(x: i32) -> f64 {
        (x & 3) as f64 / 4.0
    }

    pub fn yfract8(y: i32) -> f64 {
        (y & 7) as f64 / 8.0
    }

    pub fn zfract4(z: i32) -> f64 {
        (z & 3) as f64 / 4.0
    }
}

/// 27 corner values of a `(3,3,3)` grid: inexact decimals, with signed zeros, subnormals and
/// the extremes in the low corners and an infinity and NaN only in the high ones, so some cells
/// stay finite.
fn edge_values() -> Vec<f64> {
    let finite = [0.1, -0.7, 3.0, 1e300, -1e-300, 2.5e-8, -123.456];
    let mut v: Vec<f64> = (0..27).map(|i| finite[(i * 5) % finite.len()]).collect();
    let idx = |x: usize, y: usize, z: usize| x + 3 * y + 9 * z;
    v[idx(0, 0, 0)] = -0.0;
    v[idx(1, 0, 0)] = 0.0;
    v[idx(0, 1, 0)] = 5e-324;
    v[idx(1, 1, 0)] = f64::MIN_POSITIVE;
    v[idx(0, 0, 1)] = f64::MAX;
    v[idx(1, 0, 1)] = f64::MIN;
    v[idx(2, 2, 1)] = f64::INFINITY;
    v[idx(2, 1, 2)] = f64::NEG_INFINITY;
    v[idx(2, 2, 2)] = f64::NAN;
    v
}

#[test]
fn lerp_agrees_with_frontend_interpolate() {
    let src = "program {
        host h: [f64; 27];
        fn leaf() -> f64 @hosts(h) {
            let c: pos3 = ambient(pos3);
            return host(h)[((c.x + (3 * c.y)) + (9 * c.z))];
        }
        unit leaf_u { main leaf; hosts [h]; }
        input g = leaf_u @dims(3, 3, 3) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @lerp(4, 8, 4);
        fn top() -> f64 { return density(g); }
        unit t { main top; inputs [g]; }
        entry t @dims(8, 16, 8) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    }";
    let v = edge_values();
    let host = Value::Array(v.iter().map(|x| Value::F64(x.to_bits())).collect());
    for memo in [true, false] {
        let r = run_with(src, &[("h", host.clone())], EvalOptions { memo, ..EvalOptions::default() });
        let cells = &r.entries[0];
        assert_eq!(cells.len(), 8 * 16 * 8);
        let at = |x: i32, y: i32, z: i32| v[(x + 3 * y + 9 * z) as usize];
        let mut nans = 0;
        for (k, cell) in cells.iter().enumerate() {
            let k = k as i32;
            let (x, y, z) = (k % 8, (k / 8) % 16, k / 128);
            let (gx, gy, gz) = (x >> 2, y >> 3, z >> 2);
            let want = mc::interpolate(
                at(gx, gy, gz),
                at(gx + 1, gy, gz),
                at(gx, gy + 1, gz),
                at(gx + 1, gy + 1, gz),
                at(gx, gy, gz + 1),
                at(gx + 1, gy, gz + 1),
                at(gx, gy + 1, gz + 1),
                at(gx + 1, gy + 1, gz + 1),
                mc::xfract4(x),
                mc::yfract8(y),
                mc::zfract4(z),
            );
            nans += want.is_nan() as usize;
            assert_eq!(cell, &Ok(Value::F64(want.to_bits())), "cell ({x}, {y}, {z})");
        }
        // The values reach every class: some results are NaN, some aren't.
        assert!(nans > 0 && nans < cells.len());
    }
}
