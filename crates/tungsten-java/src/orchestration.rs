/*
Orchestration codegen for Java: the nested classes the device code is generated against
(`HostConsts`, `AutoConsts`, `InternalBuffers`), the `Orchestration` class that runs kernels in wave
order, and assembly of the single compilation unit.
*/

use super::types::{auto_const_type, convert_type, unit_return_type};
use super::{
    AUTO_CONSTS_CLASS, HELPERS_CLASS, HOST_CONSTS_CLASS, INTERNAL_BUFFERS_CLASS, JavaCodegen,
    ORCHESTRATION_CLASS, PARAM_AUTO_CONSTS, PARAM_BUFFERS, PARAM_HOST_CONSTS, size,
};
use tungsten_interface::naming::{NamingRules, java_sanitize, unique, unit_base_name};
use tungsten_interface::{DeviceCodegen, Instance, Interface, OrchestrationCodegen};
use crate::model::codegen::JavaCodeGenerator;
use crate::model as java;
use tungsten_ir::model::{HostInput, OptimizedProgram};
use tungsten_rust::orchestration::{AutoConstArg, permutation_table_specs};

use java::Expression as J;
use java::Statement as S;

/// Number of elements in the instance's buffer.
fn flat_len(inst: &Instance) -> usize {
    let (x, y, z) = inst.dimensions;
    (x * y * z) as usize
}

/// `public static final class HostConsts`: one public mutable field per static host input.
fn host_consts_class(program: &OptimizedProgram, interface: &Interface) -> java::Class {
    let mut fields = Vec::new();
    for (r, input) in program.host_inputs.iter() {
        if let HostInput::Static { t, .. } = input {
            fields.push(java::Field {
                modifiers: "public".into(),
                t: convert_type(t),
                name: interface.host_field(r).to_string(),
                init: None,
            });
        }
    }
    java::Class {
        doc: Some("Static host inputs, filled by the caller. Do not mutate during an orchestrate call.".into()),
        modifiers: "public static final".into(),
        name: HOST_CONSTS_CLASS.into(),
        fields,
        ..Default::default()
    }
}

/// `public static final class AutoConsts`: seed-derived permutation tables, immutable and shareable.
fn auto_consts_class(program: &OptimizedProgram, interface: &Interface) -> java::Class {
    let specs = permutation_table_specs(program, interface);
    let mut fields = Vec::new();
    let mut assignments = Vec::new();
    for ((_, table), spec) in program.permutation_tables.iter().zip(&specs) {
        fields.push(java::Field {
            modifiers: "public final".into(),
            t: auto_const_type(table),
            name: spec.field.clone(),
            init: None,
        });
        let args = spec
            .args
            .iter()
            .map(|a| match a {
                AutoConstArg::Seed => J::name("seed"),
                AutoConstArg::I64(v) => J::LongLit(*v),
            })
            .collect();
        assignments.push(S::Assign {
            target: J::field(J::name("this"), &spec.field),
            value: J::call(Some(J::name(HELPERS_CLASS)), &spec.initializer, args),
        });
    }
    let auto = java::Type::class(AUTO_CONSTS_CLASS);
    let constructor = java::Method {
        doc: None,
        modifiers: "private".into(),
        return_type: None,
        name: AUTO_CONSTS_CLASS.into(),
        params: vec![(java::Type::Long, "seed".into())],
        body: assignments,
    };
    let create = java::Method {
        doc: Some("Builds every permutation table for `seed`; the result may be shared across threads.".into()),
        modifiers: "public static".into(),
        return_type: Some(auto),
        name: "create".into(),
        params: vec![(java::Type::Long, "seed".into())],
        body: vec![S::Return(Some(J::New {
            class: AUTO_CONSTS_CLASS.into(),
            args: vec![J::name("seed")],
            comment: None,
        }))],
    };
    java::Class {
        doc: Some("Seed-derived constants. Immutable after construction and safe to share across threads.".into()),
        modifiers: "public static final".into(),
        name: AUTO_CONSTS_CLASS.into(),
        fields,
        methods: vec![constructor, create],
        ..Default::default()
    }
}

/// `public static final class InternalBuffers`: one array per instance, allocated once.
fn internal_buffers_class(program: &OptimizedProgram, interface: &Interface) -> java::Class {
    let fields = interface
        .instances()
        .iter()
        .map(|inst| {
            let element = convert_type(unit_return_type(program, inst.cu));
            java::Field {
                modifiers: "final".into(),
                t: java::Type::Array(Box::new(element.clone())),
                name: interface.buffer_field(inst).to_string(),
                init: Some(J::NewArraySized { element, len: flat_len(inst) }),
            }
        })
        .collect();
    java::Class {
        doc: Some("Output buffer of every scheduled instance.".into()),
        modifiers: "public static final".into(),
        name: INTERNAL_BUFFERS_CLASS.into(),
        fields,
        ..Default::default()
    }
}

