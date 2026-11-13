//! `ProgramBuilder`: owns the program's arenas and hands out small `Copy` handles.

use std::collections::HashMap;
use std::panic::Location;

use tungsten_interface::Scale;
use tungsten_ir::{
    Arena, AutoConst, AutoConstArg, AutoParam, ComputeUnit, ConstExpr, DensityInput,
    DensityMapping, EntryPoint, Expression, Function, HostInput, Name, OwnerTag, Program, Ref,
    Refinement, Rerun, UnaryOperator, VariableType,
};

use crate::derive::{Summary, const_deps, summarize};
use crate::error::{BuildError, BuildErrorKind, BuildErrors};
use crate::expr::{Expr, Ty, intern, intern_list, name};
use crate::function::FnBuilder;

/// A finished unit. Only finished units can be read or made entries, so unit cycles can't be
/// written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unit {
    pub(crate) r: Ref<ComputeUnit>,
    pub(crate) ret: Ty,
}

impl Unit {
    /// The unit's cell type: its main function's return type.
    pub fn ty(&self) -> &'static VariableType {
        self.ret
    }
}

/// A finished helper function. Call it with [`Helper::call`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Helper {
    pub(crate) r: Ref<Function>,
    pub(crate) params: &'static [Ty],
    pub(crate) ret: Ty,
}

/// An entry point, for declaring refinements on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    index: usize,
    owner: OwnerTag,
}

/// A density input. As an `Expr` it is an unindexed read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Input {
    pub(crate) r: Ref<DensityInput>,
    pub(crate) ty: Option<Ty>,
}

/// A static host input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host {
    pub(crate) r: Ref<HostInput>,
    pub(crate) ty: Ty,
}

/// A value supplied when the orchestration is created (`I64`, `F64` or `Bool`). Programs can't
/// read it; it is only an argument of [`ProgramBuilder::auto_const`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Param {
    pub(crate) r: Ref<AutoParam>,
    pub(crate) ty: Ty,
}

/// A value built when the orchestration is created. Reads like any expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Auto {
    pub(crate) r: Ref<AutoConst>,
    pub(crate) ty: Ty,
}

/// An argument of an autoconst initializer.
#[derive(Debug, Clone, PartialEq)]
pub enum AutoArg {
    Param(Param),
    I64(i64),
    F64(f64),
    Bool(bool),
    Str(String),
}

impl From<f64> for AutoArg {
    fn from(v: f64) -> Self {
        AutoArg::F64(v)
    }
}

impl From<bool> for AutoArg {
    fn from(v: bool) -> Self {
        AutoArg::Bool(v)
    }
}

impl From<&str> for AutoArg {
    fn from(v: &str) -> Self {
        AutoArg::Str(v.to_string())
    }
}

impl From<String> for AutoArg {
    fn from(v: String) -> Self {
        AutoArg::Str(v)
    }
}

impl From<Param> for AutoArg {
    fn from(p: Param) -> Self {
        AutoArg::Param(p)
    }
}

impl From<i64> for AutoArg {
    fn from(v: i64) -> Self {
        AutoArg::I64(v)
    }
}

/// A program constant. Its type is its value's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Constant {
    pub(crate) r: Ref<tungsten_ir::Constant>,
    pub(crate) ty: Option<Ty>,
}

/// Dimensions and scales of an entry or an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Grid {
    pub(crate) dimensions: (i32, i32, i32),
    pub(crate) scaled_origin: Scale,
    pub(crate) scaled_position: Scale,
}

impl Grid {
    /// A grid of `dims` cells, with both scales `(1, 1, 1)`.
    pub fn new(dims: (i32, i32, i32)) -> Self {
        Self {
            dimensions: dims,
            scaled_origin: Scale::ones(),
            scaled_position: Scale::ones(),
        }
    }

    pub fn origin_scale(mut self, s: Scale) -> Self {
        self.scaled_origin = s;
        self
    }

    pub fn position_scale(mut self, s: Scale) -> Self {
        self.scaled_position = s;
        self
    }

    pub fn dims(&self) -> (i32, i32, i32) {
        self.dimensions
    }

    // fn key(&self) -> ((i32, i32, i32), Scale, Scale) {
    //     let (o, p) = (self.scaled_origin, self.scaled_position);
    //     (self.dimensions, o, p)
    // }
}

