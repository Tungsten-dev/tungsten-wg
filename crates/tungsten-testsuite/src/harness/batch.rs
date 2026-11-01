//! Batches, the content-hash cache and the per-test fallback (SPEC §7.2).
//!
//! A batch is `(backend, top-level corpus dir, stage)`. The first trial that needs a batch
//! builds all of it; the others wait on the same `OnceLock`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::rust::{RUNTIME_FILES, RUSTC_FLAGS, generate_test, main_source, records_source, test_id};
use crate::corpus::{self, CorpusFile};
use crate::env::TestEnv;
use crate::oracle;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    Original,
    Inlined,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Stage::Original => "original",
            Stage::Inlined => "inlined",
        }
    }
}

/// What a test can run from.
#[derive(Debug, Clone)]
pub enum Artifact {
    /// The test id is the first argument of the binary.
    Binary { path: PathBuf, id: String },
    /// `generate` panicked or failed.
    GenFailed(String),
    /// The compiler rejected the test, even when built alone.
    BuildFailed { output: String, source: PathBuf },
    /// Nothing to run (every case is skipped, or the file isn't a backend test).
    NothingToRun,
}

type BatchResult = Arc<HashMap<String, Artifact>>;
type BatchCell = Arc<OnceLock<BatchResult>>;

pub struct Harness {
    pub cache_root: PathBuf,
    pub rustc: String,
    pub rustc_version: String,
    pub large: bool,
    /// Number of compiler invocations so far (the cache check of SPEC §12 reads it).
    pub builds: AtomicUsize,
    batches: Mutex<HashMap<(String, Stage), BatchCell>>,
}

/// The compiler command (`$RUSTC` or `rustc`).
pub fn rustc_command() -> String {
    std::env::var("RUSTC").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "rustc".into())
}

