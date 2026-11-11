//! Interpreter runs and `@expect` matching (crate README, "Rules and contracts").

use tungsten_check::{
    AutoArgValue, CaseResult, DEFAULT_FUEL, EvalError, EvalFailure, EvalOptions, EvaluateError, Invalid,
    NamedCase, Prepared, RefineCall, SequenceCase, SequenceResult, Value, prepare,
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
        dbg.chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    }
    match inv {
        Invalid::Lower(e) => {
            e.0.iter()
                .map(|e| variant(format!("{:?}", e.kind)))
                .collect()
        }
        Invalid::Check(e) => {
            e.0.iter()
                .map(|e| variant(format!("{:?}", e.kind)))
                .collect()
        }
    }
}

/// Checks a rejected program against `@expect reject[(Kind)]`.
pub fn check_reject(tf: &TestFile, env: &TestEnv) -> Result<(), String> {
    let Some(want) = &tf.reject else {
        return Err("not a reject file".into());
    };
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

/// Binds hosts by name (crate README, "Rules and contracts"), so inlining can't invalidate them.
pub fn named_case(
    tf: &TestFile,
    i: usize,
    env: &TestEnv,
) -> Result<(NamedCase, Vec<(String, Value)>), String> {
    let c = &tf.cases[i];
    let params = c
        .params
        .iter()
        .map(|(r, v)| {
            Ok((
                tf.program.auto_params[*r].name.clone(),
                to_core(v, &env.registry)?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut hosts = vec![];
    for (r, v) in &c.hosts {
        let name = match &tf.program.host_inputs[*r] {
            HostInput::Static {
                name: Name::Named(n),
                ..
            } => n.clone(),
            other => {
                return Err(format!(
                    "case `{}`: host {other:?} has no name to bind",
                    c.name
                ));
            }
        };
        hosts.push((name, to_core(v, &env.registry)?));
    }
    Ok((
        NamedCase {
            origin: c.origin,
            hosts,
            auto_values: vec![],
        },
        params,
    ))
}

/// The host name of `r`, for binding by name.
fn host_name(tf: &TestFile, r: tungsten_ir::Ref<HostInput>) -> Result<String, String> {
    match &tf.program.host_inputs[r] {
        HostInput::Static { name: Name::Named(n), .. } => Ok(n.clone()),
        other => Err(format!("host {other:?} has no name to bind")),
    }
}

/// Case `i` with its `@refine` calls. Each call gets every host: the ones it binds, and the rest
/// carried over from the call before (the case's own for the first).
pub fn sequence_case(
    tf: &TestFile,
    i: usize,
    env: &TestEnv,
) -> Result<(SequenceCase, Vec<(String, Value)>), String> {
    let (run, params) = named_case(tf, i, env)?;
    let mut hosts = run.hosts.clone();
    let mut steps = vec![];
    for call in &tf.cases[i].refines {
        for (r, v) in &call.hosts {
            let name = host_name(tf, *r)?;
            let v = to_core(v, &env.registry)?;
            match hosts.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = v,
                None => hosts.push((name, v)),
            }
        }
        steps.push(RefineCall { refinement: call.refinement, offset: call.offset, hosts: hosts.clone() });
    }
    Ok((SequenceCase { run, steps }, params))
}

/// Runs case `i`'s orchestrate step and its refine calls.
pub fn run_sequence(
    prep: &Prepared,
    case: &SequenceCase,
    params: &[(String, Value)],
    opts: EvalOptions,
) -> Result<SequenceResult, String> {
    let params = params
        .iter()
        .map(|(n, v)| {
            let v = match v {
                Value::I64(x) => AutoArgValue::I64(*x),
                Value::F64(b) => AutoArgValue::F64(f64::from_bits(*b)),
                Value::Bool(b) => AutoArgValue::Bool(*b),
                other => return Err(format!("param `{n}`: {other:?} isn't a param value")),
            };
            Ok((n.clone(), v))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let inits = prep
        .map()
        .resolved_auto_consts(&params)
        .map_err(|e| e.to_string())?;
    let (ext, auto_values) = CaseExterns::new(prep.map(), &inits)?;
    let mut case = case.clone();
    case.run.auto_values = auto_values;
    prep.eval_sequence(&case, &ext, opts).map_err(|e| match e {
        EvaluateError::Eval(EvalFailure::Case(c)) => format!("case doesn't fit: {c:?}"),
        e => e.to_string(),
    })
}

pub fn run_case(
    prep: &Prepared,
    named: &NamedCase,
    params: &[(String, Value)],
    opts: EvalOptions,
) -> Result<CaseResult, String> {
    let params = params
        .iter()
        .map(|(n, v)| {
            let v = match v {
                Value::I64(x) => AutoArgValue::I64(*x),
                Value::F64(b) => AutoArgValue::F64(f64::from_bits(*b)),
                Value::Bool(b) => AutoArgValue::Bool(*b),
                other => return Err(format!("param `{n}`: {other:?} isn't a param value")),
            };
            Ok((n.clone(), v))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let inits = prep
        .map()
        .resolved_auto_consts(&params)
        .map_err(|e| e.to_string())?;
    let (ext, auto_values) = CaseExterns::new(prep.map(), &inits)?;
    let mut named = named.clone();
    named.auto_values = auto_values;
    let case = prep.case(&named).map_err(|e| e.to_string())?;
    prep.eval(&case, &ext, opts).map_err(|e| match e {
        EvalFailure::Case(c) => format!("case doesn't fit: {c:?}"),
        s @ EvalFailure::Stuck { .. } => s.to_string(),
    })
}

/// Result of every case on the original stage.
pub struct Interp {
    pub prepared: Prepared,
    /// Orchestrate, per case, hidden entries included.
    pub results: Vec<CaseResult>,
    /// Per case, one per `@refine` that ran (a failing step ends the sequence).
    pub steps: Vec<Vec<CaseResult>>,
}

/// Runs every case with memo on, and again with memo off. A rejected program, `Stuck`, or a memo
/// difference is an error.
pub fn run_all(tf: &TestFile, env: &TestEnv) -> Result<Interp, String> {
    let prepared = prepare(&tf.program, &env.registry).map_err(|e| format!("rejected:\n{e}"))?;
    let mut results = vec![];
    let mut steps = vec![];
    for (i, c) in tf.cases.iter().enumerate() {
        let (named, params) = sequence_case(tf, i, env)?;
        let on = EvalOptions {
            fuel: fuel(tf),
            memo: true,
        };
        let off = EvalOptions {
            fuel: fuel(tf),
            memo: false,
        };
        let a = run_sequence(&prepared, &named, &params, on)
            .map_err(|e| format!("case `{}`: {e}", c.name))?;
        let b = run_sequence(&prepared, &named, &params, off)
            .map_err(|e| format!("case `{}`: {e}", c.name))?;
        if a != b {
            return Err(format!(
                "case `{}`: memo on and off differ:\n{a:?}\n{b:?}",
                c.name
            ));
        }
        results.push(a.run);
        steps.push(a.steps);
    }
    Ok(Interp { prepared, results, steps })
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
    r.entries
        .iter()
        .flatten()
        .any(|c| matches!(c, Err(EvalError::OutOfFuel)))
}

/// Whether case `i` is expected to fail with `OutOfFuel` at orchestrate.
pub fn expects_out_of_fuel(tf: &TestFile, i: usize) -> bool {
    expects_out_of_fuel_at(tf, i, 0)
}

/// Whether case `i` is expected to fail with `OutOfFuel` at step `n` (0 is orchestrate).
fn expects_out_of_fuel_at(tf: &TestFile, i: usize, n: usize) -> bool {
    tf.expects.iter().any(|e| {
        e.case == i && e.after.unwrap_or(0) == n && matches!(&e.kind, ExpectKind::Error(k) if k == "OutOfFuel")
    })
}

/// Cases no backend must be sent (crate README, "Rules and contracts").
pub fn skipped_for_backends(r: &CaseResult) -> bool {
    has_out_of_fuel(r)
}

/// Cases `backend` must not be sent: `OutOfFuel` everywhere, and on CUDA every case where the
/// interpreter errors, because trap errors are sticky and cost a process (crate README, "Rules and contracts").
pub fn skipped_for_backend(r: &CaseResult, backend: &str) -> bool {
    skipped_for_backends(r)
        || (backend == "cuda"
            && !(r.failures.is_empty() && r.entries.iter().flatten().all(|c| c.is_ok())))
}

fn flat(c: [i32; 3], dims: (i32, i32, i32)) -> usize {
    (c[0] + c[1] * dims.0 + c[2] * dims.0 * dims.1) as usize
}

/// Matches the `@expect`s of the file against the original-stage results.
pub fn check_expects(tf: &TestFile, env: &TestEnv, run: &Interp) -> Result<(), String> {
    for (i, c) in tf.cases.iter().enumerate() {
        let steps = std::iter::once((0, &run.results[i])).chain(run.steps[i].iter().enumerate().map(|(n, r)| (n + 1, r)));
        for (n, r) in steps {
            if has_out_of_fuel(r) && !expects_out_of_fuel_at(tf, i, n) {
                let after = if n == 0 { String::new() } else { format!("after({n}) ") };
                return Err(format!(
                    "case `{}`: OutOfFuel (add `@expect \"{}\" {after}error(OutOfFuel)` if intended)",
                    c.name, c.name
                ));
            }
        }
    }
    for e in &tf.expects {
        let case = &tf.cases[e.case];
        let at = |msg: String| Err(format!("case `{}`: {msg}", case.name));
        let r = match e.after {
            None => &run.results[e.case],
            Some(n) => match run.steps[e.case].get(n - 1) {
                Some(r) => r,
                None => {
                    return at(format!(
                        "`after({n})`, but the sequence ended after {} refine calls, since a step failed",
                        run.steps[e.case].len()
                    ));
                }
            },
        };
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
                // A refine step's entry 0 is its refinement's entry.
                let cells = &r.entries[if e.after.is_some() { 0 } else { ent }];
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
                    ExpectKind::Grid(vs) => {
                        vs.iter().enumerate().try_for_each(|(k, v)| check(k, v))
                    }
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
    let prepared =
        prepare(&program, &env.registry).map_err(|e| format!("inlined program rejected:\n{e}"))?;
    for (i, c) in tf.cases.iter().enumerate() {
        let (named, params) = sequence_case(tf, i, env)?;
        let opts = EvalOptions {
            fuel: fuel(tf),
            memo: true,
        };
        let r = run_sequence(&prepared, &named, &params, opts)
            .map_err(|e| format!("case `{}` (inlined): {e}", c.name))?;
        same_entries(&run.results[i], &r.run)
            .map_err(|m| format!("case `{}`: inlined differs: {m}", c.name))?;
        if r.steps.len() != run.steps[i].len() {
            return Err(format!(
                "case `{}`: inlined runs {} refine calls, the original {}",
                c.name,
                r.steps.len(),
                run.steps[i].len()
            ));
        }
        for (n, (a, b)) in run.steps[i].iter().zip(&r.steps).enumerate() {
            same_entries(a, b).map_err(|m| format!("case `{}`: inlined differs after({}): {m}", c.name, n + 1))?;
        }
    }
    Ok(())
}
