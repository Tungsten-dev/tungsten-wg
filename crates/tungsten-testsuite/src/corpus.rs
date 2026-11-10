//! Discovery and loading of `corpus/**/*.tw`.

use std::path::{Path, PathBuf};

use tungsten_parse::{FileId, TestFile, load};

use crate::env::TestEnv;
use crate::selection::KNOWN_BACKENDS;

pub const VALID_ERROR_KINDS: &[&str] = &["DivByZero", "OutOfBounds", "OutOfFuel", "ExternDomain"];

pub fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus")
}

#[derive(Debug, Clone)]
pub struct CorpusFile {
    /// Relative to `corpus/`, without the extension, with `/` separators.
    pub name: String,
    pub path: PathBuf,
}

impl CorpusFile {
    /// The top-level directory (one backend batch).
    pub fn dir(&self) -> &str {
        self.name.split('/').next().unwrap_or("")
    }

    pub fn is_regression(&self) -> bool {
        self.dir() == "regressions"
    }
}

/// Every `.tw` file under `corpus/`, sorted by name.
pub fn discover() -> Vec<CorpusFile> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<CorpusFile>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, root, out);
            } else if p.extension().is_some_and(|e| e == "tw") {
                let rel = p.strip_prefix(root).unwrap().with_extension("");
                let name = rel.to_string_lossy().replace('\\', "/");
                out.push(CorpusFile { name, path: p });
            }
        }
    }
    let root = corpus_root();
    let mut out = vec![];
    walk(&root, &root, &mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A loaded file, or the reason it can't be used.
pub struct Loaded {
    pub file: CorpusFile,
    pub result: Result<TestFile, String>,
}

/// Parses `src` and validates the directives the parser leaves to the harness (crate README, "Rules and contracts").
pub fn load_source(src: &str, env: &TestEnv) -> Result<TestFile, String> {
    let tf = load(src, FileId(0), env).map_err(|d| d.to_string())?;
    validate(&tf)?;
    Ok(tf)
}

pub fn validate(tf: &TestFile) -> Result<(), String> {
    for kf in &tf.knownfails {
        if !KNOWN_BACKENDS.contains(&kf.backend.as_str()) {
            return Err(format!("@knownfail: unknown backend `{}`", kf.backend));
        }
        if let Some(c) = &kf.case
            && !tf.cases.iter().any(|k| &k.name == c)
        {
            return Err(format!("@knownfail: unknown case `{c}`"));
        }
        if let Some(s) = &kf.stage
            && s != "original"
            && s != "inlined"
        {
            return Err(format!("@knownfail: unknown stage `{s}`"));
        }
    }
    for e in &tf.expects {
        if let tungsten_parse::ExpectKind::Error(kind) = &e.kind
            && !VALID_ERROR_KINDS.contains(&kind.as_str())
        {
            return Err(format!(
                "@expect error({kind}): not a valid kind (valid: {})",
                VALID_ERROR_KINDS.join(", ")
            ));
        }
    }
    Ok(())
}

pub fn load_file(file: CorpusFile, env: &TestEnv) -> Loaded {
    let result = std::fs::read_to_string(&file.path)
        .map_err(|e| format!("{}: {e}", file.path.display()))
        .and_then(|src| load_source(&src, env));
    Loaded { file, result }
}

pub fn load_all(env: &TestEnv) -> Vec<Loaded> {
    discover().into_iter().map(|f| load_file(f, env)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
program {
    fn f() -> f64 { return 1.0; }
    unit u { main f; }
    entry u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
@case "c" origin(0, 0, 0) seed(1)
"#;

    #[test]
    fn bad_knownfail_backend() {
        let env = TestEnv::new();
        let src = format!("@knownfail(cobol, \"x\")\n{BASE}");
        assert!(load_source(&src, &env).unwrap_err().contains("unknown backend"));
        let src = format!("@knownfail(rust, \"x\", case=\"nope\")\n{BASE}");
        assert!(load_source(&src, &env).unwrap_err().contains("names no case"));
        let src = format!("@knownfail(rust, \"x\", case=\"c\")\n{BASE}");
        load_source(&src, &env).unwrap();
    }

    #[test]
    fn bad_error_kind() {
        let env = TestEnv::new();
        let src = format!("{BASE}@expect \"c\" error(Stuck)\n");
        assert!(load_source(&src, &env).is_err());
    }
}
