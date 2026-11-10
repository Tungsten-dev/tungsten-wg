//! AST + `Env` → `OptimizedProgram` + `SourceMap` + envelope.
//!
//! Two passes. The first collects labels and creates every arena slot in textual order (so slot
//! order is textual order and forward references work). The second elaborates constants, bodies
//! and the envelope.

mod body;
mod envelope;
mod library;
pub(crate) mod types;

use std::collections::HashMap;
use std::rc::Rc;

use tungsten_ir::*;

use crate::ast::{self, Item, Label, LabelKey, LabelKind};
use crate::intern::intern;
use crate::source_map::SourceMap;
use crate::span::{Diagnostic, FileId, SourceFiles, Span};
use types::var_type;

pub(crate) type R<T> = Result<T, Diagnostic>;

/// Dims, scaled origin and scaled position, as `EntryPoint` and `DensityInput` hold them.
pub(crate) type Grid = ((i32, i32, i32), (f64, f64, f64), (f64, f64, f64));

#[derive(Debug, Clone, PartialEq)]
pub struct ExternSig {
    pub parameter_types: Vec<VariableType>,
    pub return_type: VariableType,
}

/// What the elaborator needs from the outside world.
pub trait Env {
    fn extern_fn(&self, name: &str) -> Option<ExternSig>;
    /// Ordered fields of a registry record type.
    fn extern_type(&self, name: &str) -> Option<Vec<(&'static str, VariableType)>>;
    /// Source text of a library (`use prelude::…` → `library("prelude")`). Every library needs a
    /// `FileId` of its own.
    fn library(&self, name: &str) -> Option<(FileId, &str)>;
    /// The name diagnostics use for a file, a path say. Without one, a library file is named
    /// after its library and any other file `file<N>`.
    fn file_name(&self, file: FileId) -> Option<String> {
        let _ = file;
        None
    }
}

/// Knows nothing. Canonical text must elaborate with it.
pub struct NoEnv;

impl Env for NoEnv {
    fn extern_fn(&self, _: &str) -> Option<ExternSig> {
        None
    }
    fn extern_type(&self, _: &str) -> Option<Vec<(&'static str, VariableType)>> {
        None
    }
    fn library(&self, _: &str) -> Option<(FileId, &str)> {
        None
    }
}

/// Equality compares floats by bits.
#[derive(Debug, Clone)]
pub enum Value {
    Bool(bool),
    I8(i8),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Vec3([f64; 3]),
    Pos3([i32; 3]),
    Array(Vec<Value>),
    Record(&'static str, Vec<(&'static str, Value)>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        use Value::*;
        match (self, other) {
            (Bool(a), Bool(b)) => a == b,
            (I8(a), I8(b)) => a == b,
            (I32(a), I32(b)) => a == b,
            (I64(a), I64(b)) => a == b,
            (F32(a), F32(b)) => a.to_bits() == b.to_bits(),
            (F64(a), F64(b)) => a.to_bits() == b.to_bits(),
            (Vec3(a), Vec3(b)) => a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()),
            (Pos3(a), Pos3(b)) => a == b,
            (Array(a), Array(b)) => a == b,
            (Record(n, a), Record(m, b)) => n == m && a == b,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Case {
    pub name: String,
    pub origin: [f64; 3],
    pub seed: u64,
    pub hosts: Vec<(Ref<HostInput>, Value)>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpectKind {
    Cell([i32; 3], Value),
    All(Value),
    /// Every cell, x-fastest.
    Grid(Vec<Value>),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct Expect {
    /// Index into `cases`.
    pub case: usize,
    /// Index into `entry_points`. `None` for `error(Kind)`, which covers the whole case.
    pub entry: Option<usize>,
    pub kind: ExpectKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KnownFail {
    pub backend: String,
    pub reason: String,
    pub case: Option<String>,
    pub stage: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellSample {
    pub sample: u64,
    pub seed: u64,
}

pub struct TestFile {
    pub program: OptimizedProgram,
    pub source_map: SourceMap,
    /// Index = `entry_points` index; informational only.
    pub entry_labels: Vec<String>,
    pub cases: Vec<Case>,
    pub expects: Vec<Expect>,
    pub reject: Option<Option<String>>,
    pub knownfails: Vec<KnownFail>,
    pub fuel: Option<u64>,
    pub tier: Option<String>,
    pub cells: Option<CellSample>,
}

pub(crate) struct UnitInfo {
    pub r: Ref<ComputeUnit>,
    pub main: Ref<Function>,
    pub helpers: Vec<Ref<Function>>,
    pub inputs: HashMap<LabelKey, Ref<DensityInput>>,
}

pub(crate) struct Ctx<'a> {
    pub env: &'a dyn Env,
    pub files: SourceFiles,
    pub libs: HashMap<String, Rc<library::Lib>>,
    pub lib_stack: Vec<String>,
    pub imports: library::Imports,
    /// Where an imported fn or const label came from (library name, definition), for the note
    /// on a collision with a local label.
    pub imported_from: HashMap<(bool, LabelKey), (String, Span)>,

    pub prog: OptimizedProgram,
    pub map: SourceMap,
    pub hosts: HashMap<LabelKey, Ref<HostInput>>,
    pub tables: HashMap<LabelKey, Ref<PermutationTableInput>>,
    pub consts: HashMap<LabelKey, Ref<Constant>>,
    pub fns: HashMap<LabelKey, Ref<Function>>,
    pub fn_vars: HashMap<Ref<Function>, HashMap<LabelKey, Ref<Var>>>,
    pub units: HashMap<LabelKey, UnitInfo>,
    pub unit_order: Vec<Ref<ComputeUnit>>,
    pub const_asts: HashMap<Ref<Constant>, ast::Expr>,
    pub const_types: HashMap<Ref<Constant>, Option<VariableType>>,
    pub entry_labels: Vec<String>,
    pub entry_keys: HashMap<LabelKey, usize>,
}

pub fn elaborate(file: &ast::File, env: &dyn Env) -> Result<TestFile, Diagnostic> {
    let mut cx = Ctx {
        env,
        files: SourceFiles::default(),
        libs: HashMap::new(),
        lib_stack: vec![],
        imports: Default::default(),
        imported_from: HashMap::new(),
        prog: OptimizedProgram {
            compute_units: Arena::new(),
            functions: Arena::new(),
            entry_points: vec![],
            constants: Arena::new(),
            host_inputs: Arena::new(),
            permutation_tables: Arena::new(),
        },
        map: SourceMap::default(),
        hosts: HashMap::new(),
        tables: HashMap::new(),
        consts: HashMap::new(),
        fns: HashMap::new(),
        fn_vars: HashMap::new(),
        units: HashMap::new(),
        unit_order: vec![],
        const_asts: HashMap::new(),
        const_types: HashMap::new(),
        entry_labels: vec![],
        entry_keys: HashMap::new(),
    };
    cx.files.add(file.file, env.file_name(file.file), &file.src);
    cx.run(file).map_err(|d| d.with_files(cx.files.clone()))
}

/// `parse` + `elaborate`. The common entry point.
pub fn load(src: &str, file: FileId, env: &dyn Env) -> Result<TestFile, Diagnostic> {
    let ast = crate::parser::parse(src, file)
        .map_err(|d| d.with_files(SourceFiles::single(file, env.file_name(file), src)))?;
    elaborate(&ast, env)
}

pub(crate) fn err<T>(span: Span, msg: impl Into<String>) -> R<T> {
    Err(Diagnostic::elab(span, msg))
}

pub(crate) fn declare<V>(map: &mut HashMap<LabelKey, V>, label: &Label, v: V, what: &str) -> R<()> {
    if map.insert(label.key(), v).is_some() {
        return err(
            label.span,
            format!("duplicate {what} label `{}`", label.display()),
        );
    }
    Ok(())
}

fn var_name(l: &Label) -> Name {
    match l.kind {
        LabelKind::Plain | LabelKind::HashHash(_) => Name::Named(l.name.clone()),
        LabelKind::Hash(_) => Name::Prefixed(l.name.clone()),
        LabelKind::Anon(_) => Name::Anonymous,
    }
}

impl<'a> Ctx<'a> {
    fn run(&mut self, file: &ast::File) -> R<TestFile> {
        for item in &file.program.items {
            if let Item::Use(u) = item {
                library::import(self, u)?;
            }
        }
        let (imported_fns, imported_consts) = library::imported(self)?;

        // ----- pass 1: labels and slots, in textual order -----
        for item in &file.program.items {
            if let Item::Host(h) = item {
                self.declare_host(h)?;
            }
        }
        for item in &file.program.items {
            if let Item::Table(t) = item {
                self.declare_table(t)?;
            }
        }
        let local_consts = file.program.items.iter().filter_map(|i| match i {
            Item::Const(c) => Some(c),
            _ => None,
        });
        for c in imported_consts.iter().chain(local_consts) {
            self.declare_const(c)?;
        }
        let fn_items: Vec<&ast::FnItem> = imported_fns
            .iter()
            .chain(file.program.items.iter().filter_map(|i| match i {
                Item::Fn(f) => Some(f),
                _ => None,
            }))
            .collect();
        let mut fn_refs = vec![];
        for f in &fn_items {
            fn_refs.push(self.declare_fn(f)?);
        }
        let unit_items: Vec<&ast::UnitItem> = file
            .program
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Unit(u) => Some(u),
                _ => None,
            })
            .collect();
        for u in &unit_items {
            self.declare_unit(u)?;
        }
        for u in &unit_items {
            self.declare_inputs(u)?;
        }
        for item in &file.program.items {
            if let Item::Entry(e) = item {
                self.declare_entry(e)?;
            }
        }

        // ----- pass 2: constants, bodies -----
        let order: Vec<Ref<Constant>> = self.prog.constants.iter().map(|(r, _)| r).collect();
        for c in order {
            self.ensure_const(c, file.program.span)?;
        }
        for (f, item) in fn_refs.iter().zip(&fn_items) {
            let mut path = vec![];
            let body = self.stmts(*f, &item.body, &mut path)?;
            self.prog.functions[*f].body = body;
        }

        self.envelope(file)
    }

    /// `declare` for fns and consts, with a note when the clash is with an imported label.
    fn declare_item<V>(
        map: &mut HashMap<LabelKey, V>,
        from: &HashMap<(bool, LabelKey), (String, Span)>,
        is_fn: bool,
        label: &Label,
        v: V,
    ) -> R<()> {
        let what = if is_fn { "function" } else { "const" };
        declare(map, label, v, what).map_err(|d| match from.get(&(is_fn, label.key())) {
            Some((lib, span)) if *span != label.span => d.note(
                *span,
                format!("`{}` is imported from library `{lib}`", label.display()),
            ),
            _ => d,
        })
    }

    pub(crate) fn declare_host(&mut self, h: &ast::HostItem) -> R<()> {
        let value = match &h.ty {
            None => HostInput::Dynamic,
            Some(t) => HostInput::Static {
                name: var_name(&h.label),
                t: var_type(t),
            },
        };
        let r = self.prog.host_inputs.insert(value);
        declare(&mut self.hosts, &h.label, r, "host")?;
        self.map.hosts.insert(r, h.span);
        Ok(())
    }

    fn declare_table(&mut self, t: &ast::TableItem) -> R<()> {
        let value = match &t.def {
            ast::TableDef::Base3d => PermutationTableInput::Base3DNoise,
            ast::TableDef::Perlin {
                ident,
                subident,
                index,
            } => PermutationTableInput::PerlinNoise {
                ident: ident.clone(),
                subident: subident.clone(),
                subident_index: *index as usize,
            },
        };
        let r = self.prog.permutation_tables.insert(value);
        declare(&mut self.tables, &t.label, r, "table")?;
        self.map.tables.insert(r, t.span);
        Ok(())
    }

    fn declare_const(&mut self, c: &ast::ConstItem) -> R<()> {
        if matches!(c.label.kind, LabelKind::Anon(_) | LabelKind::HashHash(_)) {
            return err(c.label.span, "a constant label is `name` or `name#N`");
        }
        let r = self.prog.constants.insert(Constant {
            value: ConstExpr::Bool(false),
            name: c.label.name.clone(),
        });
        Self::declare_item(&mut self.consts, &self.imported_from, false, &c.label, r)?;
        self.const_asts.insert(r, c.value.clone());
        self.map.constants.insert(r, c.span);
        Ok(())
    }

    fn lookup<V: Copy>(map: &HashMap<LabelKey, V>, l: &Label, what: &str) -> R<V> {
        match map.get(&l.key()) {
            Some(v) => Ok(*v),
            None => err(l.span, format!("unknown {what} `{}`", l.display())),
        }
    }

    pub(crate) fn lookup_host(&self, l: &Label) -> R<Ref<HostInput>> {
        Self::lookup(&self.hosts, l, "host")
    }
    pub(crate) fn lookup_table(&self, l: &Label) -> R<Ref<PermutationTableInput>> {
        Self::lookup(&self.tables, l, "table")
    }
    pub(crate) fn lookup_const(&self, l: &Label) -> R<Ref<Constant>> {
        Self::lookup(&self.consts, l, "constant")
    }
    pub(crate) fn lookup_fn(&self, l: &Label) -> R<Ref<Function>> {
        Self::lookup(&self.fns, l, "function")
    }
    pub(crate) fn lookup_unit(&self, l: &Label) -> R<&UnitInfo> {
        match self.units.get(&l.key()) {
            Some(u) => Ok(u),
            None => err(l.span, format!("unknown unit `{}`", l.display())),
        }
    }

    fn label_list<V: Copy>(
        map: &HashMap<LabelKey, V>,
        list: Option<&[Label]>,
        what: &str,
    ) -> R<Vec<V>> {
        list.into_iter()
            .flatten()
            .map(|l| Self::lookup(map, l, what))
            .collect()
    }

    fn declare_fn(&mut self, f: &ast::FnItem) -> R<Ref<Function>> {
        let canonical_name = match f.label.kind {
            LabelKind::Plain | LabelKind::Hash(_) => Some(f.label.name.clone()),
            LabelKind::Anon(_) => None,
            LabelKind::HashHash(_) => {
                return err(f.label.span, "a function label is `name`, `name#N` or `%N`");
            }
        };

        let mut variables: Arena<Var> = Arena::new();
        let mut vars: HashMap<LabelKey, Ref<Var>> = HashMap::new();
        let mut var_spans: Vec<(Ref<Var>, Span)> = vec![];
        fn add(
            variables: &mut Arena<Var>,
            vars: &mut HashMap<LabelKey, Ref<Var>>,
            spans: &mut Vec<(Ref<Var>, Span)>,
            (l, t): (&Label, &ast::Ty),
        ) -> R<()> {
            let r = variables.insert(Var {
                name: var_name(l),
                t: var_type(t),
            });
            declare(vars, l, r, "variable")?;
            spans.push((r, l.span));
            Ok(())
        }
        let typed = f
            .params
            .iter()
            .filter_map(|p| Some((&p.label, p.ty.as_ref()?)));
        for v in typed.chain(f.vars.iter().map(|(l, t)| (l, t))) {
            add(&mut variables, &mut vars, &mut var_spans, v)?;
        }
        let mut parameters = vec![];
        for p in &f.params {
            let Some(r) = vars.get(&p.label.key()) else {
                return err(
                    p.label.span,
                    format!(
                        "parameter `{0}` has no type: write `{0}: T` or declare it in `vars {{ }}`",
                        p.label.display()
                    ),
                );
            };
            // a repeated param is the checker's business
            parameters.push(*r);
        }
        for v in ast::lets(&f.body) {
            add(&mut variables, &mut vars, &mut var_spans, v)?;
        }

        let host_inputs =
            Self::label_list(&self.hosts, f.hosts.as_ref().map(|h| &h.0[..]), "host")?;
        let r = self.prog.functions.insert(Function {
            canonical_name,
            host_inputs,
            body: vec![],
            variables,
            parameters,
            return_type: var_type(&f.ret),
            source_hash: f.hash.unwrap_or(0),
        });
        Self::declare_item(&mut self.fns, &self.imported_from, true, &f.label, r)?;
        self.fn_vars.insert(r, vars);
        self.map.functions.insert(r, f.span);
        for (v, s) in var_spans {
            self.map.vars.insert((r, v), s);
        }
        Ok(r)
    }

    fn declare_unit(&mut self, u: &ast::UnitItem) -> R<()> {
        let Some(main_label) = &u.main else {
            return err(u.label.span, "a unit needs `main f;`");
        };
        let main = self.lookup_fn(main_label)?;
        let helpers = Self::label_list(&self.fns, u.helpers.as_deref(), "function")?;
        let tables = Self::label_list(&self.tables, u.tables.as_deref(), "table")?;
        let hosts = Self::label_list(&self.hosts, u.hosts.as_deref(), "host")?;
        let consts = Self::label_list(&self.consts, u.consts.as_deref(), "constant")?;
        let r = self
            .prog
            .compute_units
            .insert(ComputeUnit::DensityFunction {
                function: main,
                density_inputs: Arena::new(),
                permutation_table_inputs: tables,
                host_inputs: hosts,
                helper_functions: helpers.clone(),
                constants: consts,
                source_hash: u.hash.unwrap_or(0),
            });
        let info = UnitInfo {
            r,
            main,
            helpers,
            inputs: HashMap::new(),
        };
        declare(&mut self.units, &u.label, info, "unit")?;
        self.unit_order.push(r);
        self.map.units.insert(r, u.span);
        Ok(())
    }

    /// Dims, origin scale and position scale. Dims of any sign are accepted: the checker judges
    /// them (TESTING.md §5.3).
    pub(crate) fn grid(&self, g: &ast::GridAttrs) -> R<Grid> {
        let triple_f = |a: &Option<([ast::Lit; 3], Span)>, name: &str| -> R<(f64, f64, f64)> {
            let Some((lits, _)) = a else {
                return err(g.span, format!("missing `@{name}(…)`"));
            };
            let [x, y, z] = lits.each_ref().map(envelope::attr_f64);
            Ok((x?, y?, z?))
        };
        let Some((dims, span)) = &g.dims else {
            return err(g.span, "missing `@dims(…)`");
        };
        let [x, y, z] = dims.map(|d| {
            i32::try_from(d)
                .or_else(|_| err(*span, format!("dimension {d} is out of range for i32")))
        });
        Ok((
            (x?, y?, z?),
            triple_f(&g.origin_scale, "origin_scale")?,
            triple_f(&g.position_scale, "position_scale")?,
        ))
    }

    fn declare_inputs(&mut self, u: &ast::UnitItem) -> R<()> {
        let (unit, main) = {
            let info = self.lookup_unit(&u.label)?;
            (info.r, info.main)
        };
        for input in &u.inputs {
            if self.units[&u.label.key()]
                .inputs
                .contains_key(&input.label.key())
            {
                return err(
                    input.label.span,
                    format!("duplicate input label `{}`", input.label.display()),
                );
            }
            let target = self.lookup_unit(&input.target)?.r;
            let (dimensions, scaled_origin, scaled_position) = self.grid(&input.grid)?;
            let var = match &input.var {
                Some((f, v)) => {
                    let f = self.lookup_fn(f)?;
                    match self.fn_vars[&f].get(&v.key()) {
                        Some(r) => *r,
                        None => return err(v.span, format!("unknown variable `{}`", v.display())),
                    }
                }
                None => {
                    if self.fn_vars[&main].contains_key(&input.label.key()) {
                        return err(
                            input.label.span,
                            format!(
                                "the main function already has a variable `{}`; \
                                 name it with `@var(f.v)` or rename the input",
                                input.label.display()
                            ),
                        );
                    }
                    let v = self.prog.functions[main].variables.insert(Var {
                        name: var_name(&input.label),
                        t: VariableType::DensityInput,
                    });
                    self.fn_vars
                        .get_mut(&main)
                        .unwrap()
                        .insert(input.label.key(), v);
                    self.map.vars.insert((main, v), input.label.span);
                    v
                }
            };
            let ComputeUnit::DensityFunction { density_inputs, .. } =
                &mut self.prog.compute_units[unit];
            let d = density_inputs.insert(DensityInput {
                var,
                density_function: target,
                scaled_origin,
                scaled_position,
                dimensions,
            });
            let info = self.units.get_mut(&u.label.key()).unwrap();
            info.inputs.insert(input.label.key(), d);
            self.map.inputs.insert((unit, d), input.span);
        }
        Ok(())
    }

    fn declare_entry(&mut self, e: &ast::EntryItem) -> R<()> {
        let unit = self.lookup_unit(&e.unit)?.r;
        let (dimensions, scaled_origin, scaled_position) = self.grid(&e.grid)?;
        let label = e.label.as_ref().unwrap_or(&e.unit);
        let idx = self.prog.entry_points.len();
        declare(&mut self.entry_keys, label, idx, "entry")?;
        self.prog.entry_points.push(EntryPoint {
            density_function: unit,
            dimensions,
            scaled_origin,
            scaled_position,
        });
        self.entry_labels.push(label.display());
        self.map.entries.push(e.span);
        Ok(())
    }

    /// Elaborates a constant (and the constants it reads, first) unless it already is.
    pub(crate) fn ensure_const(&mut self, c: Ref<Constant>, at: Span) -> R<()> {
        if self.const_types.contains_key(&c) {
            return Ok(());
        }
        let Some(ast) = self.const_asts.remove(&c) else {
            return err(at, "constants depend on each other in a cycle");
        };
        Self::check_const_expr(&ast)?;
        let (e, t) = self.expr(&body::Scope::constant(c), &ast, &mut vec![])?;
        let Some(value) = types::to_const(e) else {
            return err(ast.span, "not a constant expression");
        };
        self.prog.constants[c].value = value;
        self.const_types.insert(c, t);
        Ok(())
    }

    fn check_const_expr(e: &ast::Expr) -> R<()> {
        use ast::ExprKind::*;
        let check = |e: &ast::Expr| match &e.kind {
            Host(_)
            | Table(_)
            | Density { .. }
            | Ambient(_)
            | Call { .. }
            | ExtCall { .. }
            | ConstructExtern { .. } => err(e.span, "not allowed in a constant expression"),
            _ => Ok(()),
        };
        check(e)?;
        e.kind.try_for_each_descendant(&mut |c| check(c))
    }

    pub(crate) fn intern_name(s: &str) -> &'static str {
        intern(s)
    }
}

impl std::fmt::Debug for TestFile {
    /// The IR has no `Debug` for whole programs; print the canonical text instead.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::print_test_file(self))
    }
}
