//! Grid layout: `len(d) = d.x*d.y*d.z`, `flat(c, d) = c.x + c.y*d.x + c.z*d.x*d.y` (x fastest)
//! and `decode`, its inverse on `[0, len(d))`. All arithmetic is checked; `None` means overflow,
//! a negative dimension, or a cell outside the grid (`Stuck(Layout)` in exec, unreachable after
//! the checker's `DimsTooSmall` / `GridTooLarge`).

use vstd::prelude::*;

use crate::ir::*;
use crate::value::*;

verus! {

/// A dims component as `usize`, or `None` if negative.
pub fn dim(x: i32) -> Option<usize> {
    if x < 0 {
        None
    } else {
        Some(x as usize)
    }
}

pub fn len(d: &Dims) -> Option<usize> {
    let x = dim(d.x)?;
    let y = dim(d.y)?;
    let z = dim(d.z)?;
    let xy = x.checked_mul(y)?;
    xy.checked_mul(z)
}

pub fn is_unit(d: &Dims) -> bool {
    d.x == 1 && d.y == 1 && d.z == 1
}

pub fn flat(c: &I32x3, d: &Dims) -> Option<usize> {
    if c.x < 0 || c.y < 0 || c.z < 0 || c.x >= d.x || c.y >= d.y || c.z >= d.z {
        return None;
    }
    let dx = dim(d.x)?;
    let dy = dim(d.y)?;
    let x = c.x as usize;
    let y = c.y as usize;
    let z = c.z as usize;
    let ydx = y.checked_mul(dx)?;
    let dxy = dx.checked_mul(dy)?;
    let zdxy = z.checked_mul(dxy)?;
    let s = x.checked_add(ydx)?;
    s.checked_add(zdxy)
}

/// A non-negative `usize` below `bound` (itself an `i32`) as `i32`.
fn to_i32(v: usize, bound: i32) -> Option<i32> {
    if bound < 0 || v >= bound as usize {
        None
    } else {
        Some(v as i32)
    }
}

pub fn decode(k: usize, d: &Dims) -> Option<I32x3> {
    let dx = dim(d.x)?;
    let dy = dim(d.y)?;
    if dx == 0 || dy == 0 {
        return None;
    }
    let x = k % dx;
    let rest = k / dx;
    let y = rest % dy;
    let z = rest / dy;
    let x = to_i32(x, d.x)?;
    let y = to_i32(y, d.y)?;
    let z = to_i32(z, d.z)?;
    Some(I32x3 { x, y, z })
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    fn dims(x: i32, y: i32, z: i32) -> Dims {
        Dims { x, y, z }
    }

    #[test]
    fn decode_flat_roundtrip() {
        for d in [dims(1, 1, 1), dims(4, 1, 4), dims(3, 5, 2), dims(2, 2, 2)] {
            let n = len(&d).unwrap();
            for k in 0..n {
                let c = decode(k, &d).unwrap();
                assert_eq!(flat(&c, &d), Some(k));
            }
            assert_eq!(decode(n, &d), None, "past the end");
        }
        let d = dims(3, 5, 2);
        let c = I32x3 { x: 2, y: 4, z: 1 };
        assert_eq!(decode(flat(&c, &d).unwrap(), &d), Some(c));
        assert_eq!(flat(&I32x3 { x: 1, y: 0, z: 0 }, &d), Some(1), "x varies fastest");
        assert_eq!(flat(&I32x3 { x: 0, y: 1, z: 0 }, &d), Some(3));
        assert_eq!(flat(&I32x3 { x: 0, y: 0, z: 1 }, &d), Some(15));
    }

    #[test]
    fn edges() {
        assert_eq!(len(&dims(0, 4, 4)), Some(0));
        assert_eq!(len(&dims(-1, 4, 4)), None);
        assert_eq!(len(&dims(i32::MAX, i32::MAX, i32::MAX)).is_some(), (i32::MAX as usize).checked_mul(i32::MAX as usize).and_then(|v| v.checked_mul(i32::MAX as usize)).is_some());
        assert_eq!(flat(&I32x3 { x: 4, y: 0, z: 0 }, &dims(4, 1, 1)), None);
        assert_eq!(flat(&I32x3 { x: -1, y: 0, z: 0 }, &dims(4, 1, 1)), None);
        assert_eq!(decode(0, &dims(0, 1, 1)), None);
    }
}
