//! Whole-program rebuilds: `compact` (canonical slot order), `permute` (shuffled slot order) and
//! `isomorphic` (structural equality up to slot tags, generations and free lists).
//!
//! Every arena is copied value by value into a fresh arena in the chosen order, then every
//! embedded `Ref` is rewritten through old → new maps. A `Ref` that doesn't resolve in the source
//! program panics.

use std::collections::HashMap;

use crate::model::*;
use crate::retag::{RefVisitor, ref_kinds, walk_program};

type RefMap<T> = HashMap<Ref<T>, Ref<T>>;

fn lookup<T>(map: &RefMap<T>, r: &mut Ref<T>, what: &str) {
    *r = *map
        .get(r)
        .unwrap_or_else(|| panic!("dangling Ref to a {what} that is not in the program"));
}

macro_rules! maps {
    ($($method:ident: $t:ty, $field:ident, $what:literal, $ord:literal;)*) => {
        /// Old → new refs, per kind.
        #[derive(Default)]
        struct Remap {
            $($field: RefMap<$t>,)*
        }

        impl RefVisitor for Remap {
            $(fn $method(&mut self, r: &mut Ref<$t>) {
                lookup(&self.$field, r, $what)
            })*
        }

        /// Gives every ref an ordinal that depends only on its slot position, and records the
        /// refs in visiting order as (kind, ordinal).
        #[derive(Default)]
        struct Fingerprint {
            $($field: HashMap<Ref<$t>, u64>,)*
            seq: Vec<(u8, u64)>,
        }

        impl RefVisitor for Fingerprint {
            $(fn $method(&mut self, r: &mut Ref<$t>) {
                let o = *self.$field.get(r).expect("dangling Ref");
                self.seq.push(($ord, o));
            })*
        }
    };
}
ref_kinds!(maps);

/// Copies `src` into `dst` in the order given by `order(len)`, recording old → new refs.
fn copy_arena<T>(
    src: &Arena<T>,
    dst: &mut Arena<T>,
    map: &mut RefMap<T>,
    order: &mut dyn FnMut(usize) -> Vec<usize>,
    mut clone: impl FnMut(&T) -> T,
) {
    let live: Vec<(Ref<T>, &T)> = src.iter().collect();
    for i in order(live.len()) {
        let (old, v) = live[i];
        map.insert(old, dst.insert(clone(v)));
    }
}

fn rebuild(p: &OptimizedProgram, order: &mut dyn FnMut(usize) -> Vec<usize>) -> OptimizedProgram {
    let mut out = OptimizedProgram {
        compute_units: Arena::new(),
        functions: Arena::new(),
        entry_points: p.entry_points.clone(),
        constants: Arena::new(),
        host_inputs: Arena::new(),
        permutation_tables: Arena::new(),
    };
    let mut m = Remap::default();

    copy_arena(
        &p.host_inputs,
        &mut out.host_inputs,
        &mut m.hosts,
        order,
        HostInput::clone,
    );
    copy_arena(
        &p.permutation_tables,
        &mut out.permutation_tables,
        &mut m.tables,
        order,
        PermutationTableInput::clone,
    );
    copy_arena(
        &p.constants,
        &mut out.constants,
        &mut m.consts,
        order,
        Constant::clone,
    );

    // Functions: the nested variable arenas are shuffled too. The shared `m.vars` map covers all.
    let fn_order = {
        let n = p.functions.iter().count();
        order(n)
    };
    let live: Vec<_> = p.functions.iter().collect();
    for i in fn_order {
        let (old, f) = live[i];
        let mut nf = f.clone_raw();
        let mut vars = Arena::new();
        copy_arena(&f.variables, &mut vars, &mut m.vars, order, Var::clone);
        nf.variables = vars;
        m.fns.insert(old, out.functions.insert(nf));
    }

    let unit_order = order(p.compute_units.iter().count());
    let live: Vec<_> = p.compute_units.iter().collect();
    for i in unit_order {
        let (old, cu) = live[i];
        let mut nu = cu.clone_raw();
        let ComputeUnit::DensityFunction {
            density_inputs: old_inputs,
            ..
        } = cu;
        let mut inputs = Arena::new();
        copy_arena(
            old_inputs,
            &mut inputs,
            &mut m.inputs,
            order,
            DensityInput::clone,
        );
        let ComputeUnit::DensityFunction { density_inputs, .. } = &mut nu;
        *density_inputs = inputs;
        m.cus.insert(old, out.compute_units.insert(nu));
    }

    walk_program(&mut out, &mut m);
    out
}

