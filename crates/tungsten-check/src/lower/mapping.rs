//! Density-input mappings (`specs/density-mappings.md`): the shape rules (§4.1), the reader-grid
//! rule (§4.2) and the desugaring of un-indexed mapped reads into core expressions (§5.1).

use std::collections::HashSet;

use tungsten_core::{Ambient, Axis, BinOp, Expr, FieldSel, InputId, Lit, Type};
use tungsten_ir::{
    AxisFactor, ComputeUnit, DensityInput, DensityMapping, Expression, OptimizedProgram, Ref, Statement,
    VariableType,
};

use super::{Ctx, get};
use crate::error::{Item, LowerErrorKind};

pub(super) const AXES: [char; 3] = ['x', 'y', 'z'];

pub(super) fn arr(d: (i32, i32, i32)) -> [i32; 3] {
    [d.0, d.1, d.2]
}

/// One grid a unit runs on: its dims and the largest `Pos3` offset it sees there (0 outside a
/// refinement).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ReaderPair {
    pub unit: Ref<ComputeUnit>,
    pub dims: (i32, i32, i32),
    pub march: (i32, i32, i32),
}

/// The return type of the main function of `unit`, if every ref resolves.
pub(super) fn unit_return_type(p: &OptimizedProgram, unit: Ref<ComputeUnit>) -> Option<&VariableType> {
    let ComputeUnit::DensityFunction { function, .. } = get(&p.compute_units, unit)?;
    get(&p.functions, *function).map(|f| &f.return_type)
}

/// The inputs a unit lists and resolves, in list order, without duplicates.
pub(super) fn listed_inputs<'p>(
    p: &'p OptimizedProgram,
    unit: Ref<ComputeUnit>,
) -> Vec<(Ref<DensityInput>, &'p DensityInput)> {
    let Some(ComputeUnit::DensityFunction { density_inputs, .. }) = get(&p.compute_units, unit) else {
        return vec![];
    };
    let mut seen = HashSet::new();
    density_inputs
        .iter()
        .filter(|r| seen.insert(**r))
        .filter_map(|&r| Some((r, get(&p.density_inputs, r)?)))
        .collect()
}

/// The inputs that `unit`'s main function or one of its helpers reads, split by whether a read
/// has an explicit index.
pub(super) fn unit_reads(p: &OptimizedProgram, unit: Ref<ComputeUnit>) -> Reads {
    let mut reads = Reads::default();
    let Some(ComputeUnit::DensityFunction { function, helper_functions, .. }) = get(&p.compute_units, unit)
    else {
        return reads;
    };
    for f in std::iter::once(function).chain(helper_functions) {
        if let Some(f) = get(&p.functions, *f) {
            reads.stmts(&f.body);
        }
    }
    reads
}

#[derive(Default)]
pub(super) struct Reads {
    pub unindexed: HashSet<Ref<DensityInput>>,
    pub indexed: HashSet<Ref<DensityInput>>,
}

impl Reads {
    fn stmts(&mut self, body: &[Statement]) {
        for s in body {
            match s {
                Statement::Assign { value, .. } | Statement::Return(value) => self.expr(value),
                Statement::If { condition, then_branch, else_branch } => {
                    self.expr(condition);
                    self.stmts(then_branch);
                    self.stmts(else_branch);
                }
                Statement::While { condition, body } => {
                    self.expr(condition);
                    self.stmts(body);
                }
                Statement::Repeat { body, .. } => self.stmts(body),
                Statement::Break => {}
            }
        }
    }

