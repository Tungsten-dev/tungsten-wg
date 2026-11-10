//! Scoring of one backend case against the interpreter (crate README, "Rules and contracts").

use tungsten_check::{CaseResult, Value};
use tungsten_parse::{CellSample, TestFile};

use crate::env::TestEnv;
use crate::harness::process::CaseOutcome;
use crate::harness::protocol::{TestIo, kind_name};
use crate::oracle::{self, error_kind_name};
use crate::values::{core_eq, show};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Not run, not scored (crate README, "Rules and contracts").
    Skipped,
    /// Passed; the note says why when the interpreter errored and the backend failed too.
    Pass(Option<String>),
    Fail(String),
}

impl Verdict {
    pub fn is_fail(&self) -> bool {
        matches!(self, Verdict::Fail(_))
    }
}

/// An interpreter result without any cell error or const/input failure.
pub fn interpreter_clean(r: &CaseResult) -> bool {
    r.failures.is_empty() && r.entries.iter().flatten().all(|c| c.is_ok())
}

/// Whether cell `k` of entry `e` takes part in the comparison (crate README, "Rules and contracts"). Deterministic: the
/// `sample` cells with the smallest hash of `(seed, entry, cell)`.
pub fn cell_selected(cells: Option<CellSample>, e: usize, k: usize, total: usize) -> bool {
    use crate::externs::rt_utils::splitmix64;
    let Some(c) = cells else { return true };
    if c.sample as usize >= total {
        return true;
    }
    let h = |i: usize| splitmix64(c.seed ^ ((e as u64) << 40) ^ i as u64);
    let mine = h(k);
    let rank = (0..total).filter(|&i| h(i) < mine || (h(i) == mine && i < k)).count();
    rank < c.sample as usize
}

fn xyz(flat: usize, dims: (i32, i32, i32)) -> (i32, i32, i32) {
    let f = flat as i32;
    (f % dims.0, (f / dims.0) % dims.1, f / (dims.0 * dims.1))
}

fn describe(o: &CaseOutcome) -> String {
    match o {
        CaseOutcome::Returned(_) => "returned a value".into(),
        CaseOutcome::Failed { kind, message } => format!("failed with {} ({message})", kind_name(*kind)),
        CaseOutcome::Crashed { status, stderr } => format!("crashed: {status}\nstderr tail:\n{stderr}"),
        CaseOutcome::TimedOut { stderr } => format!("timed out\nstderr tail:\n{stderr}"),
        CaseOutcome::Malformed(m) => format!("harness error, malformed response: {m}"),
    }
}

/// The scoring table of crate README, "Rules and contracts" for one case that reached the backend.
pub fn score_case(
    tf: &TestFile,
    env: &TestEnv,
    io: &TestIo,
    case: usize,
    oracle_result: &CaseResult,
    outcome: &CaseOutcome,
) -> Verdict {
    let name = &tf.cases[case].name;
    if oracle::skipped_for_backends(oracle_result) {
        return Verdict::Skipped;
    }
    // A malformed response is the harness's fault, never a pass.
    if let CaseOutcome::Malformed(_) = outcome {
        return Verdict::Fail(format!("case `{name}`: {}", describe(outcome)));
    }
    let clean = interpreter_clean(oracle_result);
    match (clean, outcome) {
        (false, CaseOutcome::Returned(_)) => {
            let first = oracle::first_failure(oracle_result)
                .map(|(e, k, err)| format!("entry {e} cell {k}: {}", error_kind_name(&err)))
                .unwrap_or_else(|| format!("{} const/input failures", oracle_result.failures.len()));
            Verdict::Fail(format!(
                "case `{name}`: the interpreter errors ({first}) but the backend returned a value"
            ))
        }
        (false, other) => Verdict::Pass(Some(format!("both fail; backend {}", describe(other).lines().next().unwrap()))),
        (true, CaseOutcome::Returned(body)) => {
            let cells = match io.decode_response(env, body) {
                Ok(c) => c,
                Err(m) => return Verdict::Fail(format!("case `{name}`: harness error: {m}")),
            };
            compare_cells(tf, io, case, oracle_result, &cells)
        }
        (true, other) => Verdict::Fail(format!(
            "case `{name}`: the interpreter returns values but the backend {}",
            describe(other)
        )),
    }
}

fn compare_cells(tf: &TestFile, io: &TestIo, case: usize, want: &CaseResult, got: &[Vec<Value>]) -> Verdict {
    let name = &tf.cases[case].name;
    let mut diffs: Vec<String> = vec![];
    for (e, (w, g)) in want.entries.iter().zip(got).enumerate() {
        for (k, (wc, gc)) in w.iter().zip(g).enumerate() {
            if !cell_selected(tf.cells, e, k, w.len()) {
                continue;
            }
            let Ok(wv) = wc else { continue };
            if !core_eq(wv, gc) {
                let (x, y, z) = xyz(k, io.entries[e].dims);
                diffs.push(format!(
                    "entry `{}` cell ({x}, {y}, {z}) [flat {k}]:\n    interpreter: {}\n    backend:     {}",
                    tf.entry_labels[e],
                    show(wv),
                    show(gc)
                ));
            }
        }
    }
    match diffs.split_first() {
        None => Verdict::Pass(None),
        Some((first, rest)) => Verdict::Fail(format!(
            "case `{name}`: first differing cell: {first}{}",
            if rest.is_empty() { String::new() } else { format!("\n  and {} more differing cells", rest.len()) }
        )),
    }
}

/// A build or generation failure fails every case that would have run (crate README, "Rules and contracts").
pub fn score_unbuilt(tf: &TestFile, oracle_result: &CaseResult, case: usize, why: &str) -> Verdict {
    if oracle::skipped_for_backends(oracle_result) {
        Verdict::Skipped
    } else {
        Verdict::Fail(format!("case `{}`: {why}", tf.cases[case].name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_sampling_is_deterministic_and_sized() {
        let c = Some(CellSample { sample: 3, seed: 7 });
        let picked: Vec<usize> = (0..20).filter(|&k| cell_selected(c, 0, k, 20)).collect();
        assert_eq!(picked.len(), 3);
        let again: Vec<usize> = (0..20).filter(|&k| cell_selected(c, 0, k, 20)).collect();
        assert_eq!(picked, again);
        assert!((0..3).all(|k| cell_selected(c, 0, k, 3)), "sample >= total selects everything");
        assert!(cell_selected(None, 1, 1, 100));
    }

    #[test]
    fn xyz_order() {
        assert_eq!(xyz(0, (2, 3, 4)), (0, 0, 0));
        assert_eq!(xyz(1, (2, 3, 4)), (1, 0, 0));
        assert_eq!(xyz(2, (2, 3, 4)), (0, 1, 0));
        assert_eq!(xyz(6, (2, 3, 4)), (0, 0, 1));
    }
}
