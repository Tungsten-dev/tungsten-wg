//! "No Stuck": proven in `tungsten-core` (T4), and run here as an integration check of what the
//! proof trusts, the primitives and externs, on the lowering's output. Checker-accepted programs are
//! mutated at random on the core IR (literals, operators, local and const ids, statements,
//! types). Those the checker still accepts are evaluated on random cases with toy externs that
//! respect their decls, and no result may contain `Stuck`.
//!
//! The same mutated programs drive the memo and fuel properties (`memo.rs`).

use proptest::prelude::*;
use tungsten_core::check::{check, check_case};
use tungsten_core::exec::{CaseResult, Failure, eval_program};
use tungsten_core::*;
use tungsten_core::Function;

use super::eval::eval_registry;
use super::*;

/// Sources of checker-accepted programs (small grids, so memo-off stays cheap).
pub fn sources() -> Vec<String> {
    let mut out: Vec<String> = vec![
        include_str!("../../../tungsten-parse/tests/corpus/testing_md.tw").to_string(),
        snippet(
            "fn mk(x: f64) -> extern Pair { return new extern Pair { a: x, b: 1 }; }",
            "let q: extern Pair = mk(p); return (q.a + ext sin(ext sample(table(t), density(d))));",
        ),
        snippet(
            "",
            "let n: i32 = 0; let acc: f64 = p;
             repeat 3 { n = (n + 1); if (n > 1) { break; } acc = (acc * 2.0); }
             while (acc < 10.0) { acc = (acc + host(h)); if (acc > 50.0) { return acc; } }
             let v: vec3 = (ambient(origin) + ambient(pos3));
             let i: i64 = ((n as i64) << 3);
             if ((i > 5L) && (v.x >= 0.0)) { acc = (acc - (i as f64)); } else { acc = -(acc); }
             return (acc / const(c));",
        ),
    ];
    out.push(
        "program {
            fn leaf() -> f64 { let c: pos3 = ambient(pos3); return ((c.x + (10 * c.y)) as f64); }
            fn top() -> f64 { return (density(t.d) + density(t.d, 1)); }
            unit leaf_u { main leaf; }
            unit t { main top; input d = leaf_u @dims(2, 2, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1); }
            entry t @dims(2, 2, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
        }"
        .to_string(),
    );
    out.push(
        "program {
            const a = 3;
            const b = ((const(a) * 7) / 2);
            fn m() -> i32 {
                let x: i32 = const(b);
                let arr: [i32; 3] = [x, (x >> 1), (x ^ 5)];
                return arr[0];
            }
            unit mu { main m; consts [a, b]; }
            entry mu @dims(3, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
        }"
        .to_string(),
    );
    out
}

pub fn accepted() -> Vec<Program> {
    let r = eval_registry();
    sources()
        .iter()
        .map(|s| {
            let l = crate::lower(&load(s), &r).unwrap_or_else(|e| panic!("{e}\n{s}"));
            check(&l.program).unwrap_or_else(|e| panic!("{e:?}\n{s}"));
            l.program
        })
        .collect()
}

/// splitmix64.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

const OPS: [BinOp; 18] = [
    BinOp::Add, BinOp::Subtract, BinOp::Multiply, BinOp::Divide, BinOp::Equal, BinOp::NotEqual,
    BinOp::Less, BinOp::LessEqual, BinOp::Greater, BinOp::GreaterEqual, BinOp::And, BinOp::Or,
    BinOp::ShiftLeft, BinOp::ShiftRight, BinOp::LogicalShiftRight, BinOp::BitOr, BinOp::BitAnd,
    BinOp::BitXor,
];

fn rand_type(rng: &mut Rng) -> Type {
    match rng.below(9) {
        0 => Type::Bool,
        1 => Type::I8,
        2 => Type::I32,
        3 => Type::I64,
        4 => Type::F32,
        5 => Type::F64,
        6 => Type::Vec3,
        7 => Type::Pos3,
        _ => Type::Array(Box::new(Type::I32), 1 + rng.below(3)),
    }
}

fn rand_lit(rng: &mut Rng) -> Lit {
    let specials = [0.0f64, -0.0, 1.0, -1.0, f64::NAN, f64::INFINITY, 1e300, 0.5];
    match rng.below(6) {
        0 => Lit::Bool(rng.below(2) == 0),
        1 => Lit::I8(rng.next() as i8),
        2 => Lit::I32([0, 1, -1, i32::MIN, i32::MAX, 2, 31, 32][rng.below(8)]),
        3 => Lit::I64(rng.next() as i64),
        4 => Lit::F32((specials[rng.below(8)] as f32).to_bits()),
        _ => Lit::F64(specials[rng.below(8)].to_bits()),
    }
}

/// Visits expressions in pre-order; `target` counts down to the node to mutate.
struct M<'r> {
    rng: &'r mut Rng,
    target: usize,
    nlocals: usize,
    nconsts: usize,
}

