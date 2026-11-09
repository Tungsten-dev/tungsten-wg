//! Batches, the content-hash cache and the per-test fallback (SPEC §7.2).
//!
//! A batch is `(backend, top-level corpus dir, stage)`. The first trial that needs a batch
//! builds all of it; the others wait on the same `OnceLock`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::{cuda, java};
use super::rust::{RUNTIME_FILES, RUSTC_FLAGS, generate_test, main_source, records_source, test_id};
use crate::corpus::{self, CorpusFile};
use crate::env::TestEnv;
use crate::oracle;

/// The backend language of a harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    Java,
    Cuda,
}

impl Lang {
    /// The name used in trial names, `TW_BACKENDS` and `@knownfail`.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Java => "java",
            Lang::Cuda => "cuda",
        }
    }
}

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
    /// Rust: the binary, with the test id as its first argument. Java: the class directory, and
    /// the test's `Driver` class is `<id>.Driver`.
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
    pub lang: Lang,
    pub cache_root: PathBuf,
    /// `rustc` or `javac`.
    pub compiler: String,
    pub compiler_version: String,
    /// The `java` launcher (Java harnesses only).
    pub java: String,
    /// The `-arch=sm_XY` flag (CUDA harnesses only).
    pub arch: String,
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

/// `$JAVAC` or `javac`.
pub fn javac_command() -> String {
    std::env::var("JAVAC").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "javac".into())
}

/// `$NVCC` or `nvcc`, falling back to `/usr/local/cuda/bin/nvcc` when `nvcc` is not on the `PATH`.
pub fn nvcc_command() -> String {
    if let Some(v) = std::env::var("NVCC").ok().filter(|s| !s.is_empty()) {
        return v;
    }
    if Command::new("nvcc").arg("--version").output().is_err() && std::path::Path::new("/usr/local/cuda/bin/nvcc").exists() {
        return "/usr/local/cuda/bin/nvcc".into();
    }
    "nvcc".into()
}

/// Checks that `nvcc` works and `nvidia-smi -L` lists a GPU. Returns a description that includes the
/// compiler version and the architecture (`-arch=sm_XY` of GPU 0), and the `-arch` flag.
pub fn probe_cuda(nvcc: &str) -> Result<(String, String), String> {
    let out = Command::new(nvcc).arg("--version").output().map_err(|e| format!("`{nvcc}`: {e}"))?;
    if !out.status.success() {
        return Err(format!("`{nvcc} --version` failed"));
    }
    let version = String::from_utf8_lossy(&out.stdout).into_owned();
    let smi = Command::new("nvidia-smi").arg("-L").output().map_err(|e| format!("`nvidia-smi`: {e}"))?;
    let gpus = String::from_utf8_lossy(&smi.stdout).into_owned();
    if !smi.status.success() || !gpus.lines().any(|l| l.starts_with("GPU ")) {
        return Err("`nvidia-smi -L` lists no GPU".into());
    }
    let cap = Command::new("nvidia-smi")
        .args(["--query-gpu=compute_cap", "--format=csv,noheader"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().next().map(|l| l.trim().replace('.', "")))
        .filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_digit()));
    let arch = cap.map(|c| format!("-arch=sm_{c}")).unwrap_or_else(|| "-arch=native".into());
    Ok((format!("{}\n{}\n{arch}", version.trim(), gpus.trim()), arch))
}

/// `$JAVA` or `java`.
pub fn java_command() -> String {
    std::env::var("JAVA").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "java".into())
}

/// The major version in `javac -version` / `java -version` output (`javac 21.0.1`, `openjdk version "21.0.1"`).
fn major_version(text: &str) -> Option<u32> {
    let rest = text.split(|c: char| c.is_ascii_digit()).next().map(|p| &text[p.len()..])?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let major: u32 = digits.parse().ok()?;
    // Java 8 and older report `1.8.0`.
    if major == 1 {
        let minor: String = rest[2..].chars().take_while(|c| c.is_ascii_digit()).collect();
        return minor.parse().ok();
    }
    Some(major)
}

/// Checks that `javac` and `java` exist and are at least version 21; returns a description.
pub fn probe_java(javac: &str, java: &str) -> Result<String, String> {
    let mut desc = String::new();
    for (tool, text) in [(javac, "javac"), (java, "java")] {
        let out = Command::new(tool).arg("-version").output().map_err(|e| format!("`{tool}`: {e}"))?;
        if !out.status.success() {
            return Err(format!("`{tool} -version` failed"));
        }
        // `java -version` prints to stderr, `javac -version` to stdout.
        let mut v = String::from_utf8_lossy(&out.stdout).into_owned();
        v.push_str(&String::from_utf8_lossy(&out.stderr));
        match major_version(&v) {
            Some(m) if m >= 21 => {}
            _ => return Err(format!("{text} 21 or newer is required, `{tool} -version` says: {}", v.trim())),
        }
        desc.push_str(v.trim());
        desc.push('\n');
    }
    Ok(desc)
}

