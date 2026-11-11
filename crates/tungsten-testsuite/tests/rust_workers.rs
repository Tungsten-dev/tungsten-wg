//! Rust worker checks that the corpus can't express (`specs/workers.md` §8): concurrency, the
//! bounds modes, the static assertion and the record contract. Each builds a generated program
//! with a hand-written driver through the real Rust harness. Skipped without `rustc`, unless
//! `TW_REQUIRE` lists `rust`.

use std::path::PathBuf;
use std::time::Duration;

use tungsten_interface::{Backend, BufferBounds, Interface};
use tungsten_parse::{FileId, NoEnv, load};
use tungsten_rust::backend::{RustBackend, RustConfig};
use tungsten_rust::orchestration::rust_worker_names;
use tungsten_testsuite::env::TestEnv;
use tungsten_testsuite::harness::batch::{Harness, TestSource};
use tungsten_testsuite::harness::process::{CaseOutcome, run_cases};
use tungsten_testsuite::harness::rust::records_source;

const TIMEOUT: Duration = Duration::from_secs(30);

fn harness() -> Option<Harness> {
    match Harness::new(PathBuf::from(env!("CARGO_TARGET_TMPDIR")), false) {
        Ok(h) => Some(h),
        Err(e) => {
            let required = std::env::var("TW_REQUIRE").unwrap_or_default();
            assert!(!required.split(',').any(|b| b.trim() == "rust"), "TW_REQUIRE lists rust, but: {e}");
            eprintln!("skipped: no Rust toolchain: {e}");
            None
        }
    }
}

/// The generated modules of `src`, loaded without a registry.
fn generate(src: &str, bounds: BufferBounds) -> (String, Vec<tungsten_interface::naming::WorkerNames>) {
    let program = load(src, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}")).program;
    let backend = RustBackend::new(RustConfig { buffer_bounds: bounds, ..RustConfig::default() });
    let interface = Interface::build(&program, &backend.naming_rules());
    let out = backend.generate(&program, &interface).unwrap();
    (out.full_source, rust_worker_names(&program, &backend.naming_rules()))
}

/// A test module: runtime imports, `items` (runtime additions the program names), the generated
/// modules and a driver whose `run_case` is `body`, answering 28-byte requests (the 24 of an origin, and `steps = 0`).
fn module(id: &str, items: &str, generated: &str, body: &str) -> TestSource {
    TestSource {
        name: id.to_string(),
        id: id.to_string(),
        source: format!(
            "use crate::mathf64::*;\nuse crate::utilsf64::*;\n{items}\n{generated}\n\
             pub mod driver {{\n    use super::*;\n    use super::orchestration::*;\n    \
             pub const REQUEST_LEN: usize = 28;\n    pub const STEP_LEN: usize = 16;\n    \
             pub fn run_case(r: &mut crate::proto::Reader) -> Vec<u8> {{\n{body}\n    }}\n}}\n"
        ),
    }
}

fn records() -> String {
    records_source(TestEnv::new().spec())
}

/// Builds `t` alone and runs one request on it.
fn run_one(h: &Harness, t: &TestSource, body_len: usize) -> CaseOutcome {
    let bin = h.build(&records(), &[t]).unwrap_or_else(|(out, dir)| panic!("build failed ({}):\n{out}", dir.display()));
    let mut outcomes = run_cases(&bin, &t.id, &[vec![0u8; 28]], body_len, TIMEOUT);
    outcomes.remove(0)
}

/// The compiler output of `t`, which must not build.
fn build_error(h: &Harness, t: &TestSource) -> String {
    match h.build(&records(), &[t]) {
        Ok(_) => panic!("`{}` built, but it must not", t.id),
        Err((out, _)) => out,
    }
}

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

