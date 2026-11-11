//! `prepare` (lower + check, once per program) and `Prepared::eval` (once per case), plus the
//! one-shot `evaluate`.

use std::collections::{HashMap, HashSet};

use tungsten_core::check::{CaseErrors, check, check_case};
use tungsten_core::exec::{CaseResult, Failure, eval_program, new_memo};
use tungsten_core::{Case, EvalError, ExternId, Externs, F64x3, HostId, I32x3, Program, StuckReason, Value};
use tungsten_ir::OptimizedProgram;

use crate::check::{CheckErrors, Invalid, render};
use crate::map::{Instance, LowerMap};
use crate::registry::Registry;
use crate::{Lowered, lower};

/// `While` fuel when nothing else is asked for: one `While` execution may run this many
/// iterations.
pub const DEFAULT_FUEL: u64 = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalOptions {
    pub fuel: u64,
    /// `false` recomputes every const and input read (test-only; results are identical).
    pub memo: bool,
}

impl Default for EvalOptions {
    fn default() -> Self {
        EvalOptions { fuel: DEFAULT_FUEL, memo: true }
    }
}

/// A case with hosts bound by name (`HostInput::Static` names).
#[derive(Debug, Clone, PartialEq)]
pub struct NamedCase {
    pub origin: [f64; 3],
    pub hosts: Vec<(String, Value)>,
    /// The values of the value autoconsts, by the host they lower to
    /// (`AutoConstInfo::core`). Separate from `hosts`, because autoconst names can collide with
    /// host names and with each other.
    pub auto_values: Vec<(HostId, Value)>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaseBindError {
    #[error("the program has no host `{0}`")]
    Unknown(String),
    #[error("no value for host `{0}`")]
    Missing(String),
    #[error("host `{0}` is given more than once")]
    Duplicate(String),
    #[error("no value for autoconst `{name}`")]
    AutoValueMissing { name: String },
    /// A `HostId` that isn't a value autoconst, or is given twice.
    #[error("`auto_values` has an entry for host #{} that isn't a value autoconst, or has it twice", .0.0)]
    AutoValueUnknown(HostId),
    /// The bound case fails `check_case` (a value of the wrong type).
    #[error("the case doesn't fit the program: {0:?}")]
    Invalid(CaseErrors),
    #[error("the program has no refinement #{0}")]
    UnknownRefinement(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvalFailure {
    /// `check_case` failed; nothing ran.
    #[error("the case doesn't fit the program: {0:?}")]
    Case(CaseErrors),
    /// Evaluation got stuck: an interpreter or checker bug. `reasons` are `(entry, cell,
    /// reason)` for the cells; `failures` are the const/input diagnostics that hold `Stuck`.
    #[error("evaluation got stuck (an interpreter or checker bug): {reasons:?} {failures:?}")]
    Stuck {
        reasons: Vec<(usize, usize, StuckReason)>,
        failures: Vec<Failure>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvaluateError {
    #[error("{0}")]
    Invalid(Invalid),
    #[error("{0}")]
    Bind(CaseBindError),
    #[error("{0}")]
    Eval(EvalFailure),
}

/// A lowered program that passed the checker.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    lowered: Lowered,
}

/// Lowers `p` against `reg` and runs the checker. Do this once per program.
pub fn prepare(p: &OptimizedProgram, reg: &Registry) -> Result<Prepared, Invalid> {
    let lowered = lower(p, reg).map_err(Invalid::Lower)?;
    if let Err(errs) = check(&lowered.program) {
        return Err(Invalid::Check(render(&lowered.program, &lowered.map, errs)));
    }
    for step in &lowered.steps {
        if let Err(errs) = check(&step.program) {
            return Err(Invalid::Check(render(&step.program, &step.map, errs)));
        }
    }
    Ok(Prepared { lowered })
}

/// Runs only the checker on an already lowered program (hand-built tests).
pub fn check_lowered(l: &Lowered) -> Result<(), CheckErrors> {
    check(&l.program).map_err(|e| render(&l.program, &l.map, e))
}

impl Prepared {
    pub fn program(&self) -> &Program {
        &self.lowered.program
    }

    pub fn map(&self) -> &LowerMap {
        &self.lowered.map
    }

    pub fn host_id(&self, name: &str) -> Option<HostId> {
        self.lowered
            .map
            .host_names
            .iter()
            .position(|n| n.as_deref() == Some(name))
            .map(HostId)
    }

    pub fn extern_id(&self, name: &str) -> Option<ExternId> {
        self.lowered.map.externs.iter().position(|n| n == name).map(ExternId)
    }

    /// Binds hosts by name, then runs `check_case`.
    pub fn case(&self, c: &NamedCase) -> Result<Case, CaseBindError> {
        bind(self.program(), self.map(), c, vec![])
    }

    pub fn lowered(&self) -> &Lowered {
        &self.lowered
    }

    /// Runs `check_case`, then the interpreter. Any `Stuck` is an `Err`. The result has the
    /// hidden entries after the visible ones (`Lowered::visible`).
    pub fn eval<E: Externs>(
        &self,
        case: &Case,
        ext: &E,
        opts: EvalOptions,
    ) -> Result<CaseResult, EvalFailure> {
        eval_checked(self.program(), case, ext, opts)
    }

    /// Runs the orchestrate step, then each refine call in order, keeping one store of buffers
    /// per entry between them (`specs/refinement.md` §7.1). A step in which any cell fails ends
    /// the sequence: later calls don't run.
    pub fn eval_sequence<E: Externs>(
        &self,
        case: &SequenceCase,
        ext: &E,
        opts: EvalOptions,
    ) -> Result<SequenceResult, EvaluateError> {
        let l = &self.lowered;
        let run_case = self.case(&case.run).map_err(EvaluateError::Bind)?;
        let run = self.eval(&run_case, ext, opts).map_err(EvaluateError::Eval)?;
        let mut out = SequenceResult { run, steps: vec![] };
        if case.steps.is_empty() || failed(&out.run) {
            return Ok(out);
        }
        let mut stores: HashMap<usize, HashMap<Instance, Vec<Value>>> = HashMap::new();
        for step in &l.steps {
            let store = stores.entry(step.entry).or_default();
            for (_, inst) in &step.buffers {
                if store.contains_key(inst) {
                    continue;
                }
                let k = l.instances.iter().position(|i| i == inst).expect("run computes every buffer a step reads");
                store.insert(*inst, ok_cells(&out.run.entries[k]));
            }
        }
        for call in &case.steps {
            let step = l.steps.get(call.refinement).ok_or(EvaluateError::Bind(CaseBindError::UnknownRefinement(call.refinement)))?;
            let store = stores.get_mut(&step.entry).expect("every step's entry has a store");
            let [x, y, z] = call.offset;
            let mut extra = vec![Value::Pos3(I32x3 { x, y, z })];
            for (_, inst) in &step.buffers {
                extra.push(Value::Array(store[inst].clone()));
            }
            let named = NamedCase { origin: case.run.origin, hosts: call.hosts.clone(), auto_values: case.run.auto_values.clone() };
            let c = bind(&step.program, &step.map, &named, extra).map_err(EvaluateError::Bind)?;
            let r = eval_checked(&step.program, &c, ext, opts).map_err(EvaluateError::Eval)?;
            let stop = failed(&r);
            if !stop {
                for (k, inst) in step.running.iter().enumerate() {
                    store.insert(*inst, ok_cells(&r.entries[k]));
                }
            }
            out.steps.push(r);
            if stop {
                break;
            }
        }
        Ok(out)
    }
}

/// One refine call: the refinement, its offset, and every program host by name.
#[derive(Debug, Clone, PartialEq)]
pub struct RefineCall {
    /// Index into `program.refinements`.
    pub refinement: usize,
    pub offset: [i32; 3],
    pub hosts: Vec<(String, Value)>,
}

/// An orchestrate step, then refine calls in order.
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceCase {
    pub run: NamedCase,
    pub steps: Vec<RefineCall>,
}

#[derive(Debug, PartialEq)]
pub struct SequenceResult {
    /// The orchestrate step, hidden entries included.
    pub run: CaseResult,
    /// One per refine call that ran. Entry 0 is the refinement's entry; the others are its other
    /// running instances, in `LoweredStep::running` order.
    pub steps: Vec<CaseResult>,
}

fn failed(r: &CaseResult) -> bool {
    r.entries.iter().flatten().any(|c| c.is_err())
}

fn ok_cells(cells: &[Result<Value, EvalError>]) -> Vec<Value> {
    cells.iter().map(|c| c.clone().expect("a failed step ends the sequence")).collect()
}

/// Binds hosts by name: the program's own, then the value autoconsts from `auto_values`, then
/// `extra` for the hosts after them (a refine step's). Then runs `check_case`.
fn bind(program: &Program, map: &LowerMap, c: &NamedCase, extra: Vec<Value>) -> Result<Case, CaseBindError> {
    let n = program.hosts.len();
    let real = map.hosts.len();
    let autos = real + map.auto_host_names.len();
    let mut hosts: Vec<Option<Value>> = vec![None; n];
    for (h, v) in &c.auto_values {
        if h.0 < real || h.0 >= autos || hosts[h.0].is_some() {
            return Err(CaseBindError::AutoValueUnknown(*h));
        }
        hosts[h.0] = Some(v.clone());
    }
    for (k, v) in extra.into_iter().enumerate() {
        hosts[autos + k] = Some(v);
    }
    let mut given = HashSet::new();
    for (name, v) in &c.hosts {
        if !given.insert(name.as_str()) {
            return Err(CaseBindError::Duplicate(name.clone()));
        }
        let id = map
            .host_names
            .iter()
            .position(|n| n.as_deref() == Some(name.as_str()))
            .map(HostId)
            .filter(|h| h.0 < real)
            .ok_or_else(|| CaseBindError::Unknown(name.clone()))?;
        hosts[id.0] = Some(v.clone());
    }
    let hosts = hosts
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            v.ok_or_else(|| {
                let name = map.host_name(HostId(i)).unwrap_or("?").to_string();
                if i >= real {
                    CaseBindError::AutoValueMissing { name }
                } else {
                    CaseBindError::Missing(name)
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let case = Case {
        origin: F64x3 {
            x: c.origin[0].to_bits(),
            y: c.origin[1].to_bits(),
            z: c.origin[2].to_bits(),
        },
        hosts,
    };
    check_case(program, &case).map_err(CaseBindError::Invalid)?;
    Ok(case)
}

fn eval_checked<E: Externs>(program: &Program, case: &Case, ext: &E, opts: EvalOptions) -> Result<CaseResult, EvalFailure> {
    check_case(program, case).map_err(EvalFailure::Case)?;
    let mut memo = new_memo(program, opts.memo);
    let r = eval_program(program, case, ext, opts.fuel, &mut memo);
    let mut reasons = vec![];
    for (e, cells) in r.entries.iter().enumerate() {
        for (k, c) in cells.iter().enumerate() {
            if let Err(EvalError::Stuck(s)) = c {
                reasons.push((e, k, *s));
            }
        }
    }
    let failures: Vec<Failure> = r
        .failures
        .iter()
        .copied()
        .filter(|f| match f {
            Failure::Const { error, .. } | Failure::Input { error, .. } => {
                matches!(error, EvalError::Stuck(_))
            }
        })
        .collect();
    if reasons.is_empty() && failures.is_empty() {
        Ok(r)
    } else {
        Err(EvalFailure::Stuck { reasons, failures })
    }
}

/// `prepare`, bind, `eval` in one go.
pub fn evaluate<E: Externs>(
    p: &OptimizedProgram,
    reg: &Registry,
    case: &NamedCase,
    ext: &E,
    opts: EvalOptions,
) -> Result<CaseResult, EvaluateError> {
    let prep = prepare(p, reg).map_err(EvaluateError::Invalid)?;
    let case = prep.case(case).map_err(EvaluateError::Bind)?;
    prep.eval(&case, ext, opts).map_err(EvaluateError::Eval)
}
