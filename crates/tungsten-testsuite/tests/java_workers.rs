//! Java worker checks that the corpus can't express (`specs/workers.md` §8, `specs/refinement.md`
//! §9): concurrency, the bounds modes, marched slices and worker isolation. Each builds a generated
//! program with a hand-written driver through the real Java harness. Skipped without `javac`,
//! unless `TW_REQUIRE` lists `java`.

use std::path::PathBuf;
use std::time::Duration;

use tungsten_interface::naming::WorkerNames;
use tungsten_interface::{Backend, BufferBounds, Interface};
use tungsten_java::JavaConfig;
use tungsten_java::orchestration::java_worker_names;
use tungsten_parse::{FileId, NoEnv, load};
use tungsten_testsuite::env::TestEnv;
use tungsten_testsuite::harness::batch::{Harness, TestSource};
use tungsten_testsuite::harness::java::records_source;
use tungsten_testsuite::harness::process::{CaseOutcome, STARTUP_ALLOWANCE, run_cases_cmd};

const TIMEOUT: Duration = Duration::from_secs(60);

fn harness() -> Option<Harness> {
    match Harness::new_java(PathBuf::from(env!("CARGO_TARGET_TMPDIR")), false) {
        Ok(h) => Some(h),
        Err(e) => {
            let required = std::env::var("TW_REQUIRE").unwrap_or_default();
            assert!(!required.split(',').any(|b| b.trim() == "java"), "TW_REQUIRE lists java, but: {e}");
            eprintln!("skipped: no Java toolchain: {e}");
            None
        }
    }
}

/// What a generated program offers a driver: its source, worker names and refine method names.
struct Generated {
    source: String,
    names: Vec<WorkerNames>,
    refine: Vec<String>,
}

/// The generated `Gen.java` of `src` in package `id`, loaded without a registry.
fn generate(id: &str, src: &str, bounds: BufferBounds) -> Generated {
    let program = load(src, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}")).program;
    let config = JavaConfig {
        package: id.to_string(),
        class_name: "Gen".into(),
        runtime_package: "tungsten.runtime".into(),
        buffer_bounds: bounds,
    };
    let backend = config.to_backend();
    let rules = backend.naming_rules();
    let interface = Interface::build(&program, &rules);
    let out = backend.generate(&program, &interface).unwrap();
    let refine = (0..program.refinements.len()).map(|r| interface.refine_method(r).to_string()).collect();
    Generated { source: out.source, names: java_worker_names(&program, &rules), refine }
}

/// A test: the generated `Gen`, and a `Driver` whose `runCase` is `body` and whose other members are
/// `extra`. It answers 28-byte requests: the 24 of an origin, and `steps = 0`.
fn module(id: &str, generated: &Generated, body: &str, extra: &str) -> TestSource {
    let driver = format!(
        "package {id};\nimport tungsten.runtime.*;\npublic final class Driver {{\n    \
         static final int REQUEST_LEN = 28;\n    static final int STEP_LEN = 16;\n    {extra}\n    \
         static byte[] runCase(Proto.Reader r) {{\n        try {{\n{body}\n        }} catch (RuntimeException e) {{ throw e; }} catch (Exception e) {{ throw new RuntimeException(e); }}\n    }}\n    \
         public static void main(String[] a) {{ Proto.serve(REQUEST_LEN, STEP_LEN, Driver::runCase); }}\n}}\n"
    );
    TestSource {
        name: id.to_string(),
        id: id.to_string(),
        source: format!("//@@FILE Gen.java\n{}\n//@@FILE Driver.java\n{driver}", generated.source),
    }
}

fn records() -> String {
    records_source(TestEnv::new().spec())
}

/// Builds `t` alone and runs one request on it.
fn run_one(h: &Harness, t: &TestSource, body_len: usize) -> CaseOutcome {
    let bin = h.build(&records(), &[t]).unwrap_or_else(|(out, dir)| panic!("build failed ({}):\n{out}", dir.display()));
    let make = || h.command(&bin, &t.id);
    run_cases_cmd(&make, &[vec![0u8; 28]], body_len, TIMEOUT, STARTUP_ALLOWANCE).remove(0)
}

