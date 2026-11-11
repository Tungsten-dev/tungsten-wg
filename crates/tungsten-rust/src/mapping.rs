/*
Density mappings (`specs/density-mappings.md` §3 and §5.2): the inline index of an `Expand` read and
the helper function of a `Lerp` read. Everything is folded from the input's dims and factors, so
no runtime symbols are involved.
*/

use std::rc::Rc;

use crate::expression::read_pool;
use crate::model as rcl;
use crate::pools::cells;
use crate::types::convert_type;
use tungsten_interface::{BackendError, BufferBounds, Interface, LerpHelper};
use tungsten_ir::model::{AxisFactor, DensityInput, DensityMapping, OptimizedProgram, VariableType};

/// Parameter names of a lerp helper.
const POOL: &str = "pool";
const OFFSET: &str = "off";
const POS: &str = "p";

const AXES: [&str; 3] = ["x", "y", "z"];

/// What generated code can't express at all: a factor below 1, and a lerp of a non-float element.
/// Everything else is legal, even when the checked path would have rejected it (§5.3).
pub fn check_emittable(program: &OptimizedProgram, interface: &Interface) -> Result<(), BackendError> {
    for (r, d) in program.density_inputs.iter() {
        let name = interface.offset_param(r).trim_start_matches("off_");
        let (DensityMapping::Expand(factors) | DensityMapping::Lerp(factors)) = d.mapping else {
            continue;
        };
        if let Some(n) = factors.iter().find_map(|f| match f {
            AxisFactor::Factor(n) if *n < 1 => Some(n),
            _ => None,
        }) {
            return Err(BackendError::Codegen(format!(
                "density input `{name}` has mapping factor {n}, which is below 1"
            )));
        }
        if matches!(d.mapping, DensityMapping::Lerp(_)) {
            let element = crate::pools::unit_return_type(program, d.density_function);
            if !matches!(element, VariableType::F32 | VariableType::F64) {
                return Err(BackendError::Codegen(format!(
                    "density input `{name}` is lerped, but its element type {element:?} is not f32 or f64"
                )));
            }
        }
    }
    Ok(())
}

/// `pos.<axis>`, an `i32`.
fn coordinate<'m>(pos: &rcl::Expression<'m>, axis: usize) -> rcl::Expression<'m> {
    rcl::Expression::Field { base: Box::new(pos.clone()), field: AXES[axis].into() }
}

fn bin<'m>(op: rcl::BinaryOperator, l: rcl::Expression<'m>, r: rcl::Expression<'m>) -> rcl::Expression<'m> {
    rcl::Expression::BinaryOp { op, left: Box::new(l), right: Box::new(r) }
}

fn cast<'m>(e: rcl::Expression<'m>, t: rcl::Type) -> rcl::Expression<'m> {
    rcl::Expression::Cast { expr: Box::new(e), to_type: t }
}

/// `c / n` of a non-negative `i32`, as a shift when `n` is a power of two.
fn divide<'m>(c: rcl::Expression<'m>, n: i32) -> rcl::Expression<'m> {
    if (n as u32).is_power_of_two() {
        bin(rcl::BinaryOperator::RightShift, c, rcl::Expression::I32Literal(n.trailing_zeros() as i32))
    } else {
        bin(rcl::BinaryOperator::Divide, c, rcl::Expression::I32Literal(n))
    }
}

/// `c % n` of a non-negative `i32`, as a mask when `n` is a power of two.
fn remainder<'m>(c: rcl::Expression<'m>, n: i32) -> rcl::Expression<'m> {
    if (n as u32).is_power_of_two() {
        bin(rcl::BinaryOperator::BitwiseAnd, c, rcl::Expression::I32Literal(n - 1))
    } else {
        bin(rcl::BinaryOperator::Modulo, c, rcl::Expression::I32Literal(n))
    }
}

/// `sum of coordinate * stride` over the axes that have a coordinate, with unit strides folded.
fn flat_index<'m>(
    (sx, sy, _): (i32, i32, i32),
    coordinates: [Option<rcl::Expression<'m>>; 3],
) -> rcl::Expression<'m> {
    let strides = [1, sx.max(0) as usize, (sx.max(0) as usize).saturating_mul(sy.max(0) as usize)];
    let mut sum: Option<rcl::Expression<'m>> = None;
    for (c, stride) in coordinates.into_iter().zip(strides) {
        let Some(c) = c else { continue };
        let term = if stride == 1 {
            c
        } else {
            bin(rcl::BinaryOperator::Multiply, c, rcl::Expression::UsizeLiteral(stride))
        };
        sum = Some(match sum {
            None => term,
            Some(s) => bin(rcl::BinaryOperator::Add, s, term),
        });
    }
    sum.unwrap_or(rcl::Expression::UsizeLiteral(0))
}

/// The flat index of an `Expand` read of `input` by the reader at `pos` (a `Pos3` expression):
/// `Repeat` drops its term, `Factor(1)` reads the coordinate, `Factor(n)` divides it (§3.2).
pub fn expand_index<'m>(
    input: &DensityInput,
    factors: [AxisFactor; 3],
    pos: &rcl::Expression<'m>,
) -> rcl::Expression<'m> {
    let coordinates = [0, 1, 2].map(|a| match factors[a] {
        AxisFactor::Repeat => None,
        AxisFactor::Factor(1) => Some(cast(coordinate(pos, a), rcl::Type::Usize)),
        AxisFactor::Factor(n) => Some(cast(divide(coordinate(pos, a), n), rcl::Type::Usize)),
    });
    flat_index(input.dimensions, coordinates)
}

