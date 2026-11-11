//! CUDA worker checks that the corpus can't express (`specs/workers.md` §8, `specs/refinement.md`
//! §9): two workers on two host threads, marched slices and worker isolation. Each builds a generated
//! program with a hand-written driver through the real CUDA harness. Skipped without `nvcc` and a
//! GPU, unless `TW_REQUIRE` lists `cuda`.

use std::path::PathBuf;
use std::time::Duration;

use tungsten_cuda::CudaConfig;
use tungsten_cuda::orchestration::cuda_worker_names;
use tungsten_interface::naming::WorkerNames;
use tungsten_interface::{Backend, Interface};
use tungsten_parse::{FileId, NoEnv, load};
use tungsten_testsuite::env::TestEnv;
use tungsten_testsuite::harness::batch::{Harness, TestSource};
use tungsten_testsuite::harness::cuda::records_source;
use tungsten_testsuite::harness::process::{CaseOutcome, STARTUP_ALLOWANCE, run_cases_cmd};

const TIMEOUT: Duration = Duration::from_secs(120);

fn harness() -> Option<Harness> {
    match Harness::new_cuda(PathBuf::from(env!("CARGO_TARGET_TMPDIR")), false) {
        Ok(h) => Some(h),
        Err(e) => {
            let required = std::env::var("TW_REQUIRE").unwrap_or_default();
            assert!(!required.split(',').any(|b| b.trim() == "cuda"), "TW_REQUIRE lists cuda, but: {e}");
            eprintln!("skipped: no CUDA toolchain: {e}");
            None
        }
    }
}

struct Generated {
    files: Vec<(String, String)>,
    names: Vec<WorkerNames>,
    /// `enqueue_refine_<name>` of each refinement.
    refine: Vec<String>,
}

fn generate(id: &str, src: &str) -> Generated {
    let program = load(src, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}")).program;
    let config = CudaConfig { namespace: id.to_string(), ..CudaConfig::default() };
    let backend = config.to_backend();
    let rules = backend.naming_rules();
    let interface = Interface::build(&program, &rules);
    let out = backend.generate(&program, &interface).unwrap();
    let refine = (0..program.refinements.len()).map(|r| format!("enqueue_{}", interface.refine_method(r))).collect();
    Generated { files: out.files(), names: cuda_worker_names(&program, &rules), refine }
}

/// A test: the generated files, and a driver whose `run` is `body` (it answers 28-byte requests) and
/// whose other members are `extra`, in the test's namespace.
fn module(id: &str, g: &Generated, body: &str, extra: &str) -> TestSource {
    let mut source = String::new();
    for (name, text) in &g.files {
        source.push_str(&format!("//@@FILE {id}/{name}\n{text}\n"));
    }
    source.push_str(&format!(
        "//@@FILE {id}.cu\n#include \"tw/tungsten_runtime.cuh\"\n#include \"tw/proto.h\"\n#include <thread>\n#include <vector>\n\
         #include \"{id}/orchestration.h\"\n#include \"{id}/density.cu\"\n#include \"{id}/orchestration.cu\"\n\
         namespace {id} {{\nstatic unsigned long long bit(double d) {{ unsigned long long u; std::memcpy(&u, &d, 8); return u; }}\n{extra}\n\
         static bool run(tw::proto::Reader& r, tw::proto::Writer& w, std::string& err) {{\n{body}\n    return true;\n}}\n}}\n\
         int tw_serve_{id}() {{ cudaSetDevice(0); return tw::proto::serve(28, 16, {id}::run); }}\n"
    ));
    TestSource { name: id.to_string(), id: id.to_string(), source }
}

fn run_one(h: &Harness, t: &TestSource) -> CaseOutcome {
    let bin = h
        .build(&records_source(TestEnv::new().spec()), &[t])
        .unwrap_or_else(|(out, dir)| panic!("build failed ({}):\n{out}", dir.display()));
    let make = || h.command(&bin, &t.id);
    run_cases_cmd(&make, &[vec![0u8; 28]], 2, TIMEOUT, STARTUP_ALLOWANCE).remove(0)
}

