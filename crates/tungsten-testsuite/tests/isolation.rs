//! Isolation (specs/auto-consts.md §10): the library crates know nothing about Minecraft noise
//! tables. `perlin`, `base3d` and `md5` may appear in the test runtime (`tungsten-testsuite`),
//! in tests, and in docs, but not in library code or dependencies.

use std::path::{Path, PathBuf};

const FORBIDDEN: [&str; 3] = ["perlin", "base3d", "md5"];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if matches!(name.as_str(), "tests" | "target" | "generated" | "examples" | "benches") {
                continue;
            }
            rust_files(&path, out);
        // `rcl_new.rs` is a file a Java test writes. `random.rs` is a leftover of the
        // pre-autoconst seed code: no `mod` refers to it, and it still has to be deleted.
        } else if name.ends_with(".rs") && !matches!(name.as_str(), "tests.rs" | "rcl_new.rs" | "random.rs") {
            out.push(path);
        }
    }
}

/// The part of `text` before its first `#[cfg(test)]`.
fn library_part(text: &str) -> &str {
    text.find("#[cfg(test)]").map_or(text, |i| &text[..i])
}

#[test]
fn library_code_has_no_noise_table_knowledge() {
    let crates = workspace().join("crates");
    let mut found = vec![];
    for entry in std::fs::read_dir(&crates).unwrap() {
        let dir = entry.unwrap().path();
        if dir.file_name().unwrap() == "tungsten-testsuite" || !dir.is_dir() {
            continue;
        }
        let mut files = vec![];
        rust_files(&dir, &mut files);
        files.push(dir.join("Cargo.toml"));
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            let text = if f.extension().is_some_and(|e| e == "rs") { library_part(&text) } else { &text };
            let lower = text.to_lowercase();
            for word in FORBIDDEN {
                if lower.contains(word) {
                    found.push(format!("{}: `{word}`", f.strip_prefix(workspace()).unwrap().display()));
                }
            }
        }
    }
    let manifest = std::fs::read_to_string(workspace().join("Cargo.toml")).unwrap();
    for word in FORBIDDEN {
        if manifest.to_lowercase().contains(word) {
            found.push(format!("Cargo.toml: `{word}`"));
        }
    }
    assert!(found.is_empty(), "noise table knowledge outside the test runtime:\n{}", found.join("\n"));
}
