//! Items: id assignment, then hosts, constants, functions, units, inputs and entries in walk
//! order (see README.md).

use std::collections::{HashMap, HashSet};

use tungsten_core as core;
use tungsten_core::{ConstId, FnId, HostId, InputId, LocalId, TableId, UnitId};
use tungsten_ir::{
    AutoConst, AutoConstArg, ComputeUnit, DensityInput, EntryPoint, Function, HostInput, Name,
    Program as IrProgram, VariableType,
};

use super::refine::{Inst, RefineGraph};
use super::step::StepCfg;
use super::{Ctx, get};
use crate::error::{EntityKind, Item, LowerError, LowerErrorKind, ParameterProblem, Step};
use crate::map::{AutoArgSpec, AutoConstInfo, AutoCoreId, AutoParamId, IdMap, Instance, LowerMap};
use crate::registry::{AutoArgType, Registry};
use super::const_to_expr;

fn name_str(n: &Name) -> Option<&str> {
    match n {
        Name::Named(s) | Name::Prefixed(s) => Some(s),
        Name::Anonymous => None,
    }
}

/// What one lowering produced, without the extern/record tables (the caller copies those from
/// the registry).
pub(crate) struct Output {
    pub program: core::Program,
    pub map: LowerMap,
    pub errors: Vec<LowerError>,
    /// By core entry: the instance it computes.
    pub instances: Vec<Instance>,
    /// A refine step only: its offset host and buffer hosts, with the instance each holds.
    pub offset: Option<HostId>,
    pub buffers: Vec<(HostId, Instance)>,
}

/// Lowers `p` as the normal program: the entries, then a hidden entry per instance a refine call
/// reads from `run` that no entry computes (`specs/refinement.md` §7.1).
pub(crate) fn run(p: &IrProgram, reg: &Registry) -> Output {
    lower_with(p, reg, None)
}

/// Lowers refine call `refinement` of `p`, which lowered without errors (§7.2).
pub(crate) fn run_step(p: &IrProgram, reg: &Registry, refinement: usize) -> Output {
    lower_with(p, reg, Some(refinement))
}

fn lower_with(p: &IrProgram, reg: &Registry, refinement: Option<usize>) -> Output {
    let mut cx = Ctx {
        p,
        reg,
        map: build_ids(p),
        errors: vec![],
        item: Item::Registry,
        path: vec![],
        func: None,
        locals: HashMap::new(),
        handles: reg.handle_names.clone(),
        auto_reads: vec![],
        step: None,
    };
    cx.auto_params();
    let (tables, auto_hosts) = cx.auto_consts();
    let mut hosts = cx.hosts();
    hosts.extend(auto_hosts);
    if let Some(r) = refinement {
        cx.step = StepCfg::new(p, r, hosts.len());
        hosts.extend(cx.step_hosts());
        cx.map.step_host_names = cx.step_host_names();
    }
    let consts = cx.consts();
    let functions = cx.functions();
    let units = cx.units();
    let inputs = cx.inputs();
    let (entries, instances) = cx.entries();
    if refinement.is_none() {
        cx.refinements();
        cx.mapping_readers();
    }
    let (offset, buffers) = match &cx.step {
        Some(cfg) => (
            Some(cfg.offset),
            cfg.buffer_insts.iter().enumerate().map(|(k, i)| (cfg.hosts[k + 1], cx.instance(i))).collect(),
        ),
        None => (None, vec![]),
    };
    let program = core::Program {
        units,
        functions,
        consts,
        hosts,
        tables,
        inputs,
        entries,
        externs: vec![],
        records: vec![],
    };
    cx.map.handle_types = std::mem::take(&mut cx.handles);
    Output { program, map: cx.map, errors: cx.errors, instances, offset, buffers }
}