/// One test's generated module (Java: a blob of `//@@FILE` sections, see `java::split_files`).
pub struct TestSource {
    pub name: String,
    pub id: String,
    pub source: String,
}

fn key_of(
    version: &str,
    flags: &[&str],
    runtime: &[(&str, &str)],
    records: &str,
    tests: &[&TestSource],
) -> String {
    let mut ctx = crate::hash::Context::new();
    ctx.consume(version.as_bytes());
    ctx.consume(flags.join(" ").as_bytes());
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
    /// A Rust harness. `Err` if there is no `rustc`.
    pub fn new(cache_root: PathBuf, large: bool) -> Result<Harness, String> {
        let compiler = rustc_command();
        let compiler_version = probe_rustc(&compiler)?;
        Ok(Harness {
            lang: Lang::Rust,
            cache_root,
            compiler,
            compiler_version,
            java: String::new(),
            arch: String::new(),
            large,
            builds: AtomicUsize::new(0),
            batches: Mutex::new(HashMap::new()),
        })
    }

    /// A Java harness. `Err` if `javac` or `java` is missing or older than 21.
    pub fn new_java(cache_root: PathBuf, large: bool) -> Result<Harness, String> {
        let compiler = javac_command();
        let java = java_command();
        let compiler_version = probe_java(&compiler, &java)?;
        Ok(Harness {
            lang: Lang::Java,
            cache_root,
            compiler,
            compiler_version,
            java,
            arch: String::new(),
            large,
            builds: AtomicUsize::new(0),
            batches: Mutex::new(HashMap::new()),
        })
    }

    /// A CUDA harness. `Err` if `nvcc` is missing or no GPU is listed.
    pub fn new_cuda(cache_root: PathBuf, large: bool) -> Result<Harness, String> {
        let compiler = nvcc_command();
        let (compiler_version, arch) = probe_cuda(&compiler)?;
        Ok(Harness {
            lang: Lang::Cuda,
            cache_root,
            compiler,
            compiler_version,
            java: String::new(),
            arch,
            large,
            builds: AtomicUsize::new(0),
            batches: Mutex::new(HashMap::new()),
        })
    }

    /// Whether a `Failed` response ends the process, so the next case needs a new one (CUDA: a
    /// device error is sticky, SPEC §7.3).
    pub fn restart_after_failure(&self) -> bool {
        self.lang == Lang::Cuda
    }

    fn lang_dir(&self) -> PathBuf {
        self.cache_root.join("tw-cache").join(self.lang.name())
    }

    /// The command running test `id` of the artifact at `path`.
    pub fn command(&self, path: &std::path::Path, id: &str) -> Command {
        match self.lang {
            Lang::Rust | Lang::Cuda => {
                let mut c = Command::new(path);
                c.arg(id);
                c
            }
            Lang::Java => {
                let mut c = Command::new(&self.java);
                c.args(java::JAVA_FLAGS).arg("-cp").arg(path).arg(format!("{id}.Driver"));
                c
            }
        }
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
        let records = match self.lang {
            Lang::Rust => records_source(env.spec()),
            Lang::Java => java::records_source(env.spec()),
            Lang::Cuda => cuda::records_source(env.spec()),
        };
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
        if run.results.iter().all(|r| oracle::skipped_for_backend(r, self.lang.name())) {
            return Ok(None);
        }
        let mut program = tf.program.clone();
        if stage == Stage::Inlined {
            tungsten_optimizer::perform_inline(&mut program);
        }
        let id = test_id(&file.name);
        let source = match self.lang {
            Lang::Rust => generate_test(&program, &env)?,
            Lang::Java => java::generate_test(&program, &env, &id)?,
            Lang::Cuda => cuda::generate_test(&program, &env, &id)?,
        };
        Ok(Some(TestSource { name: file.name.clone(), id, source }))
    }

    /// Builds one batch holding `tests` (cached by content). `Err` carries the compiler output
    /// and the directory with the sources.
    pub fn build(&self, records: &str, tests: &[&TestSource]) -> Result<PathBuf, (String, PathBuf)> {
        let (flags, runtime) = match self.lang {
            Lang::Rust => (RUSTC_FLAGS, RUNTIME_FILES),
            Lang::Java => (java::JAVAC_FLAGS, java::RUNTIME_FILES),
            Lang::Cuda => (cuda::NVCC_FLAGS, cuda::RUNTIME_FILES),
        };
        let key = key_of(&self.compiler_version, flags, runtime, records, tests);
        let root = self.lang_dir();
        let final_dir = root.join(&key);
        let artifact = final_dir.join(match self.lang {
            Lang::Rust | Lang::Cuda => "bin",
            Lang::Java => "classes",
        });
        if artifact.exists() {
            return Ok(artifact);
        }
        let n = self.builds.fetch_add(1, Ordering::SeqCst);
        let tmp = root.join(format!("{key}.tmp-{}-{n}", std::process::id()));
        let io = |e: std::io::Error| (format!("io error: {e}"), tmp.clone());
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(io)?;
        let output = match self.lang {
            Lang::Rust => self.compile_rust(&tmp, records, tests),
            Lang::Java => self.compile_java(&tmp, records, tests),
            Lang::Cuda => self.compile_cuda(&tmp, records, tests),
        };
        let out = match output {
            Ok(o) => o,
            Err(e) => return Err((e, tmp.clone())),
        };
        if !out.status.success() {
            let mut text = String::from_utf8_lossy(&out.stderr).into_owned();
            if text.is_empty() {
                text = String::from_utf8_lossy(&out.stdout).into_owned();
            }
            let failed = root.join(format!("{key}.failed"));
            let _ = std::fs::remove_dir_all(&failed);
            let kept = if std::fs::rename(&tmp, &failed).is_ok() { failed } else { tmp };
            return Err((text, kept));
        }
        match std::fs::rename(&tmp, &final_dir) {
            Ok(()) => Ok(artifact),
            // Another test got there first; its artifact is identical.
            Err(_) if artifact.exists() => {
                let _ = std::fs::remove_dir_all(&tmp);
                Ok(artifact)
            }
            Err(e) => Err((format!("could not move the build into the cache: {e}"), tmp)),
        }
    }

    fn compile_rust(&self, tmp: &std::path::Path, records: &str, tests: &[&TestSource]) -> Result<std::process::Output, String> {
        let io = |e: std::io::Error| format!("io error: {e}");
        for (name, text) in RUNTIME_FILES {
            std::fs::write(tmp.join(name), text).map_err(io)?;
        }
        std::fs::write(tmp.join("records.rs"), records).map_err(io)?;
        let ids: Vec<String> = tests.iter().map(|t| t.id.clone()).collect();
        std::fs::write(tmp.join("main.rs"), main_source(&ids)).map_err(io)?;
        for t in tests {
            std::fs::write(tmp.join(format!("{}.rs", t.id)), &t.source).map_err(io)?;
        }
        Command::new(&self.compiler)
            .args(RUSTC_FLAGS)
            .args(["--crate-name", "tw_batch", "--crate-type", "bin", "-o"])
            .arg(tmp.join("bin"))
            .arg(tmp.join("main.rs"))
            .output()
            .map_err(|e| format!("could not run {}: {e}", self.compiler))
    }

    /// One `nvcc` run per test (parallel, objects cached by content), then one link.
    fn compile_cuda(&self, tmp: &std::path::Path, records: &str, tests: &[&TestSource]) -> Result<std::process::Output, String> {
        use std::os::unix::process::ExitStatusExt;
        let io = |e: std::io::Error| format!("io error: {e}");
        let inc = tmp.join("inc");
        let put = |rel: &str, text: &str| -> Result<(), String> {
            let path = inc.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).map_err(io)?;
            std::fs::write(&path, text).map_err(io)
        };
        for (name, text) in cuda::RUNTIME_FILES {
            put(name, text)?;
        }
        put("tw/records.cuh", records)?;
        let ids: Vec<String> = tests.iter().map(|t| t.id.clone()).collect();
        std::fs::write(tmp.join("main.cu"), cuda::main_source(&ids)).map_err(io)?;
        for t in tests {
            for (name, text) in java::split_files(&t.source) {
                put(&name, &text)?;
            }
        }
        let fail = |text: String| std::process::Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: vec![],
            stderr: text.into_bytes(),
        };
        let obj_dir = self.lang_dir().join("obj");
        std::fs::create_dir_all(&obj_dir).map_err(io)?;
        let flag_key = format!("{} {}", cuda::NVCC_FLAGS.join(" "), self.arch);
        let obj_key = |t: &TestSource| {
            let mut ctx = crate::hash::Context::new();
            ctx.consume(self.compiler_version.as_bytes());
            ctx.consume(flag_key.as_bytes());
            for (n, text) in cuda::RUNTIME_FILES {
                ctx.consume(n.as_bytes());
                ctx.consume(text.as_bytes());
            }
            ctx.consume(records.as_bytes());
            ctx.consume(t.source.as_bytes());
            format!("{:x}", ctx.finalize())
        };
        let nvcc = |args: &[&std::ffi::OsStr]| {
            self.builds.fetch_add(1, Ordering::SeqCst);
            Command::new(&self.compiler)
                .args(cuda::NVCC_FLAGS)
                .arg(&self.arch)
                .args(args)
                .output()
                .map_err(|e| format!("could not run {}: {e}", self.compiler))
        };
        let next = AtomicUsize::new(0);
        let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8).min(tests.len().max(1));
        let results: Mutex<Vec<(usize, Result<PathBuf, String>)>> = Mutex::new(vec![]);
        std::thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        let Some(t) = tests.get(i) else { return };
                        let key = obj_key(t);
                        let obj = obj_dir.join(format!("{key}.o"));
                        let r = if obj.exists() {
                            Ok(obj)
                        } else {
                            let part = obj_dir.join(format!("{key}.tmp-{}-{i}.o", std::process::id()));
                            let src = inc.join(format!("{}.cu", t.id));
                            let inc_flag = format!("-I{}", inc.display());
                            let out = nvcc(&["-c".as_ref(), inc_flag.as_ref(), src.as_os_str(), "-o".as_ref(), part.as_os_str()]);
                            match out {
                                Ok(o) if o.status.success() => match std::fs::rename(&part, &obj) {
                                    Ok(()) => Ok(obj),
                                    Err(_) if obj.exists() => Ok(obj),
                                    Err(e) => Err(format!("could not cache the object of {}: {e}", t.id)),
                                },
                                Ok(o) => Err(format!(
                                    "{}:\n{}{}",
                                    t.id,
                                    String::from_utf8_lossy(&o.stderr),
                                    String::from_utf8_lossy(&o.stdout)
                                )),
                                Err(e) => Err(e),
                            }
                        };
                        results.lock().unwrap().push((i, r));
                    }
                });
            }
        });
        let mut results = results.into_inner().unwrap();
        results.sort_by_key(|(i, _)| *i);
        let errors: Vec<String> = results.iter().filter_map(|(_, r)| r.as_ref().err().cloned()).collect();
        if !errors.is_empty() {
            return Ok(fail(errors.join("\n")));
        }
        let main_obj = tmp.join("main.o");
        let o = nvcc(&["-c".as_ref(), tmp.join("main.cu").as_os_str(), "-o".as_ref(), main_obj.as_os_str()])?;
        if !o.status.success() {
            return Ok(o);
        }
        let mut link: Vec<std::ffi::OsString> = vec![main_obj.into_os_string()];
        link.extend(results.into_iter().map(|(_, r)| r.unwrap().into_os_string()));
        link.push("-o".into());
        link.push(tmp.join("bin").into_os_string());
        let refs: Vec<&std::ffi::OsStr> = link.iter().map(|s| s.as_os_str()).collect();
        nvcc(&refs)
    }

    fn compile_java(&self, tmp: &std::path::Path, records: &str, tests: &[&TestSource]) -> Result<std::process::Output, String> {
        let io = |e: std::io::Error| format!("io error: {e}");
        let src = tmp.join("src");
        let mut files: Vec<String> = vec![];
        let mut put = |rel: String, text: &str| -> Result<(), String> {
            let path = src.join(&rel);
            std::fs::create_dir_all(path.parent().unwrap()).map_err(io)?;
            std::fs::write(&path, text).map_err(io)?;
            files.push(format!("src/{rel}"));
            Ok(())
        };
        for (name, text) in java::RUNTIME_FILES {
            put(name.to_string(), text)?;
        }
        for (name, text) in java::split_files(records) {
            put(name, &text)?;
        }
        for t in tests {
            for (name, text) in java::split_files(&t.source) {
                put(format!("{}/{name}", t.id), &text)?;
            }
        }
        std::fs::create_dir_all(tmp.join("classes")).map_err(io)?;
        std::fs::write(tmp.join("files.txt"), files.join("\n")).map_err(io)?;
        Command::new(&self.compiler)
            .current_dir(tmp)
            .args(java::JAVAC_FLAGS)
            .args(["-d", "classes", "@files.txt"])
            .output()
            .map_err(|e| format!("could not run {}: {e}", self.compiler))
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
    key_of(version, RUSTC_FLAGS, runtime, records, &refs)
}