/// Everything that describes an input.
#[derive(Debug, Clone)]
pub struct InputSpec {
    source: Unit,
    grid: Grid,
    name: Option<String>,
    mapping: DensityMapping,
    previous: bool,
}

impl InputSpec {
    pub fn new(source: Unit, grid: Grid) -> Self {
        Self {
            source,
            grid,
            name: None,
            mapping: DensityMapping::Identity,
            previous: false,
        }
    }

    /// Names the input. When it merges with an existing input, the first name wins.
    pub fn named(mut self, name: &str) -> Self {
        self.name = Some(name.to_string());
        self
    }

    /// How an un-indexed read maps the reader's cell to this input's buffer.
    pub fn mapping(mut self, mapping: DensityMapping) -> Self {
        self.mapping = mapping;
        self
    }

    /// Reads the instance's own output from before the current refine call.
    pub fn previous(mut self) -> Self {
        self.previous = true;
        self
    }
}

/// One instance a refinement reruns: `target` on `grid`.
#[derive(Debug, Clone, Copy)]
pub struct RerunSpec {
    target: Unit,
    grid: Grid,
    with: Option<Unit>,
}

impl RerunSpec {
    pub fn new(target: Unit, grid: Grid) -> Self {
        Self {
            target,
            grid,
            with: None,
        }
    }

    /// Runs `refined` instead of `target`, writing `target`'s buffer.
    pub fn with(mut self, refined: Unit) -> Self {
        self.with = Some(refined);
        self
    }
}

type InputKey = (Ref<ComputeUnit>, Grid, DensityMapping, bool);

/// Builds an [`Program`]. Errors from every function and declaration collect here, and
/// [`build`](Self::build) returns them all.
pub struct ProgramBuilder {
    pub(crate) program: Program,
    pub(crate) summaries: HashMap<Ref<Function>, Summary>,
    /// Each constant's transitive dependencies, dependencies first, without the constant.
    pub(crate) const_deps: HashMap<Ref<tungsten_ir::Constant>, Vec<Ref<tungsten_ir::Constant>>>,
    inputs: HashMap<InputKey, Ref<DensityInput>>,
    hosts: HashMap<String, Host>,
    params: HashMap<String, Param>,
    autos: HashMap<AutoConst, Auto>,
    fresh: HashMap<String, usize>,
    errors: Vec<BuildError>,
}

impl Default for ProgramBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgramBuilder {
    pub fn new() -> Self {
        Self {
            program: Program {
                compute_units: Arena::new(),
                functions: Arena::new(),
                entry_points: Vec::new(),
                refinements: vec![],
                constants: Arena::new(),
                host_inputs: Arena::new(),
                auto_params: Arena::new(),
                auto_consts: Arena::new(),
                density_inputs: Arena::new(),
            },
            summaries: HashMap::new(),
            const_deps: HashMap::new(),
            inputs: HashMap::new(),
            hosts: HashMap::new(),
            params: HashMap::new(),
            autos: HashMap::new(),
            fresh: HashMap::new(),
            errors: Vec::new(),
        }
    }

    fn error(
        &mut self,
        kind: BuildErrorKind,
        function: Option<String>,
        at: &'static Location<'static>,
    ) {
        let mut e = BuildError::new(kind, at);
        e.function = function;
        self.errors.push(e);
    }

    /// `prefix_0`, `prefix_1`, … A deterministic name for generated functions.
    pub fn fresh_name(&mut self, prefix: &str) -> String {
        let n = self.fresh.entry(prefix.to_string()).or_insert(0);
        let name = format!("{prefix}_{n}");
        *n += 1;
        name
    }

    // ---- functions -----------------------------------------------------------------------

    /// Moves `f` into the program and derives its lists, returning the function's ref and its
    /// summary.
    fn finish(&mut self, f: FnBuilder, at: &'static Location<'static>) -> (Ref<Function>, Ty) {
        let FnBuilder {
            name: fname,
            ret,
            vars,
            params,
            body,
            errors,
            source_hash,
            ..
        } = f;
        self.errors.extend(errors);
        let (summary, direct_hosts, foreign) = summarize(self, &body);
        for what in foreign {
            self.error(
                BuildErrorKind::ForeignHandle {
                    what,
                    owner: "program builder",
                },
                Some(fname.clone()),
                at,
            );
        }
        let r = self.program.functions.insert(Function {
            canonical_name: Some(fname),
            host_inputs: direct_hosts,
            body,
            variables: vars,
            parameters: params,
            return_type: ret.clone(),
            source_hash,
        });
        self.summaries.insert(r, summary);
        (r, ret)
    }

