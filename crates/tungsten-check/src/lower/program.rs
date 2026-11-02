//! Items: id assignment, then hosts, constants, functions, units, inputs and entries in walk
//! order (see README.md).

use std::collections::{HashMap, HashSet};

use tungsten_core as core;
use tungsten_core::{ConstId, FnId, HostId, LocalId, TableId, UnitId};
use tungsten_ir::{
    ComputeUnit, EntryPoint, Function, HostInput, Name, OptimizedProgram, PermutationTableInput,
    VariableType,
};

use super::{Ctx, get};
use crate::error::{EntityKind, Item, LowerError, LowerErrorKind, ParameterProblem, Step};
use crate::map::{IdMap, LowerMap};
use crate::registry::Registry;
use super::const_to_expr;

fn name_str(n: &Name) -> Option<&str> {
    match n {
        Name::Named(s) | Name::Prefixed(s) => Some(s),
        Name::Anonymous => None,
    }
}

/// Lowers `p` without the extern/record tables (the caller copies those from the registry).
pub(crate) fn run(
    p: &OptimizedProgram,
    reg: &Registry,
) -> (core::Program, LowerMap, Vec<LowerError>) {
    let mut cx = Ctx {
        p,
        reg,
        map: build_ids(p),
        errors: vec![],
        item: Item::Registry,
        path: vec![],
        func: None,
        locals: HashMap::new(),
    };
    let tables = p
        .permutation_tables
        .values()
        .map(|t| core::TableDecl {
            kind: match t {
                PermutationTableInput::PerlinNoise { .. } => core::TableKind::Perm,
                PermutationTableInput::Base3DNoise => core::TableKind::Sampler,
            },
        })
        .collect();
    let hosts = cx.hosts();
    let consts = cx.consts();
    let functions = cx.functions();
    let units = cx.units();
    let inputs = cx.inputs();
    let entries = cx.entries();
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
    (program, cx.map, cx.errors)
}

/// Ids in arena iteration order, with display names.
fn build_ids(p: &OptimizedProgram) -> LowerMap {
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
    for (i, (r, t)) in p.permutation_tables.iter().enumerate() {
        let name = match t {
            PermutationTableInput::PerlinNoise { ident, .. } => ident.clone(),
            PermutationTableInput::Base3DNoise => format!("#{i}"),
        };
        m.tables.push(r, name);
        m.table_idents.push(t.clone());
    }
    for (_, u) in p.compute_units.iter() {
        let ComputeUnit::DensityFunction {
            function,
            density_inputs,
            ..
        } = u;
        let main = get(&p.functions, *function);
        for (r, d) in density_inputs.iter() {
            let i = m.inputs.len();
            let var = main.and_then(|f| get(&f.variables, d.var).map(|v| (d.var, v)));
            let name = var
                .and_then(|(_, v)| name_str(&v.name).map(str::to_string))
                .unwrap_or_else(|| format!("#{i}"));
            m.inputs.push(r, name);
            m.input_vars.push(var.map(|(r, _)| r));
        }
    }
    m
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

    pub(super) fn table_ref(&mut self, r: tungsten_ir::Ref<PermutationTableInput>) -> TableId {
        let id = self.map.tables.id(r);
        self.id_or_dangling(id, EntityKind::PermutationTable, TableId(0))
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
                HostInput::Static { t, .. } => self.ty(t),
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
            if v.t == VariableType::DensityInput {
                self.err(LowerErrorKind::DensityInputType);
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
            // Density-input vars are `DensityInput.var` placeholders; backends skip them too.
            if seen.contains(&r) || v.t == VariableType::DensityInput {
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
        let body = self.block(&f.body, Step::Stmt);
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
                permutation_table_inputs,
                host_inputs,
                helper_functions,
                constants,
                ..
            } = u;
            let main = self.at(Step::Main, |cx| cx.fn_ref(*function));
            let inputs = density_inputs
                .iter()
                .filter_map(|(r, _)| self.map.inputs.id(r))
                .collect();
            let tables = permutation_table_inputs
                .iter()
                .enumerate()
                .map(|(i, &r)| self.at(Step::List { list: "tables", index: i }, |cx| cx.table_ref(r)))
                .collect();
            let hosts = host_inputs
                .iter()
                .enumerate()
                .map(|(i, &r)| self.at(Step::List { list: "hosts", index: i }, |cx| cx.host_ref(r)))
                .collect();
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
        for (u, (_, unit)) in p.compute_units.iter().enumerate() {
            let ComputeUnit::DensityFunction { density_inputs, .. } = unit;
            for (r, d) in density_inputs.iter() {
                let Some(id) = self.map.inputs.id(r) else {
                    continue;
                };
                let name = self.map.inputs.name(id).unwrap_or_default().to_string();
                self.enter(Item::Input {
                    id: Some(id),
                    name,
                });
                let source = self.at(Step::Source, |cx| cx.unit_ref(d.density_function));
                out.push(core::InputDecl {
                    owner: UnitId(u),
                    source,
                    dims: dims(d.dimensions),
                    origin_scale: f64x3(d.scaled_origin),
                    position_scale: f64x3(d.scaled_position),
                });
            }
        }
        out
    }

    fn entries(&mut self) -> Vec<core::Entry> {
        let p = self.p;
        let mut out = vec![];
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
        }
        out
    }
}
