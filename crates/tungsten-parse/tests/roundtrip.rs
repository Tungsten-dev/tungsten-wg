//! P1–P4: the canonical text round-trips, over a corpus, generated programs and inlined programs.

mod common;

use common::*;
use proptest::prelude::*;
use tungsten_ir::{compact, isomorphic};
use tungsten_parse::*;

fn load_ok(src: &str) -> TestFile {
    match load(src, FileId(0), &NoEnv) {
        Ok(t) => t,
        Err(d) => panic!("{d}\n--- source ---\n{src}"),
    }
}

/// P1 and P2 for one program.
fn check_program(m: &tungsten_ir::OptimizedProgram, what: &str) {
    let text = print(m);
    let t = load_ok(&text);
    let c = compact(m);
    assert!(isomorphic(&t.program, &c), "P1 failed for {what}\n{text}");
    assert_eq!(
        print(&c),
        text,
        "printing the compacted program differs for {what}"
    );
    assert_eq!(print(&t.program), text, "P2 failed for {what}\n{text}");
}

fn check_test_file(t: &TestFile) {
    let text = print_test_file(t);
    let t2 = load_ok(&text);
    assert_eq!(print_test_file(&t2), text, "P3 failed\n{text}");
    // the values survive as well, not just the text
    assert_eq!(t2.cases.len(), t.cases.len());
    for (a, b) in t.cases.iter().zip(&t2.cases) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.seed, b.seed);
        assert_eq!(a.origin.map(f64::to_bits), b.origin.map(f64::to_bits));
        assert_eq!(a.hosts.len(), b.hosts.len());
        for ((_, x), (_, y)) in a.hosts.iter().zip(&b.hosts) {
            assert_eq!(x, y);
        }
    }
    assert_eq!(t2.expects.len(), t.expects.len());
    for (a, b) in t.expects.iter().zip(&t2.expects) {
        assert_eq!((a.case, a.entry, &a.kind), (b.case, b.entry, &b.kind));
    }
    assert_eq!(t2.fuel, t.fuel);
    assert_eq!(t2.tier, t.tier);
    assert_eq!(t2.cells, t.cells);
    assert_eq!(t2.knownfails, t.knownfails);
}

/// One library, `lib`, for the import round-trip.
struct LibEnv(String);

impl Env for LibEnv {
    fn extern_fn(&self, _: &str) -> Option<ExternSig> {
        None
    }
    fn extern_type(&self, _: &str) -> Option<Vec<(&'static str, tungsten_ir::VariableType)>> {
        None
    }
    fn library(&self, name: &str) -> Option<(FileId, &str)> {
        (name == "lib").then_some((FileId(1), self.0.as_str()))
    }
}

proptest! {
    #![proptest_config(config(600))]

    #[test]
    fn p1_p2_generated_programs(m in programs(Options::default())) {
        check_program(&m.0, "a generated program");
    }

    #[test]
    fn p1_p2_generated_programs_with_many_units(m in programs(Options {
        max_units: 10,
        ..Options::default()
    })) {
        check_program(&m.0, "a generated program");
    }

    #[test]
    fn p3_generated_test_files(t in test_files(3)) {
        check_test_file(&t.0);
    }

    /// P4 and P1 for imported programs: a generated library, and a file that imports some of it.
    #[test]
    fn p1_p4_imports(lib in programs(Options { library: true, ..Options::default() }), pick in any::<u64>()) {
        let labels = Labels::new(&lib.0);
        let mut names: Vec<&str> = lib.0.functions.iter().map(|(f, _)| labels.function(f)).collect();
        names.extend(lib.0.constants.iter().map(|(c, _)| labels.constant(c)));
        let names: Vec<&str> = names
            .into_iter()
            .enumerate()
            .filter(|(i, _)| pick >> (i % 64) & 1 == 1)
            .map(|(_, n)| n)
            .collect();
        let src = format!("program {{ use lib::{{{}}}; }}\n", names.join(", "));
        let env = LibEnv(print(&lib.0));
        let t = load(&src, FileId(0), &env).unwrap_or_else(|d| panic!("{d}\n{src}"));
        let text = print(&t.program);
        let t2 = load_ok(&text);
        prop_assert!(isomorphic(&t.program, &t2.program), "P4 failed\n{src}\n{text}");
        check_program(&t.program, "an imported program");
    }
}

fn corpus() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut out = vec![];
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "tw") {
            out.push((
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(&p).unwrap(),
            ));
        }
    }
    out.sort();
    assert!(!out.is_empty());
    out
}

/// A tiny prelude for the corpus files that `use` one.
pub struct TestEnv;

pub const PRELUDE: &str = r#"
program {
    const half = 0.5;
    const quarter = (const(half) * const(half));
    fn halve(x: f64) -> f64 { return (x * const(half)); }
    fn quarter_of(x: f64) -> f64 { return (halve(halve(x)) + const(quarter)); }
    fn unrelated() -> f64 { return 0.0; }
}
"#;

impl Env for TestEnv {
    fn extern_fn(&self, name: &str) -> Option<ExternSig> {
        match name {
            "sin" | "cos" => Some(ExternSig {
                parameter_types: vec![tungsten_ir::VariableType::F64],
                return_type: tungsten_ir::VariableType::F64,
            }),
            _ => None,
        }
    }
    fn extern_type(&self, name: &str) -> Option<Vec<(&'static str, tungsten_ir::VariableType)>> {
        match name {
            "Rec" => Some(vec![
                ("re", tungsten_ir::VariableType::F64),
                ("im", tungsten_ir::VariableType::F64),
            ]),
            _ => None,
        }
    }
    fn library(&self, name: &str) -> Option<(FileId, &str)> {
        (name == "prelude").then_some((FileId(1), PRELUDE))
    }
}

#[test]
fn p1_to_p4_corpus() {
    for (name, src) in corpus() {
        let t = match load(&src, FileId(0), &TestEnv) {
            Ok(t) => t,
            Err(d) => panic!("{name}: {d}"),
        };
        if t.reject.is_some() {
            // reject files are exercised in the L0 tests; they still print
        }
        // P3
        let text = print_test_file(&t);
        let t2 = load_ok(&text);
        assert_eq!(print_test_file(&t2), text, "P3 failed for {name}");
        // P1 on the loaded program (compact by construction)
        assert!(isomorphic(&t2.program, &t.program), "P1 failed for {name}");
        // P4: lenient and canonical agree, program and envelope
        assert_eq!(t2.cases.len(), t.cases.len());
        assert_eq!(t2.expects.len(), t.expects.len());
    }
}

#[test]
fn loaded_programs_are_compact() {
    for (name, src) in corpus() {
        let t = load(&src, FileId(0), &TestEnv).unwrap();
        assert!(isomorphic(&t.program, &compact(&t.program)), "{name}");
    }
}

#[test]
fn p1_p2_after_inlining() {
    let mut total = 0;
    for (name, src) in corpus() {
        let t = load(&src, FileId(0), &TestEnv).unwrap();
        if t.program.entry_points.is_empty() {
            continue;
        }
        let mut m = t.program;
        let stats = tungsten_optimizer::perform_inline(&mut m);
        total += stats.inlined;
        check_program(&m, &format!("{name} after {} inlines", stats.inlined));
    }
    assert!(total > 0, "no corpus file exercised the inliner");
}