/// Ids in arena iteration order, with display names.
fn build_ids(p: &IrProgram) -> LowerMap {
    let mut m = LowerMap::default();
    for (i, (r, f)) in p.functions.iter().enumerate() {
        let name = f.canonical_name.clone().unwrap_or_else(|| format!("#{i}"));
        m.functions.push(r, name);
    }
    for (i, (r, u)) in p.compute_units.iter().enumerate() {
        let ComputeUnit::DensityFunction { function, .. } = u;
        let name = get(&p.functions, *function)
            .and_then(|f| f.canonical_name.clone())
            .unwrap_or_else(|| format!("#{i}"));
        m.units.push(r, name);
    }
    for (r, c) in p.constants.iter() {
        m.consts.push(r, c.name.clone());
    }
    for (i, (r, h)) in p.host_inputs.iter().enumerate() {
        let (name, named) = match h {
            HostInput::Static { name, .. } => (
                name_str(name).map(str::to_string),
                match name {
                    Name::Named(s) => Some(s.clone()),
                    _ => None,
                },
            ),
            HostInput::Dynamic => (None, None),
        };
        m.hosts.push(r, name.unwrap_or_else(|| format!("#{i}")));
        m.host_names.push(named);
    }
    for (r, p) in p.auto_params.iter() {
        m.auto_params.push(r, p.name.clone());
        m.auto_param_types.push(p.t.clone());
    }
    for (i, (r, d)) in p.density_inputs.iter().enumerate() {
        let name = name_str(&d.name)
            .map(str::to_string)
            .unwrap_or_else(|| format!("#{i}"));
        m.inputs.push(r, name);
    }
    m
}

fn contains_handle(t: &VariableType) -> bool {
    match t {
        VariableType::Handle(_) => true,
        VariableType::Array(e, _) => contains_handle(e),
        _ => false,
    }
}

fn dims(d: (i32, i32, i32)) -> core::Dims {
    core::Dims {
        x: d.0,
        y: d.1,
        z: d.2,
    }
}

fn f64x3(v: (f64, f64, f64)) -> core::F64x3 {
    core::F64x3 {
        x: v.0.to_bits(),
        y: v.1.to_bits(),
        z: v.2.to_bits(),
    }
}

impl<'p, 'r> Ctx<'p, 'r> {
    fn id_or_dangling<I: Copy>(&mut self, id: Option<I>, kind: EntityKind, placeholder: I) -> I {
        id.unwrap_or_else(|| {
            self.err(LowerErrorKind::DanglingRef { kind });
            placeholder
        })
    }

    pub(super) fn fn_ref(&mut self, r: tungsten_ir::Ref<Function>) -> FnId {
        let id = self.map.functions.id(r);
        self.id_or_dangling(id, EntityKind::Function, FnId(0))
    }

    pub(super) fn host_ref(&mut self, r: tungsten_ir::Ref<HostInput>) -> HostId {
        let id = self.map.hosts.id(r);
        self.id_or_dangling(id, EntityKind::HostInput, HostId(0))
    }

    pub(super) fn const_ref(&mut self, r: tungsten_ir::Ref<tungsten_ir::Constant>) -> ConstId {
        let id = self.map.consts.id(r);
        self.id_or_dangling(id, EntityKind::Constant, ConstId(0))
    }

    pub(super) fn table_ref(&mut self, r: tungsten_ir::Ref<AutoConst>) -> TableId {
        let id = self.map.tables.id(r);
        self.id_or_dangling(id, EntityKind::AutoConst, TableId(0))
    }

    /// Checks the auto params. They have no core counterpart.
    fn auto_params(&mut self) {
        let p = self.p;
        let mut seen = HashSet::new();
        for (_, a) in p.auto_params.iter() {
            self.enter(Item::AutoParam { name: a.name.clone() });
            if !matches!(a.t, VariableType::I64 | VariableType::F64 | VariableType::Bool) {
                self.err(LowerErrorKind::BadAutoParamType { t: format!("{:?}", a.t) });
            }
            if !seen.insert(a.name.as_str()) {
                self.err(LowerErrorKind::DuplicateAutoParam(a.name.clone()));
            }
        }
    }

