//! Java selftests: stub drivers (hand-written Java) through the real Java build, process and
//! scoring code, plus the Java halves of the runtime checks (toy-table goldens, extern coverage).

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use libtest_mimic::{Failed, Trial};
use tungsten_ir::VariableType;
use tungsten_testsuite::env::TestEnv;
use tungsten_testsuite::harness::batch::{Artifact, Harness, TestSource};
use tungsten_testsuite::harness::java::records_source;
use tungsten_testsuite::harness::process::CaseOutcome;

use super::{expect_fail, expect_pass, program, run_stub_with};

const SHORT: Duration = Duration::from_millis(1500);

/// A stub test: the body of `runCase(Proto.Reader r)`, plus extra members of `Driver`.
fn jstub(id: &str, body: &str, extra: &str) -> TestSource {
    let source = format!(
        "//@@FILE Driver.java\npackage {id};\nimport tungsten.runtime.*;\npublic final class Driver {{\n    \
         static final int REQUEST_LEN = 32;\n    static byte[] runCase(Proto.Reader r) {{ {body} }}\n    {extra}\n    \
         public static void main(String[] a) {{ Proto.serve(REQUEST_LEN, Driver::runCase); }}\n}}\n"
    );
    TestSource { name: id.to_string(), id: id.to_string(), source }
}

fn ok_f64(x: f64) -> String {
    format!("r.f64(); Proto.Writer w = new Proto.Writer(); w.f64({x:?}); return w.finish();")
}

fn run(h: &Harness, src: &str, body: &str, extra: &str, timeout: Duration) -> Result<(Vec<CaseOutcome>, Vec<tungsten_testsuite::compare::Verdict>), Failed> {
    let env = TestEnv::new();
    run_stub_with(h, src, jstub("t_jselftest", body, extra), &records_source(env.spec()), timeout)
}

