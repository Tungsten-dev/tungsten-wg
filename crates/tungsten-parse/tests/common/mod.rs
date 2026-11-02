//! A generator of structurally valid programs, including the strange ones the printer must
//! handle: duplicate and odd names, anonymous variables, NaN payloads, shared helpers that read
//! densities, unregistered externs, repeated params, records.
//!
//! The generator draws every choice from an `Rng`: a seeded stream, or a recorded sequence of
//! choices that proptest generates and shrinks (`programs`, `test_files`). Shorter sequences and
//! smaller choices make simpler programs, and past its end every choice is 0, the simplest
//! option, so a failing case shrinks to a small program.

#![allow(dead_code)]

use std::fmt;

use proptest::prelude::*;
use tungsten_ir::*;
use tungsten_parse::*;

pub enum Rng {
    Seeded(u64),
    Replay(Vec<u64>, usize),
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng::Seeded(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    pub fn replay(choices: Vec<u64>) -> Rng {
        Rng::Replay(choices, 0)
    }
    pub fn next(&mut self) -> u64 {
        match self {
            Rng::Seeded(state) => {
                // xorshift64*
                let mut x = *state;
                x ^= x >> 12;
                x ^= x << 25;
                x ^= x >> 27;
                *state = x;
                x.wrapping_mul(0x2545_F491_4F6C_DD1D)
            }
            Rng::Replay(choices, i) => {
                *i += 1;
                choices.get(*i - 1).copied().unwrap_or(0)
            }
        }
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    pub fn chance(&mut self, num: usize, den: usize) -> bool {
        self.below(den) < num
    }
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const NAMES: &[&str] = &[
    "x",
    "y",
    "z",
    "a",
    "b",
    "t",
    "acc",
    "n0",
    "let",
    "fn",
    "minecraft:overworld/x",
    "a#1",
    "",
    "ünï",
    "9lives",
    "with space",
    "q`uote",
    "back\\slash",
    "new\nline",
    "d3",
    "u1",
    "e2",
    "inf",
    "nan",
    "table",
    "origin",
    "u",
    "d",
    "e",
    "t0",
    "%5",
];
const EXTERNS: &[&str] = &[
    "sin",
    "cos",
    "sample_noise",
    "weird name",
    "ext",
    "Rec",
    "Pt",
];
const FIELDS: &[&str] = &["x", "y", "z", "re", "im", "let", "odd field"];

pub fn f64_edge(rng: &mut Rng) -> f64 {
    match rng.below(14) {
        0 => 0.0,
        1 => -0.0,
        2 => f64::INFINITY,
        3 => f64::NEG_INFINITY,
        4 => f64::MIN_POSITIVE,
        5 => 5e-324,
        6 => f64::MAX,
        7 => f64::from_bits(0x7ff8_0000_0000_0001),
        8 => f64::from_bits(0xfff8_0000_0000_0000),
        9 => 0.1,
        10 => (rng.below(2001) as f64 - 1000.0) / 8.0,
        11 => f64::from_bits(rng.next()),
        12 => 1e16,
        _ => -1.5e-7,
    }
}

pub fn f32_edge(rng: &mut Rng) -> f32 {
    match rng.below(10) {
        0 => 0.0,
        1 => -0.0,
        2 => f32::INFINITY,
        3 => f32::NEG_INFINITY,
        4 => 1e-45,
        5 => f32::MAX,
        6 => f32::from_bits(0x7fc0_0001),
        7 => f32::from_bits(0xffc0_0000),
        8 => 0.1,
        _ => f32::from_bits(rng.next() as u32),
    }
}

fn name(rng: &mut Rng) -> String {
    rng.pick(NAMES).to_string()
}

fn var_name(rng: &mut Rng) -> Name {
    match rng.below(6) {
        0 => Name::Anonymous,
        1 | 2 => Name::Prefixed(name(rng)),
        _ => Name::Named(name(rng)),
    }
}

pub fn gen_type(rng: &mut Rng, depth: usize, valued: bool) -> VariableType {
    let all = rng.below(if valued { 10 } else { 13 });
    match all {
        0 => VariableType::F64,
        1 => VariableType::F32,
        2 => VariableType::I32,
        3 => VariableType::I64,
        4 => VariableType::I8,
        5 => VariableType::Bool,
        6 => VariableType::Vec3,
        7 => VariableType::Pos3,
        8 if depth < 2 => {
            VariableType::Array(Box::new(gen_type(rng, depth + 1, valued)), rng.below(4))
        }
        8 => VariableType::F64,
        9 if valued => VariableType::Extern(intern(rng.pick(EXTERNS))),
        9 => VariableType::DensityInput,
        10 => VariableType::PermutationTable,
        11 => VariableType::InterpolatedNoiseSampler,
        _ => VariableType::Extern(intern(rng.pick(EXTERNS))),
    }
}

fn gen_op(rng: &mut Rng) -> BinaryOperator {
    use BinaryOperator::*;
    *rng.pick(&[
        Add,
        Subtract,
        Multiply,
        Divide,
        Equal,
        NotEqual,
        Less,
        LessEqual,
        Greater,
        GreaterEqual,
        And,
        Or,
        ShiftLeft,
        ShiftRight,
        LogicalShiftRight,
        BitOr,
        BitAnd,
        BitXor,
    ])
}

fn gen_unop(rng: &mut Rng) -> UnaryOperator {
    *rng.pick(&[
        UnaryOperator::Negate,
        UnaryOperator::Not,
        UnaryOperator::Ref(false),
        UnaryOperator::Ref(true),
    ])
}

pub struct Scope<'a> {
    pub vars: Vec<Ref<Var>>,
    pub consts: &'a [Ref<Constant>],
    pub hosts: &'a [Ref<HostInput>],
    pub tables: &'a [Ref<PermutationTableInput>],
    pub fns: &'a [Ref<Function>],
    pub inputs: &'a [Ref<DensityInput>],
}

fn gen_leaf(rng: &mut Rng, sc: &Scope) -> Expression {
    loop {
        return match rng.below(14) {
            0 => Expression::Int(*rng.pick(&[0, 1, -1, i32::MIN, i32::MAX, 42])),
            1 => Expression::Long(*rng.pick(&[0, -1, i64::MIN, i64::MAX])),
            2 => Expression::Byte(*rng.pick(&[0, -128, 127, 5])),
            3 => Expression::Double(f64_edge(rng)),
            4 => Expression::Float(f32_edge(rng)),
            5 => Expression::Bool(rng.chance(1, 2)),
            6 if !sc.vars.is_empty() => Expression::Variable(*rng.pick(&sc.vars)),
            7 if !sc.consts.is_empty() => Expression::Constant(*rng.pick(sc.consts)),
            8 if !sc.hosts.is_empty() => Expression::HostInput(*rng.pick(sc.hosts)),
            9 if !sc.tables.is_empty() => Expression::PermutationTable(*rng.pick(sc.tables)),
            10 if !sc.inputs.is_empty() => Expression::DensityVariable(*rng.pick(sc.inputs), None),
            11 => Expression::Ambient(*rng.pick(&[
                AmbientKind::Origin,
                AmbientKind::Pos3,
                AmbientKind::OriginScale,
                AmbientKind::PositionScale,
            ])),
            _ if !sc.vars.is_empty() => Expression::Variable(*rng.pick(&sc.vars)),
            _ => continue,
        };
    }
}

pub fn gen_expr(rng: &mut Rng, sc: &Scope, depth: usize) -> Expression {
    if depth == 0 || rng.chance(1, 4) {
        return gen_leaf(rng, sc);
    }
    let sub = |rng: &mut Rng| gen_expr(rng, sc, depth - 1);
    let subs = |rng: &mut Rng, max: usize| -> Vec<Expression> {
        (0..rng.below(max + 1))
            .map(|_| gen_expr(rng, sc, depth - 1))
            .collect()
    };
    match rng.below(14) {
        0..=3 => Expression::BinaryOp {
            op: gen_op(rng),
            left: Box::new(sub(rng)),
            right: Box::new(sub(rng)),
        },
        4 => Expression::UnaryOp {
            op: gen_unop(rng),
            operand: Box::new(sub(rng)),
        },
        5 if !sc.fns.is_empty() => Expression::FunctionCall {
            function: *rng.pick(sc.fns),
            parameters: subs(rng, 3),
        },
        6 => {
            let parameters = subs(rng, 3);
            Expression::ExternCall {
                function_name: rng.pick(EXTERNS).to_string(),
                parameter_types: parameters.iter().map(|_| gen_type(rng, 1, false)).collect(),
                parameters,
                return_type: gen_type(rng, 1, false),
            }
        }
        7 if !sc.inputs.is_empty() => {
            Expression::DensityVariable(*rng.pick(sc.inputs), Some(Box::new(sub(rng))))
        }
        8 => Expression::Field {
            base: Box::new(sub(rng)),
            field: rng.pick(FIELDS).to_string(),
            type_of_field: gen_type(rng, 1, false),
            base_type: gen_type(rng, 1, false),
        },
        9 => Expression::ArrayAccess {
            array: Box::new(sub(rng)),
            index: Box::new(sub(rng)),
        },
        10 => {
            let t = match rng.below(3) {
                0 => VariableType::Vec3,
                1 => VariableType::Pos3,
                _ => gen_type(rng, 1, false),
            };
            Expression::Construct {
                t,
                args: subs(rng, 3),
            }
        }
        11 => Expression::ConstructExtern {
            // a non-extern `t` is the checker's business, but it must print
            t: if rng.chance(1, 4) {
                gen_type(rng, 1, false)
            } else {
                VariableType::Extern(intern(rng.pick(EXTERNS)))
            },
            args: (0..rng.below(3))
                .map(|_| (intern(rng.pick(FIELDS)), sub(rng)))
                .collect(),
        },
        12 => Expression::ArrayLiteral(subs(rng, 3)),
        _ => Expression::ExplicitCast {
            to: gen_type(rng, 1, false),
            expr: Box::new(sub(rng)),
        },
    }
}

fn gen_const_expr(rng: &mut Rng, earlier: &[Ref<Constant>], depth: usize) -> ConstExpr {
    if depth == 0 || rng.chance(1, 3) {
        return match rng.below(7) {
            0 => ConstExpr::Int(rng.below(100) as i32 - 50),
            1 => ConstExpr::Double(f64_edge(rng)),
            2 => ConstExpr::Float(f32_edge(rng)),
            3 => ConstExpr::Long(i64::MIN),
            4 => ConstExpr::Byte(-128),
            5 if !earlier.is_empty() => ConstExpr::Constant(*rng.pick(earlier)),
            _ => ConstExpr::Bool(rng.chance(1, 2)),
        };
    }
    let b = |rng: &mut Rng| Box::new(gen_const_expr(rng, earlier, depth - 1));
    match rng.below(8) {
        0 | 1 => ConstExpr::BinaryOp {
            op: gen_op(rng),
            left: b(rng),
            right: b(rng),
        },
        2 => ConstExpr::UnaryOp {
            op: gen_unop(rng),
            operand: b(rng),
        },
        3 => ConstExpr::ExplicitCast {
            to: gen_type(rng, 1, false),
            expr: b(rng),
        },
        4 => ConstExpr::Field {
            base: b(rng),
            field: rng.pick(FIELDS).to_string(),
            type_of_field: gen_type(rng, 1, false),
            base_type: gen_type(rng, 1, false),
        },
        5 => ConstExpr::ArrayAccess {
            array: b(rng),
            index: b(rng),
        },
        6 => ConstExpr::ArrayLiteral((0..rng.below(3)).map(|_| *b(rng)).collect()),
        _ => ConstExpr::Construct {
            t: if rng.chance(1, 2) {
                VariableType::Vec3
            } else {
                gen_type(rng, 1, false)
            },
            args: (0..rng.below(4)).map(|_| *b(rng)).collect(),
        },
    }
}

fn gen_stmts(rng: &mut Rng, sc: &Scope, depth: usize, in_loop: bool) -> Vec<Statement> {
    (0..rng.below(4))
        .map(|_| {
            let e = |rng: &mut Rng| gen_expr(rng, sc, 3);
            match rng.below(if depth == 0 { 3 } else { 8 }) {
                0 | 1 if !sc.vars.is_empty() => Statement::Assign {
                    target: *rng.pick(&sc.vars),
                    value: e(rng),
                },
                1 | 0 => Statement::Return(e(rng)),
                2 if in_loop => Statement::Break,
                2 => Statement::Return(e(rng)),
                3 | 4 => Statement::If {
                    condition: e(rng),
                    then_branch: gen_stmts(rng, sc, depth - 1, in_loop),
                    else_branch: match rng.below(3) {
                        0 => vec![],
                        // exactly one `if`: prints as `else if`
                        1 => vec![Statement::If {
                            condition: e(rng),
                            then_branch: gen_stmts(rng, sc, depth - 1, in_loop),
                            else_branch: gen_stmts(rng, sc, depth - 1, in_loop),
                        }],
                        _ => gen_stmts(rng, sc, depth - 1, in_loop),
                    },
                },
                5 | 6 => Statement::While {
                    condition: e(rng),
                    body: gen_stmts(rng, sc, depth - 1, true),
                },
                _ => Statement::Repeat {
                    count: rng.below(5),
                    body: gen_stmts(rng, sc, depth - 1, true),
                },
            }
        })
        .collect()
}

#[derive(Clone)]
pub struct Options {
    /// Hosts and return types are limited to types that have literal values.
    pub valued: bool,
    pub max_units: usize,
    /// Only fns and consts that a library may hold: no hosts, tables, units or entries.
    pub library: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            valued: false,
            max_units: 4,
            library: false,
        }
    }
}

