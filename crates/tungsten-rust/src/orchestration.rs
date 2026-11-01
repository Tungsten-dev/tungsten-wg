/*
Orchestration codegen for RCL: owns the structs the device code is generated against
(`HostConsts`, `AutoConsts`, `InternalBuffers`), the `Orchestration` type that runs kernels
in wave order, and the module assembly.
*/

use std::collections::HashSet;
use std::rc::Rc;

use super::types::{auto_const_type, convert_type};
use super::{
    AUTO_CONSTS_STRUCT_NAME, HOST_CONSTS_STRUCT_NAME, INTERNAL_BUFFERS_STRUCT_NAME,
    ORCHESTRATION_STRUCT_NAME, TransformRclOrchestration,
};
use crate::interned::Interned;
use crate::model as rcl;
use crate::model::codegen::RustCodeGenerator;
use crate::random::xoroshiro_seed;
use tungsten_interface::naming::{NamingRules, unique, unit_base_name};
use tungsten_interface::{DeviceCodegen, Instance, Interface, OrchestrationCodegen};
use tungsten_ir::model::{
    ComputeUnit, HostInput, OptimizedProgram, PermutationTableInput, Ref, VariableType,
};

/// An argument of an auto-const initializer.
#[derive(Debug, Clone, PartialEq)]
pub enum AutoConstArg {
    /// The world seed passed to `Orchestration::new`.
    Seed,
    I64(i64),
}

/// One field of `AutoConsts` and how to initialize it. This is the extension point:
/// a model can later supply its own specs (fields, initializer functions, parameters).
#[derive(Debug, Clone, PartialEq)]
pub struct AutoConstSpec {
    pub field: String,
    pub t: rcl::Type,
    /// Name of the runtime function returning the value.
    pub initializer: String,
    pub args: Vec<AutoConstArg>,
}

/// Specs derived from the program's permutation tables.
pub fn permutation_table_specs(
    program: &OptimizedProgram,
    interface: &Interface,
) -> Vec<AutoConstSpec> {
    program
        .permutation_tables
        .iter()
        .map(|(r, table)| {
            let (initializer, args) = match table {
                PermutationTableInput::PerlinNoise {
                    ident,
                    subident,
                    subident_index,
                } => {
                    let id = xoroshiro_seed(ident);
                    let sub = subident.as_deref().map(xoroshiro_seed).unwrap_or((0, 0));
                    (
                        "make_permutation_table",
                        vec![
                            AutoConstArg::Seed,
                            AutoConstArg::I64(id.0 as i64),
                            AutoConstArg::I64(id.1 as i64),
                            AutoConstArg::I64(*subident_index as i64),
                            AutoConstArg::I64(sub.0 as i64),
                            AutoConstArg::I64(sub.1 as i64),
                        ],
                    )
                }
                PermutationTableInput::Base3DNoise => {
                    ("make_base3d_perm_table", vec![AutoConstArg::Seed])
                }
            };
            AutoConstSpec {
                field: interface.auto_const_field(r).to_string(),
                t: auto_const_type(table),
                initializer: initializer.to_string(),
                args,
            }
        })
        .collect()
}

/// Element type of an instance's buffer is its unit's return type.
fn unit_return_type(program: &OptimizedProgram, cu: Ref<ComputeUnit>) -> &VariableType {
    let ComputeUnit::DensityFunction { function, .. } = &program.compute_units[cu];
    &program.functions[*function].return_type
}

/// Number of elements in the instance's buffer.
fn flat_len(inst: &Instance) -> usize {
    let (x, y, z) = inst.dimensions;
    (x * y * z) as usize
}

/// `#[repr(C)]` struct of all static host inputs, passed to every function as `host_consts`.
fn host_consts_struct(program: &OptimizedProgram, interface: &Interface) -> rcl::Struct {
    let mut s = rcl::Struct::new(HOST_CONSTS_STRUCT_NAME.to_string());
    for (r, input) in program.host_inputs.iter() {
        if let HostInput::Static { t, .. } = input {
            s.add_field(interface.host_field(r).to_string(), convert_type(t));
        }
    }
    s.set_repr_c(true);
    s
}