    /// One core table per handle autoconst and one extra core host per value autoconst, both in
    /// arena order. Also fills `map.auto_consts`.
    fn auto_consts(&mut self) -> (Vec<core::TableDecl>, Vec<core::HostDecl>) {
        let p = self.p;
        let mut tables = vec![];
        let mut hosts = vec![];
        let real_hosts = p.host_inputs.iter().count();
        for (r, a) in p.auto_consts.iter() {
            let is_handle = matches!(a.t, VariableType::Handle(_));
            let is_value = matches!(
                a.t,
                VariableType::Bool
                    | VariableType::I8
                    | VariableType::I32
                    | VariableType::I64
                    | VariableType::F32
                    | VariableType::F64
            );
            let table = TableId(tables.len());
            self.enter(Item::AutoConst { id: is_handle.then_some(table), name: a.name.clone() });
            let core = if is_handle {
                let ty = self.ty(&a.t);
                let core::Type::Handle(ty) = ty else { unreachable!("handles lower to handles") };
                self.map.tables.push(r, a.name.clone());
                tables.push(core::TableDecl { ty });
                AutoCoreId::Table(table)
            } else if is_value {
                let ty = self.ty(&a.t);
                let h = HostId(real_hosts + hosts.len());
                self.map.auto_hosts.insert(r, h);
                self.map.auto_host_names.push(a.name.clone());
                self.map.host_names.push(None);
                hosts.push(core::HostDecl { ty });
                AutoCoreId::Host(h)
            } else {
                self.err(LowerErrorKind::BadAutoConstType { t: format!("{:?}", a.t) });
                AutoCoreId::Table(table)
            };
            let args = a
                .args
                .iter()
                .enumerate()
                .map(|(k, arg)| match arg {
                    AutoConstArg::I64(v) => AutoArgSpec::I64(*v),
                    AutoConstArg::F64(v) => AutoArgSpec::F64(*v),
                    AutoConstArg::Bool(v) => AutoArgSpec::Bool(*v),
                    AutoConstArg::Str(v) => AutoArgSpec::Str(v.clone()),
                    AutoConstArg::Param(r) => {
                        let id = self.map.auto_params.id(*r);
                        AutoArgSpec::Param(self.at(Step::Arg(k), |cx| {
                            cx.id_or_dangling(id, EntityKind::AutoParam, AutoParamId(0))
                        }))
                    }
                })
                .collect();
            if is_handle || is_value {
                self.check_initializer(a);
            }
            self.map.auto_consts.push(AutoConstInfo {
                core,
                name: a.name.clone(),
                t: a.t.clone(),
                initializer: a.initializer.clone(),
                args,
            });
        }
        (tables, hosts)
    }

    /// The registry checks of one autoconst: the initializer exists, and its arguments and return
    /// type fit.
    fn check_initializer(&mut self, a: &AutoConst) {
        let reg = self.reg;
        let Some(decl) = reg.initializer(&a.initializer) else {
            self.err(LowerErrorKind::UnknownAutoInitializer(a.initializer.clone()));
            return;
        };
        let init = a.initializer.clone();
        if decl.params.len() != a.args.len() {
            self.err(LowerErrorKind::AutoArgCount {
                initializer: init.clone(),
                expected: decl.params.len(),
                found: a.args.len(),
            });
        } else {
            for (index, (arg, &expected)) in a.args.iter().zip(&decl.params).enumerate() {
                let found = match arg {
                    AutoConstArg::I64(_) => Some(AutoArgType::I64),
                    AutoConstArg::F64(_) => Some(AutoArgType::F64),
                    AutoConstArg::Bool(_) => Some(AutoArgType::Bool),
                    AutoConstArg::Str(_) => Some(AutoArgType::Str),
                    // A dangling param is reported with the arguments.
                    AutoConstArg::Param(r) => self.map.auto_params.id(*r).and_then(|id| {
                        match self.map.auto_param_types[id.0] {
                            VariableType::I64 => Some(AutoArgType::I64),
                            VariableType::F64 => Some(AutoArgType::F64),
                            VariableType::Bool => Some(AutoArgType::Bool),
                            _ => None,
                        }
                    }),
                };
                let dangling = matches!(arg, AutoConstArg::Param(r) if self.map.auto_params.id(*r).is_none());
                if dangling {
                    continue;
                }
                if found != Some(expected) {
                    let found = match arg {
                        AutoConstArg::Param(r) => self
                            .map
                            .auto_params
                            .id(*r)
                            .map(|id| format!("{:?}", self.map.auto_param_types[id.0]))
                            .unwrap_or_default(),
                        _ => found.map(|f| f.to_string()).unwrap_or_default(),
                    };
                    self.at(Step::Arg(index), |cx| {
                        cx.err(LowerErrorKind::AutoArgType {
                            index,
                            initializer: init.clone(),
                            expected: expected.to_string(),
                            found,
                        })
                    });
                }
                if matches!(arg, AutoConstArg::Str(s) if s.contains('\0')) {
                    self.at(Step::Arg(index), |cx| {
                        cx.err(LowerErrorKind::AutoArgHasNul { index })
                    });
                }
            }
        }
        let declared = self.ty(&a.t);
        if declared != decl.ret {
            let returns = self.type_text(&decl.ret);
            self.err(LowerErrorKind::AutoInitializerReturn {
                initializer: init,
                returns,
                declared: format!("{:?}", a.t),
            });
        }
    }

