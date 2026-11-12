//! Lowering one refine call to an ordinary core program (`specs/refinement.md` §7.2).
//!
//! The step program is the normal lowering with these changes:
//!
//! - Every `Ambient(Pos3)`, including the ones the mapping desugaring produces, is
//!   `Pos3 + Host(offset)`.
//! - A read of an input listed by a running unit whose instance is frozen, or that is
//!   `@previous`, is `Index(Host(buffer), index)`. Without an index it uses `flat` over the raw
//!   `Pos3`, or 0 for a `(1,1,1)` input. Those inputs leave every unit's input list.
//! - A rerun input whose rerun has a `with` unit gets that unit as its source.
//! - The entries are the running instances, the refinement's entry first.
//! - The new hosts follow the IR's hosts and the value autoconsts, and join the scoping lists
//!   of every unit and function.

use std::collections::HashMap;

use tungsten_core::{Ambient, Axis, BinOp, Expr, FieldSel, HostId, Lit, Type};
use tungsten_ir::{ComputeUnit, DensityInput, Program as IrProgram, Ref};

use super::mapping::{listed_inputs, unit_return_type};
use super::refine::{Inst, RefineGraph, Run};
use super::{Ctx, get};

/// What a refine step changes, for the walker.
pub(super) struct StepCfg {
    pub offset: HostId,
    /// Inputs read from a buffer host: frozen ones and `@previous` ones.
    pub buffers: HashMap<Ref<DensityInput>, HostId>,
    /// Rerun inputs whose rerun runs a `with` unit.
    pub sources: HashMap<Ref<DensityInput>, Ref<ComputeUnit>>,
    /// The offset host, then one host per buffer instance.
    pub hosts: Vec<HostId>,
    /// By buffer host, after the offset: the instance it holds.
    pub buffer_insts: Vec<Inst>,
    pub running: Vec<Run>,
}

impl StepCfg {
    /// The step of `program.refinements[refinement]`, with new hosts from `first_host`. `None`
    /// if its entry doesn't exist.
    pub fn new(p: &IrProgram, refinement: usize, first_host: usize) -> Option<Self> {
        let graph = RefineGraph::new(p);
        let walk = graph.walks.into_iter().find(|w| w.index == refinement)?;
        let offset = HostId(first_host);
        let mut buffers = HashMap::new();
        let mut sources = HashMap::new();
        let mut buffer_insts: Vec<Inst> = vec![];
        let mut hosts = vec![offset];
        for run in &walk.running {
            for (r, d) in listed_inputs(p, run.unit) {
                let inst = Inst::of_input(d);
                let rerun = walk.running.iter().find(|x| x.target == inst);
                match rerun {
                    Some(x) if !d.previous => {
                        if x.unit != x.target.cu {
                            sources.insert(r, x.unit);
                        }
                    }
                    _ => {
                        let k = match buffer_insts.iter().position(|b| *b == inst) {
                            Some(k) => k,
                            None => {
                                buffer_insts.push(inst);
                                hosts.push(HostId(first_host + buffer_insts.len()));
                                buffer_insts.len() - 1
                            }
                        };
                        buffers.insert(r, HostId(first_host + 1 + k));
                    }
                }
            }
        }
        Some(Self { offset, buffers, sources, hosts, buffer_insts, running: walk.running })
    }
}

impl<'p, 'r> Ctx<'p, 'r> {
    /// The decls of the step's own hosts: the offset, then the buffers.
    pub(super) fn step_hosts(&mut self) -> Vec<tungsten_core::HostDecl> {
        let p = self.p;
        let Some(cfg) = &self.step else { return vec![] };
        let insts = cfg.buffer_insts.clone();
        let mut out = vec![tungsten_core::HostDecl { ty: Type::Pos3 }];
        for inst in insts {
            let elem = match unit_return_type(p, inst.cu) {
                Some(t) => self.ty(t),
                None => Type::Bool,
            };
            let n = (inst.dims.0.max(0) as usize) * (inst.dims.1.max(0) as usize) * (inst.dims.2.max(0) as usize);
            out.push(tungsten_core::HostDecl { ty: Type::Array(Box::new(elem), n) });
        }
        out
    }

    /// Display names of the step's hosts, for diagnostics.
    pub(super) fn step_host_names(&self) -> Vec<String> {
        let Some(cfg) = &self.step else { return vec![] };
        let mut out = vec!["refine offset".to_string()];
        for i in &cfg.buffer_insts {
            let unit = self.map.units.id(i.cu).and_then(|u| self.map.units.name(u)).unwrap_or("?");
            out.push(format!("buffer of `{unit}` at {:?}", i.dims));
        }
        out
    }

    /// A read of `input`, as `Density` or, in a step, from its buffer host.
    pub(super) fn read(&self, r: Ref<DensityInput>, input: tungsten_core::InputId, index: Option<Expr>) -> Expr {
        let Some(h) = self.step.as_ref().and_then(|c| c.buffers.get(&r)) else {
            return Expr::Density { input, index: index.map(Box::new) };
        };
        let index = index.unwrap_or_else(|| match get(&self.p.density_inputs, r) {
            Some(d) if d.dimensions != (1, 1, 1) => raw_flat(d.dimensions),
            _ => Expr::Lit(Lit::I32(0)),
        });
        Expr::Index { array: Box::new(Expr::Host(*h)), index: Box::new(index) }
    }
}

/// `x + y*dx + z*dx*dy` over the unmarched `Pos3`: the kernel's buffer index.
fn raw_flat(d: (i32, i32, i32)) -> Expr {
    let field = |axis| Expr::Field { base: Box::new(Expr::Ambient(Ambient::Pos3)), sel: FieldSel::Pos3(axis), ty: Type::I32 };
    let bin = |op, l, r| Expr::Binary { op, left: Box::new(l), right: Box::new(r) };
    let y = bin(BinOp::Multiply, field(Axis::Y), Expr::Lit(Lit::I32(d.0)));
    let z = bin(BinOp::Multiply, field(Axis::Z), Expr::Lit(Lit::I32(d.0.wrapping_mul(d.1))));
    bin(BinOp::Add, bin(BinOp::Add, field(Axis::X), y), z)
}
