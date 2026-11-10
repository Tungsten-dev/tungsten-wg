//! CUDA selftests: stub drivers (hand-written CUDA C++) through the real CUDA build, process and
//! scoring code, plus the CUDA halves of the runtime checks (golden bytes, toy-table goldens,
//! extern coverage) and the device-error recovery of crate README, "Rules and contracts"

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use libtest_mimic::{Failed, Trial};
use tungsten_ir::VariableType;
use tungsten_testsuite::env::TestEnv;
use tungsten_testsuite::harness::batch::{Artifact, Harness, TestSource};
use tungsten_testsuite::harness::cuda::records_source;
use tungsten_testsuite::harness::process::{CaseOutcome, run_cases_opts};

use super::{expect_fail, expect_pass, program, run_stub_with};

const SHORT: Duration = Duration::from_millis(2500);

/// A stub test: the body of `bool run(Reader& r, Writer& w, std::string& err)`, plus declarations
/// before it. The profile header is included so externs are available.
fn cstub(id: &str, body: &str, extra: &str) -> TestSource {
    let source = format!(
        "//@@FILE {id}.cu\n#define TUNGSTEN_PROFILE_EXACT 1\n#include \"tw/tungsten_runtime.cuh\"\n#include \"tw/profile_exact.cuh\"\n\
         #include \"tw/proto.h\"\n#include <string>\n{extra}\n\
         static bool run(tw::proto::Reader& r, tw::proto::Writer& w, std::string& err) {{ {body} }}\n\
         int tw_serve_{id}() {{ cudaSetDevice(0); return tw::proto::serve(24, run); }}\n"
    );
    TestSource { name: id.to_string(), id: id.to_string(), source }
}

fn ok_f64(x: f64) -> String {
    format!("r.f64(); w.f64({x:?}); return true;")
}

fn run(h: &Harness, src: &str, body: &str, extra: &str, timeout: Duration) -> Result<(Vec<CaseOutcome>, Vec<tungsten_testsuite::compare::Verdict>), Failed> {
    let env = TestEnv::new();
    run_stub_with(h, src, cstub("t_cselftest", body, extra), &records_source(env.spec()), timeout)
}