pub fn gen_program(rng: &mut Rng, opts: &Options) -> OptimizedProgram {
    let mut p = OptimizedProgram {
        compute_units: Arena::new(),
        functions: Arena::new(),
        entry_points: vec![],
        constants: Arena::new(),
        host_inputs: Arena::new(),
        permutation_tables: Arena::new(),
    };
    let lib = opts.library;
    let mut hosts = vec![];
    for _ in 0..if lib { 0 } else { rng.below(4) } {
        let h = if !opts.valued && rng.chance(1, 6) {
            HostInput::Dynamic
        } else {
            HostInput::Static {
                name: var_name(rng),
                t: gen_type(rng, 0, opts.valued),
            }
        };
        hosts.push(p.host_inputs.insert(h));
    }
    let mut tables = vec![];
    for _ in 0..if lib { 0 } else { rng.below(3) } {
        let t = if rng.chance(1, 3) {
            PermutationTableInput::Base3DNoise
        } else {
            PermutationTableInput::PerlinNoise {
                ident: name(rng),
                subident: rng.chance(1, 2).then(|| name(rng)),
                subident_index: rng.below(8),
            }
        };
        tables.push(p.permutation_tables.insert(t));
    }
    let mut consts: Vec<Ref<Constant>> = vec![];
    for _ in 0..rng.below(5) {
        let value = gen_const_expr(rng, &consts, 3);
        consts.push(p.constants.insert(Constant {
            value,
            name: name(rng),
        }));
    }

    // functions first, without bodies, so bodies can call any of them
    let n_fns = 1 + rng.below(5);
    let mut fns = vec![];
    for _ in 0..n_fns {
        let mut variables = Arena::new();
        let vars: Vec<Ref<Var>> = (0..rng.below(6))
            .map(|_| {
                variables.insert(Var {
                    name: var_name(rng),
                    t: gen_type(rng, 0, false),
                })
            })
            .collect();
        let parameters = match rng.below(4) {
            1 => vars
                .iter()
                .take(rng.below(vars.len() + 1))
                .copied()
                .collect(),
            2 => {
                let mut v = vars.clone();
                v.reverse();
                v.truncate(rng.below(v.len() + 1));
                v
            }
            // repeats are the checker's business, but must print
            3 => (0..rng.below(3))
                .filter_map(|_| vars.first().copied())
                .collect(),
            _ => vec![],
        };
        let f = p.functions.insert(Function {
            canonical_name: rng.chance(4, 5).then(|| name(rng)),
            host_inputs: hosts.iter().filter(|_| rng.chance(1, 3)).copied().collect(),
            body: vec![],
            variables,
            parameters,
            return_type: gen_type(rng, 0, opts.valued),
            source_hash: if rng.chance(1, 3) { rng.next() } else { 0 },
        });
        fns.push(f);
    }

    // units, then their inputs
    let n_units = if lib {
        0
    } else {
        1 + rng.below(opts.max_units)
    };
    let mut units = vec![];
    for _ in 0..n_units {
        let u = p.compute_units.insert(ComputeUnit::DensityFunction {
            function: *rng.pick(&fns),
            density_inputs: Arena::new(),
            permutation_table_inputs: tables
                .iter()
                .filter(|_| rng.chance(1, 2))
                .copied()
                .collect(),
            host_inputs: hosts.iter().filter(|_| rng.chance(1, 2)).copied().collect(),
            helper_functions: fns.iter().filter(|_| rng.chance(1, 2)).copied().collect(),
            constants: consts
                .iter()
                .filter(|_| rng.chance(1, 2))
                .copied()
                .collect(),
            source_hash: if rng.chance(1, 3) { rng.next() } else { 0 },
        });
        units.push(u);
    }
    let all_vars: Vec<Ref<Var>> = p
        .functions
        .values()
        .flat_map(|f| f.variables.iter().map(|(r, _)| r))
        .collect();
    let mut inputs = vec![];
    if !all_vars.is_empty() {
        for &u in &units {
            let mut arena = Arena::new();
            for _ in 0..rng.below(3) {
                inputs.push(arena.insert(DensityInput {
                    var: *rng.pick(&all_vars),
                    density_function: *rng.pick(&units),
                    scaled_origin: (f64_edge(rng), 1.0, -2.5),
                    scaled_position: (0.5, f64_edge(rng), 1.0),
                    dimensions: (rng.below(5) as i32 - 1, 4, 16),
                }));
            }
            let ComputeUnit::DensityFunction { density_inputs, .. } = &mut p.compute_units[u];
            *density_inputs = arena;
        }
    }

    // bodies
    let fn_list = fns.clone();
    for &f in &fns {
        let vars: Vec<Ref<Var>> = p.functions[f].variables.iter().map(|(r, _)| r).collect();
        // `inputs` must be refs of the *final* arenas, so collect them again from the units
        let live_inputs: Vec<Ref<DensityInput>> = p
            .compute_units
            .values()
            .flat_map(|u| {
                let ComputeUnit::DensityFunction { density_inputs, .. } = u;
                density_inputs.iter().map(|(r, _)| r).collect::<Vec<_>>()
            })
            .collect();
        let sc = Scope {
            vars,
            consts: &consts,
            hosts: &hosts,
            tables: &tables,
            fns: &fn_list,
            inputs: &live_inputs,
        };
        let body = gen_stmts(rng, &sc, 3, false);
        p.functions[f].body = body;
    }
    let _ = inputs;

    for _ in 0..if lib { 0 } else { rng.below(4) } {
        p.entry_points.push(EntryPoint {
            density_function: *rng.pick(&units),
            dimensions: (1 + rng.below(3) as i32, 1 + rng.below(2) as i32, 1),
            scaled_origin: (f64_edge(rng), 0.0, 1.0),
            scaled_position: (1.0, 2.0, f64_edge(rng)),
        });
    }
    p
}