/// `rustc -vV`, or why there is no toolchain.
pub fn probe_rustc(rustc: &str) -> Result<String, String> {
    let out = Command::new(rustc).arg("-vV").output().map_err(|e| format!("`{rustc}`: {e}"))?;
    if !out.status.success() {
        return Err(format!("`{rustc} -vV` failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// One test's generated module.
pub struct TestSource {
    pub name: String,
    pub id: String,
    pub source: String,
}

fn key_of(version: &str, runtime: &[(&str, &str)], records: &str, tests: &[&TestSource]) -> String {
    let mut ctx = md5::Context::new();
    ctx.consume(version.as_bytes());
    ctx.consume(RUSTC_FLAGS.join(" ").as_bytes());
    for (name, text) in runtime {
        ctx.consume(name.as_bytes());
        ctx.consume(text.as_bytes());
    }
    ctx.consume(records.as_bytes());
    let mut sorted: Vec<_> = tests.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    for t in sorted {
        ctx.consume(t.id.as_bytes());
        ctx.consume(t.source.as_bytes());
    }
    format!("{:x}", ctx.finalize())
}

impl Harness {
    pub fn new(cache_root: PathBuf, large: bool) -> Result<Harness, String> {
        let rustc = rustc_command();
        let rustc_version = probe_rustc(&rustc)?;
        Ok(Harness {
            cache_root,
            rustc,
            rustc_version,
            large,
            builds: AtomicUsize::new(0),
            batches: Mutex::new(HashMap::new()),
        })
    }

    fn rust_dir(&self) -> PathBuf {
        self.cache_root.join("tw-cache").join("rust")
    }

    /// The artifact of corpus file `file` (name without extension) in its batch.
    pub fn artifact(&self, dir: &str, stage: Stage, file: &str) -> Artifact {
        let cell = {
            let mut m = self.batches.lock().unwrap();
            m.entry((dir.to_string(), stage)).or_default().clone()
        };
        let batch = cell.get_or_init(|| Arc::new(self.build_batch(dir, stage)));
        batch.get(file).cloned().unwrap_or(Artifact::NothingToRun)
    }

    /// Loads and generates every file of the directory, then builds.
    fn build_batch(&self, dir: &str, stage: Stage) -> HashMap<String, Artifact> {
        let mut result: HashMap<String, Artifact> = HashMap::new();
        let mut tests: Vec<TestSource> = vec![];
        for file in corpus::discover().into_iter().filter(|f| f.dir() == dir) {
            match self.generate_file(&file, stage) {
                Ok(Some(src)) => tests.push(src),
                Ok(None) => {
                    result.insert(file.name.clone(), Artifact::NothingToRun);
                }
                Err(msg) => {
                    result.insert(file.name.clone(), Artifact::GenFailed(msg));
                }
            }
        }
        if tests.is_empty() {
            return result;
        }
        let env = TestEnv::new();
        let records = records_source(env.spec());
        result.extend(self.build_tests(&records, &tests));
        result
    }

    /// Builds the tests as one crate; if that fails, builds each alone (in parallel, separately
    /// cached). The batch failure itself isn't reported, only the attributed ones (SPEC §7.2).
    pub fn build_tests(&self, records: &str, tests: &[TestSource]) -> HashMap<String, Artifact> {
        let refs: Vec<&TestSource> = tests.iter().collect();
        match self.build(records, &refs) {
            Ok(bin) => tests
                .iter()
                .map(|t| (t.name.clone(), Artifact::Binary { path: bin.clone(), id: t.id.clone() }))
                .collect(),
            Err(_) => std::thread::scope(|s| {
                let handles: Vec<_> = tests
                    .iter()
                    .map(|t| {
                        s.spawn(move || {
                            let a = match self.build(records, &[t]) {
                                Ok(bin) => Artifact::Binary { path: bin, id: t.id.clone() },
                                Err((output, source)) => Artifact::BuildFailed { output, source },
                            };
                            (t.name.clone(), a)
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            }),
        }
    }

    /// `Ok(None)`: nothing for a backend to run in this file.
    fn generate_file(&self, file: &CorpusFile, stage: Stage) -> Result<Option<TestSource>, String> {
        let env = TestEnv::new();
        let Ok(tf) = corpus::load_file(file.clone(), &env).result else { return Ok(None) };
        if tf.reject.is_some() || (tf.tier.as_deref() == Some("large") && !self.large) {
            return Ok(None);
        }
        let Ok(run) = oracle::run_all(&tf, &env) else { return Ok(None) };
        if run.results.iter().all(oracle::skipped_for_backends) {
            return Ok(None);
        }
        let mut program = tf.program.clone();
        if stage == Stage::Inlined {
            tungsten_optimizer::perform_inline(&mut program);
        }
        let source = generate_test(&program, &env)?;
        Ok(Some(TestSource { name: file.name.clone(), id: test_id(&file.name), source }))
    }

    /// Builds one crate holding `tests` (cached by content). `Err` carries the compiler output
    /// and the directory with the sources.
    pub fn build(&self, records: &str, tests: &[&TestSource]) -> Result<PathBuf, (String, PathBuf)> {
        let key = key_of(&self.rustc_version, RUNTIME_FILES, records, tests);
        let root = self.rust_dir();
        let final_dir = root.join(&key);
        let bin = final_dir.join("bin");
        if bin.exists() {
            return Ok(bin);
        }
        let n = self.builds.fetch_add(1, Ordering::SeqCst);
        let tmp = root.join(format!("{key}.tmp-{}-{n}", std::process::id()));
        let io = |e: std::io::Error| (format!("io error: {e}"), tmp.clone());
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(io)?;
        for (name, text) in RUNTIME_FILES {
            std::fs::write(tmp.join(name), text).map_err(io)?;
        }
        std::fs::write(tmp.join("records.rs"), records).map_err(io)?;
        let ids: Vec<String> = tests.iter().map(|t| t.id.clone()).collect();
        std::fs::write(tmp.join("main.rs"), main_source(&ids)).map_err(io)?;
        for t in tests {
            std::fs::write(tmp.join(format!("{}.rs", t.id)), &t.source).map_err(io)?;
        }
        let out = Command::new(&self.rustc)
            .args(RUSTC_FLAGS)
            .args(["--crate-name", "tw_batch", "--crate-type", "bin", "-o"])
            .arg(tmp.join("bin"))
            .arg(tmp.join("main.rs"))
            .output()
            .map_err(|e| (format!("could not run {}: {e}", self.rustc), tmp.clone()))?;
        if !out.status.success() {
            let text = String::from_utf8_lossy(&out.stderr).into_owned();
            let failed = root.join(format!("{key}.failed"));
            let _ = std::fs::remove_dir_all(&failed);
            let kept = if std::fs::rename(&tmp, &failed).is_ok() { failed } else { tmp };
            return Err((text, kept));
        }
        match std::fs::rename(&tmp, &final_dir) {
            Ok(()) => Ok(bin),
            // Another test binary got there first; its artifact is identical.
            Err(_) if bin.exists() => {
                let _ = std::fs::remove_dir_all(&tmp);
                Ok(bin)
            }
            Err(e) => Err((format!("could not move the build into the cache: {e}"), tmp)),
        }
    }
}

/// Cache key of a set of tests, for the invalidation check of SPEC §12.
pub fn cache_key(
    version: &str,
    runtime: &[(&str, &str)],
    records: &str,
    sources: &BTreeMap<String, String>,
) -> String {
    let tests: Vec<TestSource> = sources
        .iter()
        .map(|(id, s)| TestSource { name: id.clone(), id: id.clone(), source: s.clone() })
        .collect();
    let refs: Vec<&TestSource> = tests.iter().collect();
    key_of(version, runtime, records, &refs)
}
