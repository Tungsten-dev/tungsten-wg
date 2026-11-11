//! Rust driver generator and build command (crate README, "Rules and contracts, Batches").
//!
//! A batch is one crate: the runtime files from `runtime/rust/`, a generated `records.rs`, one
//! module `t_<hash>` per test (the backend output plus a generated `driver`), and a `main.rs`
//! that dispatches on the test id given as the first argument.

use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};

use tungsten_interface::naming::{WorkerNames, sanitize_name};
use tungsten_interface::{Backend, BufferBounds, Interface};
use tungsten_ir::model::{HostInput, OptimizedProgram};
use tungsten_ir::VariableType;
use tungsten_rust::backend::{RustBackend, RustConfig};
use tungsten_rust::orchestration::rust_worker_names;

use super::protocol::TestIo;
use crate::env::TestEnv;
use crate::registry::RegistrySpec;

/// Runtime files of the batch crate: (file name in the crate, contents).
pub const RUNTIME_FILES: &[(&str, &str)] = &[
    ("mathf64.rs", include_str!("../../runtime/rust/mathf64.rs")),
    ("utilsf64.rs", include_str!("../../runtime/rust/utilsf64.rs")),
    ("externs.rs", include_str!("../../runtime/rust/externs.rs")),
    ("api.rs", include_str!("../../runtime/rust/api.rs")),
    ("proto.rs", include_str!("../../runtime/rust/proto.rs")),
];

/// Flags of every build; part of the cache key (crate README, "Rules and contracts, Batches").
pub const RUSTC_FLAGS: &[&str] = &[
    "--edition",
    "2024",
    "-C",
    "opt-level=3",
    "-C",
    "overflow-checks=on",
    "-C",
    "panic=unwind",
    "--cap-lints",
    "allow",
];

/// A valid module name for a test: `t_` and a hash of its corpus name.
pub fn test_id(corpus_name: &str) -> String {
    format!("t_{:x}", crate::hash::compute(corpus_name.as_bytes()))[..18].to_string()
}

fn rust_type(t: &VariableType) -> String {
    match t {
        VariableType::Bool => "bool".into(),
        VariableType::I8 => "i8".into(),
        VariableType::I32 => "i32".into(),
        VariableType::I64 => "i64".into(),
        VariableType::F32 => "f32".into(),
        VariableType::F64 => "f64".into(),
        VariableType::Vec3 => "Vec3".into(),
        VariableType::Pos3 => "Pos3".into(),
        VariableType::Array(e, n) => format!("[{}; {n}]", rust_type(e)),
        VariableType::Extern(name) => sanitize_name(name),
        other => panic!("no Rust type for {other:?}"),
    }
}

/// `records.rs`: one `#[repr(C)]` `Copy + Default` struct per record of the registry (the runtime
/// contract of the Rust backend).
pub fn records_source(spec: &RegistrySpec) -> String {
    let mut s = String::from("#![allow(dead_code)]\nuse crate::mathf64::{Pos3, Vec3};\n\n");
    for r in &spec.records {
        let name = sanitize_name(&r.name);
        writeln!(s, "#[repr(C)]\n#[derive(Clone, Copy, Debug, Default)]\npub struct {name} {{").unwrap();
        for (f, t) in &r.fields {
            writeln!(s, "    pub {f}: {},", rust_type(t)).unwrap();
        }
        s.push_str("}\n\n");
        let params: Vec<String> = r.fields.iter().map(|(f, t)| format!("{f}: {}", rust_type(t))).collect();
        let names: Vec<&str> = r.fields.iter().map(|(f, _)| *f).collect();
        writeln!(
            s,
            "impl {name} {{\n    pub fn new({}) -> Self {{\n        Self {{ {} }}\n    }}\n}}\n",
            params.join(", "),
            names.join(", ")
        )
        .unwrap();
    }
    s
}

fn decode_expr(t: &VariableType, spec: &RegistrySpec) -> String {
    match t {
        VariableType::Bool => "r.bool()".into(),
        VariableType::I8 => "r.i8()".into(),
        VariableType::I32 => "r.i32()".into(),
        VariableType::I64 => "r.i64()".into(),
        VariableType::F32 => "r.f32()".into(),
        VariableType::F64 => "r.f64()".into(),
        VariableType::Vec3 => "Vec3::new(r.f64(), r.f64(), r.f64())".into(),
        VariableType::Pos3 => "Pos3::new(r.i32(), r.i32(), r.i32())".into(),
        VariableType::Array(e, n) => {
            format!("std::array::from_fn::<_, {n}, _>(|_| {})", decode_expr(e, spec))
        }
        VariableType::Extern(name) => {
            let rec = spec.records.iter().find(|r| r.name == *name).expect("record in registry");
            let args: Vec<String> = rec.fields.iter().map(|(_, ft)| decode_expr(ft, spec)).collect();
            format!("{}::new({})", sanitize_name(name), args.join(", "))
        }
        other => panic!("can't decode {other:?}"),
    }
}