pub fn trials(h: Option<Arc<Harness>>) -> Vec<Trial> {
    let Some(h) = h else { return vec![] };
    let mut out = vec![];
    macro_rules! t {
        ($name:literal, $f:expr) => {{
            let h = h.clone();
            out.push(Trial::test(concat!("selftest::cuda::", $name), move || $f(&h)));
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
    t!("sign_of_zero", |h: &Harness| {
        let (_, v) = run(h, &program("0.0", &[0.0]), &ok_f64(-0.0), "", SHORT)?;
        expect_fail(&v[0], "first differing cell")
    });
    t!("nan_payloads_pass", |h: &Harness| {
        let body = "r.f64(); unsigned long long b = 0x7ff8000000000001ULL; double d; memcpy(&d, &b, 8); w.f64(d); return true;";
        let (_, v) = run(h, &program("nan", &[0.0]), body, "#include <cstring>", SHORT)?;
        expect_pass(&v[0])
    });
    t!("crash", |h: &Harness| {
        let (o, v) = run(h, &program("1.5", &[0.0]), "std::abort();", "#include <cstdlib>", SHORT)?;
        assert!(matches!(o[0], CaseOutcome::Crashed { .. }), "{o:?}");
        expect_fail(&v[0], "crashed")
    });
    t!("hang", |h: &Harness| {
        let (o, v) = run(h, &program("1.5", &[0.0]), "for (;;) {}", "", Duration::from_millis(1500))?;
        assert!(matches!(o[0], CaseOutcome::TimedOut { .. }), "{o:?}");
        expect_fail(&v[0], "timed out")
    });
    t!("truncated_response", |h: &Harness| {
        let body = "r.f64(); const unsigned char part[4] = {0, 1, 2, 3}; tw::proto::write_all(part, 4); _exit(0); return true;";
        let (o, v) = run(h, &program("1.5", &[0.0]), body, "", SHORT)?;
        assert!(matches!(o[0], CaseOutcome::Malformed(_)), "{o:?}");
        expect_fail(&v[0], "malformed")
    });
    t!("resume_after_crash", |h: &Harness| {
        let body = "double x = r.f64(); if (x == 1.0) std::abort(); w.f64(0.0); return true;";
        let (o, v) = run(h, &program("0.0", &[0.0, 1.0, 2.0]), body, "#include <cstdlib>", SHORT)?;
        assert!(matches!(o[1], CaseOutcome::Crashed { .. }), "{o:?}");
        expect_pass(&v[0])?;
        expect_fail(&v[1], "crashed")?;
        expect_pass(&v[2])
    });
    // A real sticky device error: the kernel traps, the driver reports it and exits, and the next
    // case runs in a fresh process and still gets a correct answer (crate README, "Rules and contracts, Processes").
    t!("device_trap_ends_process_and_resumes", |h: &Harness| {
        let extra = "__global__ void boom() { __trap(); }";
        let body = "double x = r.f64(); if (x == 1.0) { boom<<<1, 1>>>(); cudaError_t e = cudaDeviceSynchronize(); \
                    err = std::string(\"device error: \") + cudaGetErrorString(e); return false; } w.f64(0.0); return true;";
        let (o, v) = run(h, &program("0.0", &[0.0, 1.0, 2.0]), body, extra, SHORT)?;
        assert!(matches!(&o[1], CaseOutcome::Failed { message, .. } if message.contains("device error")), "{o:?}");
        expect_pass(&v[0])?;
        expect_fail(&v[1], "failed")?;
        expect_pass(&v[2])
    });
    t!("interpreter_error_cases_are_skipped", |_h: &Harness| {
        let env = TestEnv::new();
        let tf = tungsten_testsuite::corpus::load_source(&program("((1 / 0) as f64)", &[0.0]), &env)?;
        let run = tungsten_testsuite::oracle::run_all(&tf, &env)?;
        if tungsten_testsuite::oracle::skipped_for_backend(&run.results[0], "cuda")
            && !tungsten_testsuite::oracle::skipped_for_backend(&run.results[0], "rust")
        {
            Ok(())
        } else {
            Err(Failed::from("an erroring case must be skipped on cuda only"))
        }
    });
    t!("build_error_in_one_test_of_a_batch", |h: &Harness| {
        let env = TestEnv::new();
        let good = cstub("t_cgood", &ok_f64(0.0), "");
        let bad = TestSource {
            name: "t_cbad".into(),
            id: "t_cbad".into(),
            source: "//@@FILE t_cbad.cu\nint tw_serve_t_cbad() { this is not cuda }\n".into(),
        };
        let arts = h.build_tests(&records_source(env.spec()), &[good, bad]);
        match (&arts["t_cgood"], &arts["t_cbad"]) {
            (Artifact::Binary { .. }, Artifact::BuildFailed { output, .. }) if output.contains("error") => Ok(()),
            other => Err(Failed::from(format!("expected (Binary, BuildFailed), got {other:?}"))),
        }
    });
    t!("second_build_is_cached", |h: &Harness| {
        let env = TestEnv::new();
        let records = records_source(env.spec());
        let t = cstub("t_ccached", &ok_f64(123.0), "");
        let own = Harness::new_cuda(h.cache_root.clone(), false).map_err(Failed::from)?;
        own.build_tests(&records, std::slice::from_ref(&t));
        let first = own.builds.load(std::sync::atomic::Ordering::SeqCst);
        own.build_tests(&records, std::slice::from_ref(&t));
        if first == own.builds.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(())
        } else {
            Err(Failed::from("the second build invoked nvcc"))
        }
    });
    // The driver-side Reader/Writer against the golden bytes shared with the host side (crate README, "Rules and contracts, Protocol").
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
        let bits = |x: f64| format!("{:#x}ULL", x.to_bits());
        let body = format!(
            "auto bits = [](double d) {{ unsigned long long b; memcpy(&b, &d, 8); return b; }}; \
             double x = r.f64(), y = r.f64(), z = r.f64(); unsigned long long seed = r.u64(); \
             double pa = r.f64(); int pb = r.i32(); double vx = r.f64(), vy = r.f64(), vz = r.f64(); signed char n = r.i8(); \
             if (bits(x) != {} || bits(y) != {} || z != 2.0 || seed != 0x0102030405060708ULL || bits(pa) != {} || pb != -2 \
                 || vx != 1.5 || !(vy != vy) || vz != 0.0 || n != -3) {{ err = \"request\"; return false; }} \
             w.i32(1); w.i32(-1); w.i32(1); w.i32(2); w.i32(3); w.i32(-1); w.i32(0); w.i32(7); return true;",
            bits(1.5),
            bits(-0.0),
            bits(-0.0)
        );
        let mut t = cstub("t_cgolden", &body, "#include <cstring>");
        t.source = t.source.replace("serve(24,", &format!("serve({},", request.len()));
        let env = TestEnv::new();
        let arts = h.build_tests(&records_source(env.spec()), std::slice::from_ref(&t));
        let Artifact::Binary { path, id } = &arts["t_cgolden"] else {
            return Err(Failed::from(format!("the stub didn't build: {:?}", arts["t_cgolden"])));
        };
        let make = || h.command(path, id);
        let o = run_cases_opts(&make, &[request], response.len() - 1, SHORT, Duration::from_secs(10), true);
        match &o[0] {
            CaseOutcome::Returned(b) if *b == response[1..] => Ok(()),
            other => Err(Failed::from(format!("expected the golden response, got {other:?}"))),
        }
    });
    // Toy-table goldens (crate README, "Rules and contracts, Toy tables"): the Rust and Java values, computed on the host and on the device.
    t!("toy_table_goldens", |h: &Harness| {
        let extra = "\
__managed__ unsigned long long g_out[12];
static const unsigned want_perm[8] = {216, 28, 120, 37, 55, 25, 49, 241};
__global__ void toy() {
    tw::PerlinNoiseSampler p = tw::make_permutation_table(1, 2, 3, 4, 5, 6);
    for (int i = 0; i < 8; i++) g_out[i] = p.perm[i];
    tw::InterpolatedNoiseSampler b = tw::make_base3d_perm_table(7);
    g_out[8] = b.state;
    double v = tw::sampler_sample(&b, double3{1.5, -2.0, 3.0});
    unsigned long long bits; memcpy(&bits, &v, 8);
    g_out[9] = bits;
}
";
        let body = "r.f64(); \
            toy<<<1, 1>>>(); cudaError_t e = cudaDeviceSynchronize(); if (e != cudaSuccess) { err = cudaGetErrorString(e); return false; } \
            tw::PerlinNoiseSampler p = tw::make_permutation_table(1, 2, 3, 4, 5, 6); \
            for (int i = 0; i < 8; i++) if (p.perm[i] != want_perm[i] || g_out[i] != want_perm[i]) { err = \"perm \" + std::to_string(i); return false; } \
            tw::InterpolatedNoiseSampler b = tw::make_base3d_perm_table(7); \
            double v = tw::sampler_sample(&b, double3{1.5, -2.0, 3.0}); unsigned long long bits; memcpy(&bits, &v, 8); \
            if (b.state != 0x63cbe1e459320dd7ULL || g_out[8] != b.state) { err = \"base3d state\"; return false; } \
            if (bits != 0x3fede2357dfdf669ULL || g_out[9] != bits) { err = \"sample\"; return false; } \
            w.f64(1.0); return true;";
        let (_, v) = run(h, &program("1.0", &[0.0]), body, &format!("#include <cstring>\n{extra}"), SHORT)?;
        expect_pass(&v[0])
    });
    // One call to every registered extern, on the device (crate README, "Things worth knowing"): a missing one is a compile error.
    t!("runtime_coverage", |h: &Harness| {
        let env = TestEnv::new();
        let mut calls = String::new();
        for e in &env.spec().externs {
            let args: Vec<String> = e
                .params
                .iter()
                .map(|t| match t {
                    VariableType::F64 => "1.5".into(),
                    VariableType::F32 => "1.5f".into(),
                    VariableType::I32 => "3".into(),
                    VariableType::I64 => "3LL".into(),
                    VariableType::Vec3 => "double3{0, 0, 0}".into(),
                    VariableType::Handle("PerlinNoiseSampler") => "&perm".into(),
                    VariableType::Handle("InterpolatedNoiseSampler") => "&samp".into(),
                    other => panic!("no sample argument for {other:?}"),
                })
                .collect();
            write!(calls, "sink += (double)tw::{}({}); ", e.name, args.join(", ")).unwrap();
        }
        let extra = format!(
            "__managed__ double g_sink;\n__global__ void cover(tw::PerlinNoiseSampler perm, tw::InterpolatedNoiseSampler samp) {{ double sink = 0; {calls} g_sink = sink; }}\n"
        );
        let body = "r.f64(); cover<<<1, 1>>>(tw::make_permutation_table(1, 2, 3, 4, 5, 6), tw::make_base3d_perm_table(1)); \
            cudaError_t e = cudaDeviceSynchronize(); if (e != cudaSuccess) { err = cudaGetErrorString(e); return false; } w.f64(1.0); return true;";
        let (_, v) = run(h, &program("1.0", &[0.0]), body, &extra, SHORT)?;
        expect_pass(&v[0])
    });
    out
}