/// `#[repr(C)]` struct with one field per spec, passed to every function as `auto_constants`.
fn auto_consts_struct(specs: &[AutoConstSpec]) -> rcl::Struct {
    let mut s = rcl::Struct::new(AUTO_CONSTS_STRUCT_NAME.to_string());
    for spec in specs {
        s.add_field(spec.field.clone(), spec.t.clone());
    }
    s.set_repr_c(true);
    s.derivations.push("Clone".to_string());
    s
}

/// `#[repr(C)]` struct holding every instance's output buffer; zero-initialized via `make_boxed`.
fn internal_buffers_struct(program: &OptimizedProgram, interface: &Interface) -> rcl::Struct {
    let mut s = rcl::Struct::new(INTERNAL_BUFFERS_STRUCT_NAME.to_string());
    for inst in interface.instances() {
        let element = convert_type(unit_return_type(program, inst.cu));
        s.add_field(
            interface.buffer_field(inst).to_string(),
            rcl::Type::Array(Box::new(element), flat_len(inst)),
        );
    }
    s.derivations.push("Clone".to_string());
    s.derivations.push("Copy".to_string());
    s.set_repr_c(true);
    s
}

/// The `Orchestration` type: owns the buffers and auto constants across runs.
fn orchestration_struct() -> rcl::Struct {
    let mut s = rcl::Struct::new(ORCHESTRATION_STRUCT_NAME.to_string());
    s.add_field(
        "buffers".to_string(),
        rcl::Type::Box(Box::new(rcl::Type::Struct(
            INTERNAL_BUFFERS_STRUCT_NAME.to_string(),
        ))),
    );
    s.add_field(
        "auto_constants".to_string(),
        rcl::Type::Box(Box::new(rcl::Type::Struct(
            AUTO_CONSTS_STRUCT_NAME.to_string(),
        ))),
    );
    s.derivations.push("Clone".to_string());
    s
}

fn var(name: &str, t: rcl::Type) -> Rc<rcl::Variable> {
    Rc::new(rcl::Variable {
        name: Some(name.to_string()),
        t,
        mutable: false,
    })
}

/// `fn make_auto_consts(seed, &mut AutoConsts)`: assigns every field from its spec's initializer.
fn make_auto_consts_fn<'m>(specs: &[AutoConstSpec]) -> rcl::Function<'m> {
    use rcl::Expression as E;
    let mut f = rcl::Function::new(Some("make_auto_consts".to_string()), None);
    let target_type = rcl::Type::MutRef(Box::new(rcl::Type::Struct(
        AUTO_CONSTS_STRUCT_NAME.to_string(),
    )));
    f.add_parameter("seed".to_string(), rcl::Type::I64);
    f.add_parameter("auto_consts".to_string(), target_type.clone());
    let seed = var("seed", rcl::Type::I64);
    let auto_consts = var("auto_consts", target_type);

    for spec in specs {
        let arguments = spec
            .args
            .iter()
            .map(|a| match a {
                AutoConstArg::Seed => E::Variable(seed.clone()),
                AutoConstArg::I64(v) => E::I64Literal(*v),
            })
            .collect::<Vec<_>>();
        f.add_statement(rcl::Statement::AssignPlace {
            target: E::Field {
                base: Box::new(E::Variable(auto_consts.clone())),
                field: spec.field.clone(),
            },
            value: E::LateBoundCall {
                function_name: spec.initializer.clone(),
                argument_types: vec![rcl::Type::I64; arguments.len()],
                return_type: spec.t.clone(),
                arguments,
            },
        });
    }
    f
}

fn boxed_struct(name: &str) -> rcl::Type {
    rcl::Type::Box(Box::new(rcl::Type::Struct(name.to_string())))
}

