//! CUDA driver generator and build inputs (crate README, "Batches").
//!
//! A batch is one binary. Per test there is a directory `t_<hash>/` with the four generated files
//! of `tungsten-cuda` (namespace `t_<hash>`) and a `t_<hash>.cu` that includes `density.cu`,
//! `orchestration.cu` and a generated driver, so one `nvcc` run per test builds one object. A
//! `main.cu` dispatches on the test id given as the first argument. The runtime is header-only
//! (`runtime/cuda/tw/`, plus a generated `records.cuh`). Files of one text blob are separated by
//! `//@@FILE <path>` lines (see [`super::java::split_files`]).

use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};

use tungsten_cuda::{CudaConfig, Profile};
use tungsten_interface::naming::entry_base_names;
use tungsten_interface::{Backend, Interface};
use tungsten_ir::VariableType;
use tungsten_ir::model::{HostInput, OptimizedProgram};

use super::protocol::TestIo;
use crate::env::TestEnv;
use crate::registry::RegistrySpec;

pub use super::rust::test_id;

const MARKER: &str = "//@@FILE ";

/// Runtime files: (path under the include root, contents). `records.cuh` is generated.
pub const RUNTIME_FILES: &[(&str, &str)] = &[
    ("tw/tungsten_runtime.cuh", include_str!("../../runtime/cuda/tw/tungsten_runtime.cuh")),
    ("tw/profile_exact.cuh", include_str!("../../runtime/cuda/tw/profile_exact.cuh")),
    ("tw/proto.h", include_str!("../../runtime/cuda/tw/proto.h")),
];

/// `Orchestration::required_flags` of the exact profile, split. Never `--use_fast_math`.
pub fn exact_flags() -> Vec<&'static str> {
    Profile::Exact.required_flags().split_whitespace().collect()
}

/// Flags of every `nvcc` compile (the architecture is added by the harness); part of the cache key.
pub const NVCC_FLAGS: &[&str] = &["-std=c++20", "-O3", "-w", "--fmad=false", "--prec-div=true", "--prec-sqrt=true", "--ftz=false"];

fn cuda_type(t: &VariableType) -> String {
    match t {
        VariableType::Bool => "bool".into(),
        VariableType::I8 => "signed char".into(),
        VariableType::I32 => "int".into(),
        VariableType::I64 => "long long".into(),
        VariableType::F32 => "float".into(),
        VariableType::F64 => "double".into(),
        VariableType::Vec3 => "double3".into(),
        VariableType::Pos3 => "int3".into(),
        VariableType::Array(e, n) => format!("tw::Array<{}, {n}>", cuda_type(e)),
        VariableType::Extern(name) => name.to_string(),
        other => panic!("no CUDA type for {other:?}"),
    }
}

/// `tw/records.cuh`: one aggregate per registry record, with a `make` and in IR field order
/// (designated initializers in generated code check the order).
pub fn records_source(spec: &RegistrySpec) -> String {
    let mut s = String::from("#pragma once\n\nnamespace tw {\n\n");
    for r in &spec.records {
        writeln!(s, "struct {} {{", r.name).unwrap();
        for (f, t) in &r.fields {
            writeln!(s, "    {} {f};", cuda_type(t)).unwrap();
        }
        let params: Vec<String> = r.fields.iter().map(|(f, t)| format!("{} {f}", cuda_type(t))).collect();
        let names: Vec<&str> = r.fields.iter().map(|(f, _)| *f).collect();
        writeln!(
            s,
            "    static constexpr __host__ __device__ {} make({}) {{ return {}{{{}}}; }}\n}};\n",
            r.name,
            params.join(", "),
            r.name,
            names.join(", ")
        )
        .unwrap();
    }
    s.push_str("}  // namespace tw\n");
    s
}

struct Ids(usize);

impl Ids {
    fn next(&mut self) -> usize {
        self.0 += 1;
        self.0
    }
}

