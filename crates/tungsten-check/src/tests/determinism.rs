//! `lower(p) == lower(p.clone())` and `lower(p) == lower(compact(p))`.

use tungsten_ir::compact;

use super::*;

pub const CORPUS: &[&str] = &[
    include_str!("../../../tungsten-parse/tests/corpus/basic.tw"),
    include_str!("../../../tungsten-parse/tests/corpus/testing_md.tw"),
    include_str!("../../../tungsten-parse/tests/corpus/inline.tw"),
];

/// The parse corpus, plus snippets that use records and externs.
pub fn corpus() -> Vec<OptimizedProgram> {
    let mut out: Vec<OptimizedProgram> = CORPUS.iter().map(|s| load(s)).collect();
    out.push(load(&snippet(
        "fn mk(x: f64) -> extern Pair { return new extern Pair { a: x, b: 1 }; }",
        "let q: extern Pair = mk(p); return (q.a + ext sin(ext sample(table(t), density(d))));",
    )));
    out
}

#[test]
fn clone_and_compact() {
    for p in corpus() {
        let a = ok(&p, &registry());
        let b = ok(&p.clone(), &registry());
        let c = ok(&compact(&p), &registry());
        assert_eq!(a.program, b.program);
        assert_eq!(a.program, c.program);
        let r = registry();
        let d = ok(&p, &r);
        let e = ok(&compact(&p), &r);
        assert_eq!(d.program, e.program);
    }
}

#[test]
fn clone_errors_equal() {
    let r = registry();
    let p = load(&snippet("host hd dynamic;", "return ext nope::<() -> f64>(p.x::<f64>);"));
    assert_eq!(
        crate::lower(&p, &r).unwrap_err(),
        crate::lower(&p.clone(), &r).unwrap_err()
    );
}
