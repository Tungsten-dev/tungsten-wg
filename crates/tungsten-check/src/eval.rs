//! `prepare` (lower + check, once per program) and `Prepared::eval` (once per case), plus the
//! one-shot `evaluate`.

use std::collections::HashSet;

use tungsten_core::check::{CaseErrors, check, check_case};
use tungsten_core::exec::{CaseResult, Failure, eval_program, new_memo};
use tungsten_core::{Case, EvalError, ExternId, Externs, F64x3, HostId, Program, StuckReason, Value};
use tungsten_ir::OptimizedProgram;

use crate::check::{CheckErrors, Invalid, render};
use crate::map::LowerMap;
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
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaseBindError {
    #[error("the program has no host `{0}`")]
    Unknown(String),
    #[error("no value for host `{0}`")]
    Missing(String),
    #[error("host `{0}` is given more than once")]
    Duplicate(String),
    /// The bound case fails `check_case` (a value of the wrong type).
    #[error("the case doesn't fit the program: {0:?}")]
    Invalid(CaseErrors),
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
        let n = self.program().hosts.len();
        let mut hosts: Vec<Option<Value>> = vec![None; n];
        let mut given = HashSet::new();
        for (name, v) in &c.hosts {
            if !given.insert(name.as_str()) {
                return Err(CaseBindError::Duplicate(name.clone()));
            }
            let id = self.host_id(name).ok_or_else(|| CaseBindError::Unknown(name.clone()))?;
            hosts[id.0] = Some(v.clone());
        }
        let hosts = hosts
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                v.ok_or_else(|| {
                    CaseBindError::Missing(
                        self.map().hosts.name(HostId(i)).unwrap_or("?").to_string(),
                    )
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
        check_case(self.program(), &case).map_err(CaseBindError::Invalid)?;
        Ok(case)
    }

    /// Runs `check_case`, then the interpreter. Any `Stuck` is an `Err`.
    pub fn eval<E: Externs>(
        &self,
        case: &Case,
        ext: &E,
        opts: EvalOptions,
    ) -> Result<CaseResult, EvalFailure> {
        check_case(self.program(), case).map_err(EvalFailure::Case)?;
        let mut memo = new_memo(self.program(), opts.memo);
        let r = eval_program(self.program(), case, ext, opts.fuel, &mut memo);
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
