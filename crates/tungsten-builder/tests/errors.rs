//! Poison and collect (spec §4.2), `ForeignHandle` (§4.4) and the other statement errors.
//! One mistake gives one error at the line it was written on, several mistakes are all
//! reported, and nothing panics.

mod common;

use tungsten_builder::prelude::*;
use tungsten_builder::{BuildErrorKind, BuildErrors, Expr, FieldProblem};

fn errors(pb: ProgramBuilder) -> BuildErrors {
    match pb.build() {
        Ok(_) => panic!("expected errors"),
        Err(e) => e,
    }
}

/// `line!()` of the statement right after the call to `here()`.
macro_rules! next_line {
    () => {
        line!() + 1
    };
}

#[test]
fn one_mistake_one_error_at_its_line() {
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("m", F64);
    let x = f.local("x", F64);
    f.assign(x, 1.0);
    let line = next_line!();
    let bad = x + 1; // f64 + i32
    // The unknown result spreads through further operations without new errors.
    let worse = (bad * 2.0 - x).lt(3.0).and(true);
    let r = f.local("r", F64);
    f.if_then(worse, |f| f.assign(r, 1.0));
    f.ret(r);
    pb.finish_unit(f);
    let errs = errors(pb);
    assert_eq!(errs.len(), 1, "{errs}");
    let e = &errs.0[0];
    assert_eq!(
        e.kind,
        BuildErrorKind::BadBinary {
            op: "+",
            left: "f64".into(),
            right: "i32".into()
        }
    );
    assert_eq!(e.function.as_deref(), Some("m"));
    assert_eq!((e.location.file(), e.location.line()), (file!(), line));
    assert!(e.to_string().contains("fn `m`: `+` can't be applied to `f64` and `i32`"), "{e}");
}

#[test]
fn several_mistakes_are_all_reported_in_order() {
    let mut pb = ProgramBuilder::new();
    let h = pb.host("h", I32);
    let mut f = FnBuilder::new("m", F64);
    let v = f.local("v", Vec3);
    let i = f.local("i", I32);
    let mut lines = vec![];
    lines.push(next_line!());
    f.assign(v, vec3!(1.0, 2, 3.0)); // BadConstruct
    lines.push(next_line!());
    f.assign(i, v.x()); // AssignType: f64 into i32
    lines.push(next_line!());
    f.assign(i, v.field("w", F64).cast(I32)); // BadField (no such field), then the cast is fine
    lines.push(next_line!());
    f.assign(i, v.y().field("x", F64).cast(I32)); // BadField (not a struct)
    lines.push(next_line!());
    f.assign(i, pos3().x().cast(I32) + v.z().cast(I32).index(0)); // NotArray
    lines.push(next_line!());
    f.assign(i, array![1, 2.0].index(0)); // ArrayElement
    lines.push(next_line!());
    f.assign(i, array![1, 2].index(1.0)); // BadIndex
    lines.push(next_line!());
    f.assign(v, -(h.lt(1))); // BadUnary
    lines.push(next_line!());
    f.assign(v, v * 2.0); // BadBinary: no scalar broadcast
    lines.push(next_line!());
    f.if_then(i, |_| {}); // ConditionType
    lines.push(next_line!());
    f.assign(v, Expr::from(true).cast(F64)); // BadCast, and the f64 doesn't fit v
    lines.push(next_line!());
    f.ret(i); // ReturnType
    lines.push(next_line!());
    f.local("v", F64); // DuplicateLocal
    lines.push(next_line!());
    f.assign(i, pos3().field("x", F32).cast(I32)); // BadField (annotation)
    pb.finish_unit(f);

    let errs = errors(pb);
    let kinds: Vec<_> = errs.iter().map(|e| e.kind.clone()).collect();
    use BuildErrorKind as K;
    assert!(matches!(kinds[0], K::BadConstruct { index: 1, .. }), "{errs}");
    assert!(matches!(kinds[1], K::AssignType { .. }), "{errs}");
    assert!(
        matches!(&kinds[2], K::BadField { problem: FieldProblem::NoSuchField, .. }),
        "{errs}"
    );
    assert!(matches!(&kinds[3], K::BadField { problem: FieldProblem::NotAStruct, .. }), "{errs}");
    assert!(matches!(kinds[4], K::NotArray { .. }), "{errs}");
    assert!(matches!(kinds[5], K::ArrayElement { index: 1, .. }), "{errs}");
    assert!(matches!(kinds[6], K::BadIndex { .. }), "{errs}");
    assert!(matches!(kinds[7], K::BadUnary { op: "-", .. }), "{errs}");
    assert!(matches!(kinds[8], K::BadBinary { op: "*", .. }), "{errs}");
    assert!(matches!(kinds[9], K::ConditionType { .. }), "{errs}");
    assert!(matches!(kinds[10], K::BadCast { .. }), "{errs}");
    assert!(matches!(kinds[11], K::AssignType { .. }), "{errs}");
    assert!(matches!(kinds[12], K::ReturnType { .. }), "{errs}");
    assert!(matches!(kinds[13], K::DuplicateLocal { .. }), "{errs}");
    assert!(
        matches!(&kinds[14], K::BadField { problem: FieldProblem::Annotation { .. }, .. }),
        "{errs}"
    );
    assert_eq!(kinds.len(), 15, "{errs}");
    let got: Vec<u32> = errs.iter().map(|e| e.location.line()).collect();
    let mut want = lines.clone();
    want.insert(11, lines[10]); // the cast line has two errors
    assert_eq!(got, want);
}

