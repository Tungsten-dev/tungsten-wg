//! Java driver generator and build inputs (crate README, "Rules and contracts, Batches").
//!
//! A batch is one source tree: the runtime from `runtime/java/`, one class per registry record in
//! `tungsten.runtime`, and per test a package `t_<hash>` holding the backend output (`Gen`) and a
//! generated `Driver` with the `main` that serves requests. Files of one text blob are separated
//! by `//@@FILE <path>` lines (see [`split_files`]).

use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};

use tungsten_interface::naming::{WorkerNames, java_sanitize};
use tungsten_interface::{Backend, BufferBounds, Interface};
use tungsten_ir::VariableType;
use tungsten_ir::model::{HostInput, Program};
use tungsten_java::JavaConfig;
use tungsten_java::orchestration::java_worker_names;

use super::protocol::TestIo;
use crate::env::TestEnv;
use crate::registry::RegistrySpec;

pub use super::rust::test_id;

/// Runtime files: (path under the source root, contents).
pub const RUNTIME_FILES: &[(&str, &str)] = &[
    ("tungsten/runtime/Vec3.java", include_str!("../../runtime/java/tungsten/runtime/Vec3.java")),
    ("tungsten/runtime/Pos3.java", include_str!("../../runtime/java/tungsten/runtime/Pos3.java")),
    ("tungsten/runtime/Config.java", include_str!("../../runtime/java/tungsten/runtime/Config.java")),
    (
        "tungsten/runtime/PositionIterator.java",
        include_str!("../../runtime/java/tungsten/runtime/PositionIterator.java"),
    ),
    ("tungsten/runtime/BoolRef.java", include_str!("../../runtime/java/tungsten/runtime/BoolRef.java")),
    ("tungsten/runtime/ByteRef.java", include_str!("../../runtime/java/tungsten/runtime/ByteRef.java")),
    ("tungsten/runtime/IntRef.java", include_str!("../../runtime/java/tungsten/runtime/IntRef.java")),
    ("tungsten/runtime/LongRef.java", include_str!("../../runtime/java/tungsten/runtime/LongRef.java")),
    ("tungsten/runtime/FloatRef.java", include_str!("../../runtime/java/tungsten/runtime/FloatRef.java")),
    ("tungsten/runtime/DoubleRef.java", include_str!("../../runtime/java/tungsten/runtime/DoubleRef.java")),
    ("tungsten/runtime/Toy.java", include_str!("../../runtime/java/tungsten/runtime/Toy.java")),
    (
        "tungsten/runtime/PerlinNoiseSampler.java",
        include_str!("../../runtime/java/tungsten/runtime/PerlinNoiseSampler.java"),
    ),
    (
        "tungsten/runtime/InterpolatedNoiseSampler.java",
        include_str!("../../runtime/java/tungsten/runtime/InterpolatedNoiseSampler.java"),
    ),
    ("tungsten/runtime/Helpers.java", include_str!("../../runtime/java/tungsten/runtime/Helpers.java")),
    ("tungsten/runtime/Proto.java", include_str!("../../runtime/java/tungsten/runtime/Proto.java")),
];

/// Flags of every `javac` run; part of the cache key.
pub const JAVAC_FLAGS: &[&str] = &["--release", "21", "-nowarn", "-proc:none"];

/// Flags of every `java` run.
pub const JAVA_FLAGS: &[&str] = &["-XX:+UseSerialGC", "-Xshare:auto"];

const MARKER: &str = "//@@FILE ";

/// Splits a blob of `//@@FILE <path>` sections into (path, text).
pub fn split_files(blob: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for line in blob.split_inclusive('\n') {
        if let Some(path) = line.strip_prefix(MARKER) {
            out.push((path.trim().to_string(), String::new()));
        } else if let Some((_, text)) = out.last_mut() {
            text.push_str(line);
        }
    }
    out
}

