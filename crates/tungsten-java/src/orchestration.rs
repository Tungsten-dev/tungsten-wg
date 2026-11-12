/*
Orchestration codegen for Java (`specs/workers.md` §3.2, §5.3): the nested classes the device code is
generated against (`HostConsts`, `AutoConsts`, `InternalBuffers`), the shared `Orchestration`, one
worker and one `<Base>HostConsts` per entry, and assembly of the single compilation unit.

`Orchestration` holds the autoconsts and only creates workers. A worker runs its entry's waves on
its own pools, passing each kernel the offsets of its slots.
*/

use std::collections::HashSet;

use super::pools::Pools;
use super::types::{auto_const_type, convert_type, unit_return_type};
use super::{
    AUTO_CONSTS_CLASS, CONFIG_CLASS, HELPERS_CLASS, HOST_CONSTS_CLASS, INTERNAL_BUFFERS_CLASS, JavaCodegen,
    ORCHESTRATION_CLASS, PARAM_AUTO_CONSTS, PARAM_BUFFERS, size,
};
use crate::model as java;
use crate::model::codegen::JavaCodeGenerator;
use tungsten_interface::naming::{FactoryStyle, NamingRules, WorkerNames, java_sanitize, unique, worker_names};
use tungsten_interface::{BackendError, DeviceCodegen, Instance, Interface, OrchestrationCodegen};
use tungsten_ir::model::AutoConstArg;
use tungsten_ir::model::{ComputeUnit, HostInput, Program, VariableType};
use tungsten_rust::pools::cells;

use java::Expression as J;
use java::Statement as S;

/// Names of every entry's worker types and factory, in entry order. Shared with frontends and
/// test drivers so they agree with the generated code. `rules` must be the backend's
/// (`JavaBackend::naming_rules`), which reserves the generated class name.
pub fn java_worker_names(program: &Program, rules: &NamingRules) -> Vec<WorkerNames> {
    let mut types: HashSet<String> = [
        ORCHESTRATION_CLASS,
        HOST_CONSTS_CLASS,
        AUTO_CONSTS_CLASS,
        INTERNAL_BUFFERS_CLASS,
        CONFIG_CLASS,
        HELPERS_CLASS,
        "Vec3",
        "Pos3",
        "PositionIterator",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    // Record and handle types the generated code names.
    let mut add = |t: &VariableType| {
        let mut t = t;
        while let VariableType::Array(e, _) = t {
            t = e;
        }
        if let VariableType::Extern(n) | VariableType::Handle(n) = t {
            types.insert(java_sanitize(n));
        }
    };
    for (_, f) in program.functions.iter() {
        add(&f.return_type);
        f.variables.iter().for_each(|(_, v)| add(&v.t));
    }
    program.host_inputs.iter().for_each(|(_, h)| {
        if let HostInput::Static { t, .. } = h {
            add(t)
        }
    });
    program.auto_consts.iter().for_each(|(_, a)| add(&a.t));
    // Methods of `Orchestration`: `create`, and what every object has.
    let members = ["create", "clone", "equals", "finalize", "getClass", "hashCode", "notify", "notifyAll", "toString", "wait"]
        .into_iter()
        .map(String::from)
        .collect();
    worker_names(program, rules, FactoryStyle::LowerCamel, types, members)
}

/// `<BASE>_LEN` of every entry, in entry order: unique among the generated class's fields.
pub fn len_constant_names(program: &Program, interface: &Interface, rules: &NamingRules, names: &[WorkerNames]) -> Vec<String> {
    let mut used = rules.used_set();
    used.extend(program.constants.iter().map(|(c, _)| interface.constant(c).to_string()));
    names.iter().map(|n| unique(&mut used, format!("{}_LEN", n.base.to_ascii_uppercase()))).collect()
}

/// Whether some refinement belongs to `entry`.
fn refined(program: &Program, entry: usize) -> bool {
    program.refinements.iter().any(|r| r.entry == entry)
}

/// `(name, type)` of every static host, in arena order, optionally only those in `only`.
fn static_hosts<'p>(
    program: &'p Program,
    interface: &Interface,
    only: Option<&[tungsten_ir::model::Ref<HostInput>]>,
) -> Vec<(String, &'p VariableType)> {
    program
        .host_inputs
        .iter()
        .filter(|(r, _)| only.is_none_or(|o| o.contains(r)))
        .filter_map(|(r, h)| match h {
            HostInput::Static { t, .. } => Some((interface.host_field(r).to_string(), t)),
            HostInput::Dynamic => None,
        })
        .collect()
}