    fn expr(&mut self, e: &Expression) {
        match e {
            Expression::DensityVariable(r, index) => match index {
                None => {
                    self.unindexed.insert(*r);
                }
                Some(i) => {
                    self.indexed.insert(*r);
                    self.expr(i);
                }
            },
            Expression::FunctionCall { parameters, .. } | Expression::ExternCall { parameters, .. } => {
                parameters.iter().for_each(|a| self.expr(a))
            }
            Expression::BinaryOp { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expression::UnaryOp { operand, .. } => self.expr(operand),
            Expression::Field { base, .. } => self.expr(base),
            Expression::ArrayAccess { array, index } => {
                self.expr(array);
                self.expr(index);
            }
            Expression::Construct { args, .. } | Expression::ArrayLiteral(args) => {
                args.iter().for_each(|a| self.expr(a))
            }
            Expression::ConstructExtern { args, .. } => args.iter().for_each(|(_, a)| self.expr(a)),
            Expression::ExplicitCast { expr, .. } => self.expr(expr),
            Expression::Constant(_)
            | Expression::Variable(_)
            | Expression::Float(_)
            | Expression::Double(_)
            | Expression::Byte(_)
            | Expression::Int(_)
            | Expression::Long(_)
            | Expression::Bool(_)
            | Expression::AutoConst(_)
            | Expression::HostInput(_)
            | Expression::Ambient(_) => {}
        }
    }
}

/// Every `(reading unit, dims, march)` reachable from the entries and from the refinements, in
/// discovery order. The normal graph comes first, with march 0. `@previous` inputs add no edge.
/// Pairs with dims below 1 are kept, and callers skip them.
pub(super) fn reader_pairs(p: &OptimizedProgram) -> Vec<ReaderPair> {
    let mut out: Vec<ReaderPair> = vec![];
    let mut seen: HashSet<ReaderPair> = HashSet::new();
    let push = |out: &mut Vec<ReaderPair>, seen: &mut HashSet<ReaderPair>, pair: ReaderPair| {
        if seen.insert(pair) {
            out.push(pair);
        }
    };

    // Normal roots: the entries, then the extra instances `run` computes for refinements
    // (`specs/refinement.md` §4.5).
    let mut roots: Vec<(Ref<ComputeUnit>, (i32, i32, i32))> =
        p.entry_points.iter().map(|e| (e.density_function, e.dimensions)).collect();
    let refine = super::refine::RefineGraph::new(p);
    for r in &refine.walks {
        for frozen in &r.frozen_reads {
            roots.push((frozen.cu, frozen.dims));
        }
        for t in &r.with_targets {
            roots.push((t.cu, t.dims));
        }
    }

    let mut i = out.len();
    for (unit, dims) in roots {
        push(&mut out, &mut seen, ReaderPair { unit, dims, march: (0, 0, 0) });
    }
    while i < out.len() {
        let pair = out[i];
        i += 1;
        for (_, d) in listed_inputs(p, pair.unit) {
            if d.previous {
                continue;
            }
            push(&mut out, &mut seen, ReaderPair { unit: d.density_function, dims: d.dimensions, march: (0, 0, 0) });
        }
    }

    // Running units of each refinement, at their target's dims and the refinement's march.
    for r in &refine.walks {
        for run in &r.running {
            push(&mut out, &mut seen, ReaderPair { unit: run.unit, dims: run.target.dims, march: r.march });
        }
    }
    out
}

impl<'p, 'r> Ctx<'p, 'r> {
    /// §4.1, for one input. Runs with the input as the current item.
    pub(super) fn mapping_shape(&mut self, d: &DensityInput) {
        let factors = match d.mapping {
            DensityMapping::Identity => return,
            DensityMapping::Expand(f) | DensityMapping::Lerp(f) => f,
        };
        let size = arr(d.dimensions);
        for (k, f) in factors.iter().enumerate() {
            match *f {
                AxisFactor::Factor(n) if n < 1 => {
                    self.err(LowerErrorKind::MappingBadFactor { axis: AXES[k], factor: n })
                }
                AxisFactor::Repeat if size[k] != 1 => {
                    self.err(LowerErrorKind::MappingRepeatOnWideAxis { axis: AXES[k], size: size[k] })
                }
                _ => {}
            }
        }
        if let DensityMapping::Lerp(_) = d.mapping
            && let Some(t) = unit_return_type(self.p, d.density_function)
            && !matches!(t, VariableType::F32 | VariableType::F64)
        {
            self.err(LowerErrorKind::MappingNotFloat { t: format!("{t:?}") });
        }
    }