fn java_type(t: &VariableType) -> String {
    match t {
        VariableType::Bool => "boolean".into(),
        VariableType::I8 => "byte".into(),
        VariableType::I32 => "int".into(),
        VariableType::I64 => "long".into(),
        VariableType::F32 => "float".into(),
        VariableType::F64 => "double".into(),
        VariableType::Vec3 => "Vec3".into(),
        VariableType::Pos3 => "Pos3".into(),
        VariableType::Array(e, _) => format!("{}[]", java_type(e)),
        VariableType::Extern(name) => java_sanitize(name),
        other => panic!("no Java type for {other:?}"),
    }
}

/// One public class per record, in `tungsten.runtime`.
pub fn records_source(spec: &RegistrySpec) -> String {
    let mut s = String::new();
    for r in &spec.records {
        let name = java_sanitize(&r.name);
        writeln!(s, "{MARKER}tungsten/runtime/{name}.java").unwrap();
        writeln!(s, "package tungsten.runtime;\n\npublic final class {name} {{").unwrap();
        for (f, t) in &r.fields {
            writeln!(s, "    public {} {f};", java_type(t)).unwrap();
        }
        let params: Vec<String> = r.fields.iter().map(|(f, t)| format!("{} {f}", java_type(t))).collect();
        writeln!(s, "\n    public {name}({}) {{", params.join(", ")).unwrap();
        for (f, _) in &r.fields {
            writeln!(s, "        this.{f} = {f};").unwrap();
        }
        s.push_str("    }\n}\n");
    }
    s
}

struct Ids(usize);

impl Ids {
    fn next(&mut self) -> usize {
        self.0 += 1;
        self.0
    }
}

/// Statements declaring `var` (of type `t`) from the reader `r`.
fn decode_stmts(out: &mut String, var: &str, t: &VariableType, ids: &mut Ids, spec: &RegistrySpec) {
    let jt = java_type(t);
    match t {
        VariableType::Bool => writeln!(out, "{jt} {var} = r.bool();").unwrap(),
        VariableType::I8 => writeln!(out, "{jt} {var} = r.i8();").unwrap(),
        VariableType::I32 => writeln!(out, "{jt} {var} = r.i32();").unwrap(),
        VariableType::I64 => writeln!(out, "{jt} {var} = r.i64();").unwrap(),
        VariableType::F32 => writeln!(out, "{jt} {var} = r.f32();").unwrap(),
        VariableType::F64 => writeln!(out, "{jt} {var} = r.f64();").unwrap(),
        VariableType::Vec3 => writeln!(out, "Vec3 {var} = new Vec3(r.f64(), r.f64(), r.f64());").unwrap(),
        VariableType::Pos3 => writeln!(out, "Pos3 {var} = new Pos3(r.i32(), r.i32(), r.i32());").unwrap(),
        VariableType::Array(e, n) => {
            let ejt = java_type(e);
            let (base, dims) = match ejt.find('[') {
                Some(i) => (&ejt[..i], &ejt[i..]),
                None => (ejt.as_str(), ""),
            };
            let i = ids.next();
            writeln!(out, "{jt} {var} = new {base}[{n}]{dims};").unwrap();
            writeln!(out, "for (int i{i} = 0; i{i} < {n}; i{i}++) {{").unwrap();
            let el = format!("e{i}");
            decode_stmts(out, &el, e, ids, spec);
            writeln!(out, "{var}[i{i}] = {el};\n}}").unwrap();
        }
        VariableType::Extern(name) => {
            let rec = spec.records.iter().find(|r| r.name == *name).expect("record in registry");
            let mut args = vec![];
            for (f, ft) in &rec.fields {
                let v = format!("f{}_{f}", ids.next());
                decode_stmts(out, &v, ft, ids, spec);
                args.push(v);
            }
            writeln!(out, "{jt} {var} = new {jt}({});", args.join(", ")).unwrap();
        }
        other => panic!("can't decode {other:?}"),
    }
}