/// A copy with every arena in iteration order and without holes: all generations are 0, every
/// free list is empty, and every `Ref` is fresh. Panics on dangling refs.
pub fn compact(p: &OptimizedProgram) -> OptimizedProgram {
    rebuild(p, &mut |n| (0..n).collect())
}

/// Shuffles the slot order of every arena (hosts, tables, consts, fns, vars of each fn, units,
/// density inputs of each unit) and rewrites all refs. The program means the same afterwards.
pub fn permute(p: &mut OptimizedProgram, seed: u64) {
    let mut state = seed;
    let mut next = move || {
        // splitmix64
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let shuffled = rebuild(p, &mut |n| {
        let mut v: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            v.swap(i, (next() % (i as u64 + 1)) as usize);
        }
        v
    });
    *p = shuffled;
}

/// The bit patterns of every float in the program, in a fixed order. `Debug` and `PartialEq`
/// can't tell NaN payloads apart.
fn float_bits(p: &OptimizedProgram) -> Vec<u64> {
    fn expr(e: &Expression, out: &mut Vec<u64>) {
        match e {
            Expression::Float(v) => out.push(v.to_bits() as u64),
            Expression::Double(v) => out.push(v.to_bits()),
            Expression::FunctionCall { parameters, .. }
            | Expression::ExternCall { parameters, .. } => {
                parameters.iter().for_each(|e| expr(e, out))
            }
            Expression::DensityVariable(_, idx) => {
                if let Some(i) = idx {
                    expr(i, out)
                }
            }
            Expression::BinaryOp { left, right, .. } => {
                expr(left, out);
                expr(right, out);
            }
            Expression::UnaryOp { operand, .. } => expr(operand, out),
            Expression::Field { base, .. } => expr(base, out),
            Expression::ArrayAccess { array, index } => {
                expr(array, out);
                expr(index, out);
            }
            Expression::Construct { args, .. } | Expression::ArrayLiteral(args) => {
                args.iter().for_each(|e| expr(e, out))
            }
            Expression::ConstructExtern { args, .. } => args.iter().for_each(|(_, e)| expr(e, out)),
            Expression::ExplicitCast { expr: e, .. } => expr(e, out),
            Expression::Constant(_)
            | Expression::Variable(_)
            | Expression::Byte(_)
            | Expression::Int(_)
            | Expression::Long(_)
            | Expression::Bool(_)
            | Expression::PermutationTable(_)
            | Expression::HostInput(_)
            | Expression::Ambient(_) => {}
        }
    }
    fn cexpr(e: &ConstExpr, out: &mut Vec<u64>) {
        match e {
            ConstExpr::Float(v) => out.push(v.to_bits() as u64),
            ConstExpr::Double(v) => out.push(v.to_bits()),
            ConstExpr::BinaryOp { left, right, .. } => {
                cexpr(left, out);
                cexpr(right, out);
            }
            ConstExpr::UnaryOp { operand, .. } => cexpr(operand, out),
            ConstExpr::ExplicitCast { expr, .. } => cexpr(expr, out),
            ConstExpr::Field { base, .. } => cexpr(base, out),
            ConstExpr::ArrayAccess { array, index } => {
                cexpr(array, out);
                cexpr(index, out);
            }
            ConstExpr::ArrayLiteral(items) | ConstExpr::Construct { args: items, .. } => {
                items.iter().for_each(|e| cexpr(e, out))
            }
            ConstExpr::Constant(_)
            | ConstExpr::Byte(_)
            | ConstExpr::Int(_)
            | ConstExpr::Long(_)
            | ConstExpr::Bool(_) => {}
        }
    }
    fn stmts(body: &[Statement], out: &mut Vec<u64>) {
        for s in body {
            match s {
                Statement::Assign { value, .. } => expr(value, out),
                Statement::Return(e) => expr(e, out),
                Statement::If {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    expr(condition, out);
                    stmts(then_branch, out);
                    stmts(else_branch, out);
                }
                Statement::While { condition, body } => {
                    expr(condition, out);
                    stmts(body, out);
                }
                Statement::Repeat { body, .. } => stmts(body, out),
                Statement::Break => {}
            }
        }
    }
    let mut out = vec![];
    let triple = |t: (f64, f64, f64), out: &mut Vec<u64>| {
        out.extend([t.0.to_bits(), t.1.to_bits(), t.2.to_bits()])
    };
    for e in &p.entry_points {
        triple(e.scaled_origin, &mut out);
        triple(e.scaled_position, &mut out);
    }
    for c in p.constants.values() {
        cexpr(&c.value, &mut out);
    }
    for f in p.functions.values() {
        stmts(&f.body, &mut out);
    }
    for u in p.compute_units.values() {
        let ComputeUnit::DensityFunction { density_inputs, .. } = u;
        for d in density_inputs.values() {
            triple(d.scaled_origin, &mut out);
            triple(d.scaled_position, &mut out);
        }
    }
    out
}

