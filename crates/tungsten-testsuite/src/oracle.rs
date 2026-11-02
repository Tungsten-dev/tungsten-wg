//! Interpreter runs and `@expect` matching (SPEC §3.3, §4.1).

use tungsten_check::{
    CaseResult, EvalError, EvalFailure, EvalOptions, Invalid, NamedCase, Prepared, Value,
    prepare, DEFAULT_FUEL,
};
use tungsten_ir::{HostInput, Name};
use tungsten_parse::{ExpectKind, TestFile};

use crate::env::TestEnv;
use crate::externs::CaseExterns;
use crate::values::{core_eq, show, to_core};

pub fn fuel(tf: &TestFile) -> u64 {
    tf.fuel.unwrap_or(DEFAULT_FUEL)
}

pub fn error_kind_name(e: &EvalError) -> String {
    match e {
        EvalError::DivByZero => "DivByZero".into(),
        EvalError::OutOfBounds => "OutOfBounds".into(),
        EvalError::OutOfFuel => "OutOfFuel".into(),
        EvalError::ExternDomain => "ExternDomain".into(),
        EvalError::Stuck(r) => format!("Stuck({r:?})"),
    }
}

/// The variant names of the lowering and checker errors of a rejection.
pub fn reject_kinds(inv: &Invalid) -> Vec<String> {
    fn variant(dbg: String) -> String {
        dbg.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect()
    }
    match inv {
        Invalid::Lower(e) => e.0.iter().map(|e| variant(format!("{:?}", e.kind))).collect(),
        Invalid::Check(e) => e.0.iter().map(|e| variant(format!("{:?}", e.kind))).collect(),
    }
}

/// Checks a rejected program against `@expect reject[(Kind)]`.
pub fn check_reject(tf: &TestFile, env: &TestEnv) -> Result<(), String> {
    let Some(want) = &tf.reject else { return Err("not a reject file".into()) };
    match prepare(&tf.program, &env.registry) {
        Ok(_) => Err("expected a rejection, but the program was accepted".into()),
        Err(inv) => match want {
            None => Ok(()),
            Some(kind) => {
                let kinds = reject_kinds(&inv);
                if kinds.iter().any(|k| k == kind) {
                    Ok(())
                } else {
                    Err(format!("expected reject({kind}), got {kinds:?}:\n{inv}"))
                }
            }
        },
    }
}

/// Binds hosts by name (SPEC §3.3), so inlining can't invalidate them.
pub fn named_case(tf: &TestFile, i: usize, env: &TestEnv) -> Result<NamedCase, String> {
    let c = &tf.cases[i];
    let mut hosts = vec![];
    for (r, v) in &c.hosts {
        let name = match &tf.program.host_inputs[*r] {
            HostInput::Static { name: Name::Named(n), .. } => n.clone(),
            other => return Err(format!("case `{}`: host {other:?} has no name to bind", c.name)),
        };
        hosts.push((name, to_core(v, &env.registry)?));
    }
    Ok(NamedCase { origin: c.origin, hosts })
}

pub fn run_case(
    prep: &Prepared,
    named: &NamedCase,
    seed: u64,
    opts: EvalOptions,
) -> Result<CaseResult, String> {
    let case = prep.case(named).map_err(|e| e.to_string())?;
    let ext = CaseExterns::new(prep.map(), seed);
    prep.eval(&case, &ext, opts).map_err(|e| match e {
        EvalFailure::Case(c) => format!("case doesn't fit: {c:?}"),
        s @ EvalFailure::Stuck { .. } => s.to_string(),
    })
}

/// Result of every case on the original stage.
pub struct Interp {
    pub prepared: Prepared,
    pub results: Vec<CaseResult>,
}

/// Runs every case with memo on, and again with memo off. A rejected program, `Stuck`, or a memo
/// difference is an error.
pub fn run_all(tf: &TestFile, env: &TestEnv) -> Result<Interp, String> {
    let prepared = prepare(&tf.program, &env.registry).map_err(|e| format!("rejected:\n{e}"))?;
    let mut results = vec![];
    for (i, c) in tf.cases.iter().enumerate() {
        let named = named_case(tf, i, env)?;
        let on = EvalOptions { fuel: fuel(tf), memo: true };
        let off = EvalOptions { fuel: fuel(tf), memo: false };
        let a = run_case(&prepared, &named, c.seed, on).map_err(|e| format!("case `{}`: {e}", c.name))?;
        let b = run_case(&prepared, &named, c.seed, off).map_err(|e| format!("case `{}`: {e}", c.name))?;
        if a != b {
            return Err(format!("case `{}`: memo on and off differ:\n{a:?}\n{b:?}", c.name));
        }
        results.push(a);
    }
    Ok(Interp { prepared, results })
}

/// Lowest entry index, then flat order.
pub fn first_failure(r: &CaseResult) -> Option<(usize, usize, EvalError)> {
    for (e, cells) in r.entries.iter().enumerate() {
        for (k, c) in cells.iter().enumerate() {
            if let Err(err) = c {
                return Some((e, k, *err));
            }
        }
    }
    None
}

fn has_out_of_fuel(r: &CaseResult) -> bool {
    r.entries.iter().flatten().any(|c| matches!(c, Err(EvalError::OutOfFuel)))
}