    /// §4.2: every un-indexed read of a listed input by a reachable `(unit, dims, march)`.
    /// One error per `(input, reader dims)`, inputs in arena order, readers in discovery order.
    pub(super) fn mapping_readers(&mut self) {
        let p = self.p;
        let pairs: Vec<ReaderPair> = reader_pairs(p)
            .into_iter()
            .filter(|r| r.dims.0 >= 1 && r.dims.1 >= 1 && r.dims.2 >= 1)
            .collect();
        let reads: Vec<HashSet<Ref<DensityInput>>> =
            pairs.iter().map(|pair| unit_reads(p, pair.unit).unindexed).collect();
        let lists: Vec<Vec<Ref<DensityInput>>> =
            pairs.iter().map(|pair| listed_inputs(p, pair.unit).into_iter().map(|(r, _)| r).collect()).collect();
        for (i, (r, d)) in p.density_inputs.iter().enumerate() {
            if d.previous {
                continue;
            }
            let mut reported: HashSet<(i32, i32, i32)> = HashSet::new();
            for (k, pair) in pairs.iter().enumerate() {
                if !lists[k].contains(&r) || !reads[k].contains(&r) || reported.contains(&pair.dims) {
                    continue;
                }
                if let Some(kind) = self.reader_error(d, pair) {
                    reported.insert(pair.dims);
                    let name = self.map.inputs.name(InputId(i)).unwrap_or_default().to_string();
                    self.enter(Item::Input { id: Some(InputId(i)), name });
                    self.err(kind);
                }
            }
        }
    }

    fn reader_error(&self, d: &DensityInput, pair: &ReaderPair) -> Option<LowerErrorKind> {
        let unit = self.map.units.id(pair.unit).and_then(|u| self.map.units.name(u)).unwrap_or_default().to_string();
        let s = arr(d.dimensions);
        let r = arr(pair.dims);
        let m = arr(pair.march);
        let factors = match d.mapping {
            DensityMapping::Identity => {
                let k = (0..3).find(|&k| s[k] != r[k])?;
                return Some(LowerErrorKind::MappingDimsMismatch { unit, axis: AXES[k], input: s[k], reader: r[k] });
            }
            DensityMapping::Expand(f) => f.map(|f| (f, false)),
            DensityMapping::Lerp(f) => f.map(|f| (f, true)),
        };
        for k in 0..3 {
            let AxisFactor::Factor(n) = factors[k].0 else { continue };
            if n < 1 {
                continue;
            }
            let (n, last) = (n as i64, r[k] as i64 - 1 + m[k] as i64);
            let high = if factors[k].1 && n > 1 { last / n + 1 } else { last / n };
            if high >= s[k] as i64 {
                return Some(LowerErrorKind::MappingOutOfRange {
                    unit,
                    axis: AXES[k],
                    input: s[k],
                    reader: r[k],
                    march: m[k],
                });
            }
        }
        None
    }

    /// The core expression for an un-indexed read of `input` (§5.1). `None` keeps the plain
    /// `Density { index: None }`.
    pub(super) fn desugar_read(&mut self, r: Ref<DensityInput>, input: InputId) -> Option<Expr> {
        let p = self.p;
        let d = get(&p.density_inputs, r)?;
        let s = arr(d.dimensions);
        let (factors, lerp) = match d.mapping {
            DensityMapping::Identity => return None,
            DensityMapping::Expand(f) if f.iter().all(|a| *a == AxisFactor::Repeat) => return None,
            DensityMapping::Expand(f) => (f, None),
            DensityMapping::Lerp(f) => {
                let t = match unit_return_type(p, d.density_function) {
                    Some(VariableType::F32) => Float::F32,
                    Some(VariableType::F64) => Float::F64,
                    // `MappingNotFloat`, reported with the input.
                    _ => return Some(placeholder()),
                };
                (f, Some(t))
            }
        };
        if factors.iter().any(|a| matches!(a, AxisFactor::Factor(n) if *n < 1)) {
            // `MappingBadFactor`, reported with the input.
            return Some(placeholder());
        }
        let m = Mapped { cx: self, r, input, s, f: factors };
        Some(match lerp {
            None => m.load([0, 1, 2].map(|k| match factors[k] {
                AxisFactor::Repeat => Coord::Zero,
                AxisFactor::Factor(n) => Coord::Low(n),
            })),
            Some(t) => m.lerp(t, 2, [Coord::Zero; 3]),
        })
    }

    /// The reader's `Pos3`. Inside a refine step this is the marched position.
    pub(super) fn marched_pos3(&self) -> Expr {
        match &self.step {
            None => Expr::Ambient(Ambient::Pos3),
            Some(cfg) => Expr::Binary {
                op: BinOp::Add,
                left: Box::new(Expr::Ambient(Ambient::Pos3)),
                right: Box::new(Expr::Host(cfg.offset)),
            },
        }
    }
}

#[derive(Clone, Copy)]
enum Float {
    F32,
    F64,
}

impl Float {
    fn ty(self) -> Type {
        match self {
            Float::F32 => Type::F32,
            Float::F64 => Type::F64,
        }
    }