    /// Finishes `f` as a unit's main function. A main with parameters is `UnitHasParams`.
    #[track_caller]
    pub fn finish_unit(&mut self, f: FnBuilder) -> Unit {
        let at = Location::caller();
        if !f.params.is_empty() {
            let kind = BuildErrorKind::UnitHasParams {
                params: f.params.len(),
            };
            self.error(kind, Some(f.name.clone()), at);
        }
        let source_hash = f.source_hash;
        let (function, ret) = self.finish(f, at);
        let s = &self.summaries[&function];
        let r = self
            .program
            .compute_units
            .insert(ComputeUnit::DensityFunction {
                function,
                density_inputs: s.inputs.clone(),
                auto_consts: s.autos.clone(),
                host_inputs: s.hosts.clone(),
                helper_functions: s.helpers.clone(),
                constants: s.consts.clone(),
                source_hash,
            });
        Unit { r, ret }
    }

    /// Finishes `f` as a helper that units and other helpers can call.
    #[track_caller]
    pub fn finish_helper(&mut self, f: FnBuilder) -> Helper {
        let at = Location::caller();
        let params = intern_list(
            f.params
                .iter()
                .map(|p| intern(f.vars[*p].t.clone()))
                .collect(),
        );
        let (r, ret) = self.finish(f, at);
        Helper { r, params, ret }
    }

    /// Adds an entry point for `unit`.
    #[track_caller]
    pub fn entry(&mut self, unit: Unit, grid: Grid) -> Entry {
        let handle = Entry {
            index: self.program.entry_points.len(),
            owner: self.program.compute_units.tag(),
        };
        if !self.program.compute_units.contains(unit.r) {
            self.foreign("a unit", Location::caller());
            return handle;
        }
        self.program.entry_points.push(EntryPoint {
            density_function: unit.r,
            dimensions: grid.dimensions,
            scaled_origin: grid.scaled_origin.as_f64(),
            scaled_position: grid.scaled_position.as_f64(),
        });
        handle
    }

    /// Declares a refinement of `entry`. `march` is the largest offset per axis. Whether the
    /// refinement is valid is checked when the program is lowered.
    #[track_caller]
    pub fn refinement(
        &mut self,
        name: &str,
        entry: Entry,
        march: (i32, i32, i32),
        reruns: impl IntoIterator<Item = RerunSpec>,
    ) {
        let at = Location::caller();
        if entry.owner != self.program.compute_units.tag() {
            self.foreign("an entry", at);
            return;
        }
        let reruns: Vec<RerunSpec> = reruns.into_iter().collect();
        let units = reruns
            .iter()
            .flat_map(|r| [Some(r.target), r.with])
            .flatten();
        if units
            .into_iter()
            .any(|u| !self.program.compute_units.contains(u.r))
        {
            self.foreign("a unit", at);
            return;
        }
        self.program.refinements.push(Refinement {
            name: name.to_string(),
            entry: entry.index,
            march,
            reruns: reruns
                .into_iter()
                .map(|r| Rerun {
                    cu: r.target.r,
                    dimensions: r.grid.dimensions,
                    scaled_origin: r.grid.scaled_origin.as_f64(),
                    scaled_position: r.grid.scaled_position.as_f64(),
                    with: r.with.map(|u| u.r),
                })
                .collect(),
        });
    }

    fn foreign(&mut self, what: &'static str, at: &'static Location<'static>) {
        self.error(
            BuildErrorKind::ForeignHandle {
                what,
                owner: "program builder",
            },
            None,
            at,
        );
    }

    // ---- inputs and declarations ---------------------------------------------------------

    /// An input reading `source` on `grid`. An equal input (same source, dims and scales by
    /// bits, same mapping and `previous`) is returned instead of a new one.
    #[track_caller]
    pub fn input(&mut self, source: Unit, grid: Grid) -> Input {
        self.input_with(InputSpec::new(source, grid))
    }