/// Statements decoding a value of type `t` from `r` into the lvalue `lhs`.
fn decode_into(out: &mut String, lhs: &str, t: &VariableType, ids: &mut Ids, spec: &RegistrySpec) {
    match t {
        VariableType::Bool => writeln!(out, "{lhs} = r.b();").unwrap(),
        VariableType::I8 => writeln!(out, "{lhs} = r.i8();").unwrap(),
        VariableType::I32 => writeln!(out, "{lhs} = r.i32();").unwrap(),
        VariableType::I64 => writeln!(out, "{lhs} = r.i64();").unwrap(),
        VariableType::F32 => writeln!(out, "{lhs} = r.f32();").unwrap(),
        VariableType::F64 => writeln!(out, "{lhs} = r.f64();").unwrap(),
        VariableType::Vec3 => writeln!(out, "{lhs}.x = r.f64(); {lhs}.y = r.f64(); {lhs}.z = r.f64();").unwrap(),
        VariableType::Pos3 => writeln!(out, "{lhs}.x = r.i32(); {lhs}.y = r.i32(); {lhs}.z = r.i32();").unwrap(),
        VariableType::Array(e, n) => {
            let i = ids.next();
            writeln!(out, "for (int i{i} = 0; i{i} < {n}; i{i}++) {{").unwrap();
            decode_into(out, &format!("{lhs}[i{i}]"), e, ids, spec);
            out.push_str("}\n");
        }
        VariableType::Extern(name) => {
            let rec = spec.records.iter().find(|r| r.name == *name).expect("record in registry");
            for (f, ft) in &rec.fields {
                decode_into(out, &format!("{lhs}.{f}"), ft, ids, spec);
            }
        }
        other => panic!("can't decode {other:?}"),
    }
}

/// Statements writing `expr` (a value of type `t`) to `w`.
fn encode_stmts(out: &mut String, expr: &str, t: &VariableType, ids: &mut Ids, spec: &RegistrySpec) {
    match t {
        VariableType::Bool => writeln!(out, "w.b({expr});").unwrap(),
        VariableType::I8 => writeln!(out, "w.i8({expr});").unwrap(),
        VariableType::I32 => writeln!(out, "w.i32({expr});").unwrap(),
        VariableType::I64 => writeln!(out, "w.i64({expr});").unwrap(),
        VariableType::F32 => writeln!(out, "w.f32({expr});").unwrap(),
        VariableType::F64 => writeln!(out, "w.f64({expr});").unwrap(),
        VariableType::Vec3 => writeln!(out, "w.f64(({expr}).x); w.f64(({expr}).y); w.f64(({expr}).z);").unwrap(),
        VariableType::Pos3 => writeln!(out, "w.i32(({expr}).x); w.i32(({expr}).y); w.i32(({expr}).z);").unwrap(),
        VariableType::Array(e, n) => {
            let i = ids.next();
            writeln!(out, "for (int i{i} = 0; i{i} < {n}; i{i}++) {{").unwrap();
            encode_stmts(out, &format!("({expr})[i{i}]"), e, ids, spec);
            out.push_str("}\n");
        }
        VariableType::Extern(name) => {
            let rec = spec.records.iter().find(|r| r.name == *name).expect("record in registry");
            for (f, ft) in &rec.fields {
                encode_stmts(out, &format!("({expr}).{f}"), ft, ids, spec);
            }
        }
        other => panic!("can't encode {other:?}"),
    }
}