    /// A core type as text, with handles by name.
    fn type_text(&self, t: &core::Type) -> String {
        match t {
            core::Type::Handle(h) => format!("handle {}", self.handles.get(h.0).map(String::as_str).unwrap_or("?")),
            core::Type::Bool => "bool".into(),
            core::Type::I8 => "i8".into(),
            core::Type::I32 => "i32".into(),
            core::Type::I64 => "i64".into(),
            core::Type::F32 => "f32".into(),
            core::Type::F64 => "f64".into(),
            other => format!("{other:?}"),
        }
    }

    fn input_ref(&mut self, r: tungsten_ir::Ref<DensityInput>) -> InputId {
        let id = self.map.inputs.id(r);
        self.id_or_dangling(id, EntityKind::DensityInput, InputId(0))
    }

    fn unit_ref(&mut self, r: tungsten_ir::Ref<ComputeUnit>) -> UnitId {
        let id = self.map.units.id(r);
        self.id_or_dangling(id, EntityKind::ComputeUnit, UnitId(0))
    }

    fn hosts(&mut self) -> Vec<core::HostDecl> {
        let p = self.p;
        let mut out = vec![];
        for (i, (_, h)) in p.host_inputs.iter().enumerate() {
            let id = HostId(i);
            let name = self.map.hosts.name(id).unwrap_or_default().to_string();
            self.enter(Item::Host { id: Some(id), name });
            let ty = match h {
                HostInput::Static { t, .. } => {
                    if contains_handle(t) {
                        self.err(LowerErrorKind::HandleHost { t: format!("{t:?}") });
                    }
                    self.ty(t)
                }
                HostInput::Dynamic => {
                    self.err(LowerErrorKind::DynamicHost);
                    core::Type::Bool
                }
            };
            out.push(core::HostDecl { ty });
        }
        out
    }

    fn consts(&mut self) -> Vec<core::ConstDecl> {
        let p = self.p;
        self.func = None;
        self.locals.clear();
        let mut out = vec![];
        for (i, (_, c)) in p.constants.iter().enumerate() {
            self.enter(Item::Constant {
                id: Some(ConstId(i)),
                name: c.name.clone(),
            });
            let init = self.expr(&const_to_expr(&c.value));
            out.push(core::ConstDecl { init });
        }
        out
    }

    fn functions(&mut self) -> Vec<core::Function> {
        let p = self.p;
        let mut out = vec![];
        for (i, (_, f)) in p.functions.iter().enumerate() {
            out.push(self.function(FnId(i), f));
        }
        self.func = None;
        self.locals.clear();
        out
    }

