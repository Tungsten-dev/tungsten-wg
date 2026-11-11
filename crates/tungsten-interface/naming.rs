use std::collections::HashSet;

use tungsten_ir::model::{AutoConst, ComputeUnit, HostInput, OptimizedProgram, Ref};

/// Per-language naming: names that must never be generated, and how to make a legal identifier.
/// Each backend builds its own [`super::Interface`] with its rules, so names may differ between backends.
#[derive(Debug, Clone)]
pub struct NamingRules {
    /// Identifiers that must never be produced (keywords, runtime type names, ...).
    pub reserved: Vec<String>,
    /// Turns an arbitrary name into a legal identifier of the target language.
    pub sanitize: fn(&str) -> String,
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "try", "typeof", "unsized", "virtual", "yield", "gen",
];

/// Java keywords (including contextual ones), literals, and names the generated Java file
/// declares or uses unqualified.
pub const JAVA_RESERVED: &[&str] = &[
    "abstract",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "try",
    "void",
    "volatile",
    "while",
    "var",
    "record",
    "yield",
    "sealed",
    "permits",
    "non_sealed",
    "when",
    "module",
    "open",
    "exports",
    "requires",
    "true",
    "false",
    "null",
    "_",
    // generated and runtime types
    "HostConsts",
    "AutoConsts",
    "InternalBuffers",
    "Orchestration",
    "Config",
    "Vec3",
    "Pos3",
    "PositionIterator",
    "Helpers",
    "DoubleRef",
    "FloatRef",
    "IntRef",
    "LongRef",
    "BoolRef",
    // java.lang names used unqualified
    "Math",
    "Double",
    "Float",
    "Integer",
    "Long",
    "Boolean",
    "Object",
    "String",
    "System",
    // uniform parameters, kernel locals and Orchestration members
    "p",
    "config",
    "buffers",
    "auto_constants",
    "host_consts",
    "origin_x",
    "origin_y",
    "origin_z",
    "out",
    "idx",
    "ix",
    "iy",
    "iz",
    "create",
];

/// C++20 keywords and alternative tokens, CUDA built-ins used unqualified, and names the generated
/// CUDA files declare or use.
pub const CUDA_RESERVED: &[&str] = &[
    // C++20 keywords
    "alignas", "alignof", "asm", "auto", "bool", "break", "case", "catch", "char", "char8_t",
    "char16_t", "char32_t", "class", "concept", "const", "consteval", "constexpr", "constinit",
    "const_cast", "continue", "co_await", "co_return", "co_yield", "decltype", "default", "delete",
    "do", "double", "dynamic_cast", "else", "enum", "explicit", "export", "extern", "false",
    "float", "for", "friend", "goto", "if", "inline", "int", "long", "mutable", "namespace", "new",
    "noexcept", "nullptr", "operator", "private", "protected", "public", "register",
    "reinterpret_cast", "requires", "return", "short", "signed", "sizeof", "static",
    "static_assert", "static_cast", "struct", "switch", "template", "this", "thread_local",
    "throw", "true", "try", "typedef", "typeid", "typename", "union", "unsigned", "using",
    "virtual", "void", "volatile", "wchar_t", "while",
    // alternative tokens
    "and", "and_eq", "bitand", "bitor", "compl", "not", "not_eq", "or", "or_eq", "xor", "xor_eq",
    // common macros and globals
    "NULL", "std", "assert", "printf", "main",
    // CUDA built-ins
    "threadIdx", "blockIdx", "blockDim", "gridDim", "warpSize", "int3", "double3", "float3",
    "make_int3", "make_double3", "dim3", "cudaStream_t", "cudaError_t",
    // generated and runtime names
    "tw", "dev", "Ctx", "Config", "PositionIterator", "HostConsts", "DeviceHostConsts",
    "AutoConsts", "BufferLayout", "Orchestration", "Status", "Vec3", "Pos3", "Array", "Impl",
    // uniform parameters and kernel locals
    "p", "ctx", "local", "origin", "buffers", "auto_constants", "host_consts", "r", "config",
];

/// The checked buffer read the Rust backend generates next to the density functions.
pub const RUST_READ_HELPER: &str = "rd";

impl NamingRules {
    pub fn rust() -> Self {
        Self {
            reserved: RUST_KEYWORDS.iter().chain([&RUST_READ_HELPER]).map(|s| s.to_string()).collect(),
            sanitize: sanitize_name,
        }
    }

    pub fn java() -> Self {
        Self {
            reserved: JAVA_RESERVED.iter().map(|s| s.to_string()).collect(),
            sanitize: java_sanitize,
        }
    }

    pub fn cuda() -> Self {
        Self {
            reserved: CUDA_RESERVED.iter().map(|s| s.to_string()).collect(),
            sanitize: cuda_sanitize,
        }
    }

