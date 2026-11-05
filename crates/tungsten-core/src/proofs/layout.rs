//! T3: `flat` and `decode` are inverses on in-range cells.

use vstd::prelude::*;
use vstd::arithmetic::div_mod::*;
use vstd::arithmetic::mul::*;

use crate::ir::*;
use crate::spec::layout::*;
use crate::value::*;

verus! {

/// `decode(flat(c, d), d) == c`.
pub proof fn lemma_decode_flat(c: I32x3, d: Dims)
    requires
        d.x >= 1, d.y >= 1, d.z >= 1,
        spec_flat(c, d) is Some,
    ensures
        spec_decode(spec_flat(c, d)->0, d) == Some(c),
{
    reveal(spec_decode);
    let (dx, dy) = (d.x as int, d.y as int);
    let (x, y, z) = (c.x as int, c.y as int, c.z as int);
    let k = spec_flat(c, d)->0 as int;
    let rest = y + z * dy;
    assert(k == dx * rest + x) by (nonlinear_arith)
        requires k == x + y * dx + z * (dx * dy), rest == y + z * dy;
    assert(rest >= 0) by (nonlinear_arith) requires rest == y + z * dy, y >= 0, z >= 0, dy >= 1;
    lemma_fundamental_div_mod_converse(k, dx, rest, x);
    assert(rest == dy * z + y) by (nonlinear_arith) requires rest == y + z * dy;
    lemma_fundamental_div_mod_converse(rest, dy, z, y);
}

/// `k < len(d)` decodes to a cell in range, and `flat` takes it back to `k`.
pub proof fn lemma_flat_decode(k: usize, d: Dims)
    requires
        d.x >= 1, d.y >= 1, d.z >= 1,
        spec_len(d) is Some,
        (k as int) < spec_len(d)->0,
    ensures
        spec_decode(k, d) is Some,
        spec_flat(spec_decode(k, d)->0, d) == Some(k),
        0 <= spec_decode(k, d)->0.x < d.x,
        0 <= spec_decode(k, d)->0.y < d.y,
        0 <= spec_decode(k, d)->0.z < d.z,
{
    reveal(spec_decode);
    let (dx, dy, dz) = (d.x as int, d.y as int, d.z as int);
    let ki = k as int;
    let x = ki % dx;
    let rest = ki / dx;
    let y = rest % dy;
    let z = rest / dy;
    lemma_fundamental_div_mod(ki, dx);
    lemma_fundamental_div_mod(rest, dy);
    lemma_mod_pos_bound(ki, dx);
    lemma_div_pos_is_pos(ki, dx);
    lemma_mod_pos_bound(rest, dy);
    lemma_div_pos_is_pos(rest, dy);
    assert(ki == x + y * dx + z * (dx * dy)) by (nonlinear_arith)
        requires ki == dx * rest + x, rest == dy * z + y;
    assert(z < dz) by (nonlinear_arith)
        requires
            ki == x + y * dx + z * (dx * dy),
            ki < dx * dy * dz,
            x >= 0, y >= 0, dx >= 1, dy >= 1,
    {
        if z >= dz {
            assert(z * (dx * dy) >= dz * (dx * dy));
            assert(y * dx >= 0);
        }
    }
    assert(0 <= y * dx) by (nonlinear_arith) requires y >= 0, dx >= 1;
    assert(0 <= z * (dx * dy)) by (nonlinear_arith) requires z >= 0, dx >= 1, dy >= 1;
}

} // verus!