pub fn trials(h: Option<Arc<Harness>>) -> Vec<Trial> {
    let Some(h) = h else { return vec![] };
    let mut out = vec![];
    macro_rules! t {
        ($name:literal, $f:expr) => {{
            let h = h.clone();
            out.push(Trial::test(concat!("selftest::java::", $name), move || $f(&h)));
        }};
    }
    t!("wrong_value", |h: &Harness| {
        let (_, v) = run(h, &program("1.5", &[0.0]), &ok_f64(2.5), "", SHORT)?;
        expect_fail(&v[0], "first differing cell")
    });
    t!("right_value_passes", |h: &Harness| {
        let (_, v) = run(h, &program("1.5", &[0.0]), &ok_f64(1.5), "", SHORT)?;
        expect_pass(&v[0])
    });
    t!("crash", |h: &Harness| {
        let (o, v) = run(h, &program("1.5", &[0.0]), "Runtime.getRuntime().halt(134); return null;", "", SHORT)?;
        assert!(matches!(o[0], CaseOutcome::Crashed { .. }), "{o:?}");
        expect_fail(&v[0], "crashed")
    });
    t!("hang", |h: &Harness| {
        let (o, v) = run(h, &program("1.5", &[0.0]), "while (true) {} ", "", Duration::from_millis(700))?;
        assert!(matches!(o[0], CaseOutcome::TimedOut { .. }), "{o:?}");
        expect_fail(&v[0], "timed out")
    });
    t!("division_failure_matches", |h: &Harness| {
        let (_, v) = run(
            h,
            &program("((1 / 0) as f64)", &[0.0]),
            "r.f64(); throw new ArithmeticException(\"/ by zero\");",
            "",
            SHORT,
        )?;
        expect_pass(&v[0])
    });
    t!("failing_initializer_is_reported_as_the_original_failure", |h: &Harness| {
        // The first case fails inside a static initialiser (ExceptionInInitializerError), the second
        // sees NoClassDefFoundError; both must still score as a division by zero (crate README, "Rules and contracts, Processes").
        let extra = "static final class Boom { static int zero() { return 0; } static final int X = 1 / zero(); }";
        let (o, v) = run(h, &program("((1 / 0) as f64)", &[0.0, 1.0]), "r.f64(); return new byte[]{(byte) Boom.X};", extra, SHORT)?;
        assert!(o.iter().all(|x| matches!(x, CaseOutcome::Failed { kind: 0, .. })), "{o:?}");
        expect_pass(&v[0])?;
        expect_pass(&v[1])
    });
    t!("build_error_in_one_test_of_a_batch", |h: &Harness| {
        let env = TestEnv::new();
        let good = jstub("t_jgood", &ok_f64(0.0), "");
        let bad = TestSource {
            name: "t_jbad".into(),
            id: "t_jbad".into(),
            source: "//@@FILE Driver.java\npackage t_jbad;\npublic final class Driver { this is not java }\n".into(),
        };
        let arts = h.build_tests(&records_source(env.spec()), &[good, bad]);
        match (&arts["t_jgood"], &arts["t_jbad"]) {
            (Artifact::Binary { .. }, Artifact::BuildFailed { output, .. }) if output.contains("error") => Ok(()),
            other => Err(Failed::from(format!("expected (Binary, BuildFailed), got {other:?}"))),
        }
    });
    t!("second_build_is_cached", |h: &Harness| {
        let env = TestEnv::new();
        let records = records_source(env.spec());
        let t = jstub("t_jcached", &ok_f64(123.0), "");
        // A harness of its own, so parallel selftests don't move the build counter.
        let own = Harness::new_java(h.cache_root.clone(), false).map_err(Failed::from)?;
        own.build_tests(&records, std::slice::from_ref(&t));
        let first = own.builds.load(std::sync::atomic::Ordering::SeqCst);
        own.build_tests(&records, std::slice::from_ref(&t));
        if first == own.builds.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(())
        } else {
            Err(Failed::from("the second build invoked javac"))
        }
    });
    // The Java Reader/Writer against the golden bytes shared with the host side (crate README, "Rules and contracts, Protocol").
    t!("golden_bytes", |h: &Harness| {
        let golden = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/protocol.txt"))
            .map_err(|e| Failed::from(e.to_string()))?;
        let field = |name: &str| -> Result<Vec<u8>, Failed> {
            let hex = golden
                .lines()
                .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
                .ok_or_else(|| Failed::from(format!("no `{name}` line in the golden file")))?;
            Ok((0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect())
        };
        let (request, response) = (field("request")?, field("response")?);
        let body = format!(
            "double x = r.f64(), y = r.f64(), z = r.f64(); long seed = r.u64(); \
             double pa = r.f64(); int pb = r.i32(); double vx = r.f64(), vy = r.f64(), vz = r.f64(); byte n = r.i8(); \
             if (Double.doubleToRawLongBits(x) != Double.doubleToRawLongBits(1.5) || Double.doubleToRawLongBits(y) != Long.MIN_VALUE \
                 || z != 2.0 || seed != 0x0102030405060708L || Double.doubleToRawLongBits(pa) != Long.MIN_VALUE || pb != -2 \
                 || vx != 1.5 || !Double.isNaN(vy) || vz != 0.0 || n != -3) throw new IllegalStateException(\"request\"); \
             Proto.Writer w = new Proto.Writer(); w.i32(1); w.i32(-1); w.i32(1); w.i32(2); w.i32(3); w.i32(-1); w.i32(0); w.i32(7); \
             return w.finish();"
        );
        let mut t = jstub("t_jgolden", &body, "");
        t.source = t.source.replace("REQUEST_LEN = 32", &format!("REQUEST_LEN = {}", request.len()));
        let env = TestEnv::new();
        let arts = h.build_tests(&records_source(env.spec()), std::slice::from_ref(&t));
        let Artifact::Binary { path, id } = &arts["t_jgolden"] else {
            return Err(Failed::from(format!("the stub didn't build: {:?}", arts["t_jgolden"])));
        };
        let make = || h.command(path, id);
        let o = tungsten_testsuite::harness::process::run_cases_cmd(
            &make,
            &[request],
            response.len() - 1,
            SHORT,
            Duration::from_secs(10),
        );
        match &o[0] {
            CaseOutcome::Returned(b) if *b == response[1..] => Ok(()),
            other => Err(Failed::from(format!("expected the golden response, got {other:?}"))),
        }
    });
    // Toy-table goldens (crate README, "Rules and contracts, Toy tables"), the same values as the Rust side in src/externs.rs.
    t!("toy_table_goldens", |h: &Harness| {
        let body = "r.f64(); \
            byte[] p = Helpers.make_permutation_table(1, 2, 3, 4, 5, 6).perm; \
            int[] want = {216, 28, 120, 37, 55, 25, 49, 241}; \
            for (int i = 0; i < 8; i++) if ((p[i] & 0xff) != want[i]) throw new IllegalStateException(\"perm \" + i); \
            InterpolatedNoiseSampler b = Helpers.make_base3d_perm_table(7); \
            if (b.state != 0x63cbe1e459320dd7L) throw new IllegalStateException(\"base3d state\"); \
            double v = Helpers.sampler_sample(b, new Vec3(1.5, -2.0, 3.0)); \
            if (Double.doubleToRawLongBits(v) != 0x3fede2357dfdf669L) throw new IllegalStateException(\"sample\"); \
            Proto.Writer w = new Proto.Writer(); w.f64(1.0); return w.finish();";
        let (_, v) = run(h, &program("1.0", &[0.0]), body, "", SHORT)?;
        expect_pass(&v[0])
    });
    // One call to every registered extern (crate README, "Things worth knowing"): a missing one is a compile error.
    t!("runtime_coverage", |h: &Harness| {
        let env = TestEnv::new();
        let mut body = String::from("r.f64(); ");
        for e in &env.spec().externs {
            let args: Vec<String> = e
                .params
                .iter()
                .map(|t| match t {
                    VariableType::F64 => "1.5".into(),
                    VariableType::F32 => "1.5f".into(),
                    VariableType::I32 => "3".into(),
                    VariableType::I64 => "3L".into(),
                    VariableType::Vec3 => "new Vec3(0, 0, 0)".into(),
                    VariableType::PermutationTable => "Helpers.make_permutation_table(1, 2, 3, 4, 5, 6)".into(),
                    VariableType::InterpolatedNoiseSampler => "Helpers.make_base3d_perm_table(1)".into(),
                    other => panic!("no sample argument for {other:?}"),
                })
                .collect();
            write!(body, "Helpers.{}({}); ", e.name, args.join(", ")).unwrap();
        }
        body.push_str("Proto.Writer w = new Proto.Writer(); w.f64(1.0); return w.finish();");
        let (_, v) = run(h, &program("1.0", &[0.0]), &body, "", SHORT)?;
        expect_pass(&v[0])
    });
    out
}
