/*
Orchestration codegen for CUDA: `interface.cuh` (the structs, the per-worker layouts and the
kernel declarations the device code is generated against), the CUDA-free public header
`orchestration.h` (`Orchestration` and one worker class per entry), the host source
`orchestration.cu`, and assembly of all four files (`specs/workers.md` §3.3).

The host source is mostly fixed text with per-program loops, so it is written as raw lines.
*/

use std::collections::HashSet;

use tungsten_interface::naming::{FactoryStyle, NamingRules, WorkerNames, worker_names};
use tungsten_interface::{DeviceCodegen, Instance, Interface, OrchestrationCodegen, RefineRun};
use tungsten_ir::model::{AutoConstArg, ComputeUnit, HostInput, Program, Ref, VariableType};

use super::function::{Kernel, build_kernel_wrapper};
use super::launch::launch_shape;
use super::types::{auto_const_type, device_type, public_type, unit_return_type};
use super::{
    AUTO_CONSTS_STRUCT, CONFIG_TYPE, CTX_STRUCT, DEVICE_HOST_CONSTS_STRUCT, DEVICE_NAMESPACE,
};
use crate::backend::CudaConfig;
use crate::model as cuda;
use crate::model::codegen::{CudaCodeGenerator, double_literal, split_c_array, type_string};

/// The four generated files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CudaFiles {
    pub public_header: String,
    pub interface_header: String,
    pub density_source: String,
    pub orchestration_source: String,
}

/// Number of elements in the instance's buffer.
fn flat_len(inst: &Instance) -> usize {
    let (x, y, z) = inst.dimensions;
    (x as usize) * (y as usize) * (z as usize)
}

/// Type and member names the generated host code uses next to the entries' own.
const RESERVED_TYPES: [&str; 5] = ["Orchestration", "OrchestrationShared", "Status", "Vec3", "Pos3"];
const RESERVED_MEMBERS: [&str; 5] = ["create", "profile", "required_flags", "shared_", "impl_"];

/// Names of every entry's worker, in entry order: `<Base>Worker`, `<Base>HostConsts`,
/// `<Base>Layout` and `create_<base>_worker`. Frontends and drivers get them from here.
pub fn cuda_worker_names(program: &Program, rules: &NamingRules) -> Vec<WorkerNames> {
    worker_names(
        program,
        rules,
        FactoryStyle::CreateSnake,
        RESERVED_TYPES.iter().map(|s| s.to_string()).collect(),
        RESERVED_MEMBERS.iter().map(|s| s.to_string()).collect(),
    )
}

/// One entry point as seen by the host: its worker class.
struct Entry {
    index: usize,
    names: WorkerNames,
    inst: Instance,
    element: VariableType,
    /// Public element type, as spelled in the worker class (arrays go through `element`).
    public_element: String,
    /// `using element = T[N];` for array elements.
    alias: Option<String>,
    canonical_name: String,
    /// The refinements of this entry, as indices into `program.refinements`.
    refinements: Vec<usize>,
}

impl Entry {
    fn len(&self) -> usize {
        flat_len(&self.inst)
    }

    fn single(&self) -> bool {
        self.len() == 1
    }

    /// The element type as named outside the worker class.
    fn qualified_element(&self) -> String {
        match &self.alias {
            Some(_) => format!("{}::{}", self.names.worker, self.public_element),
            None => self.public_element.clone(),
        }
    }
}

fn entries(program: &Program, rules: &NamingRules) -> Vec<Entry> {
    let names = cuda_worker_names(program, rules);
    program
        .entry_points
        .iter()
        .zip(names)
        .enumerate()
        .map(|(index, (e, names))| {
            let inst = Instance::from_entry(e);
            let element = unit_return_type(program, inst.cu).clone();
            let public = public_type(&element).unwrap_or_else(|| {
                panic!("entry point `{}` returns {:?}, which has no public mirror type", names.base, element)
            });
            let (public_element, alias) = match &public {
                cuda::Type::CArray(..) => {
                    let using = format!("using element = {};", array_type_string(&public));
                    ("element".to_string(), Some(using))
                }
                t => (type_string(t), None),
            };
            let canonical_name = canonical_name(program, &inst);
            let refinements =
                program.refinements.iter().enumerate().filter(|(_, r)| r.entry == index).map(|(r, _)| r).collect();
            Entry { index, names, inst, element, public_element, alias, canonical_name, refinements }
        })
        .collect()
}

/// A C array type as a type-id, e.g. `double[3][2]`.
fn array_type_string(t: &cuda::Type) -> String {
    let (base, dims) = split_c_array(t);
    format!("{}{}", type_string(base), dims)
}

fn canonical_name(program: &Program, inst: &Instance) -> String {
    let ComputeUnit::DensityFunction { function, .. } = &program.compute_units[inst.cu];
    program.functions[*function].canonical_name.clone().unwrap_or_else(|| "density function".into())
}

/// Static host inputs with their interface field names, in arena order.
fn static_host_inputs<'p>(program: &'p Program, interface: &Interface) -> Vec<(String, &'p VariableType)> {
    program
        .host_inputs
        .iter()
        .map(|(r, input)| match input {
            HostInput::Static { t, .. } => (interface.host_field(r).to_string(), t),
            HostInput::Dynamic => panic!("dynamic host inputs are not supported by the CUDA backend"),
        })
        .collect()
}

