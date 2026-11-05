//! Grid layout (SEMANTICS.md §3): the same steps as `crate::layout`, including its intermediate
//! overflow checks, because `x * y` can overflow with `z == 0`.

use vstd::prelude::*;

use crate::ir::*;
use crate::value::*;

verus! {

pub open spec fn spec_fits(n: int) -> bool {
    0 <= n <= usize::MAX
}

pub open spec fn spec_len(d: Dims) -> Option<usize> {
    if d.x < 0 || d.y < 0 || d.z < 0 {
        None
    } else if !spec_fits(d.x * d.y) || !spec_fits(d.x * d.y * d.z) {
        None
    } else {
        Some((d.x * d.y * d.z) as usize)
    }
}

pub open spec fn spec_is_unit(d: Dims) -> bool {
    d.x == 1 && d.y == 1 && d.z == 1
}

pub open spec fn spec_flat(c: I32x3, d: Dims) -> Option<usize> {
    if c.x < 0 || c.y < 0 || c.z < 0 || c.x >= d.x || c.y >= d.y || c.z >= d.z {
        None
    } else if !spec_fits(c.y * d.x) || !spec_fits(d.x * d.y) || !spec_fits(c.z * (d.x * d.y))
        || !spec_fits(c.x + c.y * d.x) || !spec_fits(c.x + c.y * d.x + c.z * (d.x * d.y)) {
        None
    } else {
        Some((c.x + c.y * d.x + c.z * (d.x * d.y)) as usize)
    }
}

/// `x = k % dx`, `y = (k / dx) % dy`, `z = k / dx / dy`, each of which must be in its range.
/// Opaque, so that its division and modulus stay out of proofs that never look inside it.
#[verifier::opaque]
pub open spec fn spec_decode(k: usize, d: Dims) -> Option<I32x3> {
    if d.x <= 0 || d.y <= 0 {
        None
    } else {
        let x = k as int % d.x as int;
        let rest = k as int / d.x as int;
        let y = rest % d.y as int;
        let z = rest / d.y as int;
        if d.z < 0 || z >= d.z {
            None
        } else {
            Some(I32x3 { x: x as i32, y: y as i32, z: z as i32 })
        }
    }
}

} // verus!