    fn function(&mut self, id: FnId, f: &'p Function) -> core::Function {
        let name = self.map.functions.name(id).unwrap_or_default().to_string();
        self.enter(Item::Function {
            id: Some(id),
            name,
        });
        self.func = Some(f);
        self.locals.clear();

        let mut locals: IdMap<tungsten_ir::Var, LocalId> = IdMap::default();
        let mut types = vec![];
        let mut seen = HashSet::new();
        for (i, &r) in f.parameters.iter().enumerate() {
            self.path = vec![Step::Param(i)];
            let Some(v) = get(&f.variables, r) else {
                self.err(LowerErrorKind::Parameter {
                    problem: ParameterProblem::NotInArena,
                });
                continue;
            };
            if !seen.insert(r) {
                self.err(LowerErrorKind::Parameter {
                    problem: ParameterProblem::Duplicate,
                });
                continue;
            }
            let t = self.ty(&v.t);
            let name = name_str(&v.name)
                .map(str::to_string)
                .unwrap_or_else(|| format!("#{}", types.len()));
            locals.push(r, name);
            types.push(t);
        }
        let params = types.len();
        for (r, v) in f.variables.iter() {
            if seen.contains(&r) {
                continue;
            }
            self.path = vec![Step::Local(types.len())];
            let t = self.ty(&v.t);
            let name = name_str(&v.name)
                .map(str::to_string)
                .unwrap_or_else(|| format!("#{}", types.len()));
            locals.push(r, name);
            types.push(t);
        }
        self.path.clear();
        self.locals = locals.iter().map(|(id, r, _)| (r, id)).collect();

        let ret = self.at(Step::Ret, |cx| cx.ty(&f.return_type));
        let hosts = f
            .host_inputs
            .iter()
            .enumerate()
            .map(|(i, &h)| self.at(Step::List { list: "hosts", index: i }, |cx| cx.host_ref(h)))
            .collect();
        self.auto_reads.clear();
        let body = self.block(&f.body, Step::Stmt);
        let mut hosts: Vec<HostId> = hosts;
        for h in std::mem::take(&mut self.auto_reads) {
            if !hosts.contains(&h) {
                hosts.push(h);
            }
        }
        if let Some(cfg) = &self.step {
            hosts.extend(cfg.hosts.iter().copied());
        }
        self.map.locals.push(locals);
        core::Function {
            params,
            locals: types,
            ret,
            hosts,
            body,
        }
    }

    fn units(&mut self) -> Vec<core::Unit> {
        let p = self.p;
        let mut out = vec![];
        for (i, (_, u)) in p.compute_units.iter().enumerate() {
            let id = UnitId(i);
            let name = self.map.units.name(id).unwrap_or_default().to_string();
            self.enter(Item::Unit { id: Some(id), name });
            let ComputeUnit::DensityFunction {
                function,
                density_inputs,
                auto_consts,
                host_inputs,
                helper_functions,
                constants,
                ..
            } = u;
            let main = self.at(Step::Main, |cx| cx.fn_ref(*function));
            let buffered: Vec<bool> = density_inputs
                .iter()
                .map(|r| self.step.as_ref().is_some_and(|c| c.buffers.contains_key(r)))
                .collect();
            let inputs = density_inputs
                .iter()
                .enumerate()
                .filter(|(i, _)| !buffered[*i])
                .map(|(i, &r)| self.at(Step::List { list: "inputs", index: i }, |cx| cx.input_ref(r)))
                .collect();
            let mut tables = vec![];
            let mut auto_hosts = vec![];
            for (i, &r) in auto_consts.iter().enumerate() {
                match self.map.auto_hosts.get(&r) {
                    Some(&h) => auto_hosts.push(h),
                    None => {
                        tables.push(self.at(Step::List { list: "autos", index: i }, |cx| cx.table_ref(r)))
                    }
                }
            }
            let mut hosts: Vec<HostId> = host_inputs
                .iter()
                .enumerate()
                .map(|(i, &r)| self.at(Step::List { list: "hosts", index: i }, |cx| cx.host_ref(r)))
                .collect();
            for h in auto_hosts {
                if !hosts.contains(&h) {
                    hosts.push(h);
                }
            }
            if let Some(cfg) = &self.step {
                hosts.extend(cfg.hosts.iter().copied());
            }
            let helpers = helper_functions
                .iter()
                .enumerate()
                .map(|(i, &r)| self.at(Step::List { list: "helpers", index: i }, |cx| cx.fn_ref(r)))
                .collect();
            let consts = constants
                .iter()
                .enumerate()
                .map(|(i, &r)| self.at(Step::List { list: "consts", index: i }, |cx| cx.const_ref(r)))
                .collect();
            out.push(core::Unit {
                main,
                inputs,
                tables,
                hosts,
                helpers,
                consts,
            });
        }
        out
    }