/// Statements writing `expr` (a `Copy` value of type `t`) to `w`.
fn encode_stmts(out: &mut String, expr: &str, t: &VariableType, depth: usize, spec: &RegistrySpec) {
    match t {
        VariableType::Bool => writeln!(out, "w.bool({expr});").unwrap(),
        VariableType::I8 => writeln!(out, "w.i8({expr});").unwrap(),
        VariableType::I32 => writeln!(out, "w.i32({expr});").unwrap(),
        VariableType::I64 => writeln!(out, "w.i64({expr});").unwrap(),
        VariableType::F32 => writeln!(out, "w.f32({expr});").unwrap(),
        VariableType::F64 => writeln!(out, "w.f64({expr});").unwrap(),
        VariableType::Vec3 => {
            writeln!(out, "{{ let t{depth} = {expr}; w.f64(t{depth}.x); w.f64(t{depth}.y); w.f64(t{depth}.z); }}")
                .unwrap()
        }
        VariableType::Pos3 => {
            writeln!(out, "{{ let t{depth} = {expr}; w.i32(t{depth}.x); w.i32(t{depth}.y); w.i32(t{depth}.z); }}")
                .unwrap()
        }
        VariableType::Array(e, _) => {
            writeln!(out, "for e{depth} in ({expr}).iter() {{ let v{depth} = *e{depth};").unwrap();
            encode_stmts(out, &format!("v{depth}"), e, depth + 1, spec);
            out.push_str("}\n");
        }
        VariableType::Extern(name) => {
            let rec = spec.records.iter().find(|r| r.name == *name).expect("record in registry");
            writeln!(out, "{{ let t{depth} = {expr};").unwrap();
            for (f, ft) in &rec.fields {
                encode_stmts(out, &format!("t{depth}.{f}"), ft, depth + 1, spec);
            }
            out.push_str("}\n");
        }
        other => panic!("can't encode {other:?}"),
    }
}

/// The `driver` module of one test: one `Orchestration` per case, one worker per entry (each given
/// only its own entry's hosts), and then the case's refine calls, each on the worker of its
/// refinement's entry.
pub fn driver_source(
    program: &OptimizedProgram,
    interface: &Interface,
    workers: &[WorkerNames],
    io: &TestIo,
    env: &TestEnv,
) -> Result<String, String> {
    let spec = env.spec();
    let mut s = String::new();
    s.push_str("pub mod driver {\n    use super::*;\n    use super::orchestration::*;\n");
    // The fixed part ends with `steps:u32`, and each step is `refinement:u32 offset:[i32;3] host*`.
    writeln!(s, "    pub const REQUEST_LEN: usize = {};", 24 + io.param_bytes(env)? + io.host_bytes(env)? + 4).unwrap();
    writeln!(s, "    pub const STEP_LEN: usize = {};", io.step_bytes(env)?).unwrap();
    s.push_str("    pub fn run_case(r: &mut crate::proto::Reader) -> Vec<u8> {\n");
    s.push_str("        let origin = Vec3::new(r.f64(), r.f64(), r.f64());\n");
    for (i, (_, t)) in io.params.iter().enumerate() {
        writeln!(s, "        let ap{i} = {};", decode_expr(t, spec)).unwrap();
    }
    // Hosts arrive in arena order; decode all of them, then bind each entry's own.
    let mut statics = vec![];
    for (r, h) in program.host_inputs.iter() {
        if let HostInput::Static { t, .. } = h {
            writeln!(s, "        let h{} = {};", statics.len(), decode_expr(t, spec)).unwrap();
            statics.push(r);
        }
    }
    let host_consts = |e: usize, prefix: &str| -> String {
        let fields: Vec<String> = interface
            .entry_hosts(e)
            .iter()
            .map(|h| {
                let k = statics.iter().position(|x| x == h).expect("entry host is static");
                format!("{}: {prefix}{k}", interface.host_field(*h))
            })
            .collect();
        format!("{} {{ {} }}", workers[e].host_consts, fields.join(", "))
    };
    let creation_args: Vec<String> = (0..io.params.len()).map(|i| format!("ap{i}")).collect();
    writeln!(s, "        let orch = Orchestration::new({});", creation_args.join(", ")).unwrap();
    s.push_str("        let mut w = crate::proto::Writer::new();\n");
    // The workers outlive the run, since refine calls continue from their buffers.
    for (i, (e, names)) in io.entries.iter().zip(workers).enumerate() {
        writeln!(s, "        let mut wk{i} = orch.{}();", names.factory).unwrap();
        writeln!(s, "        {{ let hc = {};", host_consts(i, "h")).unwrap();
        writeln!(s, "        let res = wk{i}.run(origin, &hc);").unwrap();
        encode_result(&mut s, e, spec);
        s.push_str("        }\n");
    }
    s.push_str("        let steps = r.u32();\n        for _ in 0..steps {\n");
    s.push_str("            let refinement = r.u32();\n");
    s.push_str("            let offset = Pos3::new(r.i32(), r.i32(), r.i32());\n");
    for (k, (_, t)) in io.hosts.iter().enumerate() {
        writeln!(s, "            let sh{k} = {};", decode_expr(t, spec)).unwrap();
    }
    s.push_str("            match refinement {\n");
    for (k, refinement) in program.refinements.iter().enumerate() {
        let e = refinement.entry;
        writeln!(s, "                {k} => {{ let hc = {};", host_consts(e, "sh")).unwrap();
        writeln!(s, "                let res = wk{e}.{}(offset, &hc);", interface.refine_method(k)).unwrap();
        encode_result(&mut s, &io.entries[e], spec);
        s.push_str("                }\n");
    }
    s.push_str("                _ => panic!(\"unknown refinement {}\", refinement),\n            }\n        }\n");
    s.push_str("        w.finish()\n    }\n}\n");
    Ok(s)
}