/// A value of type `t`; `t` must be a valued type (no density, …). Records get fields of any
/// valued type, in any order: without a registry they are typed by their literals.
pub fn gen_value(rng: &mut Rng, t: &VariableType) -> Value {
    match t {
        VariableType::Bool => Value::Bool(rng.chance(1, 2)),
        VariableType::I8 => Value::I8(rng.next() as i8),
        VariableType::I32 => Value::I32(rng.next() as i32),
        VariableType::I64 => Value::I64(rng.next() as i64),
        VariableType::F32 => Value::F32(f32_edge(rng)),
        VariableType::F64 => Value::F64(f64_edge(rng)),
        VariableType::Vec3 => Value::Vec3([f64_edge(rng), f64_edge(rng), 1.0]),
        VariableType::Pos3 => Value::Pos3([rng.next() as i32, 0, -1]),
        VariableType::Array(e, n) => Value::Array((0..*n).map(|_| gen_value(rng, e)).collect()),
        VariableType::Extern(name) => {
            let mut fields: Vec<&'static str> = vec![];
            for _ in 0..rng.below(4) {
                let f = intern(rng.pick(FIELDS));
                if !fields.contains(&f) {
                    fields.push(f);
                }
            }
            let fields = fields
                .into_iter()
                .map(|f| {
                    let t = gen_type(rng, 1, true);
                    (f, gen_value(rng, &t))
                })
                .collect();
            Value::Record(name, fields)
        }
        t => panic!("no values of type {t:?}"),
    }
}