/// The static hosts entry `entry`'s worker reads (`Interface::entry_hosts`), with their field names.
fn entry_host_inputs<'p>(
    program: &'p Program,
    interface: &Interface,
    entry: usize,
) -> Vec<(String, &'p VariableType)> {
    interface
        .entry_hosts(entry)
        .iter()
        .map(|&r: &Ref<HostInput>| match &program.host_inputs[r] {
            HostInput::Static { t, .. } => (interface.host_field(r).to_string(), t),
            HostInput::Dynamic => panic!("dynamic host inputs are not supported by the CUDA backend"),
        })
        .collect()
}

fn generated_comment(what: &str) -> cuda::Item {
    cuda::Item::Comment(format!(
        "Generated by tungsten-wg {}. Do not edit.\n{}",
        env!("CARGO_PKG_VERSION"),
        what
    ))
}

/// Every kernel that gets generated, in generation order: those of the scheduled units' instances,
/// then one per distinct refine run.
fn kernels(program: &Program, interface: &Interface) -> Vec<Kernel> {
    let mut out = Vec::new();
    let mut units = HashSet::new();
    for inst in interface.waves().iter().flatten() {
        if units.insert(inst.cu) {
            out.extend(interface.instances_of(inst.cu).iter().map(|i| Kernel::of_instance(interface, i)));
        }
    }
    let mut runs: HashSet<RefineRun> = HashSet::new();
    for r in 0..program.refinements.len() {
        for run in interface.refine_waves(r).iter().flatten() {
            if runs.insert(*run) {
                out.push(Kernel::of_run(interface, run));
            }
        }
    }
    out
}

/// `interface.cuh`: `DeviceHostConsts`, `AutoConsts`, one `<Base>Layout` per worker, `Ctx` and the
/// kernel declarations, in `<namespace>::dev`.
fn interface_header(program: &Program, interface: &Interface, config: &CudaConfig, entries: &[Entry]) -> cuda::File {
    let host_consts = cuda::Struct {
        doc: Some(format!(
            "Static host inputs of every entry, the device side of each `<Base>HostConsts` of `{}` (checked in orchestration.cu).",
            config.namespace
        )),
        name: DEVICE_HOST_CONSTS_STRUCT.into(),
        members: static_host_inputs(program, interface)
            .into_iter()
            .map(|(name, t)| cuda::Item::Field { t: device_type(t), name })
            .collect(),
    };

    let auto_consts = cuda::Struct {
        doc: Some("Values built once at creation. Built on the host, copied to the device once.".into()),
        name: AUTO_CONSTS_STRUCT.into(),
        members: program
            .auto_consts
            .iter()
            .map(|(r, a)| cuda::Item::Field { t: auto_const_type(&a.t), name: interface.auto_const_field(r).into() })
            .collect(),
    };

    // Offsets are computed by the C++ compiler: element sizes of runtime types are only known there.
    // Each worker has its own layout, the entry's output first (`specs/workers.md` §4.2).
    let offset = cuda::Type::UnsignedLongLong;
    let end_of = |(name, len, t): &(String, usize, String)| {
        cuda::Expression::call(
            "tw::align_up",
            vec![cuda::Expression::raw(format!("{} + {}ull * sizeof({})", name, len, t)), cuda::Expression::raw("256ull")],
        )
    };
    let mut layouts = Vec::new();
    for e in entries {
        let slots = &interface.worker(e.index).slots;
        let ordered = std::iter::once(&e.inst).chain(slots.iter().map(|(i, _)| i).filter(|i| **i != e.inst));
        let mut members = Vec::new();
        let mut previous: Option<(String, usize, String)> = None;
        for inst in ordered {
            let name = interface.buffer_field(inst).to_string();
            let element = type_string(&device_type(unit_return_type(program, inst.cu)));
            let init = match &previous {
                None => cuda::Expression::raw("0"),
                Some(p) => end_of(p),
            };
            members.push(cuda::Item::Variable { qualifiers: "static constexpr".into(), t: offset.clone(), name: name.clone(), init });
            previous = Some((name, flat_len(inst), element));
        }
        members.push(cuda::Item::Variable {
            qualifiers: "static constexpr".into(),
            t: offset.clone(),
            name: "total_bytes".into(),
            init: previous.as_ref().map(end_of).unwrap_or_else(|| cuda::Expression::raw("0")),
        });
        layouts.push(cuda::Item::Struct(cuda::Struct {
            doc: Some(format!("Byte offsets of every buffer in the slab of the `{}` (256-byte aligned).", e.names.worker)),
            name: e.names.layout.clone(),
            members,
        }));
        layouts.push(cuda::Item::Blank);
    }

    let restrict = |n: &str| cuda::Type::const_restrict_ptr(cuda::Type::named(n));
    let field = |t, n: &str| cuda::Item::Field { t, name: n.into() };
    let ctx = cuda::Struct {
        doc: Some("Uniform context of every density and helper function.".into()),
        name: CTX_STRUCT.into(),
        members: vec![
            field(cuda::Type::named(CONFIG_TYPE), "config"),
            field(cuda::Type::ptr(cuda::Type::Char), "buffers"),
            field(restrict(AUTO_CONSTS_STRUCT), "auto_constants"),
            field(restrict(DEVICE_HOST_CONSTS_STRUCT), "host_consts"),
        ],
    };

    let mut items = vec![
        cuda::Item::Struct(host_consts),
        cuda::Item::Blank,
        cuda::Item::Struct(auto_consts),
        cuda::Item::Blank,
    ];
    items.extend(layouts);
    items.push(cuda::Item::Struct(ctx));
    let kernels = kernels(program, interface);
    if !kernels.is_empty() {
        items.push(cuda::Item::Blank);
        items.push(cuda::Item::Comment("Kernels (extern \"C\": prefixed, unmangled symbols).".into()));
        for k in &kernels {
            items.push(cuda::Item::Function(build_kernel_wrapper(program, interface, config, k, false)));
        }
    }

    cuda::File {
        items: vec![
            generated_comment("Device-side interface between density.cu and orchestration.cu."),
            cuda::Item::Raw(format!("#pragma once\n\n#include \"{}\"", config.runtime_include)),
            cuda::Item::Blank,
            cuda::Item::Namespace { name: format!("{}::{}", config.namespace, DEVICE_NAMESPACE), items },
        ],
    }
}