/// A class of host fields: the program-wide `HostConsts` (package-private, what kernels read) or an
/// entry's `<Base>HostConsts` (public, what callers fill).
fn host_consts_class(name: &str, hosts: &[(String, &VariableType)], public: bool, doc: &str) -> java::Class {
    let fields = hosts
        .iter()
        .map(|(f, t)| java::Field {
            modifiers: if public { "public".into() } else { String::new() },
            t: convert_type(t),
            name: f.clone(),
            init: None,
        })
        .collect();
    java::Class {
        doc: Some(doc.into()),
        modifiers: if public { "public static final".into() } else { "static final".into() },
        name: name.to_string(),
        fields,
        ..Default::default()
    }
}

/// `static final class AutoConsts`: values built once at creation, immutable and shareable. Only
/// `Orchestration.create` builds it.
fn auto_consts_class(program: &Program, interface: &Interface) -> java::Class {
    let mut fields = Vec::new();
    let mut assignments = Vec::new();
    let params: Vec<(java::Type, String)> = program
        .auto_params
        .iter()
        .map(|(r, p)| (convert_type(&p.t), interface.auto_param(r).to_string()))
        .collect();
    for (r, a) in program.auto_consts.iter() {
        let field = interface.auto_const_field(r).to_string();
        fields.push(java::Field {
            modifiers: "final".into(),
            t: auto_const_type(&a.t),
            name: field.clone(),
            init: None,
        });
        let args = a
            .args
            .iter()
            .map(|arg| match arg {
                AutoConstArg::Param(p) => J::name(interface.auto_param(*p)),
                AutoConstArg::I64(v) => J::LongLit(*v),
                AutoConstArg::F64(v) => J::DoubleLit(*v),
                AutoConstArg::Bool(v) => J::BoolLit(*v),
                AutoConstArg::Str(v) => J::StrLit(v.clone()),
            })
            .collect();
        assignments.push(S::Assign {
            target: J::field(J::name("this"), &field),
            value: J::call(Some(J::name(HELPERS_CLASS)), &a.initializer, args),
        });
    }
    let constructor = java::Method {
        doc: None,
        modifiers: String::new(),
        return_type: None,
        name: AUTO_CONSTS_CLASS.into(),
        params,
        body: assignments,
    };
    java::Class {
        doc: Some("Values built at creation. Immutable after construction and safe to share across threads.".into()),
        modifiers: "static final".into(),
        name: AUTO_CONSTS_CLASS.into(),
        fields,
        methods: vec![constructor],
        ..Default::default()
    }
}

/// `static final class InternalBuffers`: one array per pool, each sized by its worker.
fn internal_buffers_class(pools: &Pools) -> java::Class {
    let len = |name: &str| format!("len_{name}");
    let mut fields = vec![];
    let mut params = vec![];
    let mut body = vec![];
    for (t, name) in &pools.fields {
        let element = convert_type(t);
        let array = java::Type::Array(Box::new(element.clone()));
        fields.push(java::Field { modifiers: "final".into(), t: array, name: name.clone(), init: None });
        params.push((java::Type::Int, len(name)));
        body.push(S::Assign {
            target: J::field(J::name("this"), name),
            value: J::NewArraySized { element, len: Box::new(J::name(&len(name))) },
        });
    }
    java::Class {
        doc: Some("One array per element type; a worker's instances are ranges of them.".into()),
        modifiers: "static final".into(),
        name: INTERNAL_BUFFERS_CLASS.into(),
        fields,
        methods: vec![java::Method {
            doc: None,
            modifiers: String::new(),
            return_type: None,
            name: INTERNAL_BUFFERS_CLASS.into(),
            params,
            body,
        }],
        ..Default::default()
    }
}