    /// For inspection output: no reserved names and no sanitizing, names are the raw canonical
    /// names plus the usual suffixes, de-duplicated only on real collisions.
    pub fn inspect() -> Self {
        Self {
            reserved: vec![],
            sanitize: |s| s.to_string(),
        }
    }

    /// Adds more reserved names (e.g. the generated class name).
    pub fn with_reserved(mut self, extra: impl IntoIterator<Item = String>) -> Self {
        self.reserved.extend(extra);
        self
    }

    /// A fresh `used` set seeded with the reserved names.
    pub fn used_set(&self) -> HashSet<String> {
        self.reserved.iter().cloned().collect()
    }
}

/// A legal (ASCII) Java identifier: other characters become `_`, a leading digit gets a `_` prefix,
/// and the lone `_` (a keyword since Java 9) becomes `_0`.
pub fn java_sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    if s == "_" {
        s = "_0".to_string();
    }
    s
}

/// A legal C++ identifier that is never reserved by the implementation: other characters become
/// `_`, runs of `_` collapse, and a name that is empty or starts with a digit or `_` gets a `v`
/// prefix (so `__x` and `_X` can't be produced).
pub fn cuda_sanitize(name: &str) -> String {
    let mut s = String::with_capacity(name.len() + 1);
    for c in name.chars() {
        let c = if c.is_ascii_alphanumeric() { c } else { '_' };
        if c == '_' && s.ends_with('_') {
            continue;
        }
        s.push(c);
    }
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit() || c == '_') {
        s.insert(0, 'v');
    }
    s
}

/// Replaces characters that are not valid in generated identifiers.
pub fn sanitize_name(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

/// `base`, or `base_N` for the first free N if `base` is already taken.
pub fn unique(used: &mut HashSet<String>, base: String) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    let mut n = 1;
    loop {
        let candidate = format!("{}_{}", base, n);
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

/// Names of the entry methods of the orchestration type, one per entry point in order:
/// `orchestrate_<unit>`, made unique against `used` (which each backend seeds with the names
/// already taken in that type, such as the constructor) and against each other.
pub fn entry_method_names(
    program: &OptimizedProgram,
    rules: &NamingRules,
    mut used: HashSet<String>,
) -> Vec<String> {
    program
        .entry_points
        .iter()
        .map(|entry| {
            unique(
                &mut used,
                format!("orchestrate_{}", unit_base_name(rules, program, entry.density_function)),
            )
        })
        .collect()
}

/// Base names of the entries of a CUDA-style orchestration class, one per entry point in order:
/// the unit's base name made unique against `used` (which the backend seeds with the member names
/// already taken) and against each other. `enqueue_<base>` and `result_<base>` derive from it.
pub fn entry_base_names(
    program: &OptimizedProgram,
    rules: &NamingRules,
    mut used: HashSet<String>,
) -> Vec<String> {
    program
        .entry_points
        .iter()
        .map(|entry| unique(&mut used, unit_base_name(rules, program, entry.density_function)))
        .collect()
}

/// The worker method of each refinement, in `program.refinements` order: `refine_<name>`, made
/// unique among the refinements of the same entry and against `run` and `reserved` (the other
/// members the backend gives a worker). Refinement names are unique per entry, so two entries'
/// methods may share a spelling: they live on different workers.
pub fn refinement_method_names(
    program: &OptimizedProgram,
    rules: &NamingRules,
    reserved: &HashSet<String>,
) -> Vec<String> {
    let mut per_entry: Vec<HashSet<String>> = vec![];
    program
        .refinements
        .iter()
        .map(|r| {
            if per_entry.len() <= r.entry {
                per_entry.resize_with(r.entry + 1, || {
                    let mut used = rules.used_set();
                    used.extend(reserved.iter().cloned());
                    used.insert("run".to_string());
                    used
                });
            }
            unique(&mut per_entry[r.entry], (rules.sanitize)(&format!("refine_{}", r.name)))
        })
        .collect()
}

/// How a backend spells the factory that creates a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactoryStyle {
    /// `final_density_worker` (Rust).
    Snake,
    /// `finalDensityWorker` (Java).
    LowerCamel,
    /// `create_final_density_worker` (CUDA).
    CreateSnake,
}

/// The generated names belonging to one entry's worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerNames {
    /// The entry's base name, from [`entry_base_names`].
    pub base: String,
    /// `<Base>Worker`
    pub worker: String,
    /// `<Base>HostConsts`
    pub host_consts: String,
    /// `<Base>Layout` (CUDA)
    pub layout: String,
    /// The method on `Orchestration` that creates the worker.
    pub factory: String,
}