/// `orchestration.h`: the CUDA-free public API.
fn public_header(program: &Program, interface: &Interface, config: &CudaConfig, entries: &[Entry]) -> cuda::File {
    let field = |t, n: &str| cuda::Item::Field { t, name: n.into() };
    let vec3 = cuda::Struct {
        doc: None,
        name: "Vec3".into(),
        members: ["x", "y", "z"].iter().map(|n| field(cuda::Type::Double, n)).collect(),
    };
    let pos3 = cuda::Struct {
        doc: None,
        name: "Pos3".into(),
        members: ["x", "y", "z"].iter().map(|n| field(cuda::Type::Int, n)).collect(),
    };
    let status = "\
/// Result of a call: a cudaError_t value, 0 on success.
struct Status {
    int code = 0;
    bool ok() const { return code == 0; }
    /// cudaGetErrorString of `code`.
    const char* message() const;
};";

    let mut items = vec![
        cuda::Item::Struct(vec3),
        cuda::Item::Struct(pos3),
        cuda::Item::Blank,
        cuda::Item::Raw(status.into()),
        cuda::Item::Blank,
        cuda::Item::Raw("class Orchestration;\nstruct OrchestrationShared;".into()),
        cuda::Item::Blank,
    ];

    for e in entries {
        let host_consts = cuda::Struct {
            doc: Some("The static hosts this entry reads, filled by the caller and copied on every enqueue.".into()),
            name: e.names.host_consts.clone(),
            members: entry_host_inputs(program, interface, e.index)
                .into_iter()
                .map(|(name, t)| {
                    let t = public_type(t).unwrap_or_else(|| {
                        panic!("host input `{}` has the type {:?}, which has no public mirror type", name, t)
                    });
                    cuda::Item::Field { t, name }
                })
                .collect(),
        };
        items.push(cuda::Item::Struct(host_consts));
        items.push(cuda::Item::Blank);

        let (dx, dy, dz) = e.inst.dimensions;
        let hc = &e.names.host_consts;
        let mut class = vec![
            format!("/// {} ({}x{}x{}).", e.canonical_name, dx, dy, dz),
            "/// Runs on its own CUDA stream and owns its buffers. Not thread-safe: use one per host thread.".into(),
            format!("class {} {{", e.names.worker),
            "public:".into(),
        ];
        if let Some(alias) = &e.alias {
            class.push(format!("    {}", alias));
        }
        class.push(format!("    static constexpr int len = {};", e.len()));
        class.push(format!("    static constexpr int dims[3] = {{{}, {}, {}}};", dx, dy, dz));
        class.push("    /// Queues the entry at `origin`; returns without waiting for the GPU.".into());
        class.push(format!("    Status enqueue(Vec3 origin, const {hc}& host_consts);"));
        for &r in &e.refinements {
            class.push(format!(
                "    /// Queues refinement `{}` at the origin of the last enqueue() plus `offset`. Call after enqueue().",
                program.refinements[r].name
            ));
            class.push(format!("    Status enqueue_{}(Pos3 offset, const {hc}& host_consts);", interface.refine_method(r)));
        }
        class.push("    /// Valid after sync() (or query() returning success), until the next enqueue*().".into());
        class.push(match (e.single(), e.alias.is_some()) {
            (true, false) => format!("    {} result() const;", e.public_element),
            (true, true) => format!("    const {}& result() const;", e.public_element),
            (false, _) => format!("    const {}* result() const;", e.public_element),
        });
        class.extend(
            [
                "    /// Waits for all queued work.",
                "    Status sync();",
                "    /// Non-blocking: success, cudaErrorNotReady (as `code`), or an error.",
                "    Status query();",
            ]
            .map(String::from),
        );
        class.push(format!("    ~{}();", e.names.worker));
        class.push(format!("    {0}(const {0}&) = delete;", e.names.worker));
        class.push(format!("    {0}& operator=(const {0}&) = delete;", e.names.worker));
        class.extend(
            [
                "".to_string(),
                "private:".into(),
                "    friend class Orchestration;".into(),
                "    struct Impl;".into(),
                "    std::unique_ptr<Impl> impl_;".into(),
                format!("    explicit {}(std::unique_ptr<Impl> impl);", e.names.worker),
                "};".into(),
            ],
        );
        items.push(cuda::Item::Raw(class.join("\n")));
        items.push(cuda::Item::Blank);
    }

    let profile = config.profile;
    let mut class = vec![
        "/// Immutable and thread-safe once created. Its job is to spawn workers.".to_string(),
        "class Orchestration : public std::enable_shared_from_this<Orchestration> {".into(),
        "public:".into(),
        "    /// Numeric profile the device code was generated for.".into(),
        format!("    static constexpr const char* profile = \"{}\";", profile.name()),
        "    /// nvcc flags density.cu must be compiled with (empty: any).".into(),
        format!("    static constexpr const char* required_flags = \"{}\";", profile.required_flags()),
        String::new(),
        "    /// Builds the autoconsts on the host and copies them to the current device once.".into(),
        "    /// On failure `out` is left null.".into(),
        format!("    static Status create({}std::shared_ptr<Orchestration>& out);", create_params(program, interface)),
    ];
    for e in entries {
        class.push(String::new());
        class.push(format!("    /// Creates a worker on the current device, which must be the orchestration's: otherwise"));
        class.push("    /// the status is cudaErrorInvalidDevice. Thread-safe.".into());
        class.push(format!(
            "    Status {}(std::unique_ptr<{}>& out) const;",
            e.names.factory, e.names.worker
        ));
    }
    class.extend(
        [
            "",
            "    ~Orchestration();",
            "    Orchestration(const Orchestration&) = delete;",
            "    Orchestration& operator=(const Orchestration&) = delete;",
            "",
            "private:",
            "    std::shared_ptr<const OrchestrationShared> shared_;",
            "    explicit Orchestration(std::shared_ptr<const OrchestrationShared> shared);",
            "};",
        ]
        .map(String::from),
    );
    items.push(cuda::Item::Raw(class.join("\n")));

    cuda::File {
        items: vec![
            generated_comment(
                "Public API of the generated orchestration. Needs no CUDA headers (C++17).\n\
                 Threading: an Orchestration is thread-safe and shared; a worker is NOT: use one per host thread.\n\
                 A CUDA sticky error is context-wide: after one, the Orchestration and all its workers are unusable.",
            ),
            cuda::Item::Raw("#pragma once\n\n#include <memory>".into()),
            cuda::Item::Blank,
            cuda::Item::Namespace { name: config.namespace.clone(), items },
        ],
    }
}

