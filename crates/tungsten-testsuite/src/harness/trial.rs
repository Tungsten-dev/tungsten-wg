//! One backend trial: `<path>::<backend>::original` or `::inlined` (SPEC §6.1).

use std::time::Duration;

use std::sync::Mutex;

use super::batch::{Artifact, Harness, Lang, Stage};
use super::process::{CaseOutcome, STARTUP_ALLOWANCE, run_cases_opts};
use super::protocol::TestIo;
use crate::compare::{Verdict, score_case, score_unbuilt};
use crate::corpus::{CorpusFile, load_file};
use crate::env::TestEnv;
use crate::knownfail::{CaseScore, reconcile};
use crate::oracle;

/// Serializes the processes that use the GPU (CUDA harness only).
static GPU_LOCK: Mutex<()> = Mutex::new(());

/// `TW_TIMEOUT_SECS` (default 10), ten times longer for `@tier(large)`.
pub fn case_timeout(large: bool) -> Duration {
    let base = std::env::var("TW_TIMEOUT_SECS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(10);
    Duration::from_secs(if large { base * 10 } else { base })
}

/// Runs the trial. `Ok` carries notes (expected failures) for the log; `Err` is the report.
pub fn run_backend_trial(h: &Harness, file: &CorpusFile, stage: Stage) -> Result<Vec<String>, String> {
    let env = TestEnv::new();
    let tf = load_file(file.clone(), &env).result?;
    let run = oracle::run_all(&tf, &env)?;
    let n = tf.cases.len();

    // The program the backend sees.
    let mut program = tf.program.clone();
    if stage == Stage::Inlined {
        tungsten_optimizer::perform_inline(&mut program);
    }
    let io = TestIo::from_program(&program)?;
    io.check_encodable()?;

    let artifact = h.artifact(file.dir(), stage, &file.name);
    let eligible: Vec<usize> = (0..n).filter(|&i| !oracle::skipped_for_backend(&run.results[i], h.lang.name())).collect();
    if eligible.is_empty() {
        return Ok(vec![]);
    }

    let mut scores: Vec<CaseScore> = vec![];
    match artifact {
        Artifact::NothingToRun => return Ok(vec![]),
        Artifact::GenFailed(why) => {
            for i in 0..n {
                let v = if !eligible.contains(&i) {
                    Verdict::Skipped
                } else {
                    score_unbuilt(&tf, &run.results[i], i, &format!("generation failed: {why}"))
                };
                scores.push(CaseScore { case: tf.cases[i].name.clone(), verdict: v });
            }
        }
        Artifact::BuildFailed { output, source } => {
            let why = format!("build failed (sources in {}):\n{output}", source.display());
            for i in 0..n {
                let v = if !eligible.contains(&i) { Verdict::Skipped } else { score_unbuilt(&tf, &run.results[i], i, &why) };
                scores.push(CaseScore { case: tf.cases[i].name.clone(), verdict: v });
            }
        }
        Artifact::Binary { path, id } => {
            let mut requests = vec![];
            for &i in &eligible {
                let (named, params) = oracle::named_case(&tf, i, &env)?;
                let c = &tf.cases[i];
                requests.push(io.encode_request(&env, c.origin, &params, &named.hosts)?);
            }
            let large = tf.tier.as_deref() == Some("large");
            let make = || h.command(&path, &id);
            // One GPU: jobs on it run one at a time (SPEC §7.3, §13).
            let _gpu = (h.lang == Lang::Cuda).then(|| GPU_LOCK.lock().unwrap_or_else(|e| e.into_inner()));
            let outcomes = run_cases_opts(
                &make,
                &requests,
                io.response_bytes(&env)?,
                case_timeout(large),
                STARTUP_ALLOWANCE,
                h.restart_after_failure(),
            );
            let mut by_case: Vec<Option<CaseOutcome>> = (0..n).map(|_| None).collect();
            for (&i, o) in eligible.iter().zip(outcomes) {
                by_case[i] = Some(o);
            }
            for i in 0..n {
                let verdict = match &by_case[i] {
                    None => Verdict::Skipped,
                    Some(o) => score_case(&tf, &env, &io, i, &run.results[i], o),
                };
                scores.push(CaseScore { case: tf.cases[i].name.clone(), verdict });
            }
        }
    }
    reconcile(&tf.knownfails, h.lang.name(), stage.name(), &scores)
}
