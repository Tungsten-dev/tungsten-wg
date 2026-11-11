/*
Density mappings (`specs/density-mappings.md` §3 and §5.2) and buffer reads (`specs/workers.md`
§5.1): the checked or unchecked read of a pool, the inline index of an `Expand` read and the
`private static` helper of a `Lerp` read. Everything is folded from the input's dims and factors,
so no runtime symbols are involved.
*/

use crate::model as java;
use crate::types::convert_type;
use tungsten_interface::{BufferBounds, LerpHelper};
use tungsten_ir::model::{AxisFactor, DensityInput, VariableType};
use tungsten_rust::pools::cells;

use java::BinaryOperator as Op;
use java::Expression as J;
use java::Statement as S;

/// Parameter names of a lerp helper.
const POOL: &str = "pool";
const OFFSET: &str = "off";
const COORDINATES: [&str; 3] = ["px", "py", "pz"];

const AXES: [&str; 3] = ["x", "y", "z"];

/// A read of element `index` of the input at `offset` in `pool`, whose length is `len`
/// (`specs/workers.md` §5.1). `PerBuffer` checks the index against the input's own length, which
/// fails like a separate array would (`IndexOutOfBoundsException`). `Slab` and `Unchecked` index the
/// pool directly, and the JVM checks its end.
pub fn read_pool(bounds: BufferBounds, pool: J, offset: J, len: usize, index: J) -> J {
    // A literal 0 into a non-empty input can't fail.
    let trivial = matches!(index, J::IntLit(0)) && len >= 1;
    let index = match bounds {
        BufferBounds::Slab | BufferBounds::Unchecked => index,
        // `PerBuffer`, and any mode added later: the safest.
        _ if trivial => index,
        _ => J::call(
            Some(J::name("java.util.Objects")),
            "checkIndex",
            vec![index, J::IntLit(i32::try_from(len).unwrap_or(i32::MAX))],
        ),
    };
    J::Index { base: Box::new(pool), index: Box::new(J::binary(Op::Add, offset, index)) }
}

/// `pos.<axis>`, an `int`.
fn coordinate(pos: &J, axis: usize) -> J {
    J::field(pos.clone(), AXES[axis])
}

/// `c / n` of a non-negative `int`, as a shift when `n` is a power of two.
fn divide(c: J, n: i32) -> J {
    if (n as u32).is_power_of_two() {
        J::binary(Op::ShiftRight, c, J::IntLit(n.trailing_zeros() as i32))
    } else {
        J::binary(Op::Divide, c, J::IntLit(n))
    }
}

/// `c % n` of a non-negative `int`, as a mask when `n` is a power of two.
fn remainder(c: J, n: i32) -> J {
    if (n as u32).is_power_of_two() {
        J::binary(Op::BitAnd, c, J::IntLit(n - 1))
    } else {
        J::binary(Op::Modulo, c, J::IntLit(n))
    }
}

/// `sum of coordinate * stride` over the axes that have a coordinate, with unit strides folded.
fn flat_index((sx, sy, _): (i32, i32, i32), coordinates: [Option<J>; 3]) -> J {
    let clamp = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
    let (sx, sy) = (sx.max(0) as usize, sy.max(0) as usize);
    let strides = [1, clamp(sx), clamp(sx.saturating_mul(sy))];
    let mut sum: Option<J> = None;
    for (c, stride) in coordinates.into_iter().zip(strides) {
        let Some(c) = c else { continue };
        let term = if stride == 1 { c } else { J::binary(Op::Multiply, c, J::IntLit(stride)) };
        sum = Some(match sum {
            None => term,
            Some(s) => J::binary(Op::Add, s, term),
        });
    }
    sum.unwrap_or(J::IntLit(0))
}

/// The flat index of an `Expand` read of `input` by the reader at `pos` (a `Pos3` expression):
/// `Repeat` drops its term, `Factor(1)` reads the coordinate, `Factor(n)` divides it (§3.2).
pub fn expand_index(input: &DensityInput, factors: [AxisFactor; 3], pos: &J) -> J {
    let coordinates = [0, 1, 2].map(|a| match factors[a] {
        AxisFactor::Repeat => None,
        AxisFactor::Factor(1) => Some(coordinate(pos, a)),
        AxisFactor::Factor(n) => Some(divide(coordinate(pos, a), n)),
    });
    flat_index(input.dimensions, coordinates)
}