const ORIGIN_PROGRAM: &str = r#"
program {
    fn leaf_main() -> f64 {
        return ((ambient(origin).x * 3.0) + (ambient(pos3).x as f64));
    }
    unit leaf { main leaf_main; }
    input leaf_in = leaf @dims(4, 2, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    fn mid_main() -> f64 {
        return ((density(leaf_in) * 0.5) + ambient(origin).z);
    }
    unit mid { main mid_main; inputs [leaf_in]; }
    input mid_in = mid @dims(4, 2, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    fn top_main() -> f64 {
        return (density(mid_in) - (ambient(pos3).y as f64));
    }
    unit top { main top_main; inputs [mid_in]; }
    entry top @dims(4, 2, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

const SLICES: &str = r#"
program {
    fn coarse_main() -> f64 {
        let y: i32 = ambient(pos3).y;
        return (((((y * y) + ambient(pos3).x) as f64) * 0.7) + 0.1);
    }
    unit coarse { main coarse_main; }
    input g = coarse @dims(2, 3, 2) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @lerp(1, 3, 1);
    fn f_main() -> f64 {
        let c: pos3 = ambient(pos3);
        return ((density(g) + ((c.x as f64) * 0.1)) + ((c.z as f64) * 0.01));
    }
    unit f { main f_main; inputs [g]; }
    entry full = f @dims(2, 4, 2) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    entry slice = f @dims(2, 1, 2) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    refinement down of slice @march(0, 3, 0) {
        rerun f @dims(2, 1, 2) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    }
}
"#;

const ISOLATION: &str = r#"
program {
    fn leaf_main() -> f64 { return ((ambient(origin).x * 3.0) + (ambient(pos3).x as f64)); }
    unit leaf { main leaf_main; }
    input frozen = leaf @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    fn seed_main() -> f64 { return 0.0; }
    unit seed { main seed_main; }
    input prev = seed @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @previous;
    fn step_main() -> f64 {
        return (((density(prev) + density(frozen)) + ambient(origin).x) + (ambient(pos3).y as f64));
    }
    unit step { main step_main; inputs [prev, frozen]; }
    entry a = seed @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    entry b = leaf @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    refinement go of a @march(0, 3, 0) {
        rerun seed @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) with step;
    }
}
"#;

/// Two workers of one `Orchestration`, each on its own host thread and stream, on distinct origins,
/// equal a sequential run on one worker.
#[test]
fn two_workers_on_two_host_threads_equal_a_sequential_run() {
    let Some(h) = harness() else { return };
    let id = "t_cw_threads";
    let g = generate(id, ORIGIN_PROGRAM);
    let (worker, factory, hosts) = (&g.names[0].worker, &g.names[0].factory, &g.names[0].host_consts);
    let extra = format!(
        r#"
static bool run_on(Orchestration& orch, double ox, double oz, std::vector<unsigned long long>& out) {{
    cudaSetDevice(0);
    std::unique_ptr<{worker}> w;
    if (!orch.{factory}(w).ok()) return false;
    {hosts} hc{{}};
    for (int k = 0; k < 3; k++) {{
        if (!w->enqueue(Vec3{{ox, 0.0, oz}}, hc).ok() || !w->sync().ok()) return false;
    }}
    out.clear();
    for (int i = 0; i < 32; i++) out.push_back(bit(w->result()[i]));
    return true;
}}"#
    );
    let body = r#"
    r.f64(); r.f64(); r.f64();
    std::shared_ptr<Orchestration> orch;
    if (!Orchestration::create(orch).ok()) { err = "create"; return false; }
    bool equal = true, distinct = true;
    for (int round = 0; round < 4; round++) {
        std::vector<unsigned long long> a, b, sa, sb;
        bool oka = false, okb = false;
        std::thread ta([&] { oka = run_on(*orch, round * 16.0, -round, a); });
        std::thread tb([&] { okb = run_on(*orch, round * 16.0 + 64.0, -round - 8, b); });
        ta.join(); tb.join();
        if (!oka || !okb) { err = "threaded run"; return false; }
        if (!run_on(*orch, round * 16.0, -round, sa) || !run_on(*orch, round * 16.0 + 64.0, -round - 8, sb)) { err = "sequential run"; return false; }
        equal = equal && a == sa && b == sb;
        distinct = distinct && a != b;
    }
    w.b(equal); w.b(distinct);"#;
    let t = module(id, &g, body, &extra);
    assert_eq!(run_one(&h, &t), CaseOutcome::Returned(vec![1, 1]));
}

/// Marched slices equal the full grid, by bits (`specs/refinement.md` §9), on the generated CUDA.
#[test]
fn marched_slices_equal_the_full_grid() {
    let Some(h) = harness() else { return };
    let id = "t_cw_slices";
    let g = generate(id, SLICES);
    let (full, slice, refine) = (&g.names[0], &g.names[1], &g.refine[0]);
    let body = format!(
        r#"
    r.f64(); r.f64(); r.f64();
    std::shared_ptr<Orchestration> orch;
    if (!Orchestration::create(orch).ok()) {{ err = "create"; return false; }}
    std::unique_ptr<{fw}> full; std::unique_ptr<{sw}> slice;
    if (!orch->{ff}(full).ok() || !orch->{sf}(slice).ok()) {{ err = "factory"; return false; }}
    {fh} fhc{{}}; {sh} hc{{}};
    if (!full->enqueue(Vec3{{0, 0, 0}}, fhc).ok() || !full->sync().ok()) {{ err = "full"; return false; }}
    unsigned long long fullBits[16];
    for (int i = 0; i < 16; i++) fullBits[i] = bit(full->result()[i]);
    if (!slice->enqueue(Vec3{{0, 0, 0}}, hc).ok() || !slice->sync().ok()) {{ err = "slice"; return false; }}
    bool equal = true; int layers = 0;
    for (int y = 3; y >= 0; y--) {{
        if (!slice->{refine}(Pos3{{0, y, 0}}, hc).ok() || !slice->sync().ok()) {{ err = "refine"; return false; }}
        layers++;
        for (int z = 0; z < 2; z++) for (int x = 0; x < 2; x++)
            equal = equal && bit(slice->result()[x + 2 * z]) == fullBits[x + 2 * (y + 4 * z)];
    }}
    w.b(equal); w.b(layers == 4);"#,
        fw = full.worker,
        ff = full.factory,
        fh = full.host_consts,
        sw = slice.worker,
        sf = slice.factory,
        sh = slice.host_consts,
    );
    let t = module(id, &g, &body, "");
    assert_eq!(run_one(&h, &t), CaseOutcome::Returned(vec![1, 1]));
}

/// Running a worker of another entry between `enqueue` and `enqueue_refine_*` leaves the first
/// worker's march unchanged: each worker keeps its own origin and its own frozen buffers.
#[test]
fn another_worker_between_enqueue_and_refine_changes_nothing() {
    let Some(h) = harness() else { return };
    let id = "t_cw_isolation";
    let g = generate(id, ISOLATION);
    let (a, b, refine) = (&g.names[0], &g.names[1], &g.refine[0]);
    let extra = format!(
        r#"
static bool march(Orchestration& orch, bool between, unsigned long long out[3][2]) {{
    std::unique_ptr<{aw}> a;
    if (!orch.{af}(a).ok()) return false;
    {ah} ahc{{}};
    if (!a->enqueue(Vec3{{1.5, 0.0, 0.0}}, ahc).ok() || !a->sync().ok()) return false;
    const int ys[3] = {{1, 3, 2}};
    for (int i = 0; i < 3; i++) {{
        if (between) {{
            std::unique_ptr<{bw}> other;
            {bh} bhc{{}};
            if (!orch.{bf}(other).ok() || !other->enqueue(Vec3{{40.0, 0.0, 0.0}}, bhc).ok() || !other->sync().ok()) return false;
        }}
        if (!a->{refine}(Pos3{{0, ys[i], 0}}, ahc).ok() || !a->sync().ok()) return false;
        for (int c = 0; c < 2; c++) out[i][c] = bit(a->result()[c]);
    }}
    return true;
}}"#,
        aw = a.worker,
        af = a.factory,
        ah = a.host_consts,
        bw = b.worker,
        bf = b.factory,
        bh = b.host_consts,
    );
    let body = r#"
    r.f64(); r.f64(); r.f64();
    std::shared_ptr<Orchestration> orch;
    if (!Orchestration::create(orch).ok()) { err = "create"; return false; }
    unsigned long long alone[3][2], mixed[3][2];
    if (!march(*orch, false, alone) || !march(*orch, true, mixed)) { err = "march"; return false; }
    // By hand, from origin 1.5: `frozen` is [4.5, 5.5], and each step adds `frozen`, the origin and
    // the marked y to the cell before: [7, 8], then [16, 18], then [24, 27].
    const double want[3][2] = {{7.0, 8.0}, {16.0, 18.0}, {24.0, 27.0}};
    bool same = true, right = true;
    for (int i = 0; i < 3; i++) for (int c = 0; c < 2; c++) {
        same = same && alone[i][c] == mixed[i][c];
        right = right && alone[i][c] == bit(want[i][c]);
    }
    w.b(same); w.b(right);"#;
    let t = module(id, &g, body, &extra);
    assert_eq!(run_one(&h, &t), CaseOutcome::Returned(vec![1, 1]));
}
