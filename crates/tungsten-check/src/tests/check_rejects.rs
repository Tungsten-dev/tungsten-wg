//! Lowering tests whose programs the checker rejects, on purpose. [`super::ok`] runs the checker
//! on every program it lowers; a rejection fails the test unless the test is listed here.

/// `(test path, reason)`.
pub const REJECTS: &[(&str, &str)] = &[
    (
        "tests::determinism::clone_and_compact",
        "the parse corpus files basic.tw and inline.tw have mains with a param (MainParams)",
    ),
    (
        "tests::map::forward_then_reverse",
        "the parse corpus (basic.tw, inline.tw: MainParams)",
    ),
    ("tests::ids::holes", "hand-built fns without a return, to test id holes (MissingReturn)"),
    ("tests::ids::locals", "a hand-built main with params, to test local ids (MainParams)"),
    (
        "tests::normalization::calls",
        "lowers a call to an unlisted fn and a mismatched extern annotation as written",
    ),
    ("tests::normalization::casts", "returns casts of non-f64 type from an f64 fn"),
    ("tests::normalization::const_exprs", "reads consts the unit doesn't list"),
    (
        "tests::normalization::constructs",
        "returns vec3/pos3/records from an f64 fn; missing and repeated record fields",
    ),
    ("tests::normalization::fields", "wrong base types and annotations, kept as written"),
    ("tests::normalization::index_and_arrays", "empty array literals"),
    ("tests::normalization::literals", "returns non-f64 literals from an f64 fn"),
    ("tests::normalization::operators", "returns non-f64 operator results from an f64 fn"),
    ("tests::normalization::references", "returns tables, ambients and pos3 from an f64 fn"),
];

/// Panics unless the running test is listed in [`REJECTS`].
pub fn allow(errors: &crate::CheckErrors) {
    let name = std::thread::current().name().unwrap_or("").to_string();
    if !REJECTS.iter().any(|(t, _)| *t == name) {
        panic!(
            "the checker rejects a program of `{name}`, which isn't in check_rejects.rs:\n{errors}"
        );
    }
}