#[test]
fn calls_check_arity_and_arguments() {
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("h", F64);
    let a = f.param("a", F64);
    let b = f.param("b", I32);
    f.ret(a + b.cast(F64));
    let h = pb.finish_helper(f);

    let mut f = FnBuilder::new("m", F64);
    // Wrong arity keeps the return type, so `+ 1.0` is fine.
    f.ret(h.call([1.0.into()]) + 1.0);
    let mut g = FnBuilder::new("m2", F64);
    g.ret(h.call([1.0.into(), 2.0.into()]));
    pb.finish_unit(f);
    pb.finish_unit(g);
    let errs = errors(pb);
    assert_eq!(errs.len(), 2, "{errs}");
    assert_eq!(errs.0[0].kind, BuildErrorKind::Arity { expected: 2, found: 1 });
    assert_eq!(
        errs.0[1].kind,
        BuildErrorKind::ArgumentType {
            index: 1,
            expected: "i32".into(),
            found: "f64".into()
        }
    );
}

#[test]
fn unit_with_params_and_bad_constant() {
    let mut pb = ProgramBuilder::new();
    let h = pb.host("h", F64);
    let c = pb.constant("c", h + 1.0);
    let mut f = FnBuilder::new("m", F64);
    let p = f.param("p", F64);
    f.ret(p + c); // c's type is unknown: no further error
    pb.finish_unit(f);
    let errs = errors(pb);
    assert_eq!(errs.len(), 2, "{errs}");
    assert_eq!(
        errs.0[0].kind,
        BuildErrorKind::NotConstant {
            name: "c".into(),
            found: "a host read"
        }
    );
    assert_eq!(errs.0[1].kind, BuildErrorKind::UnitHasParams { params: 1 });
}

#[test]
fn empty_array() {
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("m", F64);
    f.ret(array![].index(0));
    pb.finish_unit(f);
    let errs = errors(pb);
    assert_eq!(errs.len(), 1, "{errs}");
    assert_eq!(errs.0[0].kind, BuildErrorKind::EmptyArray);
}

fn foreign(errs: &BuildErrors) -> Vec<(&'static str, &'static str)> {
    errs.iter()
        .filter_map(|e| match e.kind {
            BuildErrorKind::ForeignHandle { what, owner } => Some((what, owner)),
            _ => None,
        })
        .collect()
}