/// Names of every entry's worker, in entry order. Type names are unique against the reserved names
/// of `rules`, `used_types` (the other generated types) and each other. Factories are unique
/// against the reserved names, `used_members` (the other members of `Orchestration`) and each other.
pub fn worker_names(
    program: &OptimizedProgram,
    rules: &NamingRules,
    style: FactoryStyle,
    used_types: HashSet<String>,
    used_members: HashSet<String>,
) -> Vec<WorkerNames> {
    let mut types = rules.used_set();
    types.extend(used_types);
    let mut members = rules.used_set();
    members.extend(used_members);
    entry_base_names(program, rules, members.clone())
        .into_iter()
        .map(|base| {
            let camel = (rules.sanitize)(&upper_camel(&base));
            let [worker, host_consts, layout] = free_type_names(&mut types, &camel);
            let snake = format!("{base}_worker");
            let factory = match style {
                FactoryStyle::Snake => snake,
                FactoryStyle::LowerCamel => lower_camel(&snake),
                FactoryStyle::CreateSnake => format!("create_{snake}"),
            };
            let factory = unique(&mut members, factory);
            WorkerNames {
                base,
                worker,
                host_consts,
                layout,
                factory,
            }
        })
        .collect()
}

/// The three type names of `camel`, or of `camel_N` for the first N that leaves all three free.
fn free_type_names(used: &mut HashSet<String>, camel: &str) -> [String; 3] {
    let mut base = camel.to_string();
    let mut n = 0;
    loop {
        let names = ["Worker", "HostConsts", "Layout"].map(|suffix| format!("{base}{suffix}"));
        if names.iter().all(|name| !used.contains(name)) {
            used.extend(names.iter().cloned());
            return names;
        }
        n += 1;
        base = format!("{camel}_{n}");
    }
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or(String::new(), |c| c.to_ascii_uppercase().to_string() + chars.as_str())
}

/// `final_density` → `FinalDensity`. Only underscores separate words, so `aB` stays `AB`.
fn upper_camel(name: &str) -> String {
    name.split('_').map(capitalized).collect()
}

/// `final_density_worker` → `finalDensityWorker`.
fn lower_camel(name: &str) -> String {
    let camel = upper_camel(name);
    let mut chars = camel.chars();
    chars.next().map_or(String::new(), |c| c.to_ascii_lowercase().to_string() + chars.as_str())
}

/// Field name of an autoconst in `AutoConsts`: its sanitized name (the caller makes it unique).
pub fn auto_const_field_name(rules: &NamingRules, a: &AutoConst) -> String {
    (rules.sanitize)(&a.name)
}

/// Names that generated creation code already uses, so no auto param may take them.
pub const CREATION_RESERVED: &[&str] = &[
    "buffers",
    "auto_constants",
    "auto_consts",
    "_init",
    "out",
    "impl",
    "this",
    // CUDA: file-scope names `create` reads, and its loop variable
    "host_consts_ring",
    "i",
    "stream",
];

/// Field name of a static host input in `HostConsts`.
pub fn host_field_name(rules: &NamingRules, input: &HostInput) -> String {
    (rules.sanitize)(&input.get_name())
}

pub fn unit_base_name(
    rules: &NamingRules,
    program: &OptimizedProgram,
    cu: Ref<ComputeUnit>,
) -> String {
    let ComputeUnit::DensityFunction {
        function,
        source_hash,
        ..
    } = &program.compute_units[cu];
    program.functions[*function]
        .canonical_name
        .as_deref()
        .map(rules.sanitize)
        .unwrap_or_else(|| format!("density_function_{:x}", source_hash))
}

#[cfg(test)]
mod tests {
    use super::{FactoryStyle, NamingRules, WorkerNames, cuda_sanitize, unique, worker_names};
    use std::collections::HashSet;
    use tungsten_ir::model::{Arena, ComputeUnit, EntryPoint, Function, OptimizedProgram, VariableType};

    /// One unit per name, each an entry.
    fn entries(names: &[&str]) -> OptimizedProgram {
        let mut p = OptimizedProgram {
            compute_units: Arena::new(),
            functions: Arena::new(),
            entry_points: vec![],
            refinements: vec![],
            constants: Arena::new(),
            host_inputs: Arena::new(),
            auto_params: Arena::new(),
            auto_consts: Arena::new(),
            density_inputs: Arena::new(),
        };
        for name in names {
            let function = p.functions.insert(Function {
                canonical_name: Some(name.to_string()),
                host_inputs: vec![],
                body: vec![],
                variables: Arena::new(),
                parameters: vec![],
                return_type: VariableType::F64,
                source_hash: 0,
            });
            let unit = p.compute_units.insert(ComputeUnit::DensityFunction {
                function,
                density_inputs: vec![],
                auto_consts: vec![],
                host_inputs: vec![],
                helper_functions: vec![],
                constants: vec![],
                source_hash: 0,
            });
            p.entry_points.push(EntryPoint {
                density_function: unit,
                dimensions: (1, 1, 1),
                scaled_origin: (1.0, 1.0, 1.0),
                scaled_position: (1.0, 1.0, 1.0),
            });
        }
        p
    }

