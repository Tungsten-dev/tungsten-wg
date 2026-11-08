//! The dependency rules of `specs/compiler.md` §2: backends never run passes, and the optimizer and
//! the checker never depend on `tungsten-interface` (that edge would close a cycle). The repo has no
//! CI, so this runs with `cargo test`.

use std::fs;
use std::path::{Path, PathBuf};

fn crates() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("crates")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn backends_never_run_passes() {
    for backend in ["tungsten-rust", "tungsten-java", "tungsten-cuda", "tungsten-inspect"] {
        let mut files = vec![];
        rust_files(&crates().join(backend).join("src"), &mut files);
        for file in files {
            if file.file_name().is_some_and(|f| f == "tests.rs") {
                continue;
            }
            let text = fs::read_to_string(&file).unwrap();
            for (n, line) in text.lines().enumerate() {
                // `tungsten-inspect` reads the optimizer's analyses (`explain_inline`, `InlineStats`,
                // ...) for its views, so only the other backends may not mention it at all.
                let forbidden = if backend == "tungsten-inspect" {
                    line.contains("perform_inline")
                } else {
                    line.contains("tungsten_optimizer")
                };
                assert!(
                    !forbidden,
                    "{}:{}: a backend must not run passes: {line}",
                    file.display(),
                    n + 1
                );
            }
        }
    }
}

#[test]
fn optimizer_and_checker_do_not_depend_on_interface() {
    for krate in ["tungsten-optimizer", "tungsten-check", "tungsten-core", "tungsten-ir"] {
        let manifest = fs::read_to_string(crates().join(krate).join("Cargo.toml")).unwrap();
        assert!(
            !manifest.contains("tungsten-interface"),
            "{krate} must not depend on tungsten-interface"
        );
    }
}