/// Writes the value or array `res` of an entry to `w`.
fn encode_result(s: &mut String, e: &super::protocol::EntryIo, spec: &RegistrySpec) {
    if e.cells() == 1 {
        encode_stmts(s, "res", &e.ty, 0, spec);
    } else {
        s.push_str("        for c in res.iter() { let v = *c;\n");
        encode_stmts(s, "v", &e.ty, 0, spec);
        s.push_str("        }\n");
    }
}

/// Module source of one test: runtime imports, both backend modules and the driver. Panics in
/// the backend (it panics on types it can't derive) become an `Err`. Corpus tests compile with
/// `BufferBounds::PerBuffer`, so bad indices fail where the interpreter's do.
pub fn generate_test(program: &OptimizedProgram, env: &TestEnv) -> Result<String, String> {
    generate_test_with(program, env, BufferBounds::PerBuffer)
}

/// [`generate_test`] with another bounds mode.
pub fn generate_test_with(
    program: &OptimizedProgram,
    env: &TestEnv,
    bounds: BufferBounds,
) -> Result<String, String> {
    let io = TestIo::from_program(program)?;
    io.check_encodable()?;
    catch_unwind(AssertUnwindSafe(|| -> Result<String, String> {
        let backend = RustBackend::new(RustConfig { buffer_bounds: bounds, ..RustConfig::default() });
        let rules = backend.naming_rules();
        let interface = Interface::build(program, &rules);
        let out = backend.generate(program, &interface).map_err(|e| format!("generate failed: {e:?}"))?;
        let names = rust_worker_names(program, &rules);
        let driver = driver_source(program, &interface, &names, &io, env)?;
        Ok(format!(
            "use crate::mathf64::*;\nuse crate::utilsf64::*;\n\n{}\n\n{}",
            out.full_source, driver
        ))
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

/// `main.rs` of a crate holding the tests `ids`.
pub fn main_source(ids: &[String]) -> String {
    let mut s = String::from(
        "#![allow(warnings)]\nmod mathf64;\nmod utilsf64;\nmod externs;\nmod api;\nmod records;\nmod proto;\n",
    );
    for id in ids {
        writeln!(s, "mod {id};").unwrap();
    }
    s.push_str("fn main() {\n    let id = std::env::args().nth(1).expect(\"test id\");\n");
    s.push_str("    let (len, step, run): (usize, usize, fn(&mut proto::Reader) -> Vec<u8>) = match id.as_str() {\n");
    for id in ids {
        writeln!(
            s,
            "        \"{id}\" => ({id}::driver::REQUEST_LEN, {id}::driver::STEP_LEN, {id}::driver::run_case),"
        )
        .unwrap();
    }
    s.push_str("        _ => { eprintln!(\"unknown test id {id}\"); std::process::exit(2) }\n    };\n");
    s.push_str("    proto::serve(len, step, run);\n}\n");
    s
}