    fn names(p: &OptimizedProgram, rules: &NamingRules, style: FactoryStyle) -> Vec<WorkerNames> {
        worker_names(p, rules, style, HashSet::new(), HashSet::new())
    }

    #[test]
    fn worker_names_by_style() {
        let p = entries(&["final_density"]);
        let n = &names(&p, &NamingRules::rust(), FactoryStyle::Snake)[0];
        assert_eq!(n.base, "final_density");
        assert_eq!(n.worker, "FinalDensityWorker");
        assert_eq!(n.host_consts, "FinalDensityHostConsts");
        assert_eq!(n.layout, "FinalDensityLayout");
        assert_eq!(n.factory, "final_density_worker");
        let java = &names(&p, &NamingRules::java(), FactoryStyle::LowerCamel)[0];
        assert_eq!(java.factory, "finalDensityWorker");
        let cuda = &names(&p, &NamingRules::cuda(), FactoryStyle::CreateSnake)[0];
        assert_eq!(cuda.factory, "create_final_density_worker");
    }

    #[test]
    fn worker_types_stay_unique_after_camel_casing() {
        let p = entries(&["a_b", "aB"]);
        for (rules, style) in [
            (NamingRules::rust(), FactoryStyle::Snake),
            (NamingRules::java(), FactoryStyle::LowerCamel),
            (NamingRules::cuda(), FactoryStyle::CreateSnake),
        ] {
            let n = names(&p, &rules, style);
            assert_ne!(n[0].worker, n[1].worker);
            assert_ne!(n[0].host_consts, n[1].host_consts);
            assert_ne!(n[0].layout, n[1].layout);
            assert_ne!(n[0].factory, n[1].factory);
        }
    }

    #[test]
    fn entry_named_orchestration_does_not_clash() {
        let p = entries(&["orchestration"]);
        for (rules, style) in [
            (NamingRules::rust(), FactoryStyle::Snake),
            (NamingRules::java(), FactoryStyle::LowerCamel),
            (NamingRules::cuda(), FactoryStyle::CreateSnake),
        ] {
            let n = &names(&p, &rules, style)[0];
            assert_eq!(n.worker, "OrchestrationWorker");
            assert_eq!(n.host_consts, "OrchestrationHostConsts");
        }
    }

    /// All three type names move together when any one is taken, and a name that camel-cases to
    /// nothing or to a leading digit stays legal.
    #[test]
    fn worker_types_avoid_taken_names_and_stay_legal() {
        let p = entries(&["buffer", "_3d", "_"]);
        let rules = NamingRules::cuda();
        let n = names(&p, &rules, FactoryStyle::CreateSnake);
        assert_eq!(n[0].layout, "Buffer_1Layout", "BufferLayout is reserved on CUDA");
        assert_eq!(n[0].worker, "Buffer_1Worker");
        for n in names(&p, &NamingRules::rust(), FactoryStyle::Snake) {
            assert!(!n.worker.starts_with(|c: char| c.is_ascii_digit()), "{}", n.worker);
        }
        let taken = HashSet::from(["FooHostConsts".to_string()]);
        let p = entries(&["foo"]);
        let n = worker_names(&p, &NamingRules::rust(), FactoryStyle::Snake, taken, HashSet::new());
        assert_eq!(n[0].worker, "Foo_1Worker");
        let members = HashSet::from(["foo_worker".to_string()]);
        let n = worker_names(&p, &NamingRules::rust(), FactoryStyle::Snake, HashSet::new(), members);
        assert_eq!(n[0].factory, "foo_worker_1");
    }

    #[test]
    fn unique_suffixes_collisions() {
        let mut used = HashSet::from(["new".to_string()]);
        assert_eq!(unique(&mut used, "new".into()), "new_1");
        assert_eq!(unique(&mut used, "new".into()), "new_2");
        assert_eq!(unique(&mut used, "a".into()), "a");
    }

    #[test]
    fn cuda_sanitize_rules() {
        assert_eq!(cuda_sanitize("minecraft:overworld/noise"), "minecraft_overworld_noise");
        assert_eq!(cuda_sanitize("a__b--c"), "a_b_c");
        assert_eq!(cuda_sanitize("__x"), "v_x");
        assert_eq!(cuda_sanitize("_X"), "v_X");
        assert_eq!(cuda_sanitize("3d"), "v3d");
        assert_eq!(cuda_sanitize(""), "v");
        assert_eq!(cuda_sanitize("ok_name"), "ok_name");
    }
}