/// The driver part of `t_<hash>.cu`, in the test's namespace, plus the `tw_serve_<id>` entry the
/// generated `main` calls.
pub fn driver_source(
    program: &OptimizedProgram,
    interface: &Interface,
    id: &str,
    entry_bases: &[String],
    io: &TestIo,
    env: &TestEnv,
) -> Result<String, String> {
    let spec = env.spec();
    let mut ids = Ids(0);
    let mut s = String::new();
    writeln!(s, "namespace {id} {{\n").unwrap();
    writeln!(s, "static constexpr size_t REQUEST_LEN = {};", 32 + io.host_bytes(env)?).unwrap();
    s.push_str("static bool fail(std::string& err, const char* what, Status st) {\n");
    s.push_str("    err = std::string(what) + \": \" + st.message();\n    return false;\n}\n\n");
    s.push_str("static bool run_case(tw::proto::Reader& r, tw::proto::Writer& w, std::string& err) {\n");
    s.push_str("    Vec3 origin; origin.x = r.f64(); origin.y = r.f64(); origin.z = r.f64();\n");
    s.push_str("    const long long seed = (long long)r.u64();\n    HostConsts host{};\n");
    for (r, h) in program.host_inputs.iter() {
        if let HostInput::Static { t, .. } = h {
            decode_into(&mut s, &format!("host.{}", interface.host_field(r)), t, &mut ids, spec);
        }
    }
    s.push_str("    std::unique_ptr<Orchestration> orch;\n");
    s.push_str("    { Status st = Orchestration::create(seed, orch); if (!st.ok()) return fail(err, \"create\", st); }\n");
    for b in entry_bases {
        writeln!(s, "    {{ Status st = orch->enqueue_{b}(origin, host); if (!st.ok()) return fail(err, \"enqueue {b}\", st); }}").unwrap();
    }
    s.push_str("    { Status st = orch->sync(); if (!st.ok()) return fail(err, \"sync\", st); }\n");
    for (e, b) in io.entries.iter().zip(entry_bases) {
        if e.cells() == 1 {
            writeln!(s, "    {{ const auto& res = orch->result_{b}();").unwrap();
            encode_stmts(&mut s, "res", &e.ty, &mut ids, spec);
        } else {
            writeln!(s, "    {{ const auto* res = orch->result_{b}();").unwrap();
            writeln!(s, "    for (int c = 0; c < {}; c++) {{", e.cells()).unwrap();
            encode_stmts(&mut s, "res[c]", &e.ty, &mut ids, spec);
            s.push_str("    }\n");
        }
        s.push_str("    }\n");
    }
    s.push_str("    return true;\n}\n\n");
    writeln!(s, "}}  // namespace {id}\n").unwrap();
    writeln!(
        s,
        "int tw_serve_{id}() {{\n    cudaSetDevice(0);\n    return tw::proto::serve({id}::REQUEST_LEN, {id}::run_case);\n}}"
    )
    .unwrap();
    Ok(s)
}

/// The generated files of one test as a blob with paths relative to `<id>/`, and the test's
/// `<id>.cu` (path `../<id>.cu` is avoided: it is written next to the directory by the batch).
/// Backend panics become an `Err`.
pub fn generate_test(program: &OptimizedProgram, env: &TestEnv, id: &str) -> Result<String, String> {
    let io = TestIo::from_program(program)?;
    io.check_encodable()?;
    catch_unwind(AssertUnwindSafe(|| -> Result<String, String> {
        let config = CudaConfig { namespace: id.to_string(), ..CudaConfig::default() };
        let mut backend = config.to_backend();
        let interface = Interface::build(program, backend.naming_rules());
        backend.generate(program, &interface).map_err(|e| format!("generate failed: {e:?}"))?;
        let out = backend.output().ok_or("backend produced no output")?;
        let rules = backend.naming_rules();
        let mut used = rules.used_set();
        used.extend(["create", "sync", "query", "profile", "required_flags", "impl_"].map(String::from));
        let bases = entry_base_names(program, rules, used);
        let driver = driver_source(program, &interface, id, &bases, &io, env)?;
        let mut blob = String::new();
        for (name, text) in out.files() {
            writeln!(blob, "{MARKER}{id}/{name}\n{text}").unwrap();
        }
        let tu = format!(
            "#include \"tw/tungsten_runtime.cuh\"\n#include \"tw/proto.h\"\n#include \"{id}/orchestration.h\"\n\
             #include \"{id}/density.cu\"\n#include \"{id}/orchestration.cu\"\n\n{driver}"
        );
        writeln!(blob, "{MARKER}{id}.cu\n{tu}").unwrap();
        Ok(blob)
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

/// `main.cu`: dispatches on the test id (first argument).
pub fn main_source(ids: &[String]) -> String {
    let mut s = String::from("#include <cstring>\n#include <cstdio>\n\n");
    for id in ids {
        writeln!(s, "int tw_serve_{id}();").unwrap();
    }
    s.push_str("\nint main(int argc, char** argv) {\n    if (argc < 2) { std::fprintf(stderr, \"usage: bin <test id>\\n\"); return 2; }\n");
    for id in ids {
        writeln!(s, "    if (!std::strcmp(argv[1], \"{id}\")) return tw_serve_{id}();").unwrap();
    }
    s.push_str("    std::fprintf(stderr, \"unknown test id %s\\n\", argv[1]);\n    return 2;\n}\n");
    s
}