/// Not cheap: it compacts the program and formats all of it. Meant for tests.
fn fingerprint(p: &OptimizedProgram) -> (String, Vec<(u8, u64)>, Vec<u64>) {
    let mut c = compact(p);
    let mut f = Fingerprint::default();
    for (i, (r, _)) in c.compute_units.iter().enumerate() {
        f.cus.insert(r, i as u64);
    }
    for (i, (r, _)) in c.functions.iter().enumerate() {
        f.fns.insert(r, i as u64);
    }
    for (i, (r, _)) in c.constants.iter().enumerate() {
        f.consts.insert(r, i as u64);
    }
    for (i, (r, _)) in c.host_inputs.iter().enumerate() {
        f.hosts.insert(r, i as u64);
    }
    for (i, (r, _)) in c.permutation_tables.iter().enumerate() {
        f.tables.insert(r, i as u64);
    }
    for (fi, (_, func)) in c.functions.iter().enumerate() {
        for (vi, (r, _)) in func.variables.iter().enumerate() {
            f.vars.insert(r, (fi as u64) << 32 | vi as u64);
        }
    }
    for (ui, (_, cu)) in c.compute_units.iter().enumerate() {
        let ComputeUnit::DensityFunction { density_inputs, .. } = cu;
        for (ii, (r, _)) in density_inputs.iter().enumerate() {
            f.inputs.insert(r, (ui as u64) << 32 | ii as u64);
        }
    }
    walk_program(&mut c, &mut f);
    let floats = float_bits(&c);
    let text = format!(
        "{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}",
        c.entry_points,
        c.host_inputs.values().collect::<Vec<_>>(),
        c.permutation_tables.values().collect::<Vec<_>>(),
        c.constants.values().collect::<Vec<_>>(),
        c.functions.values().collect::<Vec<_>>(),
        c.compute_units.values().collect::<Vec<_>>(),
    );
    (text, f.seq, floats)
}

/// Structural equality: same slot order, same values, and every ref pointing at the same slot.
/// Tags, generations and free lists are ignored. Floats compare by bit pattern. It formats both
/// programs in full, so keep it to tests.
pub fn isomorphic(a: &OptimizedProgram, b: &OptimizedProgram) -> bool {
    fingerprint(a) == fingerprint(b)
}