/// `public static final class Orchestration`: global and immutable, so safe to share. It builds the
/// autoconsts once and creates workers.
fn orchestration_class(
    program: &Program,
    interface: &Interface,
    pools: &Pools,
    names: &[WorkerNames],
) -> java::Class {
    let auto = || java::Type::class(AUTO_CONSTS_CLASS);
    let orch = || java::Type::class(ORCHESTRATION_CLASS);
    let params: Vec<(java::Type, String)> = program
        .auto_params
        .iter()
        .map(|(r, p)| (convert_type(&p.t), interface.auto_param(r).to_string()))
        .collect();
    let mut methods = vec![
        java::Method {
            doc: None,
            modifiers: "private".into(),
            return_type: None,
            name: ORCHESTRATION_CLASS.into(),
            params: vec![(auto(), PARAM_AUTO_CONSTS.into())],
            body: vec![S::Assign {
                target: J::field(J::name("this"), PARAM_AUTO_CONSTS),
                value: J::name(PARAM_AUTO_CONSTS),
            }],
        },
        java::Method {
            doc: Some("Builds every autoconst. The result is immutable and may be shared across threads.".into()),
            modifiers: "public static".into(),
            return_type: Some(orch()),
            name: "create".into(),
            body: vec![S::Return(Some(J::New {
                class: ORCHESTRATION_CLASS.into(),
                args: vec![J::New {
                    class: AUTO_CONSTS_CLASS.into(),
                    args: params.iter().map(|(_, n)| J::name(n)).collect(),
                    comment: None,
                }],
                comment: None,
            }))],
            params,
        },
    ];
    for (e, n) in names.iter().enumerate() {
        let layout = interface.worker(e);
        let lens = pools.fields.iter().map(|(t, _)| {
            J::IntLit(layout.pools.iter().find(|p| p.element == *t).map_or(0, |p| p.len) as i32)
        });
        let _ = &program.entry_points[e];
        methods.push(java::Method {
            doc: Some("Creates a worker for this entry: it allocates its own buffers. Not thread-safe once created: use one per thread.".into()),
            modifiers: "public".into(),
            return_type: Some(java::Type::class(&n.worker)),
            name: n.factory.clone(),
            params: vec![],
            body: vec![S::Return(Some(J::New {
                class: n.worker.clone(),
                args: vec![
                    J::field(J::name("this"), PARAM_AUTO_CONSTS),
                    J::New { class: INTERNAL_BUFFERS_CLASS.into(), args: lens.collect(), comment: None },
                ],
                comment: None,
            }))],
        });
    }
    java::Class {
        doc: Some("Global and immutable: safe to share across threads. Its only job is to create workers.".into()),
        modifiers: "public static final".into(),
        name: ORCHESTRATION_CLASS.into(),
        fields: vec![java::Field {
            modifiers: "private final".into(),
            t: auto(),
            name: PARAM_AUTO_CONSTS.into(),
            init: None,
        }],
        methods,
        ..Default::default()
    }
}