    fn inputs(&mut self) -> Vec<core::InputDecl> {
        let p = self.p;
        let mut out = vec![];
        for (i, (r, d)) in p.density_inputs.iter().enumerate() {
            let id = InputId(i);
            let name = self.map.inputs.name(id).unwrap_or_default().to_string();
            self.enter(Item::Input {
                id: Some(id),
                name,
            });
            let source_unit = self.step.as_ref().and_then(|c| c.sources.get(&r).copied());
            let source = self.at(Step::Source, |cx| cx.unit_ref(source_unit.unwrap_or(d.density_function)));
            self.mapping_shape(d);
            out.push(core::InputDecl {
                source,
                dims: dims(d.dimensions),
                origin_scale: f64x3(d.scaled_origin),
                position_scale: f64x3(d.scaled_position),
            });
        }
        out
    }

    /// The entries and the instance of each. Normally the program's entries, then the hidden
    /// ones; in a step, its running instances.
    fn entries(&mut self) -> (Vec<core::Entry>, Vec<Instance>) {
        let p = self.p;
        let mut out = vec![];
        let mut instances = vec![];
        if let Some(cfg) = &self.step {
            let running = cfg.running.clone();
            for run in &running {
                let unit = self.unit_ref(run.unit);
                out.push(inst_entry(unit, &run.target));
                instances.push(self.instance(&run.target));
            }
            return (out, instances);
        }
        let mut seen: Vec<Inst> = vec![];
        for (index, e) in p.entry_points.iter().enumerate() {
            let EntryPoint {
                density_function,
                dimensions,
                scaled_origin,
                scaled_position,
            } = e;
            self.enter(Item::Entry { index });
            let unit = self.at(Step::Unit, |cx| cx.unit_ref(*density_function));
            out.push(core::Entry {
                unit,
                dims: dims(*dimensions),
                origin_scale: f64x3(*scaled_origin),
                position_scale: f64x3(*scaled_position),
            });
            let inst = Inst::of_entry(e);
            instances.push(self.instance(&inst));
            seen.push(inst);
        }
        for walk in RefineGraph::new(p).walks {
            for inst in walk.frozen_reads.iter().chain(&walk.with_targets) {
                if seen.contains(inst) || self.map.units.id(inst.cu).is_none() {
                    continue;
                }
                seen.push(*inst);
                let unit = self.map.units.id(inst.cu).unwrap_or(UnitId(0));
                out.push(inst_entry(unit, inst));
                instances.push(self.instance(inst));
            }
        }
        (out, instances)
    }

    pub(super) fn instance(&self, i: &Inst) -> Instance {
        Instance {
            unit: self.map.units.id(i.cu).unwrap_or(UnitId(0)),
            dims: i.dims,
            origin_scale: i.origin,
            position_scale: i.position,
        }
    }
}

fn inst_entry(unit: UnitId, i: &Inst) -> core::Entry {
    core::Entry {
        unit,
        dims: dims(i.dims),
        origin_scale: core::F64x3 { x: i.origin[0], y: i.origin[1], z: i.origin[2] },
        position_scale: core::F64x3 { x: i.position[0], y: i.position[1], z: i.position[2] },
    }
}