impl M<'_> {
    fn expr(&mut self, e: &mut Expr) {
        if self.target == 0 {
            self.target = usize::MAX;
            self.apply(e);
            return;
        }
        self.target -= 1;
        match e {
            Expr::Density { index: Some(i), .. } => self.expr(i),
            Expr::Call { args, .. } | Expr::ExternCall { args, .. } | Expr::ArrayLit(args) => {
                args.iter_mut().for_each(|a| self.expr(a))
            }
            Expr::Binary { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => self.expr(operand),
            Expr::Field { base, .. } => self.expr(base),
            Expr::Index { array, index } => {
                self.expr(array);
                self.expr(index);
            }
            Expr::MakeVec3 { x, y, z } | Expr::MakePos3 { x, y, z } => {
                self.expr(x);
                self.expr(y);
                self.expr(z);
            }
            Expr::MakeRecord { fields, .. } => fields.iter_mut().for_each(|f| self.expr(&mut f.value)),
            _ => {}
        }
    }

    fn apply(&mut self, e: &mut Expr) {
        let rng = &mut *self.rng;
        match e {
            Expr::Lit(l) => *l = rand_lit(rng),
            Expr::Local(l) => l.0 = rng.below(self.nlocals.max(1)),
            Expr::Const(c) => c.0 = rng.below(self.nconsts.max(1)),
            Expr::Binary { op, .. } => *op = OPS[rng.below(OPS.len())].clone(),
            Expr::Cast { to, .. } => *to = rand_type(rng),
            Expr::Unary { op, .. } => *op = if rng.below(2) == 0 { UnOp::Negate } else { UnOp::Not },
            other => {
                // Wrap in a cast or a negation, or replace by a literal.
                let inner = std::mem::replace(other, Expr::Lit(Lit::Bool(false)));
                *other = match rng.below(3) {
                    0 => Expr::Cast { to: rand_type(rng), operand: Box::new(inner) },
                    1 => Expr::Unary { op: UnOp::Negate, operand: Box::new(inner) },
                    _ => Expr::Lit(rand_lit(rng)),
                };
            }
        }
    }

    fn stmts(&mut self, ss: &mut [Stmt]) {
        for s in ss.iter_mut() {
            match s {
                Stmt::Assign { value, .. } | Stmt::Return(value) => self.expr(value),
                Stmt::If { cond, then_branch, else_branch } => {
                    self.expr(cond);
                    self.stmts(then_branch);
                    self.stmts(else_branch);
                }
                Stmt::While { cond, body } => {
                    self.expr(cond);
                    self.stmts(body);
                }
                Stmt::Repeat { body, .. } => self.stmts(body),
                Stmt::Break => {}
            }
        }
    }
}

fn count_lists(ss: &[Stmt]) -> usize {
    1 + ss
        .iter()
        .map(|s| match s {
            Stmt::If { then_branch, else_branch, .. } => count_lists(then_branch) + count_lists(else_branch),
            Stmt::While { body, .. } | Stmt::Repeat { body, .. } => count_lists(body),
            _ => 0,
        })
        .sum::<usize>()
}

/// Runs `f` on the `n`-th statement list in pre-order. Returns `true` once it has.
#[allow(clippy::ptr_arg)] // `f` inserts and removes.
fn edit_list(ss: &mut Vec<Stmt>, n: &mut usize, f: &mut dyn FnMut(&mut Vec<Stmt>)) -> bool {
    if *n == 0 {
        f(ss);
        return true;
    }
    *n -= 1;
    for s in ss.iter_mut() {
        let done = match s {
            Stmt::If { then_branch, else_branch, .. } => edit_list(then_branch, n, f) || edit_list(else_branch, n, f),
            Stmt::While { body, .. } | Stmt::Repeat { body, .. } => edit_list(body, n, f),
            _ => false,
        };
        if done {
            return true;
        }
    }
    false
}

/// One random mutation of `p`.
pub fn mutate(p: &mut Program, rng: &mut Rng) {
    if p.functions.is_empty() {
        return;
    }
    let nconsts = p.consts.len();
    let fi = rng.below(p.functions.len());
    let f: &mut Function = &mut p.functions[fi];
    match rng.below(10) {
        // Statement edits: drop, duplicate or swap.
        0..=1 => {
            let mut n = rng.below(count_lists(&f.body));
            let (a, b, kind) = (rng.next() as usize, rng.next() as usize, rng.below(3));
            edit_list(&mut f.body, &mut n, &mut |l: &mut Vec<Stmt>| {
                if l.is_empty() {
                    return;
                }
                let i = a % l.len();
                match kind {
                    0 => {
                        l.remove(i);
                    }
                    1 => {
                        let s = l[i].clone();
                        l.insert(i, s);
                    }
                    _ => {
                        let j = b % l.len();
                        l.swap(i, j);
                    }
                }
            });
        }
        // A local's type.
        2 => {
            if !f.locals.is_empty() {
                let i = rng.below(f.locals.len());
                f.locals[i] = rand_type(rng);
            }
        }
        // An expression node.
        _ => {
            let target = rng.below(64);
            let nlocals = f.locals.len();
            let mut m = M { rng, target, nlocals, nconsts };
            m.stmts(&mut f.body);
        }
    }
}