/// Whether case `i` is expected to fail with `OutOfFuel`.
pub fn expects_out_of_fuel(tf: &TestFile, i: usize) -> bool {
    tf.expects
        .iter()
        .any(|e| e.case == i && matches!(&e.kind, ExpectKind::Error(k) if k == "OutOfFuel"))
}

/// Cases no backend must be sent (SPEC §4.1).
pub fn skipped_for_backends(r: &CaseResult) -> bool {
    has_out_of_fuel(r)
}

/// Cases `backend` must not be sent: `OutOfFuel` everywhere, and on CUDA every case where the
/// interpreter errors, because trap errors are sticky and cost a process (SPEC §6.3).
pub fn skipped_for_backend(r: &CaseResult, backend: &str) -> bool {
    skipped_for_backends(r) || (backend == "cuda" && !(r.failures.is_empty() && r.entries.iter().flatten().all(|c| c.is_ok())))
}

fn flat(c: [i32; 3], dims: (i32, i32, i32)) -> usize {
    (c[0] + c[1] * dims.0 + c[2] * dims.0 * dims.1) as usize
}

/// Matches the `@expect`s of the file against the original-stage results.
pub fn check_expects(tf: &TestFile, env: &TestEnv, run: &Interp) -> Result<(), String> {
    for (i, c) in tf.cases.iter().enumerate() {
        let r = &run.results[i];
        if has_out_of_fuel(r) && !expects_out_of_fuel(tf, i) {
            return Err(format!(
                "case `{}`: OutOfFuel (add `@expect \"{}\" error(OutOfFuel)` if intended)",
                c.name, c.name
            ));
        }
    }
    for e in &tf.expects {
        let case = &tf.cases[e.case];
        let r = &run.results[e.case];
        let at = |msg: String| Err(format!("case `{}`: {msg}", case.name));
        match &e.kind {
            ExpectKind::Error(kind) => match first_failure(r) {
                None => return at(format!("expected error({kind}), but no cell failed")),
                Some((_, _, err)) if error_kind_name(&err) == *kind => {}
                Some((en, k, err)) => {
                    return at(format!(
                        "expected error({kind}), first failing cell (entry {en}, cell {k}) is {}",
                        error_kind_name(&err)
                    ));
                }
            },
            kind => {
                let ent = e.entry.expect("cell expectations name an entry");
                let label = &tf.entry_labels[ent];
                let dims = tf.program.entry_points[ent].dimensions;
                let cells = &r.entries[ent];
                let check = |k: usize, want: &tungsten_parse::Value| -> Result<(), String> {
                    let want = to_core(want, &env.registry)?;
                    match cells.get(k) {
                        None => Err(format!("entry `{label}` has no cell {k}")),
                        Some(Err(err)) => Err(format!(
                            "entry `{label}` cell {k}: expected {}, got error {}",
                            show(&want),
                            error_kind_name(err)
                        )),
                        Some(Ok(got)) if core_eq(got, &want) => Ok(()),
                        Some(Ok(got)) => Err(format!(
                            "entry `{label}` cell {k}: expected {}, got {}",
                            show(&want),
                            show(got)
                        )),
                    }
                };
                let res = match kind {
                    ExpectKind::Cell(c, v) => check(flat(*c, dims), v),
                    ExpectKind::All(v) => (0..cells.len()).try_for_each(|k| check(k, v)),
                    ExpectKind::Grid(vs) => vs.iter().enumerate().try_for_each(|(k, v)| check(k, v)),
                    ExpectKind::Error(_) => unreachable!(),
                };
                if let Err(m) = res {
                    return at(m);
                }
            }
        }
    }
    Ok(())
}

/// Entries only: equal cell by cell with `core_eq`. Const and input ids can differ after
/// inlining, so `failures` aren't compared.
pub fn same_entries(a: &CaseResult, b: &CaseResult) -> Result<(), String> {
    if a.entries.len() != b.entries.len() {
        return Err("different entry counts".into());
    }
    for (e, (x, y)) in a.entries.iter().zip(&b.entries).enumerate() {
        if x.len() != y.len() {
            return Err(format!("entry {e}: different cell counts"));
        }
        for (k, (p, q)) in x.iter().zip(y).enumerate() {
            let same = match (p, q) {
                (Ok(p), Ok(q)) => core_eq(p, q),
                (Err(p), Err(q)) => p == q,
                _ => false,
            };
            if !same {
                let s = |c: &Result<Value, EvalError>| match c {
                    Ok(v) => show(v),
                    Err(e) => format!("error {}", error_kind_name(e)),
                };
                return Err(format!("entry {e} cell {k}: {} vs {}", s(p), s(q)));
            }
        }
    }
    Ok(())
}

/// L5: inline, prepare again and compare with the original-stage results.
pub fn check_inline(tf: &TestFile, env: &TestEnv, run: &Interp) -> Result<(), String> {
    let mut program = tf.program.clone();
    tungsten_optimizer::perform_inline(&mut program);
    let prepared = prepare(&program, &env.registry).map_err(|e| format!("inlined program rejected:\n{e}"))?;
    for (i, c) in tf.cases.iter().enumerate() {
        let named = named_case(tf, i, env)?;
        let opts = EvalOptions { fuel: fuel(tf), memo: true };
        let r = run_case(&prepared, &named, c.seed, opts).map_err(|e| format!("case `{}` (inlined): {e}", c.name))?;
        same_entries(&run.results[i], &r).map_err(|m| format!("case `{}`: inlined differs: {m}", c.name))?;
    }
    Ok(())
}