/// `(long long)0x...ULL`: a seed half, as the bit pattern of an `i64`.
fn hex_i64(v: i64) -> String {
    format!("(long long)0x{:016x}ULL", v as u64)
}

/// A C++ string literal holding `s` as UTF-8. Bytes outside printable ASCII are three-digit octal
/// escapes, which can't swallow the digit that follows them. `?` is escaped against trigraphs.
fn string_literal(s: &str) -> String {
    let mut out = String::from("\"");
    for &b in s.as_bytes() {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'?' => out.push_str("\\?"),
            b' '..=b'~' => out.push(b as char),
            b => out.push_str(&format!("\\{:03o}", b)),
        }
    }
    out.push('"');
    out
}

/// `long long seed, double salt, ` (with the trailing separator): the creation parameters.
fn create_params(program: &Program, interface: &Interface) -> String {
    program
        .auto_params
        .iter()
        .map(|(r, p)| {
            let t = public_type(&p.t).expect("auto params are scalars");
            format!("{} {}, ", type_string(&t), interface.auto_param(r))
        })
        .collect()
}

/// Statements of `create` that fill `auto_constants` on the host.
fn auto_const_assignments(program: &Program, interface: &Interface) -> Vec<String> {
    let mut out = Vec::new();
    for (r, a) in program.auto_consts.iter() {
        let args: Vec<String> = a
            .args
            .iter()
            .map(|arg| match arg {
                AutoConstArg::Param(p) => interface.auto_param(*p).to_string(),
                AutoConstArg::I64(v) => hex_i64(*v),
                AutoConstArg::F64(v) => double_literal(*v),
                AutoConstArg::Bool(v) => v.to_string(),
                AutoConstArg::Str(v) => string_literal(v),
            })
            .collect();
        let field = interface.auto_const_field(r);
        out.push(format!("// {}", a.name.chars().map(|c| if c.is_control() || c == '\\' || c == '?' { ' ' } else { c }).collect::<String>()));
        out.push(format!("auto_constants->{} = tw::{}({});", field, a.initializer, args.join(", ")));
    }
    out
}