/// `bits(double[] a, int n)`: the first `n` doubles as raw bits.
const BITS: &str = "static long[] bits(double[] a, int n) {\n        long[] out = new long[n];\n        \
                    for (int i = 0; i < n; i++) out[i] = Double.doubleToRawLongBits(a[i]);\n        return out;\n    }";

/// Reads the origin, so every cell depends on it, through a shared `mid` instance.
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

/// N tasks on an `ExecutorService`, each with its own worker from one shared `Orchestration`, on
/// distinct origins, equal a sequential run on one worker. Workers are created on the pool threads
/// and, for half of them, here first and handed over.
#[test]
fn concurrent_workers_equal_a_sequential_run() {
    let Some(h) = harness() else { return };
    let id = "t_jw_concurrency";
    let g = generate(id, ORIGIN_PROGRAM, BufferBounds::PerBuffer);
    let (worker, factory, hosts) = (&g.names[0].worker, &g.names[0].factory, &g.names[0].host_consts);
    let body = format!(
        r#"
        r.f64(); r.f64(); r.f64();
        final Gen.Orchestration orch = Gen.Orchestration.create();
        java.util.concurrent.ExecutorService pool = java.util.concurrent.Executors.newFixedThreadPool(4);
        java.util.List<java.util.concurrent.Future<long[]>> futures = new java.util.ArrayList<>();
        for (int i = 0; i < 8; i++) {{
            final double ox = i * 16.0, oz = -i;
            final Gen.{worker} handedOver = i % 2 == 1 ? orch.{factory}() : null;
            futures.add(pool.submit(() -> {{
                Gen.{worker} w = handedOver != null ? handedOver : orch.{factory}();
                long[] out = null;
                for (int k = 0; k < 3; k++) out = bits(w.run(ox, 0.0, oz, new Gen.{hosts}()), 32);
                return out;
            }}));
        }}
        Gen.{worker} one = orch.{factory}();
        boolean equal = true;
        boolean distinct = true;
        long[] previous = null;
        for (int i = 0; i < 8; i++) {{
            long[] threaded = futures.get(i).get();
            long[] sequential = bits(one.run(i * 16.0, 0.0, -i, new Gen.{hosts}()), 32);
            equal &= java.util.Arrays.equals(threaded, sequential);
            distinct &= previous == null || !java.util.Arrays.equals(previous, sequential);
            previous = sequential;
        }}
        pool.shutdown();
        Proto.Writer w = new Proto.Writer();
        w.bool(equal);
        w.bool(distinct);
        return w.finish();"#
    );
    let t = module(id, &g, &body, BITS);
    assert_eq!(run_one(&h, &t, 2), CaseOutcome::Returned(vec![1, 1]));
}

