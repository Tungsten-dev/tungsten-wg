//! Reconciliation of `@knownfail` directives with backend results (crate README, "Rules and contracts").

use tungsten_parse::KnownFail;

use crate::compare::Verdict;

pub struct CaseScore {
    pub case: String,
    pub verdict: Verdict,
}

fn covers(kf: &KnownFail, backend: &str, stage: &str, case: &str) -> bool {
    kf.backend == backend
        && kf.case.as_deref().is_none_or(|c| c == case)
        && kf.stage.as_deref().is_none_or(|s| s == stage)
}

/// `Ok` carries the notes to print (expected failures with their reasons). `Err` is the failure
/// report of the trial: uncovered failures and stale knownfails. Skipped cases are not evaluated.
pub fn reconcile(
    knownfails: &[KnownFail],
    backend: &str,
    stage: &str,
    scores: &[CaseScore],
) -> Result<Vec<String>, String> {
    let mut notes = vec![];
    let mut errors = vec![];
    for s in scores {
        let covering: Vec<&KnownFail> =
            knownfails.iter().filter(|k| covers(k, backend, stage, &s.case)).collect();
        match (&s.verdict, covering.is_empty()) {
            (Verdict::Skipped, _) => {}
            (Verdict::Fail(why), true) => errors.push(why.clone()),
            (Verdict::Fail(why), false) => {
                let reasons: Vec<&str> = covering.iter().map(|k| k.reason.as_str()).collect();
                notes.push(format!(
                    "expected failure (@knownfail: {}): case `{}`\n{}",
                    reasons.join("; "),
                    s.case,
                    why.lines().take(6).collect::<Vec<_>>().join("\n")
                ));
            }
            (Verdict::Pass(_), false) => errors.push(format!(
                "case `{}` passes on {backend} ({stage}), but a @knownfail covers it ({}): stale knownfail, remove it",
                s.case,
                covering.iter().map(|k| k.reason.as_str()).collect::<Vec<_>>().join("; ")
            )),
            (Verdict::Pass(_), true) => {}
        }
    }
    if errors.is_empty() { Ok(notes) } else { Err(errors.join("\n\n")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kf(case: Option<&str>, stage: Option<&str>) -> KnownFail {
        KnownFail {
            backend: "rust".into(),
            reason: "r".into(),
            case: case.map(String::from),
            stage: stage.map(String::from),
        }
    }
    fn sc(case: &str, v: Verdict) -> CaseScore {
        CaseScore { case: case.into(), verdict: v }
    }

    #[test]
    fn covered_failure_is_expected() {
        let r = reconcile(&[kf(None, None)], "rust", "original", &[sc("a", Verdict::Fail("x".into()))]);
        assert_eq!(r.unwrap().len(), 1);
    }

    #[test]
    fn uncovered_failure_fails() {
        let r = reconcile(&[kf(Some("b"), None)], "rust", "original", &[sc("a", Verdict::Fail("x".into()))]);
        assert!(r.unwrap_err().contains('x'));
    }

    #[test]
    fn stale_knownfail_fails() {
        let r = reconcile(&[kf(Some("a"), Some("original"))], "rust", "original", &[sc("a", Verdict::Pass(None))]);
        assert!(r.unwrap_err().contains("stale knownfail"));
        // A different stage or backend doesn't cover.
        assert!(reconcile(&[kf(Some("a"), Some("inlined"))], "rust", "original", &[sc("a", Verdict::Pass(None))]).is_ok());
        assert!(reconcile(&[kf(None, None)], "java", "original", &[sc("a", Verdict::Pass(None))]).is_ok());
    }

    #[test]
    fn skipped_is_not_evaluated() {
        assert!(reconcile(&[kf(None, None)], "rust", "original", &[sc("a", Verdict::Skipped)]).is_ok());
    }
}