fn var(name: &str, t: rcl::Type) -> Rc<rcl::Variable> {
    Rc::new(rcl::Variable { name: Some(name.to_string()), t, mutable: false })
}

/// `a + t * (b - a)`, in the element type: no fused multiply-add, no shortcut at `t == 0` (§3.4).
fn lerp<'m>(a: &Rc<rcl::Variable>, b: &Rc<rcl::Variable>, t: &Rc<rcl::Variable>) -> rcl::Expression<'m> {
    use rcl::{BinaryOperator as Op, Expression as E};
    let v = |x: &Rc<rcl::Variable>| E::Variable(x.clone());
    bin(Op::Add, v(a), bin(Op::Multiply, v(t), bin(Op::Subtract, v(b), v(a))))
}

/// `fn <name>(pool: &[T], off: usize, p: Pos3) -> T`: the loads and lerps of §3.3, in order. The
/// caller passes its `off_<input>`. Loads follow `bounds` like every read of a pool.
pub fn lerp_helper<'m>(h: &LerpHelper, bounds: BufferBounds) -> rcl::Function<'m> {
    use rcl::{BinaryOperator as Op, Expression as E, Statement as S};
    let element = convert_type(&h.element);
    let mut f = rcl::Function::new(Some(h.name.clone()), Some(element.clone()));
    f.inline = true;
    f.inline_always = true;
    f.add_parameter(POOL.into(), rcl::Type::Ref(Box::new(rcl::Type::Slice(Box::new(element.clone())))));
    f.add_parameter(OFFSET.into(), rcl::Type::Usize);
    f.add_parameter(POS.into(), rcl::Type::Struct("Pos3".into()));
    let pos = E::Variable(var(POS, rcl::Type::Struct("Pos3".into())));
    let declare = |f: &mut rcl::Function<'m>, v: &Rc<rcl::Variable>, init: E<'m>| {
        f.add_statement(S::Declare { variable: v.clone(), init: Some(init), mutable: false });
    };
    let literal = |n: i32| match h.element {
        VariableType::F32 => E::F32Literal(n as f32),
        _ => E::F64Literal(n as f64),
    };

    // Per axis: the low coordinate, and the fraction when the axis is interpolated.
    let mut low: [Option<Rc<rcl::Variable>>; 3] = [None, None, None];
    let mut fraction: [Option<Rc<rcl::Variable>>; 3] = [None, None, None];
    for a in 0..3 {
        let c = coordinate(&pos, a);
        match h.factors[a] {
            AxisFactor::Repeat => {}
            AxisFactor::Factor(1) => {
                let v = var(&format!("{}0", AXES[a]), rcl::Type::Usize);
                declare(&mut f, &v, cast(c, rcl::Type::Usize));
                low[a] = Some(v);
            }
            AxisFactor::Factor(n) => {
                let v = var(&format!("{}0", AXES[a]), rcl::Type::Usize);
                declare(&mut f, &v, cast(divide(c.clone(), n), rcl::Type::Usize));
                low[a] = Some(v);
                let t = var(&format!("t{}", AXES[a]), element.clone());
                // `T(p % n) / T(n)`
                declare(&mut f, &t, bin(Op::Divide, cast(remainder(c, n), element.clone()), literal(n)));
                fraction[a] = Some(t);
            }
        }
    }

    // The corners of the interpolated axes, as the bits (cx, cy, cz). Others stay at 0.
    let span = |a: usize| if fraction[a].is_some() { 2 } else { 1 };
    let len = cells(h.dimensions);
    let mut table: Vec<((usize, usize, usize), Rc<rcl::Variable>)> = vec![];
    for cz in 0..span(2) {
        for cy in 0..span(1) {
            for cx in 0..span(0) {
                let corner = [cx, cy, cz];
                let coordinates = [0, 1, 2].map(|a| {
                    let base = low[a].as_ref().map(|v| E::Variable(v.clone()))?;
                    Some(if corner[a] == 1 { bin(Op::Add, base, E::UsizeLiteral(1)) } else { base })
                });
                let index = flat_index(h.dimensions, coordinates);
                let pool = E::Deref(Box::new(E::Variable(var(POOL, rcl::Type::Void))));
                let offset = E::Variable(var(OFFSET, rcl::Type::Usize));
                let v = var(&format!("v{cx}{cy}{cz}"), element.clone());
                declare(&mut f, &v, read_pool(bounds, pool, offset, len, index));
                table.push(((cx, cy, cz), v));
            }
        }
    }

    // x first, then y, then z.
    for a in 0..3 {
        let Some(t) = &fraction[a] else { continue };
        let key = |c: &(usize, usize, usize)| [c.0, c.1, c.2];
        let mut next = vec![];
        for (corner, lo) in table.iter().filter(|(c, _)| key(c)[a] == 0) {
            let mut other = key(corner);
            other[a] = 1;
            let (_, hi) = table.iter().find(|(c, _)| key(c) == other).unwrap();
            // Named after the axes still to go: `lx01`, `ly1`, `lz`.
            let rest: String = (a + 1..3).map(|b| key(corner)[b].to_string()).collect();
            let v = var(&format!("l{}{rest}", AXES[a]), element.clone());
            declare(&mut f, &v, lerp(lo, hi, t));
            next.push((*corner, v));
        }
        table = next;
    }
    f.add_statement(S::Return(Some(E::Variable(table.remove(0).1))));
    f
}