/// `Orchestration::new(seed)`: allocates the boxed buffers and auto constants and initializes the latter.
fn new_fn<'m>() -> rcl::Function<'m> {
    use rcl::Expression as E;
    use rcl::Statement as S;
    let mut f = rcl::Function::new(
        Some("new".to_string()),
        Some(rcl::Type::Struct("Self".to_string())),
    );
    f.add_parameter("seed".to_string(), rcl::Type::I64);
    let seed = var("seed", rcl::Type::I64);
    let buffers = var("buffers", boxed_struct(INTERNAL_BUFFERS_STRUCT_NAME));
    let auto_constants = var("auto_constants", boxed_struct(AUTO_CONSTS_STRUCT_NAME));
    let init = var("_init", rcl::Type::Void);

    let make_boxed = |name: &str| E::LateBoundCall {
        function_name: format!("make_boxed::<{}>", name),
        argument_types: vec![],
        return_type: boxed_struct(name),
        arguments: vec![],
    };
    f.add_statement(S::Declare {
        variable: buffers.clone(),
        init: Some(make_boxed(INTERNAL_BUFFERS_STRUCT_NAME)),
        mutable: true,
    });
    f.add_statement(S::Declare {
        variable: auto_constants.clone(),
        init: Some(make_boxed(AUTO_CONSTS_STRUCT_NAME)),
        mutable: true,
    });
    f.add_statement(S::Declare {
        variable: init,
        init: Some(E::LateBoundCall {
            function_name: "make_auto_consts".to_string(),
            argument_types: vec![rcl::Type::I64],
            return_type: rcl::Type::Void,
            arguments: vec![
                E::Variable(seed),
                E::MutRef(Box::new(E::Variable(auto_constants.clone()))),
            ],
        }),
        mutable: false,
    });
    f.add_statement(S::Return(Some(E::StructInit {
        struct_name: "Self".to_string(),
        fields: vec![
            ("buffers".to_string(), E::Variable(buffers)),
            ("auto_constants".to_string(), E::Variable(auto_constants)),
        ],
    })));
    f
}

/// Calls are `let _kernel_call_<wave>_<i>: () = ..` because RCL has no expression statements.
/// `orchestrate_<entry>(&mut self, origin, host_consts)`: runs the entry's pruned waves and returns its buffer.
fn entry_method<'m>(
    program: &OptimizedProgram,
    interface: &Interface,
    name: String,
    entry: &Instance,
) -> rcl::Function<'m> {
    use rcl::Expression as E;
    use rcl::Statement as S;
    let vec3 = rcl::Type::Struct("Vec3".to_string());
    let host_consts_t = rcl::Type::Ref(Box::new(rcl::Type::Struct(
        HOST_CONSTS_STRUCT_NAME.to_string(),
    )));
    let element = convert_type(unit_return_type(program, entry.cu));

    let single = flat_len(entry) == 1;
    let return_type = if single {
        element.clone()
    } else {
        rcl::Type::ArrayRef(Box::new(element.clone()), flat_len(entry))
    };
    let mut f = rcl::Function::new(Some(name), Some(return_type));
    f.receiver = Some(rcl::Receiver::MutRef);
    f.add_parameter("origin".to_string(), vec3.clone());
    f.add_parameter("host_consts".to_string(), host_consts_t.clone());

    let this = var("self", rcl::Type::Struct("Self".to_string()));
    let origin = var("origin", vec3);
    let host_consts = var("host_consts", host_consts_t);
    let self_field = |field: &str| E::Field {
        base: Box::new(E::Variable(this.clone())),
        field: field.to_string(),
    };

    for (w, wave) in interface.waves_for(entry).iter().enumerate() {
        for (i, inst) in wave.iter().enumerate() {
            f.add_statement(S::Declare {
                variable: var(&format!("_kernel_call_{}_{}", w, i), rcl::Type::Void),
                init: Some(E::LateBoundCall {
                    function_name: interface.kernel(inst).to_string(),
                    argument_types: vec![],
                    return_type: rcl::Type::Void,
                    arguments: vec![
                        E::Variable(origin.clone()),
                        E::MutRef(Box::new(E::Deref(Box::new(self_field("buffers"))))),
                        E::Ref(Box::new(E::Deref(Box::new(self_field("auto_constants"))))),
                        E::Variable(host_consts.clone()),
                    ],
                }),
                mutable: false,
            });
        }
    }

    let buffer = E::Field {
        base: Box::new(self_field("buffers")),
        field: interface.buffer_field(entry).to_string(),
    };
    f.add_statement(S::Return(Some(if single {
        E::Index {
            base: Box::new(buffer),
            index: Box::new(E::I32Literal(0)),
        }
    } else {
        E::Ref(Box::new(buffer))
    })));
    f
}