/// Statements writing `expr` (a value of type `t`) to `w`.
fn encode_stmts(out: &mut String, expr: &str, t: &VariableType, ids: &mut Ids, spec: &RegistrySpec) {
    match t {
        VariableType::Bool => writeln!(out, "w.bool({expr});").unwrap(),
        VariableType::I8 => writeln!(out, "w.i8({expr});").unwrap(),
        VariableType::I32 => writeln!(out, "w.i32({expr});").unwrap(),
        VariableType::I64 => writeln!(out, "w.i64({expr});").unwrap(),
        VariableType::F32 => writeln!(out, "w.f32({expr});").unwrap(),
        VariableType::F64 => writeln!(out, "w.f64({expr});").unwrap(),
        VariableType::Vec3 => {
            let i = ids.next();
            writeln!(out, "{{ var t{i} = {expr}; w.f64(t{i}.x); w.f64(t{i}.y); w.f64(t{i}.z); }}").unwrap()
        }
        VariableType::Pos3 => {
            let i = ids.next();
            writeln!(out, "{{ var t{i} = {expr}; w.i32(t{i}.x); w.i32(t{i}.y); w.i32(t{i}.z); }}").unwrap()
        }
        VariableType::Array(e, _) => {
            let i = ids.next();
            writeln!(out, "{{ var a{i} = {expr};\nfor (int i{i} = 0; i{i} < a{i}.length; i{i}++) {{").unwrap();
            encode_stmts(out, &format!("a{i}[i{i}]"), e, ids, spec);
            out.push_str("}\n}\n");
        }
        VariableType::Extern(name) => {
            let rec = spec.records.iter().find(|r| r.name == *name).expect("record in registry");
            let i = ids.next();
            writeln!(out, "{{ var t{i} = {expr};").unwrap();
            for (f, ft) in &rec.fields {
                encode_stmts(out, &format!("t{i}.{f}"), ft, ids, spec);
            }
            out.push_str("}\n");
        }
        other => panic!("can't encode {other:?}"),
    }
}

/// `Driver.java` of one test: one `Orchestration` per case, one worker per entry (each given only
/// its own entry's hosts), and then the case's refine calls, each on the worker of its
/// refinement's entry.
pub fn driver_source(
    program: &Program,
    interface: &Interface,
    id: &str,
    workers: &[WorkerNames],
    io: &TestIo,
    env: &TestEnv,
) -> Result<String, String> {
    let spec = env.spec();
    let mut ids = Ids(0);
    let mut s = String::new();
    writeln!(s, "package {id};\n\nimport tungsten.runtime.*;\n").unwrap();
    s.push_str("public final class Driver {\n");
    // The fixed part ends with `steps:u32`, and each step is `refinement:u32 offset:[i32;3] host*`.
    writeln!(s, "    public static final int REQUEST_LEN = {};", 24 + io.param_bytes(env)? + io.host_bytes(env)? + 4).unwrap();
    writeln!(s, "    public static final int STEP_LEN = {};", io.step_bytes(env)?).unwrap();
    s.push_str("    static byte[] runCase(Proto.Reader r) {\n");
    s.push_str("        Vec3 origin = new Vec3(r.f64(), r.f64(), r.f64());\n");
    for (i, (_, t)) in io.params.iter().enumerate() {
        decode_stmts(&mut s, &format!("ap{i}"), t, &mut ids, spec);
    }
    // Hosts arrive in arena order; decode all of them, then bind each entry's own.
    let mut statics = vec![];
    for (r, h) in program.host_inputs.iter() {
        if let HostInput::Static { t, .. } = h {
            decode_stmts(&mut s, &format!("h{}", statics.len()), t, &mut ids, spec);
            statics.push(r);
        }
    }
    // `<Base>HostConsts hc` with the entry's own hosts bound from the locals `<prefix><k>`.
    let host_consts = |s: &mut String, e: usize, prefix: &str| {
        writeln!(s, "Gen.{0} hc = new Gen.{0}();", workers[e].host_consts).unwrap();
        for h in interface.entry_hosts(e) {
            let k = statics.iter().position(|x| x == h).expect("entry host is static");
            writeln!(s, "hc.{} = {prefix}{k};", interface.host_field(*h)).unwrap();
        }
    };
    let creation_args: Vec<String> = (0..io.params.len()).map(|i| format!("ap{i}")).collect();
    writeln!(s, "        Gen.Orchestration orch = Gen.Orchestration.create({});", creation_args.join(", ")).unwrap();
    s.push_str("        Proto.Writer w = new Proto.Writer();\n");
    // The workers outlive the run, since refine calls continue from their buffers.
    for (i, (e, names)) in io.entries.iter().zip(workers).enumerate() {
        writeln!(s, "        Gen.{} wk{i} = orch.{}();", names.worker, names.factory).unwrap();
        s.push_str("        {\n");
        host_consts(&mut s, i, "h");
        s.push_str("        var res = wk");
        writeln!(s, "{i}.run(origin.x, origin.y, origin.z, hc);").unwrap();
        encode_result(&mut s, e, &mut ids, spec);
        s.push_str("        }\n");
    }
    s.push_str("        int steps = r.u32();\n        for (int step = 0; step < steps; step++) {\n");
    s.push_str("            int refinement = r.u32();\n");
    s.push_str("            int ox = r.i32(), oy = r.i32(), oz = r.i32();\n");
    for (k, (_, t)) in io.hosts.iter().enumerate() {
        decode_stmts(&mut s, &format!("sh{k}"), t, &mut ids, spec);
    }
    s.push_str("            switch (refinement) {\n");
    for (k, refinement) in program.refinements.iter().enumerate() {
        let e = refinement.entry;
        writeln!(s, "                case {k}: {{").unwrap();
        host_consts(&mut s, e, "sh");
        writeln!(s, "                var res = wk{e}.{}(ox, oy, oz, hc);", interface.refine_method(k)).unwrap();
        encode_result(&mut s, &io.entries[e], &mut ids, spec);
        s.push_str("                break; }\n");
    }
    s.push_str("                default: throw new IllegalStateException(\"unknown refinement \" + refinement);\n            }\n        }\n");
    s.push_str("        return w.finish();\n    }\n\n");
    s.push_str("    public static void main(String[] args) {\n        Proto.serve(REQUEST_LEN, STEP_LEN, Driver::runCase);\n    }\n}\n");
    Ok(s)
}