/// A random value of `t`.
pub fn rand_value(p: &Program, t: &Type, rng: &mut Rng) -> Value {
    match t {
        Type::Bool => Value::Bool(rng.below(2) == 0),
        Type::I8 => Value::I8(rng.next() as i8),
        Type::I32 => Value::I32((rng.next() as i32) % 64),
        Type::I64 => Value::I64(rng.next() as i64),
        Type::F32 => Value::F32(((rng.below(200) as f32) - 100.0).to_bits()),
        Type::F64 => Value::F64(((rng.below(200) as f64) / 8.0 - 12.0).to_bits()),
        Type::Vec3 => Value::Vec3(F64x3 { x: 0, y: 1.0f64.to_bits(), z: (-2.0f64).to_bits() }),
        Type::Pos3 => Value::Pos3(I32x3 { x: 1, y: -1, z: 0 }),
        Type::Array(inner, n) => Value::Array((0..*n).map(|_| rand_value(p, inner, rng)).collect()),
        Type::Record(r) => Value::Record(
            *r,
            p.records.get(r.0).map(|d| d.fields.iter().map(|f| rand_value(p, f, rng)).collect()).unwrap_or_default(),
        ),
        Type::PermTable | Type::Sampler => {
            let want = if matches!(t, Type::PermTable) { TableKind::Perm } else { TableKind::Sampler };
            let id = p.tables.iter().position(|d| d.kind == want).unwrap_or(0);
            if want == TableKind::Perm { Value::PermTable(TableId(id)) } else { Value::Sampler(TableId(id)) }
        }
    }
}

/// Externs that return a value of the declared type, and a domain error (only where declared)
/// for a negative first argument.
pub struct DeclExterns<'a>(pub &'a Program);

impl Externs for DeclExterns<'_> {
    fn call(&self, ext: ExternId, args: &[Value]) -> Result<Value, ExternFail> {
        let d = &self.0.externs[ext.0];
        if let (true, Some(Value::F64(b))) = (d.domain_error, args.first())
            && f64::from_bits(*b) < 0.0
        {
            return Err(ExternFail::Domain);
        }
        let mut rng = Rng(ext.0 as u64 ^ args.len() as u64);
        Ok(rand_value(self.0, &d.ret, &mut rng))
    }
}

pub fn rand_case(p: &Program, rng: &mut Rng) -> Case {
    Case {
        origin: F64x3 {
            x: ((rng.below(100) as f64) - 50.0).to_bits(),
            y: 0,
            z: (rng.below(7) as f64).to_bits(),
        },
        hosts: p.hosts.iter().map(|h| rand_value(p, &h.ty, rng)).collect(),
    }
}

pub fn eval(p: &Program, case: &Case, fuel: u64, memo: bool) -> CaseResult {
    eval_program(p, case, &DeclExterns(p), fuel, memo)
}

pub fn has_stuck(r: &CaseResult) -> bool {
    r.entries.iter().flatten().any(|c| matches!(c, Err(EvalError::Stuck(_))))
        || r.failures.iter().any(|f| match f {
            Failure::Const { error, .. } | Failure::Input { error, .. } => matches!(error, EvalError::Stuck(_)),
        })
}

/// A mutated, still checker-accepted program from `prog` and `seed`, if the mutations kept it
/// valid.
pub fn mutated(progs: &[Program], prog: usize, seed: u64, steps: usize) -> Option<Program> {
    let mut rng = Rng(seed);
    let mut p = progs[prog % progs.len()].clone();
    for _ in 0..steps {
        mutate(&mut p, &mut rng);
    }
    check(&p).ok().map(|_| p)
}

#[test]
fn corpus_has_no_stuck() {
    let mut rng = Rng(1);
    for p in accepted() {
        for _ in 0..4 {
            let case = rand_case(&p, &mut rng);
            check_case(&p, &case).unwrap();
            let r = eval(&p, &case, 1000, true);
            assert!(!has_stuck(&r), "{r:?}");
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn mutants_never_get_stuck(prog in 0usize..16, seed in any::<u64>(), steps in 1usize..4) {
        let progs = accepted();
        if let Some(p) = mutated(&progs, prog, seed, steps) {
            let mut rng = Rng(seed ^ 0xabcd);
            for _ in 0..2 {
                let case = rand_case(&p, &mut rng);
                if check_case(&p, &case).is_err() {
                    continue;
                }
                let r = eval(&p, &case, 200, true);
                prop_assert!(!has_stuck(&r), "stuck: {:?}\nprogram: {:?}", r, p);
            }
        }
    }
}

#[test]
fn mutants_are_often_accepted() {
    // The property above is only meaningful if many mutants still pass the checker.
    let progs = accepted();
    let n: u64 = 400;
    let kept = (0..n).filter(|s| mutated(&progs, *s as usize, *s * 7919, 1 + (*s as usize % 3)).is_some()).count();
    eprintln!("accepted mutants: {kept}/{n}");
    assert!(kept as u64 * 5 >= n, "only {kept} of {n} mutants pass the checker");
}
