//! P5: `parse` and `load` never panic (or overflow the stack), whatever they are given.

mod common;

use common::Rng;
use proptest::prelude::*;
use proptest::sample::select;
use tungsten_parse::*;

const TOKENS: &[&str] = &[
    "program",
    "{",
    "}",
    "(",
    ")",
    "[",
    "]",
    ",",
    ";",
    ":",
    "::",
    ".",
    "->",
    "=",
    "==",
    "!=",
    "<",
    "<=",
    ">",
    ">=",
    "<<",
    ">>",
    ">>>",
    "+",
    "-",
    "*",
    "/",
    "&",
    "&&",
    "|",
    "||",
    "^",
    "!",
    "fn",
    "unit",
    "main",
    "input",
    "entry",
    "host",
    "table",
    "const",
    "use",
    "let",
    "vars",
    "return",
    "break",
    "if",
    "else",
    "while",
    "repeat",
    "true",
    "false",
    "as",
    "new",
    "extern",
    "ext",
    "mut",
    "dynamic",
    "none",
    "perlin",
    "base3d",
    "density",
    "ambient",
    "helpers",
    "tables",
    "hosts",
    "consts",
    "inf",
    "nan",
    "f64",
    "i32",
    "vec3",
    "pos3",
    "x",
    "y",
    "f",
    "u",
    "`q`",
    "x#1",
    "x##2",
    "%3",
    "1",
    "-1",
    "0x10",
    "0b1",
    "1.5",
    "1e3",
    "2L",
    "3b",
    "0.1f",
    "nan(0x7ff8000000000001)",
    "\"s\"",
    "@dims",
    "@hash",
    "@var",
    "@origin_scale",
    "@position_scale",
    "@case",
    "@expect",
    "@knownfail",
    "@fuel",
    "@cells",
    "@tier",
    "origin",
    "seed",
    "reject",
    "error",
    "//c\n",
];

fn exercise(src: &str) {
    let _ = parse(src, FileId(0));
    let _ = load(src, FileId(0), &NoEnv);
}

fn corpus() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect()
}

/// Bytes that make up most of the language, so random input gets past the lexer often.
const ALPHABET: &[u8] = b" \n`\"\\{}()[]#%@-.0123456789abcdexyz_:;=<>&|!";

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3000))]

    #[test]
    fn random_bytes(bytes in proptest::collection::vec(
        prop_oneof![any::<u8>(), select(ALPHABET)],
        0..200,
    )) {
        exercise(&String::from_utf8_lossy(&bytes));
    }

    #[test]
    fn random_token_streams(
        toks in proptest::collection::vec(select(TOKENS), 0..60),
        joined in any::<bool>(),
    ) {
        let sep = if joined { "" } else { " " };
        exercise(&toks.join(sep));
        // wrapped, so the item-level code is reached more often
        exercise(&format!("program {{ {} }}", toks.join(" ")));
    }
}

#[test]
fn mutated_corpus_files() {
    let mut rng = Rng::new(3);
    for src in corpus() {
        let chars: Vec<char> = src.chars().collect();
        for _ in 0..400 {
            let mut c = chars.clone();
            for _ in 0..1 + rng.below(4) {
                if c.is_empty() {
                    break;
                }
                let i = rng.below(c.len());
                match rng.below(5) {
                    0 => {
                        c.remove(i);
                    }
                    1 => c.insert(
                        i,
                        *rng.pick(&[
                            '{', '}', '(', ')', '`', '"', '-', '#', '%', '\u{e9}', ';', '0',
                        ]),
                    ),
                    2 => c[i] = char::from_u32(rng.next() as u32 % 0x300).unwrap_or('?'),
                    3 => c.truncate(i),
                    _ => {
                        let j = rng.below(c.len());
                        c.swap(i, j);
                    }
                }
            }
            exercise(&c.iter().collect::<String>());
        }
    }
}

#[test]
fn deep_nesting_is_an_error_not_a_stack_overflow() {
    let n = 100_000;
    let wrap = |inner: String| format!("program {{ const c = {inner}; }}");
    let cases = [
        wrap(format!("{}1{}", "(".repeat(n), ")".repeat(n))),
        wrap(format!("{}1", "-".repeat(n))),
        wrap(format!("{}1", "!".repeat(n))),
        wrap(format!("{}1{}", "[".repeat(n), "]".repeat(n))),
        wrap(format!("{}1{}", "f(".repeat(n), ")".repeat(n))),
        wrap(format!("a{}", "[0]".repeat(n))),
        wrap(format!("a{}", ".x".repeat(n))),
        wrap(format!("1{}", " + 1".repeat(n))),
        wrap(format!("1{}", " as i32".repeat(n))),
        wrap(format!("{}1", "&&".repeat(n))),
        wrap(format!("a{}", " == a".repeat(n))),
        wrap(format!("{}x{}", "new T(".repeat(n), ")".repeat(n))),
        wrap(format!("density({}1{})", "f(".repeat(n), ")".repeat(n))),
        format!(
            "program {{ fn f() -> f64 {{ {} {} }} }}",
            "if x {".repeat(n),
            "}".repeat(n)
        ),
        format!(
            "program {{ fn f() -> {}f64{} {{ }} }}",
            "[".repeat(n),
            "; 1]".repeat(n)
        ),
        format!("program {{ fn f() -> f64 {{ {}", "while x { ".repeat(n)),
        format!(
            "program {{ fn f() -> f64 {{ if x {{ }} {} }} }}",
            "else if x { }".repeat(n)
        ),
        format!(
            "@expect \"c\" e = {}1{}\nprogram {{ }}",
            "[".repeat(n),
            "]".repeat(n)
        ),
    ];
    for (i, src) in cases.iter().enumerate() {
        // any error will do; what matters is that we got here without a stack overflow
        let d = parse(src, FileId(0)).expect_err("too deep to be fine");
        if i < 10 {
            assert!(d.message.contains("deep"), "case {i}: {}", d.message);
        }
    }
}