#[test]
fn if_blocks_reports_a_block_from_another_function() {
    let mut pb = ProgramBuilder::new();
    let mut a = FnBuilder::new("a", F64);
    let y = a.local("y", F64);
    let block = a.block(|a| a.assign(y, 1.0));
    let mut b = FnBuilder::new("b", F64);
    let empty = b.block(|_| {});
    b.if_blocks(true, block, empty);
    b.ret(0.0);
    let u = pb.finish_unit(b);
    pb.entry(u, Grid::new((1, 1, 1)));
    let errs = pb.build().err().expect("a foreign block is an error");
    assert_eq!(foreign(&errs), [("a local", "function")]);
}

#[test]
fn local_from_another_function() {
    let mut pb = ProgramBuilder::new();
    let mut a = FnBuilder::new("a", F64);
    let y = a.local("y", F64);
    let mut b = FnBuilder::new("b", F64);
    let z = b.local("z", F64);
    b.assign(z, y + 1.0); // read
    b.assign(y, 1.0); // target
    let blk = a.block(|a| a.assign(y, 2.0));
    b.append(blk); // a block of another function
    b.ret(z);
    a.ret(y);
    pb.finish_unit(a);
    pb.finish_unit(b);
    let errs = errors(pb);
    assert_eq!(foreign(&errs), vec![("a local", "function"); 3], "{errs}");
    assert!(errs.iter().all(|e| e.function.as_deref() == Some("b")));
}

#[test]
fn handles_from_another_program_builder() {
    let mut other = ProgramBuilder::new();
    let mut f = FnBuilder::new("ou", F64);
    f.ret(1.0);
    let o_unit = other.finish_unit(f);
    let o_input = other.input(o_unit, Grid::new((1, 1, 1)));
    let o_host = other.host("h", F64);
    let o_table = other.auto_const("t", Handle("InterpolatedNoiseSampler"), "make", []);
    let o_const = other.constant("k", 1.0);
    let mut f = FnBuilder::new("oh", F64);
    f.ret(1.0);
    let o_helper = other.finish_helper(f);

    let mut pb = ProgramBuilder::new();
    pb.entry(o_unit, Grid::new((1, 1, 1)));
    pb.input(o_unit, Grid::new((1, 1, 1)));
    pb.constant("c", o_const * 2.0);
    let mut f = FnBuilder::new("m", F64);
    let s = f.local("s", Handle("InterpolatedNoiseSampler"));
    f.assign(s, o_table);
    f.ret(o_input.read() + o_host + o_const + o_helper.call([]));
    pb.finish_unit(f);
    let errs = errors(pb);
    let got = foreign(&errs);
    let want = [
        ("a unit", "program builder"),
        ("a unit", "program builder"),
        ("a constant", "program builder"),
        ("an autoconst", "program builder"),
        ("an input", "program builder"),
        ("a host", "program builder"),
        ("a constant", "program builder"),
        ("a helper", "program builder"),
    ];
    assert_eq!(got, want, "{errs}");
    assert_eq!(errs.len(), want.len(), "{errs}");
}

/// Statements written in `build!` are located at the `build!` call (the methods it expands to
/// are called from the macro), and operators inside them at their own line.
#[test]
fn locations_inside_build() {
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("m", F64);
    let x = f.local("x", F64);
    let call_line = line!() + 1;
    build!(f, {
        x = 1;
        x = x + 1;
        return x;
    });
    pb.finish_unit(f);
    let errs = errors(pb);
    let lines: Vec<u32> = errs.iter().map(|e| e.location.line()).collect();
    assert!(matches!(errs.0[0].kind, BuildErrorKind::AssignType { .. }), "{errs}");
    assert!(matches!(errs.0[1].kind, BuildErrorKind::BadBinary { .. }), "{errs}");
    assert_eq!(lines, vec![call_line, call_line + 2], "{errs}");
}