/// Wraps a program (generated with `valued: true`) in an envelope.
pub fn gen_test_file(rng: &mut Rng, program: OptimizedProgram) -> TestFile {
    let mut cases = vec![];
    for i in 0..1 + rng.below(2) {
        let hosts = program
            .host_inputs
            .iter()
            .filter_map(|(r, h)| match h {
                HostInput::Static { t, .. } => Some((r, gen_value(rng, t))),
                HostInput::Dynamic => None,
            })
            .collect();
        cases.push(Case {
            name: format!("case {i} \"quoted\"\n"),
            origin: [f64_edge(rng), 0.0, -1.0],
            seed: rng.next(),
            hosts,
            span: Span::default(),
        });
    }
    let mut expects = vec![];
    for (ci, _) in cases.iter().enumerate() {
        for (ei, e) in program.entry_points.iter().enumerate() {
            let ComputeUnit::DensityFunction { function, .. } =
                &program.compute_units[e.density_function];
            let ret = program.functions[*function].return_type.clone();
            let (dx, dy, dz) = e.dimensions;
            let kind = match rng.below(3) {
                0 => ExpectKind::All(gen_value(rng, &ret)),
                1 => ExpectKind::Cell([0, 0, 0], gen_value(rng, &ret)),
                _ => ExpectKind::Grid(
                    (0..(dx.max(0) * dy.max(0) * dz.max(0)))
                        .map(|_| gen_value(rng, &ret))
                        .collect(),
                ),
            };
            if matches!(kind, ExpectKind::Cell(..)) && (dx < 1 || dy < 1 || dz < 1) {
                continue;
            }
            expects.push(Expect {
                case: ci,
                entry: Some(ei),
                kind,
                span: Span::default(),
            });
        }
    }
    TestFile {
        program,
        source_map: SourceMap::default(),
        entry_labels: vec![],
        cases,
        expects,
        reject: None,
        knownfails: if rng.chance(1, 2) {
            vec![KnownFail {
                backend: "cuda".into(),
                reason: "it \"fails\"".into(),
                case: None,
                stage: Some("inlined".into()),
            }]
        } else {
            vec![]
        },
        fuel: rng.chance(1, 2).then(|| rng.next()),
        tier: rng.chance(1, 2).then(|| "full".to_string()),
        cells: rng.chance(1, 2).then(|| CellSample {
            sample: 8,
            seed: rng.next(),
        }),
    }
}