/// `a` and `b` share the f64 pool, `b` right after `a`. The read `density(a, 2)` is one past `a`.
const OUT_OF_RANGE: &str = r#"
program {
    fn a_main() -> f64 { return 1.5; }
    unit ua { main a_main; }
    input a = ua @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    fn b_main() -> f64 { return 7.25; }
    unit ub { main b_main; }
    input b = ub @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    fn top_main() -> f64 { return (density(a, 2) + (density(b) * 0.0)); }
    unit top { main top_main; inputs [a, b]; }
    entry top @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

/// Runs the entry `top` at cell `cell` and answers with it.
fn cell_body(g: &Generated, cell: usize) -> String {
    let n = &g.names[0];
    format!(
        r#"
        r.f64(); r.f64(); r.f64();
        Gen.Orchestration orch = Gen.Orchestration.create();
        Proto.Writer w = new Proto.Writer();
        var res = orch.{}().run(0.0, 0.0, 0.0, new Gen.{}());
        w.f64(res[{cell}]);
        return w.finish();"#,
        n.factory, n.host_consts
    )
}

/// The single-cell entry returns its value, not an array.
fn single_cell_body(g: &Generated) -> String {
    let n = &g.names[0];
    format!(
        r#"
        r.f64(); r.f64(); r.f64();
        Gen.Orchestration orch = Gen.Orchestration.create();
        Proto.Writer w = new Proto.Writer();
        w.f64(orch.{}().run(0.0, 0.0, 0.0, new Gen.{}()));
        return w.finish();"#,
        n.factory, n.host_consts
    )
}

/// `PerBuffer` fails an index past its input even where the pool goes on, as a separate array
/// would. `Slab` only checks the pool, so the same read lands on the neighbour. `Unchecked` behaves
/// like `Slab` on Java: the same value.
#[test]
fn bounds_modes() {
    let Some(h) = harness() else { return };
    let run = |id: &str, bounds| {
        let g = generate(id, OUT_OF_RANGE, bounds);
        let t = module(id, &g, &single_cell_body(&g), "");
        run_one(&h, &t, 8)
    };
    match run("t_jw_per_buffer", BufferBounds::PerBuffer) {
        CaseOutcome::Failed { kind: 1, message } => assert!(message.contains("out of bounds"), "{message}"),
        other => panic!("expected an OutOfBounds failure, got {other:?}"),
    }
    let neighbour = CaseOutcome::Returned(7.25f64.to_bits().to_le_bytes().to_vec());
    assert_eq!(run("t_jw_slab", BufferBounds::Slab), neighbour);
    assert_eq!(run("t_jw_unchecked", BufferBounds::Unchecked), neighbour);
}

/// A reader three cells wide over a two-cell lerp input `a`, so the last cell reads `a[2]`, the
/// high corner at weight 0. It lies past `a` and on the first cell of `b`, which is `inf`: a read
/// that reaches it gives `1.5 + 0 * (inf - 1.5) = NaN`.
const LERP_PAST_END: &str = r#"
program {
    fn a_main() -> f64 { return 1.5; }
    unit ua { main a_main; }
    input a = ua @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @lerp(2, *, *);
    fn b_main() -> f64 { return inf; }
    unit ub { main b_main; }
    input b = ub @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    fn top_main() -> f64 { return density(a); }
    unit top { main top_main; inputs [a, b]; }
    entry top @dims(3, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

/// The lerp helper follows `BufferBounds` like every other read (`specs/density-mappings.md`
/// §5.2). `PerBuffer` fails the high corner past the input, `Slab` and `Unchecked` read the
/// neighbour (cell 2 is NaN, cell 0 is untouched).
#[test]
fn lerp_follows_bounds_modes() {
    let Some(h) = harness() else { return };
    let nan = |o: CaseOutcome| match o {
        CaseOutcome::Returned(b) => f64::from_le_bytes(b.try_into().unwrap()).is_nan(),
        other => panic!("expected a value, got {other:?}"),
    };
    let run = |id: &str, bounds, cell| {
        let g = generate(id, LERP_PAST_END, bounds);
        run_one(&h, &module(id, &g, &cell_body(&g, cell), ""), 8)
    };
    // The run computes every cell, so `PerBuffer` fails it whichever cell is read.
    match run("t_jw_lerp_per", BufferBounds::PerBuffer, 0) {
        CaseOutcome::Failed { kind: 1, message } => assert!(message.contains("out of bounds"), "{message}"),
        other => panic!("expected an OutOfBounds failure, got {other:?}"),
    }
    for (mode, tag) in [(BufferBounds::Slab, "slab"), (BufferBounds::Unchecked, "unchecked")] {
        let first = run(&format!("t_jw_lerp_{tag}_0"), mode, 0);
        assert_eq!(first, CaseOutcome::Returned(1.5f64.to_bits().to_le_bytes().to_vec()));
        assert!(nan(run(&format!("t_jw_lerp_{tag}_2"), mode, 2)));
    }
}

/// `corpus/refine/march_slices_full_grid.tw`: `full` is the whole 2x4x2 volume, `slice` one y layer
/// of it, marched top-down.
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

/// Marched slices equal the full grid, by bits (`specs/refinement.md` §9), on the generated Java.
#[test]
fn marched_slices_equal_the_full_grid() {
    let Some(h) = harness() else { return };
    let id = "t_jw_slices";
    let g = generate(id, SLICES, BufferBounds::PerBuffer);
    let (full, slice, refine) = (&g.names[0], &g.names[1], &g.refine[0]);
    let body = format!(
        r#"
        r.f64(); r.f64(); r.f64();
        Gen.Orchestration orch = Gen.Orchestration.create();
        Gen.{fw} full = orch.{ff}();
        long[] fullBits = bits(full.run(0.0, 0.0, 0.0, new Gen.{fh}()), 16);
        Gen.{sw} slice = orch.{sf}();
        Gen.{sh} hc = new Gen.{sh}();
        slice.run(0.0, 0.0, 0.0, hc);
        boolean equal = true;
        int layers = 0;
        for (int y = 3; y >= 0; y--) {{
            long[] got = bits(slice.{refine}(0, y, 0, hc), 4);
            layers++;
            for (int z = 0; z < 2; z++) {{
                for (int x = 0; x < 2; x++) {{
                    equal &= got[x + 2 * z] == fullBits[x + 2 * (y + 4 * z)];
                }}
            }}
        }}
        Proto.Writer w = new Proto.Writer();
        w.bool(equal);
        w.bool(layers == 4);
        return w.finish();"#,
        fw = full.worker,
        ff = full.factory,
        fh = full.host_consts,
        sw = slice.worker,
        sf = slice.factory,
        sh = slice.host_consts,
    );
    let t = module(id, &g, &body, BITS);
    assert_eq!(run_one(&h, &t, 2), CaseOutcome::Returned(vec![1, 1]));
}

/// Entry `a` marches an accumulator over a frozen `leaf` and the origin. Entry `b` is `leaf` itself,
/// so both workers hold an instance of it, in buffers of their own.
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

/// Creating and running a worker of another entry between `run` and `refine_*` leaves the first
/// worker's march unchanged: each worker keeps its own origin and its own frozen buffers.
#[test]
fn another_worker_between_run_and_refine_changes_nothing() {
    let Some(h) = harness() else { return };
    let id = "t_jw_isolation";
    let g = generate(id, ISOLATION, BufferBounds::PerBuffer);
    let (a, b, refine) = (&g.names[0], &g.names[1], &g.refine[0]);
    let body = format!(
        r#"
        r.f64(); r.f64(); r.f64();
        Gen.Orchestration orch = Gen.Orchestration.create();
        long[][] alone = march(orch, false);
        long[][] interleaved = march(orch, true);
        long[][] want = new long[3][];
        double[][] values = {{ {{7.0, 8.0}}, {{16.0, 18.0}}, {{24.0, 27.0}} }};
        for (int i = 0; i < 3; i++) want[i] = bits(values[i], 2);
        Proto.Writer w = new Proto.Writer();
        w.bool(java.util.Arrays.deepEquals(alone, interleaved));
        // By hand, from origin 1.5: `frozen` is [4.5, 5.5], and each step adds `frozen`, the origin and
        // the marked y to the cell before: [7, 8], then [16, 18], then [24, 27]. So the march does
        // depend on the origin and on the frozen leaf, and the comparison above can fail.
        w.bool(java.util.Arrays.deepEquals(alone, want));
        return w.finish();"#
    );
    let extra = format!(
        r#"{BITS}
    static long[][] march(Gen.Orchestration orch, boolean between) {{
        Gen.{aw} a = orch.{af}();
        a.run(1.5, 0.0, 0.0, new Gen.{ah}());
        long[][] out = new long[3][];
        int[] ys = {{ 1, 3, 2 }};
        for (int i = 0; i < 3; i++) {{
            if (between) {{
                // Another origin, and a `leaf` buffer of its own.
                Gen.{bw} other = orch.{bf}();
                other.run(40.0, 0.0, 0.0, new Gen.{bh}());
            }}
            out[i] = bits(a.{refine}(0, ys[i], 0, new Gen.{ah}()), 2);
        }}
        return out;
    }}"#,
        aw = a.worker,
        af = a.factory,
        ah = a.host_consts,
        bw = b.worker,
        bf = b.factory,
        bh = b.host_consts,
    );
    let t = module(id, &g, &body, &extra);
    assert_eq!(run_one(&h, &t, 2), CaseOutcome::Returned(vec![1, 1]));
}