    fn lit(self, n: i32) -> Expr {
        Expr::Lit(match self {
            Float::F32 => Lit::F32((n as f32).to_bits()),
            Float::F64 => Lit::F64((n as f64).to_bits()),
        })
    }
}

/// An input coordinate on one axis.
#[derive(Clone, Copy)]
enum Coord {
    Zero,
    /// `p / n`, or `p` for `n == 1`.
    Low(i32),
    /// `p / n + 1`.
    High(i32),
}

struct Mapped<'a, 'p, 'r> {
    cx: &'a Ctx<'p, 'r>,
    r: Ref<DensityInput>,
    input: InputId,
    s: [i32; 3],
    f: [AxisFactor; 3],
}

impl Mapped<'_, '_, '_> {
    fn pos(&self, k: usize) -> Expr {
        let axis = match k {
            0 => Axis::X,
            1 => Axis::Y,
            _ => Axis::Z,
        };
        Expr::Field { base: Box::new(self.cx.marched_pos3()), sel: FieldSel::Pos3(axis), ty: Type::I32 }
    }

    fn coord(&self, k: usize, c: Coord) -> Option<Expr> {
        match c {
            Coord::Zero => None,
            Coord::Low(n) => Some(div(self.pos(k), n)),
            Coord::High(n) => Some(bin(BinOp::Add, div(self.pos(k), n), Expr::Lit(Lit::I32(1)))),
        }
    }

    /// `buf[flat(X, Y, Z)]`. Zero coordinates drop out of the sum.
    fn load(&self, c: [Coord; 3]) -> Expr {
        let s = self.s;
        let terms = [
            self.coord(0, c[0]),
            self.coord(1, c[1]).map(|y| mul(y, s[0])),
            self.coord(2, c[2]).map(|z| mul(z, s[0] * s[1])),
        ];
        let index = terms
            .into_iter()
            .flatten()
            .reduce(|a, b| bin(BinOp::Add, a, b))
            .unwrap_or(Expr::Lit(Lit::I32(0)));
        self.cx.read(self.r, self.input, Some(index))
    }

    /// The §3.3 formula. Axes `axis` down to x are still open; `fixed` holds the coordinates
    /// chosen above. So x is lerped first, then y, then z.
    fn lerp(&self, t: Float, axis: isize, fixed: [Coord; 3]) -> Expr {
        if axis < 0 {
            return self.load(fixed);
        }
        let k = axis as usize;
        let at = |c: Coord| {
            let mut next = fixed;
            next[k] = c;
            self.lerp(t, axis - 1, next)
        };
        match self.f[k] {
            AxisFactor::Repeat => at(Coord::Zero),
            AxisFactor::Factor(1) => at(Coord::Low(1)),
            AxisFactor::Factor(n) => {
                // T(p % n) / T(n), with p % n written as p - (p / n) * n
                let rem = bin(BinOp::Subtract, self.pos(k), mul(div(self.pos(k), n), n));
                let frac = bin(BinOp::Divide, cast(t.ty(), rem), t.lit(n));
                // a + t * (b - a), with `a` written out twice
                let b_minus_a = bin(BinOp::Subtract, at(Coord::High(n)), at(Coord::Low(n)));
                bin(BinOp::Add, at(Coord::Low(n)), bin(BinOp::Multiply, frac, b_minus_a))
            }
        }
    }
}

fn placeholder() -> Expr {
    Expr::Lit(Lit::Bool(false))
}

fn bin(op: BinOp, l: Expr, r: Expr) -> Expr {
    Expr::Binary { op, left: Box::new(l), right: Box::new(r) }
}

fn div(e: Expr, n: i32) -> Expr {
    if n == 1 { e } else { bin(BinOp::Divide, e, Expr::Lit(Lit::I32(n))) }
}

fn mul(e: Expr, n: i32) -> Expr {
    if n == 1 { e } else { bin(BinOp::Multiply, e, Expr::Lit(Lit::I32(n))) }
}

fn cast(t: Type, e: Expr) -> Expr {
    Expr::Cast { to: t, operand: Box::new(e) }
}