    /// Like [`input`](Self::input), with every option.
    #[track_caller]
    pub fn input_with(&mut self, spec: InputSpec) -> Input {
        let ty = Some(spec.source.ret);
        if !self.program.compute_units.contains(spec.source.r) {
            self.foreign("a unit", Location::caller());
        }
        // let (dims, bits) = spec.grid.key();
        let key = (spec.source.r, spec.grid, spec.mapping, spec.previous);
        if let Some(r) = self.inputs.get(&key) {
            return Input { r: *r, ty };
        }
        let r = self.program.density_inputs.insert(DensityInput {
            name: spec.name.map_or(Name::Anonymous, Name::Named),
            density_function: spec.source.r,
            scaled_origin: spec.grid.scaled_origin.as_f64(),
            scaled_position: spec.grid.scaled_position.as_f64(),
            dimensions: spec.grid.dimensions,
            mapping: spec.mapping,
            previous: spec.previous,
        });
        self.inputs.insert(key, r);
        Input { r, ty }
    }

    /// A static host input. The same name returns the same host; with another type that is
    /// `HostTypeConflict`.
    #[track_caller]
    pub fn host(&mut self, name_: &str, t: VariableType) -> Host {
        if let Some(h) = self.hosts.get(name_).copied() {
            if *h.ty != t {
                self.error(
                    BuildErrorKind::HostTypeConflict {
                        name: name_.to_string(),
                        existing: name(h.ty),
                        requested: name(&t),
                    },
                    None,
                    Location::caller(),
                );
            }
            return h;
        }
        let ty = intern(t.clone());
        let r = self.program.host_inputs.insert(HostInput::Static {
            name: Name::Named(name_.to_string()),
            t,
        });
        let h = Host { r, ty };
        self.hosts.insert(name_.to_string(), h);
        h
    }

    /// An auto param. The same name returns the same param; with another type that is
    /// `AutoParamConflict`. Params are passed to the creation API in declaration order.
    #[track_caller]
    pub fn param(&mut self, name_: &str, t: VariableType) -> Param {
        if let Some(p) = self.params.get(name_).copied() {
            if *p.ty != t {
                self.error(
                    BuildErrorKind::AutoParamConflict {
                        name: name_.to_string(),
                        existing: name(p.ty),
                        requested: name(&t),
                    },
                    None,
                    Location::caller(),
                );
            }
            return p;
        }
        let ty = intern(t.clone());
        let r = self.program.auto_params.insert(AutoParam {
            name: name_.to_string(),
            t,
        });
        let p = Param { r, ty };
        self.params.insert(name_.to_string(), p);
        p
    }

    /// A value built at creation time by calling the runtime function `initializer` with
    /// `args`. An equal autoconst (same type, initializer and arguments) is returned as is, and
    /// the first name wins. A `Handle` type is what externs take; the program reads the result
    /// like any expression.
    pub fn auto_const(
        &mut self,
        name_: &str,
        t: VariableType,
        initializer: &str,
        args: impl IntoIterator<Item = AutoArg>,
    ) -> Auto {
        let args: Vec<AutoConstArg> = args
            .into_iter()
            .map(|a| match a {
                AutoArg::Param(p) => AutoConstArg::Param(p.r),
                AutoArg::I64(v) => AutoConstArg::I64(v),
                AutoArg::F64(v) => AutoConstArg::F64(v),
                AutoArg::Bool(v) => AutoConstArg::Bool(v),
                AutoArg::Str(v) => AutoConstArg::Str(v),
            })
            .collect();
        let key = AutoConst {
            name: String::new(),
            t: t.clone(),
            initializer: initializer.to_string(),
            args: args.clone(),
        };
        if let Some(a) = self.autos.get(&key) {
            return *a;
        }
        let ty = intern(t.clone());
        let r = self.program.auto_consts.insert(AutoConst {
            name: name_.to_string(),
            t,
            initializer: initializer.to_string(),
            args,
        });
        let a = Auto { r, ty };
        self.autos.insert(key, a);
        a
    }

