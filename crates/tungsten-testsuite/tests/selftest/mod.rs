//! `selftest::`: the harness must detect failures, or a green run means nothing (crate README, "The harness checks itself").
//!
//! Each test feeds a stub driver (hand-written Rust, standing in for generated code) through the
//! real build, fallback, process and scoring code, and checks the verdict.

mod cuda;
mod java;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use libtest_mimic::{Failed, Trial};
use tungsten_testsuite::compare::{Verdict, score_case};
use tungsten_testsuite::corpus::load_source;
use tungsten_testsuite::env::TestEnv;
use tungsten_testsuite::harness::batch::{Artifact, Harness, TestSource, cache_key};
use tungsten_testsuite::harness::process::{CaseOutcome, run_cases_opts};
use tungsten_testsuite::harness::protocol::TestIo;
use tungsten_testsuite::harness::rust::{RUNTIME_FILES, records_source};
use tungsten_testsuite::knownfail::{CaseScore, reconcile};
use tungsten_testsuite::oracle;

const SHORT: Duration = Duration::from_millis(1500);
const NO_STARTUP: Duration = Duration::from_millis(500);
const JAVA_STARTUP: Duration = Duration::from_secs(10);

fn program(expr: &str, origins: &[f64]) -> String {
    let mut s = format!(
        "program {{\n    fn run() -> f64 {{ return {expr}; }}\n    unit u {{ main run; }}\n    \
         entry u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);\n}}\n"
    );
    for (i, o) in origins.iter().enumerate() {
        s += &format!("@case \"c{i}\" origin({o}, 0, 0)\n");
    }
    s
}

/// A stub test module: the body of `run_case`.
fn stub(id: &str, body: &str) -> TestSource {
    TestSource {
        name: id.to_string(),
        id: id.to_string(),
        source: format!(
            "pub mod driver {{\n    pub const REQUEST_LEN: usize = 24;\n    \
             pub fn run_case(r: &mut crate::proto::Reader) -> Vec<u8> {{ {body} }}\n}}\n"
        ),
    }
}

/// An `Ok` response holding one f64.
fn ok_f64(bits: u64) -> String {
    format!("let _ = r.f64(); let mut v = vec![0u8]; v.extend({bits}u64.to_le_bytes()); v")
}

/// Builds `stub`, runs the cases of `src` through it and scores each.
fn run_stub(h: &Harness, src: &str, body: &str, timeout: Duration) -> Result<(Vec<CaseOutcome>, Vec<Verdict>), Failed> {
    let env = TestEnv::new();
    let records = records_source(env.spec());
    run_stub_with(h, src, stub("t_selftest", body), &records, timeout)
}

/// `run_stub` for a prebuilt stub (any language) and its records.
fn run_stub_with(
    h: &Harness,
    src: &str,
    t: TestSource,
    records: &str,
    timeout: Duration,
) -> Result<(Vec<CaseOutcome>, Vec<Verdict>), Failed> {
    let env = TestEnv::new();
    let tf = load_source(src, &env)?;
    let run = oracle::run_all(&tf, &env)?;
    let io = TestIo::from_program(&tf.program)?;
    let arts = h.build_tests(records, std::slice::from_ref(&t));
    let Artifact::Binary { path, id } = &arts[&t.name] else {
        return Err(Failed::from(format!("the stub didn't build: {:?}", arts[&t.name])));
    };
    let mut requests = vec![];
    for i in 0..tf.cases.len() {
        let (named, params) = oracle::named_case(&tf, i, &env)?;
        let c = &tf.cases[i];
        requests.push(io.encode_request(&env, c.origin, &params, &named.hosts)?);
    }
    let make = || h.command(path, id);
    let startup = match h.lang {
        tungsten_testsuite::harness::batch::Lang::Java | tungsten_testsuite::harness::batch::Lang::Cuda => JAVA_STARTUP,
        _ => NO_STARTUP,
    };
    let outcomes =
        run_cases_opts(&make, &requests, io.response_bytes(&env)?, timeout, startup, h.restart_after_failure());
    let verdicts = outcomes
        .iter()
        .enumerate()
        .map(|(i, o)| score_case(&tf, &env, &io, i, &run.results[i], o))
        .collect();
    Ok((outcomes, verdicts))
}