/// A generated value whose `Debug` is its canonical text, so a shrunk failure reads as a program.
pub struct Shown<T>(pub T);

impl fmt::Debug for Shown<OptimizedProgram> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\n{}", print(&self.0))
    }
}

impl fmt::Debug for Shown<TestFile> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\n{}", print_test_file(&self.0))
    }
}

/// The recorded choices proptest generates and shrinks. Most are small, which is all `below`
/// needs and shrinks in a few steps; the rest give raw bits (hashes, float payloads).
pub fn choices() -> impl Strategy<Value = Vec<u64>> {
    proptest::collection::vec(prop_oneof![3 => 0..64u64, 1 => any::<u64>()], 0..3000)
}

/// `cases` cases, with a shrink budget large enough to get through thousands of choices (the
/// automatic one is `4 * cases`). It is only spent on a failure.
pub fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        max_shrink_iters: 200_000,
        ..ProptestConfig::default()
    }
}

pub fn programs(opts: Options) -> impl Strategy<Value = Shown<OptimizedProgram>> {
    choices().prop_map(move |c| Shown(gen_program(&mut Rng::replay(c), &opts)))
}

/// Test files around programs generated with `valued: true`.
pub fn test_files(max_units: usize) -> impl Strategy<Value = Shown<TestFile>> {
    choices().prop_map(move |c| {
        let mut rng = Rng::replay(c);
        let opts = Options {
            valued: true,
            max_units,
            library: false,
        };
        let program = gen_program(&mut rng, &opts);
        Shown(gen_test_file(&mut rng, program))
    })
}