/// `orchestrate_<entry>(x, y, z, host_consts)`: runs the entry's pruned waves and returns its buffer
/// (the internal array; overwritten by the next call) or, for a single value, the value itself.
fn entry_method(program: &OptimizedProgram, interface: &Interface, name: String, entry: &Instance) -> java::Method {
    let element = convert_type(unit_return_type(program, entry.cu));
    let single = flat_len(entry) == 1;
    let mut body = Vec::new();
    for (w, wave) in interface.waves_for(entry).iter().enumerate() {
        body.push(S::Comment(format!("wave {}", w)));
        for inst in wave {
            body.push(S::Expr(J::call(
                None,
                interface.kernel(inst),
                vec![
                    J::name("x"),
                    J::name("y"),
                    J::name("z"),
                    J::name(PARAM_BUFFERS),
                    J::name(PARAM_AUTO_CONSTS),
                    J::name(PARAM_HOST_CONSTS),
                ],
            )));
        }
    }
    let buffer = J::field(J::name(PARAM_BUFFERS), interface.buffer_field(entry));
    body.push(S::Return(Some(if single {
        J::Index { base: Box::new(buffer), index: Box::new(J::IntLit(0)) }
    } else {
        buffer
    })));
    java::Method {
        doc: Some(if single {
            "Computes the entry's single value at origin (x, y, z).".into()
        } else {
            "Computes the entry at origin (x, y, z). Returns the internal buffer: it is overwritten by\nthe next call on this instance, so copy it to keep it.".into()
        }),
        modifiers: "public".into(),
        return_type: Some(if single { element.clone() } else { java::Type::Array(Box::new(element)) }),
        name,
        params: vec![
            (java::Type::Double, "x".into()),
            (java::Type::Double, "y".into()),
            (java::Type::Double, "z".into()),
            (java::Type::class(HOST_CONSTS_CLASS), PARAM_HOST_CONSTS.into()),
        ],
        body,
    }
}

/// `public static final class Orchestration`: owns the buffers; one instance per thread.
fn orchestration_class(program: &OptimizedProgram, interface: &Interface, rules: &NamingRules) -> java::Class {
    let fields = vec![
        java::Field {
            modifiers: "private final".into(),
            t: java::Type::class(AUTO_CONSTS_CLASS),
            name: PARAM_AUTO_CONSTS.into(),
            init: None,
        },
        java::Field {
            modifiers: "private final".into(),
            t: java::Type::class(INTERNAL_BUFFERS_CLASS),
            name: PARAM_BUFFERS.into(),
            init: Some(J::New { class: INTERNAL_BUFFERS_CLASS.into(), args: vec![], comment: None }),
        },
    ];
    let mut methods = vec![java::Method {
        doc: None,
        modifiers: "public".into(),
        return_type: None,
        name: ORCHESTRATION_CLASS.into(),
        params: vec![(java::Type::class(AUTO_CONSTS_CLASS), PARAM_AUTO_CONSTS.into())],
        body: vec![S::Assign {
            target: J::field(J::name("this"), PARAM_AUTO_CONSTS),
            value: J::name(PARAM_AUTO_CONSTS),
        }],
    }];
    let mut used = rules.used_set();
    for entry in &program.entry_points {
        let entry = Instance::from_entry(entry);
        let name = unique(&mut used, format!("orchestrate_{}", unit_base_name(rules, program, entry.cu)));
        methods.push(entry_method(program, interface, name, &entry));
    }
    java::Class {
        doc: Some("Runs kernels in wave order. Not thread-safe: use one instance per thread.".into()),
        modifiers: "public static final".into(),
        name: ORCHESTRATION_CLASS.into(),
        fields,
        methods,
        ..Default::default()
    }
}

/// Orchestration-side generator: drives the device side and assembles the compilation unit.
pub struct TransformJavaOrchestration<'a> {
    device: JavaCodegen<'a>,
    package: String,
    class_name: String,
    runtime_package: String,
    unit: Option<java::CompilationUnit>,
    warnings: Vec<String>,
}

impl<'a> TransformJavaOrchestration<'a> {
    /// `device` must have been built for `class_name`, which should already be a legal Java
    /// identifier (see [`java_sanitize`]); an empty `package` emits no package declaration.
    pub fn new(device: JavaCodegen<'a>, package: &str, class_name: &str, runtime_package: &str) -> Self {
        let sanitized = java_sanitize(class_name);
        let mut warnings = Vec::new();
        if sanitized != class_name {
            warnings.push(format!("java: class name '{}' was sanitized to '{}'", class_name, sanitized));
        }
        Self {
            device,
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
}

impl<'a> OrchestrationCodegen<'a> for TransformJavaOrchestration<'a> {
    /// Generates the kernels on the device side, then the nested classes and the top-level class.
    /// Calling it again replaces the compilation unit.
    fn convert_program(&mut self, program: &'a OptimizedProgram, interface: &Interface) {
        // Device code first: one kernel per scheduled instance (repeats are no-ops).
        let waves = self.device.naming().waves().to_vec();
        for wave in &waves {
            for inst in wave {
                self.device.generate_kernel_of(vec![inst.cu]);
            }
        }

        let rules = self.device.rules();
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
        let class = java::Class {
            doc: None,
            modifiers: "public final".into(),
            name: self.class_name.clone(),
            fields: self.device.constant_fields(),
            methods,
            nested: vec![
                host_consts_class(program, interface),
                auto_consts_class(program, interface),
                internal_buffers_class(program, interface),
                orchestration_class(program, interface, rules),
            ],
            ..Default::default()
        };
        self.unit = Some(java::CompilationUnit {
            header: vec![
                format!("Generated by tungsten-wg {}. Do not edit.", env!("CARGO_PKG_VERSION")),
                "Threading: Orchestration is NOT thread-safe; use one instance per thread.".into(),
                "           AutoConsts is immutable and may be shared across threads.".into(),
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