fn expect_fail(v: &Verdict, needle: &str) -> Result<(), Failed> {
    match v {
        Verdict::Fail(m) if m.contains(needle) => Ok(()),
        other => Err(Failed::from(format!("expected a failure mentioning `{needle}`, got {other:?}"))),
    }
}

fn expect_pass(v: &Verdict) -> Result<(), Failed> {
    match v {
        Verdict::Pass(_) => Ok(()),
        other => Err(Failed::from(format!("expected a pass, got {other:?}"))),
    }
}

pub fn trials(h: Option<Arc<Harness>>, jh: Option<Arc<Harness>>, ch: Option<Arc<Harness>>) -> Vec<Trial> {
    let mut out = vec![];
    // The cache checks that need no compiler.
    out.push(Trial::test("selftest::cache_key_tracks_inputs", cache_key_tracks_inputs));
    out.extend(java::trials(jh));
    out.extend(cuda::trials(ch));
    let Some(h) = h else { return out };
    macro_rules! t {
        ($name:literal, $f:expr) => {{
            let h = h.clone();
            out.push(Trial::test(concat!("selftest::", $name), move || $f(&h)));
        }};
    }
    t!("wrong_value", |h: &Harness| {
        let (_, v) = run_stub(h, &program("1.5", &[0.0]), &ok_f64(2.5f64.to_bits()), SHORT)?;
        expect_fail(&v[0], "first differing cell")
    });
    t!("right_value_passes", |h: &Harness| {
        let (_, v) = run_stub(h, &program("1.5", &[0.0]), &ok_f64(1.5f64.to_bits()), SHORT)?;
        expect_pass(&v[0])
    });
    t!("sign_of_zero", |h: &Harness| {
        let (_, v) = run_stub(h, &program("0.0", &[0.0]), &ok_f64((-0.0f64).to_bits()), SHORT)?;
        expect_fail(&v[0], "first differing cell")
    });
    t!("nan_payloads_pass", |h: &Harness| {
        let (_, v) = run_stub(h, &program("nan", &[0.0]), &ok_f64(0x7ff8_0000_0000_0001), SHORT)?;
        expect_pass(&v[0])
    });
    t!("crash", |h: &Harness| {
        let (o, v) = run_stub(h, &program("1.5", &[0.0]), "std::process::abort()", SHORT)?;
        assert!(matches!(o[0], CaseOutcome::Crashed { .. }), "{o:?}");
        expect_fail(&v[0], "crashed")
    });
    t!("hang", |h: &Harness| {
        let (o, v) = run_stub(h, &program("1.5", &[0.0]), "loop { std::hint::spin_loop(); }", Duration::from_millis(700))?;
        assert!(matches!(o[0], CaseOutcome::TimedOut { .. }), "{o:?}");
        expect_fail(&v[0], "timed out")
    });
    t!("truncated_response", |h: &Harness| {
        // Writes a tag and part of the value, then exits: the harness must call that malformed.
        let body = "use std::io::Write; let _ = r.f64(); let mut o = std::io::stdout(); \
                    let _ = o.write_all(&[0, 1, 2, 3]); let _ = o.flush(); std::process::exit(0)";
        let (o, v) = run_stub(h, &program("1.5", &[0.0]), body, SHORT)?;
        assert!(matches!(o[0], CaseOutcome::Malformed(_)), "{o:?}");
        expect_fail(&v[0], "malformed")
    });
    t!("resume_after_crash", |h: &Harness| {
        // The second case crashes the process; the third must still get its own, correct answer.
        let body = format!(
            "let x = r.f64(); if x == 1.0 {{ std::process::abort(); }} let mut v = vec![0u8]; v.extend({}u64.to_le_bytes()); v",
            0.0f64.to_bits()
        );
        let (o, v) = run_stub(h, &program("0.0", &[0.0, 1.0, 2.0]), &body, SHORT)?;
        assert!(matches!(o[1], CaseOutcome::Crashed { .. }), "{o:?}");
        expect_pass(&v[0])?;
        expect_fail(&v[1], "crashed")?;
        expect_pass(&v[2])
    });
    t!("both_fail_passes", |h: &Harness| {
        let (_, v) = run_stub(h, &program("((1 / 0) as f64)", &[0.0]), "let _ = r.f64(); panic!(\"attempt to divide by zero\")", SHORT)?;
        expect_pass(&v[0])
    });
    t!("interpreter_error_backend_returns", |h: &Harness| {
        let (_, v) = run_stub(h, &program("((1 / 0) as f64)", &[0.0]), &ok_f64(0), SHORT)?;
        expect_fail(&v[0], "interpreter errors")
    });
    t!("build_error_in_one_test_of_a_batch", build_error_isolated);
    t!("stale_knownfail", |h: &Harness| {
        let (_, v) = run_stub(h, &program("1.5", &[0.0]), &ok_f64(1.5f64.to_bits()), SHORT)?;
        let kf = tungsten_parse::KnownFail {
            backend: "rust".into(),
            reason: "was broken".into(),
            case: None,
            stage: None,
        };
        let scores = vec![CaseScore { case: "c0".into(), verdict: v[0].clone() }];
        let e = reconcile(&[kf], "rust", "original", &scores).unwrap_err();
        if e.contains("stale knownfail, remove it") { Ok(()) } else { Err(Failed::from(e)) }
    });
    t!("second_build_is_cached", second_build_is_cached);
    out
}