/// `run(double x, double y, double z, <Base>HostConsts hc)`, or with `refinement` the method
/// `refine_<name>(int ox, int oy, int oz, <Base>HostConsts hc)`: copies the entry's hosts into the
/// private `HostConsts`, runs the waves and returns the entry's pool array, whose range
/// `[0, LEN)` is the entry's slot (offset 0 of pool 0). A single-cell entry returns the value.
///
/// `run` runs the worker's schedule and stores the origin when the entry has refinements. A refine
/// call runs its refinement's waves with the stored origin and the offset.
fn run_method(
    program: &Program,
    interface: &Interface,
    pools: &Pools,
    class_name: &str,
    entry: usize,
    names: &WorkerNames,
    refinement: Option<usize>,
) -> java::Method {
    let target = Instance::from_entry(&program.entry_points[entry]);
    let element_ir = unit_return_type(program, target.cu);
    let element = convert_type(element_ir);
    let single = cells(target.dimensions) == 1;
    let slot = |inst: &Instance| {
        interface
            .slot(entry, inst)
            .unwrap_or_else(|| panic!("entry {entry}'s worker has no slot for {inst:?}"))
    };

    let mut body = vec![];
    for (name, _) in static_hosts(program, interface, Some(interface.entry_hosts(entry))) {
        body.push(S::Assign {
            target: J::field(J::name("hosts"), &name),
            value: J::field(J::name("hc"), &name),
        });
    }
    if refinement.is_none() && refined(program, entry) {
        for (field, param) in [("origin_x", "x"), ("origin_y", "y"), ("origin_z", "z")] {
            body.push(S::Assign { target: J::field(J::name("this"), field), value: J::name(param) });
        }
    }

    // (kernel, the buffer it writes, the unit it runs) for each call, wave by wave.
    let waves: Vec<Vec<(String, Instance, tungsten_ir::model::Ref<ComputeUnit>)>> = match refinement {
        None => interface
            .worker_schedule(entry)
            .iter()
            .map(|w| w.iter().map(|i| (interface.kernel(i).to_string(), *i, i.cu)).collect())
            .collect(),
        Some(r) => interface
            .refine_waves(r)
            .iter()
            .map(|w| w.iter().map(|run| (interface.refine_kernel(run).to_string(), run.target, run.unit)).collect())
            .collect(),
    };
    for (w, wave) in waves.iter().enumerate() {
        body.push(S::Comment(format!("wave {w}")));
        for (kernel, inst, unit) in wave {
            let ComputeUnit::DensityFunction { function, .. } = &program.compute_units[*unit];
            let offsets = interface.input_params(*function).iter().map(|&d| {
                let input = Instance::from_input(&program.density_inputs[d]);
                J::IntLit(slot(&input).offset as i32)
            });
            // A refine call takes the origin of the last `run`, and the offset.
            let leading: Vec<J> = if refinement.is_some() {
                ["origin_x", "origin_y", "origin_z", "ox", "oy", "oz"].map(J::name).into()
            } else {
                ["x", "y", "z"].map(J::name).into()
            };
            let args = leading
                .into_iter()
                .chain([J::name(PARAM_BUFFERS), J::IntLit(slot(inst).offset as i32)])
                .chain(offsets)
                .chain([J::name(PARAM_AUTO_CONSTS), J::name("hosts")])
                .collect();
            body.push(S::Expr(J::call(Some(J::name(class_name)), kernel, args)));
        }
    }

    debug_assert_eq!(slot(&target).offset, 0);
    let buffer = J::field(J::name(PARAM_BUFFERS), pools.field(element_ir));
    body.push(S::Return(Some(if single {
        J::Index { base: Box::new(buffer), index: Box::new(J::IntLit(0)) }
    } else {
        buffer
    })));

    let (name, params) = match refinement {
        None => (
            "run".to_string(),
            vec![(java::Type::Double, "x".to_string()), (java::Type::Double, "y".into()), (java::Type::Double, "z".into())],
        ),
        Some(r) => (
            interface.refine_method(r).to_string(),
            vec![(java::Type::Int, "ox".to_string()), (java::Type::Int, "oy".into()), (java::Type::Int, "oz".into())],
        ),
    };
    let doc = match (refinement.is_some(), single) {
        (false, true) => "Computes the entry's single value at origin (x, y, z).",
        (false, false) => "Computes the entry at origin (x, y, z). Returns the worker's pool array, with the entry at\n[0, LEN): it is overwritten by the next call on this worker, so copy it to keep it.",
        (true, true) => "Refines the entry's value at the stored origin plus (ox, oy, oz). Call after run.",
        (true, false) => "Refines the entry at the stored origin plus (ox, oy, oz). Call after run. Returns the pool array like\nrun does, with the same overwrite rule.",
    };
    java::Method {
        doc: Some(doc.into()),
        modifiers: "public".into(),
        return_type: Some(if single { element } else { java::Type::Array(Box::new(element)) }),
        name,
        params: params.into_iter().chain([(java::Type::class(&names.host_consts), "hc".to_string())]).collect(),
        body,
    }
}