impl<'m> TransformRclOrchestration<'m> {
    /// `density_module_name` and `orchestration_module_name` name the two generated modules,
    /// which import each other.
    pub fn new(
        device_codegen: Box<dyn DeviceCodegen + 'm>,
        arena: &'m bumpalo::Bump,
        density_module_name: &str,
        orchestration_module_name: &str,
    ) -> Self {
        Self {
            device_codegen,
            arena,
            model: rcl::RCL::new(),
            density_module_name: density_module_name.to_string(),
            orchestration_module_name: orchestration_module_name.to_string(),
        }
    }

    fn intern(&self, f: rcl::Function<'m>) -> rcl::FunctionRef<'m> {
        Interned::new(self.arena.alloc(f))
    }

    fn struct_ref(&self, s: rcl::Struct) -> rcl::StructRef<'m> {
        Interned::new(self.arena.alloc(s))
    }

    /// The density module source, importing the orchestration module.
    pub fn density_module(&self) -> String {
        self.device_codegen.render(
            &self.density_module_name,
            &[format!("super::{}::*", self.orchestration_module_name)],
        )
    }

    /// The orchestration module source, importing the density module. Call after `convert_program`.
    pub fn orchestration_module(&self) -> String {
        RustCodeGenerator::new()
            .generate_inline_module(&self.model, &self.orchestration_module_name)
    }
}

impl<'a, 'm> OrchestrationCodegen<'a> for TransformRclOrchestration<'m> {
    /// Generates the kernels on the device side, then the structs and `Orchestration` impl.
    /// Calling it again replaces the orchestration model. `interface` must be the one the
    /// device codegen was built with.
    fn convert_program(&mut self, program: &'a OptimizedProgram, interface: &Interface) {
        let naming = NamingRules::rust();
        // Device code first: one kernel per scheduled instance (repeats are no-ops).
        for wave in interface.waves() {
            for inst in wave {
                self.device_codegen.generate_kernel_of(vec![inst.cu]);
            }
        }

        self.model = rcl::RCL::new();
        self.model
            .add_import(format!("super::{}::*", self.density_module_name));

        let specs = permutation_table_specs(program, interface);
        for s in [
            host_consts_struct(program, interface),
            auto_consts_struct(&specs),
            internal_buffers_struct(program, interface),
            orchestration_struct(),
        ] {
            let s = self.struct_ref(s);
            self.model.structs.push(s);
        }

        let init = self.intern(make_auto_consts_fn(&specs));
        self.model.functions.push(init);

        let mut used = HashSet::from(["new".to_string()]);
        let mut methods = vec![self.intern(new_fn())];
        for entry in &program.entry_points {
            let entry = Instance::from_entry(entry);
            let name = unique(
                &mut used,
                format!("orchestrate_{}", unit_base_name(&naming, program, entry.cu)),
            );
            let method = entry_method(program, interface, name, &entry);
            methods.push(self.intern(method));
        }
        self.model.impls.push(rcl::Impl {
            type_name: ORCHESTRATION_STRUCT_NAME.to_string(),
            methods,
        });
    }

    /// Density module followed by the orchestration module.
    fn output(&self) -> String {
        self.density_module() + "\n\n" + &self.orchestration_module()
    }
}