/// `a + t * (b - a)`, in the element type: no fused multiply-add (Java never fuses), and no
/// shortcut at `t == 0` (§3.4).
fn lerp(a: &str, b: &str, t: &str) -> J {
    let v = |x: &str| J::name(x);
    J::binary(Op::Add, v(a), J::binary(Op::Multiply, v(t), J::binary(Op::Subtract, v(b), v(a))))
}

/// `private static T <name>(T[] pool, int off, int px, int py, int pz)`: the loads and lerps of
/// §3.3, in order. The caller passes its `off_<input>` and the reader's `pos`. Loads follow
/// `bounds` like every read of a pool.
pub fn lerp_helper(h: &LerpHelper, bounds: BufferBounds) -> java::Method {
    let element = convert_type(&h.element);
    let mut body: Vec<java::Statement> = vec![];
    let mut declare = |t: &java::Type, name: &str, init: J| {
        body.push(S::Declare { t: t.clone(), name: name.to_string(), init: Some(init), is_final: true });
    };
    let literal = |n: i32| match h.element {
        VariableType::F32 => J::FloatLit(n as f32),
        _ => J::DoubleLit(n as f64),
    };

    // Per axis: the low coordinate, and the fraction when the axis is interpolated.
    let mut low: [Option<String>; 3] = [None, None, None];
    let mut fraction: [Option<String>; 3] = [None, None, None];
    for a in 0..3 {
        let c = J::name(COORDINATES[a]);
        match h.factors[a] {
            AxisFactor::Repeat => {}
            AxisFactor::Factor(1) => {
                let v = format!("{}0", AXES[a]);
                declare(&java::Type::Int, &v, c);
                low[a] = Some(v);
            }
            AxisFactor::Factor(n) => {
                let v = format!("{}0", AXES[a]);
                declare(&java::Type::Int, &v, divide(c.clone(), n));
                low[a] = Some(v);
                let t = format!("t{}", AXES[a]);
                // `T(p % n) / T(n)`
                declare(&element, &t, J::binary(Op::Divide, J::cast(element.clone(), remainder(c, n)), literal(n)));
                fraction[a] = Some(t);
            }
        }
    }

    // The corners of the interpolated axes, as the bits (cx, cy, cz). Others stay at 0.
    let span = |a: usize| if fraction[a].is_some() { 2 } else { 1 };
    let len = cells(h.dimensions);
    let mut table: Vec<([usize; 3], String)> = vec![];
    for cz in 0..span(2) {
        for cy in 0..span(1) {
            for cx in 0..span(0) {
                let corner = [cx, cy, cz];
                let coordinates = [0, 1, 2].map(|a| {
                    let base = J::name(low[a].as_ref()?);
                    Some(if corner[a] == 1 { J::binary(Op::Add, base, J::IntLit(1)) } else { base })
                });
                let index = flat_index(h.dimensions, coordinates);
                let v = format!("v{cx}{cy}{cz}");
                declare(&element, &v, read_pool_of(bounds, len, index));
                table.push((corner, v));
            }
        }
    }

    // x first, then y, then z.
    for a in 0..3 {
        let Some(t) = &fraction[a] else { continue };
        let mut next = vec![];
        for (corner, lo) in table.iter().filter(|(c, _)| c[a] == 0) {
            let mut other = *corner;
            other[a] = 1;
            let (_, hi) = table.iter().find(|(c, _)| *c == other).unwrap();
            // Named after the axes still to go: `lx01`, `ly1`, `lz`.
            let rest: String = (a + 1..3).map(|b| corner[b].to_string()).collect();
            let v = format!("l{}{rest}", AXES[a]);
            declare(&element, &v, lerp(lo, hi, t));
            next.push((*corner, v));
        }
        table = next;
    }
    body.push(S::Return(Some(J::name(&table.remove(0).1))));

    java::Method {
        doc: Some(format!("Lerp read of a {:?} input; see specs/density-mappings.md §3.3.", h.dimensions)),
        modifiers: "private static".into(),
        return_type: Some(element.clone()),
        name: h.name.clone(),
        params: vec![
            (java::Type::Array(Box::new(element)), POOL.into()),
            (java::Type::Int, OFFSET.into()),
            (java::Type::Int, COORDINATES[0].into()),
            (java::Type::Int, COORDINATES[1].into()),
            (java::Type::Int, COORDINATES[2].into()),
        ],
        body,
    }
}

/// A read inside a lerp helper: its pool and offset are parameters.
fn read_pool_of(bounds: BufferBounds, len: usize, index: J) -> J {
    read_pool(bounds, J::name(POOL), J::name(OFFSET), len, index)
}