/// `public static final class <Base>Worker`: runs one entry on its own buffers.
fn worker_class(
    program: &Program,
    interface: &Interface,
    pools: &Pools,
    class_name: &str,
    entry: usize,
    names: &WorkerNames,
) -> java::Class {
    let refined = refined(program, entry);
    let field = |t: java::Type, name: &str| java::Field {
        modifiers: "private final".into(),
        t,
        name: name.into(),
        init: None,
    };
    let mut fields = vec![
        field(java::Type::class(AUTO_CONSTS_CLASS), PARAM_AUTO_CONSTS),
        field(java::Type::class(INTERNAL_BUFFERS_CLASS), PARAM_BUFFERS),
    ];
    fields.push(java::Field {
        modifiers: "private final".into(),
        t: java::Type::class(HOST_CONSTS_CLASS),
        name: "hosts".into(),
        init: Some(J::New { class: HOST_CONSTS_CLASS.into(), args: vec![], comment: None }),
    });
    if refined {
        for axis in ["origin_x", "origin_y", "origin_z"] {
            fields.push(java::Field {
                modifiers: "private".into(),
                t: java::Type::Double,
                name: axis.into(),
                init: None,
            });
        }
    }
    let assign = |name: &str| S::Assign { target: J::field(J::name("this"), name), value: J::name(name) };
    let constructor = java::Method {
        doc: None,
        modifiers: "private".into(),
        return_type: None,
        name: names.worker.clone(),
        params: vec![
            (java::Type::class(AUTO_CONSTS_CLASS), PARAM_AUTO_CONSTS.into()),
            (java::Type::class(INTERNAL_BUFFERS_CLASS), PARAM_BUFFERS.into()),
        ],
        body: vec![assign(PARAM_AUTO_CONSTS), assign(PARAM_BUFFERS)],
    };
    let mut methods = vec![constructor];
    methods.push(run_method(program, interface, pools, class_name, entry, names, None));
    for (r, _) in program.refinements.iter().enumerate().filter(|(_, r)| r.entry == entry) {
        methods.push(run_method(program, interface, pools, class_name, entry, names, Some(r)));
    }
    java::Class {
        doc: Some("Not thread-safe: one per thread. Created by `Orchestration`.".into()),
        modifiers: "public static final".into(),
        name: names.worker.clone(),
        fields,
        methods,
        ..Default::default()
    }
}

/// Orchestration-side generator: drives the device side and assembles the compilation unit.
pub struct TransformJavaOrchestration<'a> {
    device: JavaCodegen<'a>,
    pools: Pools,
    package: String,
    class_name: String,
    runtime_package: String,
    unit: Option<java::CompilationUnit>,
    warnings: Vec<String>,
}

impl<'a> TransformJavaOrchestration<'a> {
    /// `device` must have been built for `class_name`, which should already be a legal Java
    /// identifier (see [`java_sanitize`]); an empty `package` emits no package declaration. `pools`
    /// must be the ones `device` was built with.
    pub fn new(
        device: JavaCodegen<'a>,
        pools: Pools,
        package: &str,
        class_name: &str,
        runtime_package: &str,
    ) -> Self {
        let sanitized = java_sanitize(class_name);
        let mut warnings = Vec::new();
        if sanitized != class_name {
            warnings.push(format!("java: class name '{}' was sanitized to '{}'", class_name, sanitized));
        }
        Self {
            device,
            pools,
            package: package.to_string(),
            class_name: sanitized,
            runtime_package: runtime_package.to_string(),
            unit: None,
            warnings,
        }
    }