fn indent(lines: &[String], depth: usize) -> String {
    let pad = "    ".repeat(depth);
    lines
        .iter()
        .map(|l| if l.is_empty() { String::new() } else { format!("{}{}", pad, l) })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The launches of one wave list: `kernel<<<..>>>(args)` and its error check. `origin` and
/// `offset` are the host expressions of the kernel's leading arguments.
fn launches(
    program: &Program,
    interface: &Interface,
    config: &CudaConfig,
    e: &Entry,
    waves: &[Vec<Kernel>],
) -> Vec<String> {
    let layout = format!("dev::{}", e.names.layout);
    let mut lines = Vec::new();
    for (w, wave) in waves.iter().enumerate() {
        lines.push(format!("    // wave {}", w));
        for k in wave {
            let ComputeUnit::DensityFunction { function, .. } = &program.compute_units[k.unit];
            let shape = launch_shape(k.target.dimensions, config.block_size);
            let mut args = vec!["o".to_string()];
            if k.refine {
                args.push("off".into());
            }
            args.push("s.slab".into());
            args.push(format!("{}::{}", layout, interface.buffer_field(&k.target)));
            for &d in interface.input_params(*function) {
                let input = Instance::from_input(&program.density_inputs[d]);
                args.push(format!("{}::{}", layout, interface.buffer_field(&input)));
            }
            args.push("s.shared->auto_constants".into());
            args.push("s.host_consts".into());
            lines.push(format!(
                "    dev::{}<<<{}, {}, 0, s.stream>>>({});",
                k.symbol(config),
                shape.blocks,
                shape.threads,
                args.join(", ")
            ));
            lines.push("    TUNGSTEN_CUDA_CHECK(cudaGetLastError());".into());
        }
    }
    lines
}

/// `orchestration.cu`: `OrchestrationShared`, one `Impl` and the methods of each worker, `create`,
/// the worker factories and the layout checks between the public and device structs.
fn orchestration_source(program: &Program, interface: &Interface, config: &CudaConfig, entries: &[Entry]) -> cuda::File {
    let ns = &config.namespace;
    let host_inputs = static_host_inputs(program, interface);

    let includes = format!(
        "#include \"{}\"\n#include \"{}\"\n\n#include <cuda_runtime.h>\n\n#include <cstddef>\n#include <cstring>\n#include <memory>\n#include <type_traits>\n#include <utility>",
        config.public_header_name, config.interface_header_name
    );
    let check_macro = format!(
        "// Every CUDA call goes through TUNGSTEN_CUDA_CHECK. By default a failure returns its Status;\n\
         // define the macro before this point (or -DTUNGSTEN_CUDA_CHECK_HEADER=\"my_check.h\") to throw or abort.\n\
         #ifdef TUNGSTEN_CUDA_CHECK_HEADER\n\
         #include TUNGSTEN_CUDA_CHECK_HEADER\n\
         #endif\n\
         #ifndef TUNGSTEN_CUDA_CHECK\n\
         #define TUNGSTEN_CUDA_CHECK(expr) \\\n    \
         do {{ cudaError_t tw_e_ = (expr); \\\n         \
         if (tw_e_ != cudaSuccess) return ::{}::Status{{(int)tw_e_}}; }} while (0)\n\
         #endif",
        ns
    );

    // Layout checks: the public mirrors must be byte-identical to the device types.
    let mut asserts = vec![
        "// The public mirror types must be layout-identical to the device types.".to_string(),
        "static_assert(sizeof(Vec3) == sizeof(double3) && alignof(Vec3) == alignof(double3), \"Vec3 must mirror double3\");".into(),
        "static_assert(sizeof(Pos3) == sizeof(int3) && alignof(Pos3) == alignof(int3), \"Pos3 must mirror int3\");".into(),
    ];
    for axis in ["x", "y", "z"] {
        asserts.push(format!(
            "static_assert(offsetof(Vec3, {a}) == offsetof(double3, {a}) && offsetof(Pos3, {a}) == offsetof(int3, {a}), \"vector field {a}\");",
            a = axis
        ));
    }
    asserts.push(format!(
        "static_assert(std::is_trivially_copyable<dev::{a}>::value, \"{a} is copied to the device as bytes\");",
        a = AUTO_CONSTS_STRUCT
    ));
    for e in entries {
        // A worker copies each field of its own host struct into the matching device field as bytes.
        for (name, _) in entry_host_inputs(program, interface, e.index) {
            asserts.push(format!(
                "static_assert(sizeof({h}::{n}) == sizeof(dev::{d}::{n}) && std::is_trivially_copyable<decltype({h}::{n})>::value, \"{h}::{n} must mirror dev::{d}::{n}\");",
                h = e.names.host_consts,
                n = name,
                d = DEVICE_HOST_CONSTS_STRUCT
            ));
        }
        let device = type_string(&device_type(&e.element));
        asserts.push(format!(
            "static_assert(sizeof({p}) == sizeof({d}) && alignof({p}) == alignof({d}), \"result of {b}\");",
            p = e.qualified_element(),
            d = device,
            b = e.names.base
        ));
    }

    let shared = [
        "/// The device autoconsts and the device they live on. Immutable after `create`, shared by every worker.".to_string(),
        "struct OrchestrationShared {".into(),
        "    int device = 0;".into(),
        format!("    dev::{}* auto_constants = nullptr;", AUTO_CONSTS_STRUCT),
        "    ~OrchestrationShared() {".into(),
        "        // Errors are ignored: this is a destructor.".into(),
        "        if (auto_constants) cudaFree(auto_constants);".into(),
        "    }".into(),
        "};".into(),
    ]
    .join("\n");

    let auto_consts = auto_const_assignments(program, interface);
    let mut create = vec![
        format!("Status Orchestration::create({}std::shared_ptr<Orchestration>& out) {{", create_params(program, interface)),
        "    out.reset();".into(),
        "    std::shared_ptr<OrchestrationShared> shared(new OrchestrationShared());".into(),
        "    TUNGSTEN_CUDA_CHECK(cudaGetDevice(&shared->device));".into(),
        String::new(),
        "    // Autoconsts are built on the host (on the heap: they can be large) and copied once.".into(),
        "    {".into(),
        format!("        std::unique_ptr<dev::{a}> auto_constants(new dev::{a}());", a = AUTO_CONSTS_STRUCT),
    ];
    for (r, _) in program.auto_params.iter() {
        create.push(format!("        (void){};", interface.auto_param(r)));
    }
    create.push(indent(&auto_consts, 2));
    create.extend([
        format!(
            "        TUNGSTEN_CUDA_CHECK(cudaMalloc(reinterpret_cast<void**>(&shared->auto_constants), sizeof(dev::{})));",
            AUTO_CONSTS_STRUCT
        ),
        format!(
            "        TUNGSTEN_CUDA_CHECK(cudaMemcpy(shared->auto_constants, auto_constants.get(), sizeof(dev::{}), cudaMemcpyHostToDevice));",
            AUTO_CONSTS_STRUCT
        ),
        "    }".into(),
        "    out.reset(new Orchestration(std::move(shared)));".into(),
        "    return Status{};".into(),
        "}".into(),
        String::new(),
        "Orchestration::Orchestration(std::shared_ptr<const OrchestrationShared> shared) : shared_(std::move(shared)) {}".into(),
        String::new(),
        "Orchestration::~Orchestration() = default;".into(),
    ]);

    let mut workers = Vec::new();
    for e in entries {
        let w = &e.names.worker;
        let layout = format!("dev::{}", e.names.layout);
        let hosts = entry_host_inputs(program, interface, e.index);
        let has_hosts = !hosts.is_empty();
        let refined = !e.refinements.is_empty();

        // Impl
        let mut impl_lines = vec![
            format!("/// Device and pinned host resources of one `{}`. Released in reverse order.", w),
            format!("struct {}::Impl {{", w),
            "    std::shared_ptr<const OrchestrationShared> shared;".into(),
            "    cudaStream_t stream = nullptr;".into(),
            "    char* slab = nullptr;".into(),
            format!("    dev::{}* host_consts = nullptr;", DEVICE_HOST_CONSTS_STRUCT),
            "    /// Pinned staging slots for host const uploads, each guarded by an event.".into(),
            format!("    dev::{}* staging[host_consts_ring] = {{}};", DEVICE_HOST_CONSTS_STRUCT),
            "    cudaEvent_t staging_events[host_consts_ring] = {};".into(),
            "    unsigned next_slot = 0;".into(),
        ];
        if refined {
            impl_lines.push("    /// The origin of the last enqueue(), which the refine calls continue from.".into());
            impl_lines.push("    double3 origin = {0.0, 0.0, 0.0};".into());
        }
        impl_lines.push(format!("    {}* result = nullptr; // pinned", e.qualified_element()));
        impl_lines.extend(
            [
                "",
                "    ~Impl() {",
                "        // Errors are ignored: this is a destructor.",
                "        if (stream) cudaStreamSynchronize(stream);",
                "        if (result) cudaFreeHost(result);",
                "        for (unsigned i = host_consts_ring; i-- > 0;) {",
                "            if (staging_events[i]) cudaEventDestroy(staging_events[i]);",
                "            if (staging[i]) cudaFreeHost(staging[i]);",
                "        }",
                "        if (host_consts) cudaFree(host_consts);",
                "        if (slab) cudaFree(slab);",
                "        if (stream) cudaStreamDestroy(stream);",
                "    }",
                "};",
            ]
            .map(String::from),
        );
        workers.push(impl_lines.join("\n"));

        // Factory
        let mut factory = vec![
            format!("Status Orchestration::{}(std::unique_ptr<{}>& out) const {{", e.names.factory, w),
            "    out.reset();".into(),
            "    int device = 0;".into(),
            "    TUNGSTEN_CUDA_CHECK(cudaGetDevice(&device));".into(),
            "    if (device != shared_->device) return Status{(int)cudaErrorInvalidDevice};".into(),
            format!("    std::unique_ptr<{}::Impl> impl(new {}::Impl());", w, w),
            "    impl->shared = shared_;".into(),
            "    TUNGSTEN_CUDA_CHECK(cudaStreamCreateWithFlags(&impl->stream, cudaStreamNonBlocking));".into(),
            String::new(),
            "    // One slab for every buffer of the worker, zeroed.".into(),
            format!("    if ({}::total_bytes > 0) {{", layout),
            format!("        TUNGSTEN_CUDA_CHECK(cudaMalloc(reinterpret_cast<void**>(&impl->slab), {}::total_bytes));", layout),
            format!("        TUNGSTEN_CUDA_CHECK(cudaMemsetAsync(impl->slab, 0, {}::total_bytes, impl->stream));", layout),
            "    }".into(),
            format!(
                "    TUNGSTEN_CUDA_CHECK(cudaMalloc(reinterpret_cast<void**>(&impl->host_consts), sizeof(dev::{})));",
                DEVICE_HOST_CONSTS_STRUCT
            ),
            format!(
                "    TUNGSTEN_CUDA_CHECK(cudaMemsetAsync(impl->host_consts, 0, sizeof(dev::{}), impl->stream));",
                DEVICE_HOST_CONSTS_STRUCT
            ),
        ];
        if has_hosts {
            factory.extend([
                "    for (unsigned i = 0; i < host_consts_ring; ++i) {".to_string(),
                format!(
                    "        TUNGSTEN_CUDA_CHECK(cudaHostAlloc(reinterpret_cast<void**>(&impl->staging[i]), sizeof(dev::{}), cudaHostAllocDefault));",
                    DEVICE_HOST_CONSTS_STRUCT
                ),
                format!("        std::memset(impl->staging[i], 0, sizeof(dev::{}));", DEVICE_HOST_CONSTS_STRUCT),
                "        TUNGSTEN_CUDA_CHECK(cudaEventCreateWithFlags(&impl->staging_events[i], cudaEventDisableTiming));".into(),
                "    }".into(),
            ]);
        }
        factory.extend([
            format!(
                "    TUNGSTEN_CUDA_CHECK(cudaHostAlloc(reinterpret_cast<void**>(&impl->result), {}::len * sizeof({}), cudaHostAllocDefault));",
                w,
                e.qualified_element()
            ),
            String::new(),
            "    TUNGSTEN_CUDA_CHECK(cudaStreamSynchronize(impl->stream));".into(),
            format!("    out.reset(new {}(std::move(impl)));", w),
            "    return Status{};".into(),
            "}".into(),
        ]);
        workers.push(factory.join("\n"));

        // Methods
        let mut methods = vec![
            format!("{0}::{0}(std::unique_ptr<Impl> impl) : impl_(std::move(impl)) {{}}", w),
            String::new(),
            format!("{0}::~{0}() = default;", w),
        ];
        // The shared head of `enqueue` and the refine calls: copy this entry's hosts and upload.
        let upload = |m: &mut Vec<String>| {
            if has_hosts {
                m.push("    // Blocks only if the upload that last read this slot hasn't finished.".into());
                m.push("    const unsigned slot = s.next_slot++ % host_consts_ring;".into());
                m.push("    TUNGSTEN_CUDA_CHECK(cudaEventSynchronize(s.staging_events[slot]));".into());
                for (name, _) in &hosts {
                    m.push(format!("    std::memcpy(&s.staging[slot]->{n}, &host_consts.{n}, sizeof(host_consts.{n}));", n = name));
                }
                m.push(format!(
                    "    TUNGSTEN_CUDA_CHECK(cudaMemcpyAsync(s.host_consts, s.staging[slot], sizeof(dev::{}), cudaMemcpyHostToDevice, s.stream));",
                    DEVICE_HOST_CONSTS_STRUCT
                ));
                m.push("    TUNGSTEN_CUDA_CHECK(cudaEventRecord(s.staging_events[slot], s.stream));".into());
            } else {
                m.push("    (void)host_consts; // no static host inputs".into());
            }
        };
        let copy_result = format!(
            "    TUNGSTEN_CUDA_CHECK(cudaMemcpyAsync(s.result, s.slab + {l}::{f}, {w}::len * sizeof({t}), cudaMemcpyDeviceToHost, s.stream));",
            l = layout,
            f = interface.buffer_field(&e.inst),
            w = w,
            t = e.qualified_element()
        );

        methods.push(String::new());
        methods.push(format!("Status {w}::enqueue(Vec3 origin, const {}& host_consts) {{", e.names.host_consts));
        methods.push("    Impl& s = *impl_;".into());
        upload(&mut methods);
        methods.push("    const double3 o = make_double3(origin.x, origin.y, origin.z);".into());
        if refined {
            methods.push("    s.origin = o;".into());
        }
        let schedule: Vec<Vec<Kernel>> = interface
            .worker_schedule(e.index)
            .iter()
            .map(|wave| wave.iter().map(|i| Kernel::of_instance(interface, i)).collect())
            .collect();
        methods.extend(launches(program, interface, config, e, &schedule));
        methods.push(copy_result.clone());
        methods.push("    return Status{};".into());
        methods.push("}".into());

        for &r in &e.refinements {
            methods.push(String::new());
            methods.push(format!(
                "Status {w}::enqueue_{}(Pos3 offset, const {}& host_consts) {{",
                interface.refine_method(r),
                e.names.host_consts
            ));
            methods.push("    Impl& s = *impl_;".into());
            upload(&mut methods);
            methods.push("    const double3 o = s.origin;".into());
            methods.push("    const int3 off = make_int3(offset.x, offset.y, offset.z);".into());
            let waves: Vec<Vec<Kernel>> = interface
                .refine_waves(r)
                .iter()
                .map(|wave| wave.iter().map(|run| Kernel::of_run(interface, run)).collect())
                .collect();
            methods.extend(launches(program, interface, config, e, &waves));
            methods.push(copy_result.clone());
            methods.push("    return Status{};".into());
            methods.push("}".into());
        }

        methods.push(String::new());
        methods.push(match (e.single(), e.alias.is_some()) {
            (true, false) => format!("{t} {w}::result() const {{ return impl_->result[0]; }}", t = e.public_element),
            (true, true) => format!("const {w}::{t}& {w}::result() const {{ return impl_->result[0]; }}", t = e.public_element),
            (false, true) => format!("const {w}::{t}* {w}::result() const {{ return impl_->result; }}", t = e.public_element),
            (false, false) => format!("const {t}* {w}::result() const {{ return impl_->result; }}", t = e.public_element),
        });
        methods.extend(
            [
                String::new(),
                format!("Status {w}::sync() {{"),
                "    TUNGSTEN_CUDA_CHECK(cudaStreamSynchronize(impl_->stream));".into(),
                "    return Status{};".into(),
                "}".into(),
                String::new(),
                format!("Status {w}::query() {{"),
                "    return Status{(int)cudaStreamQuery(impl_->stream)};".into(),
                "}".into(),
            ],
        );
        workers.push(methods.join("\n"));
    }
    let _ = host_inputs;

    let mut items = vec![
        cuda::Item::Raw(asserts.join("\n")),
        cuda::Item::Blank,
        cuda::Item::Raw(
            "const char* Status::message() const { return cudaGetErrorString(static_cast<cudaError_t>(code)); }".into(),
        ),
        cuda::Item::Blank,
        cuda::Item::Raw(format!("namespace {{\nconstexpr unsigned host_consts_ring = {};\n}} // namespace", config.host_consts_ring)),
        cuda::Item::Blank,
        cuda::Item::Raw(shared),
        cuda::Item::Blank,
        cuda::Item::Raw(create.join("\n")),
    ];
    for w in workers {
        items.push(cuda::Item::Blank);
        items.push(cuda::Item::Raw(w));
    }

    cuda::File {
        items: vec![
            generated_comment("Host orchestration. Compile with nvcc; launches the kernels defined in density.cu."),
            cuda::Item::Raw(includes),
            cuda::Item::Blank,
            cuda::Item::Raw(check_macro),
            cuda::Item::Blank,
            cuda::Item::Namespace { name: ns.clone(), items },
        ],
    }
}

/// Orchestration-side generator: drives the device side and assembles the four files.
pub struct TransformCudaOrchestration<'a> {
    device: Box<dyn DeviceCodegen + 'a>,
    config: &'a CudaConfig,
    rules: &'a NamingRules,
    files: Option<CudaFiles>,
}