/// N threads, each with its own worker from one `Orchestration`, on distinct origins, equal a
/// sequential run on one worker. Workers go to their threads both ways: created there from a
/// cloned `Orchestration`, and created here and moved.
#[test]
fn concurrent_workers_equal_a_sequential_run() {
    let Some(h) = harness() else { return };
    let (generated, names) = generate(ORIGIN_PROGRAM, BufferBounds::PerBuffer);
    let (factory, hosts) = (&names[0].factory, &names[0].host_consts);
    let body = format!(
        r#"
        let _ = (r.f64(), r.f64(), r.f64());
        let orch = Orchestration::new();
        let hc = {hosts}::default();
        let origins: Vec<Vec3> = (0..8).map(|i| Vec3::new(i as f64 * 16.0, 0.0, -(i as f64))).collect();
        let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<u64>>();
        let handles: Vec<_> = origins.iter().enumerate().map(|(i, &o)| {{
            let hc = hc;
            if i % 2 == 0 {{
                let orch = orch.clone();
                std::thread::spawn(move || {{
                    let mut w = orch.{factory}();
                    let mut out = vec![];
                    for _ in 0..3 {{ out = bits(w.run(o, &hc)); }}
                    out
                }})
            }} else {{
                let mut w = orch.{factory}();
                std::thread::spawn(move || {{
                    let mut out = vec![];
                    for _ in 0..3 {{ out = bits(w.run(o, &hc)); }}
                    out
                }})
            }}
        }}).collect();
        let threaded: Vec<Vec<u64>> = handles.into_iter().map(|t| t.join().unwrap()).collect();
        let mut one = orch.{factory}();
        let sequential: Vec<Vec<u64>> = origins.iter().map(|&o| bits(one.run(o, &hc))).collect();
        let mut w = crate::proto::Writer::new();
        w.bool(threaded == sequential);
        w.bool(sequential.windows(2).all(|p| p[0] != p[1]));
        w.finish()"#
    );
    let t = module("t_rw_concurrency", "", &generated, &body);
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

const OUT_OF_RANGE_DRIVER: &str = r#"
        let _ = (r.f64(), r.f64(), r.f64());
        let orch = Orchestration::new();
        let mut w = crate::proto::Writer::new();
        w.f64(orch.top_main_worker().run(Vec3::new(0.0, 0.0, 0.0), &TopMainHostConsts::default()));
        w.finish()"#;

/// `PerBuffer` fails an index past its input even where the pool goes on, as a separate array
/// would. `Slab` only checks the pool, so the same read lands on the neighbour (safe Rust, so it
/// runs). `Unchecked` compiles, and isn't run, since the read is UB.
#[test]
fn bounds_modes() {
    let Some(h) = harness() else { return };
    let per_buffer = module("t_rw_per_buffer", "", &generate(OUT_OF_RANGE, BufferBounds::PerBuffer).0, OUT_OF_RANGE_DRIVER);
    match run_one(&h, &per_buffer, 8) {
        CaseOutcome::Failed { kind: 1, message } => assert!(message.contains("out of bounds"), "{message}"),
        other => panic!("expected an OutOfBounds failure, got {other:?}"),
    }
    let slab = module("t_rw_slab", "", &generate(OUT_OF_RANGE, BufferBounds::Slab).0, OUT_OF_RANGE_DRIVER);
    assert_eq!(run_one(&h, &slab, 8), CaseOutcome::Returned(7.25f64.to_bits().to_le_bytes().to_vec()));
    let (unchecked, _) = generate(OUT_OF_RANGE, BufferBounds::Unchecked);
    assert!(unchecked.contains("unsafe {"));
    let unchecked = module("t_rw_unchecked", "", &unchecked, OUT_OF_RANGE_DRIVER);
    h.build(&records(), &[&unchecked]).unwrap_or_else(|(out, _)| panic!("Unchecked didn't build:\n{out}"));
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

/// Reads cell `cell` of the entry.
fn lerp_driver(cell: usize) -> String {
    format!(
        r#"
        let _ = (r.f64(), r.f64(), r.f64());
        let orch = Orchestration::new();
        let mut w = crate::proto::Writer::new();
        w.f64(orch.top_main_worker().run(Vec3::new(0.0, 0.0, 0.0), &TopMainHostConsts::default())[{cell}]);
        w.finish()"#
    )
}

/// The lerp helper follows `BufferBounds` like every other read (`specs/density-mappings.md`
/// §5.2). `PerBuffer` fails the high corner past the input, `Slab` reads the neighbour (cell 2 is NaN,
/// cell 0 is untouched), and `Unchecked` builds.
#[test]
fn lerp_follows_bounds_modes() {
    let Some(h) = harness() else { return };
    let nan = |o: CaseOutcome| match o {
        CaseOutcome::Returned(b) => f64::from_le_bytes(b.try_into().unwrap()).is_nan(),
        other => panic!("expected a value, got {other:?}"),
    };
    let build = |id: &str, bounds, cell| module(id, "", &generate(LERP_PAST_END, bounds).0, &lerp_driver(cell));

    // The run computes every cell, so `PerBuffer` fails it whichever cell is read.
    match run_one(&h, &build("t_rw_lerp_per", BufferBounds::PerBuffer, 0), 8) {
        CaseOutcome::Failed { kind: 1, message } => assert!(message.contains("out of bounds"), "{message}"),
        other => panic!("expected an OutOfBounds failure, got {other:?}"),
    }
    let slab_first = build("t_rw_lerp_slab_0", BufferBounds::Slab, 0);
    assert_eq!(run_one(&h, &slab_first, 8), CaseOutcome::Returned(1.5f64.to_bits().to_le_bytes().to_vec()));
    assert!(nan(run_one(&h, &build("t_rw_lerp_slab_2", BufferBounds::Slab, 2), 8)));

    let unchecked = build("t_rw_lerp_unchecked", BufferBounds::Unchecked, 0);
    assert!(unchecked.source.contains("unsafe {"));
    h.build(&records(), &[&unchecked]).unwrap_or_else(|(out, _)| panic!("Unchecked didn't build:\n{out}"));
}

/// A handle type the program names, and the extern that reads it.
fn table_items(cell: bool) -> String {
    let field = if cell { "std::cell::Cell<i64>" } else { "i64" };
    let value = if cell { "std::cell::Cell::new(seed)" } else { "seed" };
    let get = if cell { "t.c.get()" } else { "t.c" };
    format!(
        "#[derive(Clone)]\npub struct LocalTable {{ pub c: {field} }}\n\
         pub fn make_local_table(seed: i64) -> LocalTable {{ LocalTable {{ c: {value} }} }}\n\
         pub fn local_get(t: &LocalTable) -> f64 {{ {get} as f64 }}\n"
    )
}

const HANDLE_PROGRAM: &str = r#"
program {
    param seed: i64;
    auto t: handle LocalTable = make_local_table(param(seed));
    fn run() -> f64 {
        return ext local_get::<(handle LocalTable) -> f64>(auto(t));
    }
    unit u { main run; autos [t]; }
    entry u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

const STUB_DRIVER: &str = "        let _ = r; crate::proto::Writer::new().finish()";

/// A runtime handle that isn't `Sync` fails at the generated assertion, which names
/// `Orchestration`. The same handle without the `Cell` builds.
#[test]
fn static_assertion_names_orchestration() {
    let Some(h) = harness() else { return };
    let (generated, _) = generate(HANDLE_PROGRAM, BufferBounds::PerBuffer);
    let ok = module("t_rw_sync_handle", &table_items(false), &generated, STUB_DRIVER);
    h.build(&records(), &[&ok]).unwrap_or_else(|(out, _)| panic!("the Sync handle didn't build:\n{out}"));
    let bad = module("t_rw_cell_handle", &table_items(true), &generated, STUB_DRIVER);
    let out = build_error(&h, &bad);
    assert!(out.contains("cannot be shared between threads safely"), "{out}");
    assert!(out.contains("s::<Orchestration>()"), "the error isn't at the assertion:\n{out}");
}

/// A record used as a unit output must be `Default`; without it the build fails naming the bound.
/// The test runtime's records derive it.
#[test]
fn records_must_be_default() {
    let Some(h) = harness() else { return };
    assert!(records().contains("#[derive(Clone, Copy, Debug, Default)]\npub struct Pair"));
    let program = r#"
program {
    fn nd_main() -> extern NoDefault {
        return ext make_nd::<(f64) -> extern NoDefault>(1.0);
    }
    unit nd { main nd_main; }
    entry nd @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;
    let items = "#[derive(Clone, Copy)]\npub struct NoDefault { pub x: f64 }\n\
                 pub fn make_nd(x: f64) -> NoDefault { NoDefault { x } }\n";
    let t = module("t_rw_no_default", items, &generate(program, BufferBounds::PerBuffer).0, STUB_DRIVER);
    let out = build_error(&h, &t);
    assert!(out.contains("NoDefault: Default"), "{out}");
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

/// Marched slices equal the full grid, by bits (`specs/refinement.md` §9), on the generated Rust.
#[test]
fn marched_slices_equal_the_full_grid() {
    let Some(h) = harness() else { return };
    let (generated, names) = generate(SLICES, BufferBounds::PerBuffer);
    let (full, slice) = (&names[0], &names[1]);
    let body = format!(
        r#"
        let _ = (r.f64(), r.f64(), r.f64());
        let orch = Orchestration::new();
        let origin = Vec3::new(0.0, 0.0, 0.0);
        let mut full = orch.{ff}();
        let full_bits: Vec<u64> = full.run(origin, &{fh}::default()).iter().map(|x| x.to_bits()).collect();
        let hc = {sh}::default();
        let mut slice = orch.{sf}();
        let _ = slice.run(origin, &hc);
        let mut equal = true;
        let mut layers = 0;
        for y in (0..4).rev() {{
            let got = slice.refine_down(Pos3::new(0, y, 0), &hc);
            layers += 1;
            for z in 0..2 {{
                for x in 0..2 {{
                    equal &= got[x + 2 * z].to_bits() == full_bits[x + 2 * (y as usize + 4 * z)];
                }}
            }}
        }}
        let mut w = crate::proto::Writer::new();
        w.bool(equal);
        w.bool(layers == 4);
        w.finish()"#,
        ff = full.factory,
        fh = full.host_consts,
        sf = slice.factory,
        sh = slice.host_consts,
    );
    let t = module("t_rw_slices", "", &generated, &body);
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
    let (generated, names) = generate(ISOLATION, BufferBounds::PerBuffer);
    let (a, b) = (&names[0], &names[1]);
    let body = format!(
        r#"
        let _ = (r.f64(), r.f64(), r.f64());
        let orch = Orchestration::new();
        let (ha, hb) = ({ah}::default(), {bh}::default());
        let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<u64>>();
        let march = |between: bool| -> Vec<Vec<u64>> {{
            let mut a = orch.{af}();
            let _ = a.run(Vec3::new(1.5, 0.0, 0.0), &ha);
            let mut out = vec![];
            for y in [1, 3, 2] {{
                if between {{
                    // Another origin, and a `leaf` buffer of its own.
                    let mut other = orch.{bf}();
                    let _ = other.run(Vec3::new(40.0, 0.0, 0.0), &hb);
                }}
                out.push(bits(a.refine_go(Pos3::new(0, y, 0), &ha)));
            }}
            out
        }};
        let (alone, interleaved) = (march(false), march(true));
        let mut w = crate::proto::Writer::new();
        w.bool(alone == interleaved);
        // By hand, from origin 1.5: `frozen` is [4.5, 5.5], and each step adds `frozen`, the origin and
        // the marked y to the cell before: [7, 8], then [16, 18], then [24, 27]. So the march does
        // depend on the origin and on the frozen leaf, and the comparison above can fail.
        let want: Vec<Vec<u64>> = [[7.0, 8.0], [16.0, 18.0], [24.0, 27.0]].iter().map(|v| bits(v)).collect();
        w.bool(alone == want);
        w.finish()"#,
        ah = a.host_consts,
        bh = b.host_consts,
        af = a.factory,
        bf = b.factory,
    );
    let t = module("t_rw_isolation", "", &generated, &body);
    assert_eq!(run_one(&h, &t, 2), CaseOutcome::Returned(vec![1, 1]));
}