fn build_error_isolated(h: &Harness) -> Result<(), Failed> {
    let env = TestEnv::new();
    let good = stub("t_good", &ok_f64(0));
    let bad = TestSource {
        name: "t_bad".into(),
        id: "t_bad".into(),
        source: "pub mod driver { this is not rust }".into(),
    };
    let arts = h.build_tests(&records_source(env.spec()), &[good, bad]);
    match (&arts["t_good"], &arts["t_bad"]) {
        (Artifact::Binary { .. }, Artifact::BuildFailed { output, .. }) if output.contains("error") => Ok(()),
        other => Err(Failed::from(format!("expected (Binary, BuildFailed), got {other:?}"))),
    }
}

fn second_build_is_cached(h: &Harness) -> Result<(), Failed> {
    let env = TestEnv::new();
    let records = records_source(env.spec());
    // A unique body so earlier runs of other selftests can't be what is cached.
    let t = stub("t_cached", &ok_f64(0xcafe));
    // A harness of its own, so parallel selftests don't move the build counter.
    let own = Harness::new(h.cache_root.clone(), false).map_err(Failed::from)?;
    own.build_tests(&records, std::slice::from_ref(&t));
    let after_first = own.builds.load(std::sync::atomic::Ordering::SeqCst);
    own.build_tests(&records, std::slice::from_ref(&t));
    let after_second = own.builds.load(std::sync::atomic::Ordering::SeqCst);
    if after_first == after_second { Ok(()) } else { Err(Failed::from("the second build invoked the compiler")) }
}

fn cache_key_tracks_inputs() -> Result<(), Failed> {
    let sources: BTreeMap<String, String> = [("t_a".to_string(), "mod a;".to_string())].into();
    let base = cache_key("rustc 1", RUNTIME_FILES, "records", &sources);
    assert_eq!(base, cache_key("rustc 1", RUNTIME_FILES, "records", &sources), "deterministic");
    assert_ne!(base, cache_key("rustc 2", RUNTIME_FILES, "records", &sources), "compiler version");
    assert_ne!(base, cache_key("rustc 1", RUNTIME_FILES, "records2", &sources), "records");
    let mut other = sources.clone();
    other.insert("t_a".into(), "mod b;".into());
    assert_ne!(base, cache_key("rustc 1", RUNTIME_FILES, "records", &other), "generated source");
    // Changing one runtime file changes the key.
    let mut changed: Vec<(&str, String)> = RUNTIME_FILES.iter().map(|(n, t)| (*n, t.to_string())).collect();
    changed[0].1.push_str("\n// edited\n");
    let changed: Vec<(&str, &str)> = changed.iter().map(|(n, t)| (*n, t.as_str())).collect();
    assert_ne!(base, cache_key("rustc 1", &changed, "records", &sources), "runtime file");
    Ok(())
}