impl<'a> TransformCudaOrchestration<'a> {
    /// `rules` must be the naming rules the interface was built with.
    pub fn new(device: Box<dyn DeviceCodegen + 'a>, config: &'a CudaConfig, rules: &'a NamingRules) -> Self {
        Self { device, config, rules, files: None }
    }

    /// The generated files; call after `convert_program`.
    pub fn files(&self) -> CudaFiles {
        self.files.clone().expect("convert_program must be called before files")
    }

    /// Device and orchestration diagnostics. Call after `convert_program`.
    pub fn warnings(&self) -> Vec<String> {
        self.device.warnings()
    }
}

impl<'a> OrchestrationCodegen<'a> for TransformCudaOrchestration<'a> {
    /// Generates the kernels on the device side in wave order, then the refine kernels, then the headers and host source.
    /// Calling it again replaces the files.
    fn convert_program(&mut self, program: &'a Program, interface: &Interface) {
        for wave in interface.waves() {
            for inst in wave {
                self.device.generate_kernel_of(vec![inst.cu]);
            }
        }
        for r in 0..program.refinements.len() {
            for run in interface.refine_waves(r).iter().flatten() {
                self.device.generate_refine_kernel(run);
            }
        }
        let entries = entries(program, self.rules);
        let print = |file: cuda::File| CudaCodeGenerator::new().generate(&file);
        let device_namespace = format!("{}::{}", self.config.namespace, DEVICE_NAMESPACE);
        self.files = Some(CudaFiles {
            public_header: print(public_header(program, interface, self.config, &entries)),
            interface_header: print(interface_header(program, interface, self.config, &entries)),
            density_source: self.device.render(&device_namespace, &[self.config.interface_header_name.clone()]),
            orchestration_source: print(orchestration_source(program, interface, self.config, &entries)),
        });
    }

    /// Every file, each preceded by a `// ==== <name> ====` banner; call after `convert_program`.
    fn output(&self) -> String {
        let files = self.files();
        let c = self.config;
        [
            (&c.public_header_name, &files.public_header),
            (&c.interface_header_name, &files.interface_header),
            (&c.density_source_name, &files.density_source),
            (&c.orchestration_source_name, &files.orchestration_source),
        ]
        .iter()
        .map(|(name, contents)| format!("// ==== {} ====\n{}", name, contents))
        .collect::<Vec<_>>()
        .join("\n")
    }
}