/// Writes the value or array `res` of an entry to `w`. An array result is the worker's whole pool,
/// so only the entry's cells at `[0, cells)` count.
fn encode_result(s: &mut String, e: &super::protocol::EntryIo, ids: &mut Ids, spec: &RegistrySpec) {
    if e.cells() == 1 {
        encode_stmts(s, "res", &e.ty, ids, spec);
    } else {
        writeln!(s, "        for (int c = 0; c < {}; c++) {{", e.cells()).unwrap();
        encode_stmts(s, "res[c]", &e.ty, ids, spec);
        s.push_str("        }\n");
    }
}

/// The files of one test (`Gen.java`, `Driver.java`) as a blob with paths relative to the test's
/// package directory. Backend panics become an `Err`.
pub fn generate_test(program: &Program, env: &TestEnv, id: &str) -> Result<String, String> {
    generate_test_with(program, env, id, BufferBounds::PerBuffer)
}

/// [`generate_test`] with another bounds mode. Corpus tests compile with `BufferBounds::PerBuffer`,
/// so bad indices fail where the interpreter's do.
pub fn generate_test_with(
    program: &Program,
    env: &TestEnv,
    id: &str,
    bounds: BufferBounds,
) -> Result<String, String> {
    let io = TestIo::from_program(program)?;
    io.check_encodable()?;
    catch_unwind(AssertUnwindSafe(|| -> Result<String, String> {
        let config = JavaConfig {
            package: id.to_string(),
            class_name: "Gen".into(),
            runtime_package: "tungsten.runtime".into(),
            buffer_bounds: bounds,
        };
        let backend = config.to_backend();
        let rules = backend.naming_rules();
        let interface = Interface::build(program, &rules);
        let out = backend.generate(program, &interface).map_err(|e| format!("generate failed: {e:?}"))?;
        let names = java_worker_names(program, &rules);
        let driver = driver_source(program, &interface, id, &names, &io, env)?;
        Ok(format!("{MARKER}Gen.java\n{}\n{MARKER}Driver.java\n{}", out.source, driver))
    }))
    .unwrap_or_else(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".into());
        Err(format!("the backend panicked: {msg}"))
    })
}