    /// A named constant. Its value must be a constant expression: literals, other constants,
    /// operators, casts, fields, indexing, arrays and `vec3`/`pos3`.
    #[track_caller]
    pub fn constant(&mut self, name_: &str, value: impl Into<Expr>) -> Constant {
        let at = Location::caller();
        let mut value = value.into();
        self.errors.append(&mut value.problems);
        let mut found = None;
        let mut foreign = false;
        let cexpr = to_const(&value.node, &mut found, &mut |c| {
            if !self.program.constants.contains(c) {
                foreign = true;
            }
        });
        if foreign {
            self.foreign("a constant", at);
        }
        if let Some(what) = found {
            self.error(
                BuildErrorKind::NotConstant {
                    name: name_.to_string(),
                    found: what,
                },
                None,
                at,
            );
        }
        let ok = found.is_none() && !foreign;
        let cexpr = cexpr.unwrap_or(ConstExpr::Bool(false));
        let deps = if ok {
            const_deps(self, &cexpr)
        } else {
            Vec::new()
        };
        let r = self.program.constants.insert(tungsten_ir::Constant {
            value: cexpr,
            name: name_.to_string(),
        });
        self.const_deps.insert(r, deps);
        Constant {
            r,
            ty: if ok { value.ty } else { None },
        }
    }

    /// The program, or every error recorded while building it.
    pub fn build(self) -> Result<Program, BuildErrors> {
        if self.errors.is_empty() {
            Ok(self.program)
        } else {
            Err(BuildErrors(self.errors))
        }
    }
}

/// Converts to a `ConstExpr`, setting `found` to the first thing that isn't constant.
fn to_const(
    e: &Expression,
    found: &mut Option<&'static str>,
    seen: &mut dyn FnMut(Ref<tungsten_ir::Constant>),
) -> Option<ConstExpr> {
    let mut go = |e: &Expression, found: &mut Option<&'static str>| to_const(e, found, seen);
    let not = |found: &mut Option<&'static str>, what| {
        found.get_or_insert(what);
        None
    };
    Some(match e {
        Expression::Constant(c) => {
            seen(*c);
            ConstExpr::Constant(*c)
        }
        Expression::Float(v) => ConstExpr::Float(*v),
        Expression::Double(v) => ConstExpr::Double(*v),
        Expression::Byte(v) => ConstExpr::Byte(*v),
        Expression::Int(v) => ConstExpr::Int(*v),
        Expression::Long(v) => ConstExpr::Long(*v),
        Expression::Bool(v) => ConstExpr::Bool(*v),
        Expression::BinaryOp { op, left, right } => {
            let l = go(left, found);
            let r = go(right, found);
            ConstExpr::BinaryOp {
                op: *op,
                left: Box::new(l?),
                right: Box::new(r?),
            }
        }
        Expression::UnaryOp { op, operand } => {
            if matches!(op, UnaryOperator::Ref(_)) {
                return not(found, "a reference");
            }
            ConstExpr::UnaryOp {
                op: *op,
                operand: Box::new(go(operand, found)?),
            }
        }
        Expression::ExplicitCast { to, expr } => ConstExpr::ExplicitCast {
            to: to.clone(),
            expr: Box::new(go(expr, found)?),
        },
        Expression::Field {
            base,
            field,
            type_of_field,
            base_type,
        } => ConstExpr::Field {
            base: Box::new(go(base, found)?),
            field: field.clone(),
            type_of_field: type_of_field.clone(),
            base_type: base_type.clone(),
        },
        Expression::ArrayAccess { array, index } => {
            let a = go(array, found);
            let i = go(index, found);
            ConstExpr::ArrayAccess {
                array: Box::new(a?),
                index: Box::new(i?),
            }
        }
        Expression::ArrayLiteral(es) => {
            let es: Vec<_> = es.iter().map(|e| go(e, found)).collect();
            ConstExpr::ArrayLiteral(es.into_iter().collect::<Option<_>>()?)
        }
        Expression::Construct { t, args } => {
            let args: Vec<_> = args.iter().map(|e| go(e, found)).collect();
            ConstExpr::Construct {
                t: t.clone(),
                args: args.into_iter().collect::<Option<_>>()?,
            }
        }
        Expression::Variable(_) => return not(found, "a local"),
        Expression::FunctionCall { .. } => return not(found, "a function call"),
        Expression::ExternCall { .. } => return not(found, "an extern call"),
        Expression::DensityVariable(..) => return not(found, "a density read"),
        Expression::AutoConst(_) => return not(found, "an autoconst"),
        Expression::HostInput(_) => return not(found, "a host read"),
        Expression::Ambient(_) => return not(found, "an ambient"),
        Expression::ConstructExtern { .. } => return not(found, "a record construction"),
    })
}
