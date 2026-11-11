/*
Density mappings (`specs/density-mappings.md` §3 and §5.2) and buffer reads (`specs/workers.md`
§5.1): the checked or unchecked read of an input, the inline index of an `Expand` read and the
`__device__ __forceinline__` helper of a `Lerp` read. Everything is folded from the input's dims
and factors, so no runtime symbols are involved beyond `TW_CHECK_INDEX`.
*/

use tungsten_interface::{BufferBounds, LerpHelper};
use tungsten_ir::model::{AxisFactor, DensityInput, VariableType};

use crate::model as cuda;
use crate::types::device_type;

use cuda::BinaryOperator as Op;
use cuda::Expression as C;
use cuda::Statement as S;

/// Parameter names of a lerp helper.
const POOL: &str = "pool";
const POSITION: &str = "pos";

const AXES: [&str; 3] = ["x", "y", "z"];

/// Cells of a grid. Dims below 1 occur on the unchecked path, and count as empty.
pub fn cells((x, y, z): (i32, i32, i32)) -> usize {
    [x, y, z].into_iter().fold(1usize, |n, d| n.saturating_mul(d.max(0) as usize))
}

/// A read of element `index` of the input whose cells start at `pointer` and whose length is
/// `len` (`specs/workers.md` §5.1). `PerBuffer` checks the index against the input's own length
/// with `TW_CHECK_INDEX`. `Slab` and `Unchecked` index the pointer directly: the code is shared by
/// every worker, so it has no slab length to check against.
pub fn read_pointer(bounds: BufferBounds, pointer: C, len: usize, index: C, location: &str) -> C {
    // A literal 0 into a non-empty input can't fail.
    let trivial = matches!(index, C::IntLit(0)) && len >= 1;
    let index = match bounds {
        BufferBounds::Slab | BufferBounds::Unchecked => index,
        // `PerBuffer`, and any mode added later: the safest.
        _ if trivial => index,
        _ => C::call(
            "TW_CHECK_INDEX",
            vec![index, C::IntLit(i32::try_from(len).unwrap_or(i32::MAX)), C::Raw(format!("\"{location}\""))],
        ),
    };
    C::index(pointer, index)
}

/// `pos.<axis>`, an `int`.
fn coordinate(pos: &C, axis: usize) -> C {
    C::field(pos.clone(), AXES[axis])
}

/// `c / n` of a non-negative `int`, as a shift when `n` is a power of two.
fn divide(c: C, n: i32) -> C {
    if (n as u32).is_power_of_two() {
        C::binary(Op::ShiftRight, c, C::IntLit(n.trailing_zeros() as i32))
    } else {
        C::binary(Op::Divide, c, C::IntLit(n))
    }
}

/// `c % n` of a non-negative `int`, as a mask when `n` is a power of two.
fn remainder(c: C, n: i32) -> C {
    if (n as u32).is_power_of_two() {
        C::binary(Op::BitAnd, c, C::IntLit(n - 1))
    } else {
        C::binary(Op::Modulo, c, C::IntLit(n))
    }
}

/// `sum of coordinate * stride` over the axes that have a coordinate, with unit strides folded.
fn flat_index((sx, sy, _): (i32, i32, i32), coordinates: [Option<C>; 3]) -> C {
    let clamp = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
    let (sx, sy) = (sx.max(0) as usize, sy.max(0) as usize);
    let strides = [1, clamp(sx), clamp(sx.saturating_mul(sy))];
    let mut sum: Option<C> = None;
    for (c, stride) in coordinates.into_iter().zip(strides) {
        let Some(c) = c else { continue };
        let term = if stride == 1 { c } else { C::binary(Op::Multiply, c, C::IntLit(stride)) };
        sum = Some(match sum {
            None => term,
            Some(s) => C::binary(Op::Add, s, term),
        });
    }
    sum.unwrap_or(C::IntLit(0))
}

/// The flat index of an `Expand` read of `input` by the reader at `pos` (an `int3` expression):
/// `Repeat` drops its term, `Factor(1)` reads the coordinate, `Factor(n)` divides it (§3.2).
pub fn expand_index(input: &DensityInput, factors: [AxisFactor; 3], pos: &C) -> C {
    let coordinates = [0, 1, 2].map(|a| match factors[a] {
        AxisFactor::Repeat => None,
        AxisFactor::Factor(1) => Some(coordinate(pos, a)),
        AxisFactor::Factor(n) => Some(divide(coordinate(pos, a), n)),
    });
    flat_index(input.dimensions, coordinates)
}

/// `a + t * (b - a)`, in the element type. The exact profile compiles with `--fmad=false`, so
/// nothing fuses, and there is no shortcut at `t == 0` (§3.4).
fn lerp(a: &str, b: &str, t: &str) -> C {
    let v = |x: &str| C::name(x);
    C::binary(Op::Add, v(a), C::binary(Op::Multiply, v(t), C::binary(Op::Subtract, v(b), v(a))))
}

/// `__device__ __forceinline__ T <name>(const T* pool, int3 pos)`: the loads and lerps of §3.3, in
/// order. The caller passes `tw::buffer<T>(ctx.buffers, off_<input>)` and the reader's `p.pos`.
/// Loads follow `bounds` like every read of an input.
pub fn lerp_helper(h: &LerpHelper, bounds: BufferBounds) -> cuda::Function {
    let element = device_type(&h.element);
    let mut body: Vec<cuda::Statement> = vec![];
    let mut declare = |t: &cuda::Type, name: &str, init: C| {
        body.push(S::Declare { t: t.clone(), name: name.to_string(), init: Some(init), is_const: true });
    };
    let literal = |n: i32| match h.element {
        VariableType::F32 => C::FloatLit(n as f32),
        _ => C::DoubleLit(n as f64),
    };
    let pos = C::name(POSITION);

    // Per axis: the low coordinate, and the fraction when the axis is interpolated.
    let mut low: [Option<String>; 3] = [None, None, None];
    let mut fraction: [Option<String>; 3] = [None, None, None];
    for a in 0..3 {
        let c = coordinate(&pos, a);
        match h.factors[a] {
            AxisFactor::Repeat => {}
            AxisFactor::Factor(1) => {
                let v = format!("{}0", AXES[a]);
                declare(&cuda::Type::Int, &v, c);
                low[a] = Some(v);
            }
            AxisFactor::Factor(n) => {
                let v = format!("{}0", AXES[a]);
                declare(&cuda::Type::Int, &v, divide(c.clone(), n));
                low[a] = Some(v);
                let t = format!("t{}", AXES[a]);
                // `T(p % n) / T(n)`
                declare(&element, &t, C::binary(Op::Divide, C::cast(element.clone(), remainder(c, n)), literal(n)));
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
                    let base = C::name(low[a].as_ref()?);
                    Some(if corner[a] == 1 { C::binary(Op::Add, base, C::IntLit(1)) } else { base })
                });
                let index = flat_index(h.dimensions, coordinates);
                let v = format!("v{cx}{cy}{cz}");
                declare(&element, &v, read_pointer(bounds, C::name(POOL), len, index, &h.name));
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
    body.push(S::Return(Some(C::name(&table.remove(0).1))));

    cuda::Function {
        doc: Some(format!("Lerp read of a {:?} input; see specs/density-mappings.md §3.3.", h.dimensions)),
        qualifiers: "__device__ __forceinline__".into(),
        return_type: element.clone(),
        attributes: String::new(),
        name: h.name.clone(),
        params: vec![(cuda::Type::const_ptr(element), POOL.into()), (cuda::Type::named("int3"), POSITION.into())],
        body: Some(body),
    }
}