    /// Diagnostics: class-name sanitizing and method size estimates. Call after `convert_program`.
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings = self.warnings.clone();
        if let Some(unit) = &self.unit {
            size::check_class(&unit.class, &mut warnings);
        }
        warnings
    }

    /// Fails if a generated method has more parameter slots than the JVM allows. Call after
    /// `convert_program`.
    pub fn check_limits(&self) -> Result<(), BackendError> {
        match &self.unit {
            Some(unit) => size::check_parameter_limit(&unit.class).map_err(BackendError::Codegen),
            None => Ok(()),
        }
    }
}

impl<'a> OrchestrationCodegen<'a> for TransformJavaOrchestration<'a> {
    /// Generates the kernels on the device side, then the nested classes and the top-level class.
    /// Calling it again replaces the compilation unit.
    fn convert_program(&mut self, program: &'a Program, interface: &Interface) {
        // Device code first: one kernel per scheduled instance (repeats are no-ops).
        let waves = self.device.naming().waves().to_vec();
        for wave in &waves {
            for inst in wave {
                self.device.generate_kernel_of(vec![inst.cu]);
            }
        }

        for r in 0..program.refinements.len() {
            for run in interface.refine_waves(r).iter().flatten() {
                self.device.generate_refine_kernel(run);
            }
        }

        let rules = self.device.rules();
        let names = java_worker_names(program, rules);
        let lens = len_constant_names(program, interface, rules, &names);
        let private_constructor = java::Method {
            doc: None,
            modifiers: "private".into(),
            return_type: None,
            name: self.class_name.clone(),
            params: vec![],
            body: vec![],
        };
        let mut methods = vec![private_constructor];
        methods.extend(self.device.methods());
        let mut fields = self.device.constant_fields();
        for (e, name) in lens.iter().enumerate() {
            fields.push(java::Field {
                modifiers: "public static final".into(),
                t: java::Type::Int,
                name: name.clone(),
                init: Some(J::IntLit(cells(program.entry_points[e].dimensions) as i32)),
            });
        }
        let mut nested = vec![
            host_consts_class(
                HOST_CONSTS_CLASS,
                &static_hosts(program, interface, None),
                false,
                "Every static host: what the kernels read. A worker copies its entry's hosts into its own.",
            ),
            auto_consts_class(program, interface),
            internal_buffers_class(&self.pools),
            orchestration_class(program, interface, &self.pools, &names),
        ];
        for (e, n) in names.iter().enumerate() {
            let own = static_hosts(program, interface, Some(interface.entry_hosts(e)));
            nested.push(host_consts_class(
                &n.host_consts,
                &own,
                true,
                "The static hosts this entry reads, filled by the caller. Do not mutate during a call.",
            ));
            nested.push(worker_class(program, interface, &self.pools, &self.class_name, e, n));
        }
        let class = java::Class {
            doc: None,
            modifiers: "public final".into(),
            name: self.class_name.clone(),
            fields,
            methods,
            nested,
            ..Default::default()
        };
        self.unit = Some(java::CompilationUnit {
            header: vec![
                format!("Generated by tungsten-wg {}. Do not edit.", env!("CARGO_PKG_VERSION")),
                "Threading: Orchestration is immutable and may be shared across threads.".into(),
                "           A worker is NOT thread-safe; create one per thread.".into(),
            ],
            package: (!self.package.is_empty()).then(|| self.package.clone()),
            imports: vec![format!("{}.*", self.runtime_package)],
            class,
        });
    }

    /// The generated Java source; call after `convert_program`.
    fn output(&self) -> String {
        let unit = self.unit.as_ref().expect("convert_program must be called before output");
        JavaCodeGenerator::new().generate(unit)
    }
}
