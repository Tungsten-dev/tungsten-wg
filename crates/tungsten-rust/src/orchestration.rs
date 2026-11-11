/*
Orchestration codegen for RCL (`specs/workers.md` §3.1, §5.2): the structs the device code is
generated against (`HostConsts`, `AutoConsts`, `InternalBuffers`), the shared `Orchestration`,
one worker and one `<Base>HostConsts` per entry, and the module assembly.

`Orchestration` holds the autoconsts behind an `Arc` and only creates workers. A worker runs its
entry's waves on its own pools, passing each kernel the offsets of its slots.
*/

use std::collections::HashSet;
use std::rc::Rc;

use super::types::{auto_const_type, convert_type};
use super::{
    AUTO_CONSTS_STRUCT_NAME, CONFIG_STRUCT_NAME, HOST_CONSTS_STRUCT_NAME,
    INTERNAL_BUFFERS_STRUCT_NAME, ORCHESTRATION_STRUCT_NAME, TransformRclOrchestration,
};
use crate::interned::Interned;
use crate::model as rcl;
use crate::model::codegen::RustCodeGenerator;
use crate::pools::{Pools, cells, unit_return_type, zero};
use tungsten_interface::naming::{FactoryStyle, NamingRules, WorkerNames, sanitize_name, worker_names};
use tungsten_interface::{DeviceCodegen, Instance, Interface, OrchestrationCodegen};
use tungsten_ir::model::{
    AutoConstArg, ComputeUnit, HostInput, OptimizedProgram, VariableType,
};

/// The private struct behind `Orchestration`'s `Arc`.
pub const SHARED_STRUCT_NAME: &str = "Shared";

/// Visibility of the implementation types: the density module (a sibling) uses them, the
/// frontend doesn't.
const INTERNAL: &str = "pub(super) ";

/// Names of every entry's worker types and factory, in entry order. Shared with frontends and
/// test drivers so they agree with the generated code.
pub fn rust_worker_names(program: &OptimizedProgram, rules: &NamingRules) -> Vec<WorkerNames> {
    let mut types: HashSet<String> = [
        ORCHESTRATION_STRUCT_NAME,
        SHARED_STRUCT_NAME,
        HOST_CONSTS_STRUCT_NAME,
        AUTO_CONSTS_STRUCT_NAME,
        INTERNAL_BUFFERS_STRUCT_NAME,
        CONFIG_STRUCT_NAME,
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
            types.insert(sanitize_name(n));
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
    let members = HashSet::from(["new".to_string(), "clone".to_string()]);
    worker_names(program, rules, FactoryStyle::Snake, types, members)
}

/// One field of `AutoConsts` and how to initialize it.
struct AutoConstSpec {
    field: String,
    t: rcl::Type,
    /// Name of the runtime function returning the value.
    initializer: String,
    args: Vec<(AutoConstArg, rcl::Type)>,
}

/// Params in declaration order, as (name, type).
fn auto_params(program: &OptimizedProgram, interface: &Interface) -> Vec<(String, rcl::Type)> {
    program
        .auto_params
        .iter()
        .map(|(r, p)| (interface.auto_param(r).to_string(), convert_type(&p.t)))
        .collect()
}

fn auto_const_specs(program: &OptimizedProgram, interface: &Interface) -> Vec<AutoConstSpec> {
    program
        .auto_consts
        .iter()
        .map(|(r, a)| AutoConstSpec {
            field: interface.auto_const_field(r).to_string(),
            t: auto_const_type(&a.t),
            initializer: a.initializer.clone(),
            args: a
                .args
                .iter()
                .map(|arg| match arg {
                    AutoConstArg::I64(v) => (AutoConstArg::I64(*v), rcl::Type::I64),
                    AutoConstArg::F64(v) => (AutoConstArg::F64(*v), rcl::Type::F64),
                    AutoConstArg::Bool(v) => (AutoConstArg::Bool(*v), rcl::Type::Bool),
                    AutoConstArg::Str(v) => (AutoConstArg::Str(v.clone()), rcl::Type::StaticStr),
                    AutoConstArg::Param(p) => {
                        (AutoConstArg::Param(*p), convert_type(&program.auto_params[*p].t))
                    }
                })
                .collect(),
        })
        .collect()
}

fn var(name: &str, t: rcl::Type) -> Rc<rcl::Variable> {
    Rc::new(rcl::Variable {
        name: Some(name.to_string()),
        t,
        mutable: false,
    })
}

fn struct_type(name: &str) -> rcl::Type {
    rcl::Type::Struct(name.to_string())
}

fn field<'m>(base: rcl::Expression<'m>, name: &str) -> rcl::Expression<'m> {
    rcl::Expression::Field { base: Box::new(base), field: name.to_string() }
}

/// `self.a.b..`
fn self_path<'m>(path: &[&str]) -> rcl::Expression<'m> {
    let this = rcl::Expression::Variable(var("self", struct_type("Self")));
    path.iter().fold(this, |e, f| field(e, f))
}

/// `(name, type)` of every static host, in arena order.
fn static_hosts<'p>(
    program: &'p OptimizedProgram,
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

/// A struct of host fields, and its generated `Default` impl from the zero values (`[T; N]` has
/// no `Default` past 32 elements).
fn host_struct<'m>(
    name: &str,
    hosts: &[(String, &VariableType)],
    visibility: &'static str,
) -> (rcl::Struct, rcl::Function<'m>) {
    let mut s = rcl::Struct::new(name.to_string());
    for (f, t) in hosts {
        s.add_field(f.clone(), convert_type(t));
    }
    s.visibility = visibility;
    s.set_repr_c(true);
    s.derivations.extend(["Clone".to_string(), "Copy".to_string()]);
    let mut default = rcl::Function::new(Some("default".into()), Some(struct_type("Self")));
    default.add_statement(rcl::Statement::Return(Some(rcl::Expression::StructInit {
        struct_name: "Self".into(),
        fields: hosts.iter().map(|(f, t)| (f.clone(), zero(t))).collect(),
    })));
    (s, default)
}

/// `#[repr(C)]` struct with one field per spec, built once by `Orchestration::new`.
fn auto_consts_struct(specs: &[AutoConstSpec]) -> rcl::Struct {
    let mut s = rcl::Struct::new(AUTO_CONSTS_STRUCT_NAME.to_string());
    for spec in specs {
        s.add_field(spec.field.clone(), spec.t.clone());
    }
    s.visibility = INTERNAL;
    s.set_repr_c(true);
    s.derivations.push("Clone".to_string());
    s
}

/// One `Box<[T]>` per pool, program-wide so the density code is shared.
fn internal_buffers_struct(pools: &Pools) -> rcl::Struct {
    let mut s = rcl::Struct::new(INTERNAL_BUFFERS_STRUCT_NAME.to_string());
    for (t, name) in &pools.fields {
        s.add_field(name.clone(), rcl::Type::Box(Box::new(rcl::Type::Slice(Box::new(convert_type(t))))));
    }
    s.visibility = INTERNAL;
    s
}

fn shared_struct() -> rcl::Struct {
    let mut s = rcl::Struct::new(SHARED_STRUCT_NAME.to_string());
    s.add_field("auto_constants".into(), struct_type(AUTO_CONSTS_STRUCT_NAME));
    s.visibility = "";
    s
}

const ARC_SHARED: &str = "std::sync::Arc<Shared>";

/// `#[derive(Clone)] pub struct Orchestration { shared: Arc<Shared> }`: cloning is an `Arc` bump.
fn orchestration_struct() -> rcl::Struct {
    let mut s = rcl::Struct::new(ORCHESTRATION_STRUCT_NAME.to_string());
    s.add_field("shared".into(), struct_type(ARC_SHARED));
    s.public_fields = false;
    s.derivations.push("Clone".to_string());
    s
}

fn worker_struct(names: &WorkerNames) -> rcl::Struct {
    let mut s = rcl::Struct::new(names.worker.clone());
    s.add_field("orch".into(), struct_type(ORCHESTRATION_STRUCT_NAME));
    s.add_field("buffers".into(), struct_type(INTERNAL_BUFFERS_STRUCT_NAME));
    s.add_field("hosts".into(), struct_type(HOST_CONSTS_STRUCT_NAME));
    s.public_fields = false;
    s
}

/// `Orchestration::new(<params>)`: builds the autoconsts as one struct literal, each initializer
/// called in arena order.
fn new_fn<'m>(
    program: &OptimizedProgram,
    interface: &Interface,
    specs: &[AutoConstSpec],
) -> rcl::Function<'m> {
    use rcl::Expression as E;
    let mut f = rcl::Function::new(Some("new".to_string()), Some(struct_type("Self")));
    let params = auto_params(program, interface);
    for (name, t) in &params {
        f.add_parameter(name.clone(), t.clone());
    }
    let param_vars: std::collections::HashMap<_, _> = program
        .auto_params
        .iter()
        .zip(&params)
        .map(|((r, _), (name, t))| (r, var(name, t.clone())))
        .collect();
    let fields = specs
        .iter()
        .map(|spec| {
            let arguments = spec
                .args
                .iter()
                .map(|(a, _)| match a {
                    AutoConstArg::Param(p) => E::Variable(param_vars[p].clone()),
                    AutoConstArg::I64(v) => E::I64Literal(*v),
                    AutoConstArg::F64(v) => E::F64Literal(*v),
                    AutoConstArg::Bool(v) => E::BoolLiteral(*v),
                    AutoConstArg::Str(v) => E::StrLiteral(v.clone()),
                })
                .collect();
            let value = E::LateBoundCall {
                function_name: spec.initializer.clone(),
                argument_types: spec.args.iter().map(|(_, t)| t.clone()).collect(),
                return_type: spec.t.clone(),
                arguments,
            };
            (spec.field.clone(), value)
        })
        .collect();
    let shared = E::StructInit {
        struct_name: SHARED_STRUCT_NAME.into(),
        fields: vec![(
            "auto_constants".into(),
            E::StructInit { struct_name: AUTO_CONSTS_STRUCT_NAME.into(), fields },
        )],
    };
    f.add_statement(rcl::Statement::Return(Some(E::StructInit {
        struct_name: "Self".to_string(),
        fields: vec![(
            "shared".into(),
            E::LateBoundCall {
                function_name: "std::sync::Arc::new".into(),
                argument_types: vec![struct_type(SHARED_STRUCT_NAME)],
                return_type: struct_type(ARC_SHARED),
                arguments: vec![shared],
            },
        )],
    })));
    f
}

/// `<factory>(&self) -> <Base>Worker`: one allocation per pool, sized for this entry. Pools the
/// worker doesn't use are empty.
fn factory_fn<'m>(
    interface: &Interface,
    pools: &Pools,
    entry: usize,
    names: &WorkerNames,
) -> rcl::Function<'m> {
    use rcl::Expression as E;
    let layout = interface.worker(entry);
    let mut f = rcl::Function::new(Some(names.factory.clone()), Some(struct_type(&names.worker)));
    f.receiver = Some(rcl::Receiver::Ref);
    let buffers = pools
        .fields
        .iter()
        .map(|(t, name)| {
            let len = layout.pools.iter().find(|p| p.element == *t).map_or(0, |p| p.len);
            let value = E::MethodCall {
                receiver: Box::new(E::VecRepeat {
                    element: Box::new(zero(t)),
                    count: Box::new(E::UsizeLiteral(len)),
                }),
                method: "into_boxed_slice".into(),
                arguments: vec![],
            };
            (name.clone(), value)
        })
        .collect();
    let default_hosts = E::LateBoundCall {
        function_name: format!("<{HOST_CONSTS_STRUCT_NAME} as Default>::default"),
        argument_types: vec![],
        return_type: struct_type(HOST_CONSTS_STRUCT_NAME),
        arguments: vec![],
    };
    let this = E::Variable(var("self", struct_type("Self")));
    f.add_statement(rcl::Statement::Return(Some(E::StructInit {
        struct_name: names.worker.clone(),
        fields: vec![
            (
                "orch".into(),
                E::MethodCall { receiver: Box::new(this), method: "clone".into(), arguments: vec![] },
            ),
            (
                "buffers".into(),
                E::StructInit { struct_name: INTERNAL_BUFFERS_STRUCT_NAME.into(), fields: buffers },
            ),
            ("hosts".into(), default_hosts),
        ],
    })));
    f
}

/// `run(&mut self, origin, host_consts: &<Base>HostConsts)`: copies the entry's hosts into the
/// private `HostConsts`, runs the worker's waves and returns a view of the entry's slot, which is
/// at offset 0 of pool 0. A single-cell entry returns the value.
///
/// Calls are `let _kernel_call_<wave>_<i>: () = ..` because RCL has no expression statements.
fn run_fn<'m>(
    program: &OptimizedProgram,
    interface: &Interface,
    pools: &Pools,
    entry: usize,
    names: &WorkerNames,
) -> rcl::Function<'m> {
    use rcl::Expression as E;
    use rcl::Statement as S;
    let target = Instance::from_entry(&program.entry_points[entry]);
    let element_ir = unit_return_type(program, target.cu);
    let element = convert_type(element_ir);
    let len = cells(target.dimensions);
    let pool = pools.field(element_ir);

    let vec3 = struct_type("Vec3");
    let hosts_t = rcl::Type::Ref(Box::new(struct_type(&names.host_consts)));
    let single = len == 1;
    let return_type = if single {
        element.clone()
    } else {
        rcl::Type::ArrayRef(Box::new(element.clone()), len)
    };
    let mut f = rcl::Function::new(Some("run".into()), Some(return_type.clone()));
    f.receiver = Some(rcl::Receiver::MutRef);
    f.add_parameter("origin".to_string(), vec3.clone());
    f.add_parameter("host_consts".to_string(), hosts_t.clone());
    let origin = var("origin", vec3);
    let host_consts = var("host_consts", hosts_t);

    for (name, _) in static_hosts(program, interface, Some(interface.entry_hosts(entry))) {
        f.add_statement(S::AssignPlace {
            target: self_path(&["hosts", name.as_str()]),
            value: field(E::Variable(host_consts.clone()), &name),
        });
    }

    let slot = |inst: &Instance| {
        interface
            .slot(entry, inst)
            .unwrap_or_else(|| panic!("entry {entry}'s worker has no slot for {inst:?}"))
    };
    for (w, wave) in interface.worker_schedule(entry).iter().enumerate() {
        for (i, inst) in wave.iter().enumerate() {
            let ComputeUnit::DensityFunction { function, .. } = &program.compute_units[inst.cu];
            let offsets = interface.input_params(*function).iter().map(|&d| {
                let input = Instance::from_input(&program.density_inputs[d]);
                E::UsizeLiteral(slot(&input).offset)
            });
            let arguments = [
                E::Variable(origin.clone()),
                E::MutRef(Box::new(self_path(&["buffers"]))),
                E::UsizeLiteral(slot(inst).offset),
            ]
            .into_iter()
            .chain(offsets)
            .chain([
                E::Ref(Box::new(self_path(&["orch", "shared", "auto_constants"]))),
                E::Ref(Box::new(self_path(&["hosts"]))),
            ])
            .collect();
            f.add_statement(S::Declare {
                variable: var(&format!("_kernel_call_{}_{}", w, i), rcl::Type::Void),
                init: Some(E::LateBoundCall {
                    function_name: interface.kernel(inst).to_string(),
                    argument_types: vec![],
                    return_type: rcl::Type::Void,
                    arguments,
                }),
                mutable: false,
            });
        }
    }

    debug_assert_eq!(slot(&target).offset, 0);
    let buffer = self_path(&["buffers", pool]);
    f.add_statement(S::Return(Some(if single {
        E::Index { base: Box::new(buffer), index: Box::new(E::UsizeLiteral(0)) }
    } else {
        E::InlineRust(format!("<{return_type}>::try_from(&self.buffers.{pool}[0..{len}]).unwrap()"))
    })));
    f
}

/// `const _: () = ..`: a runtime handle or record that isn't `Send + Sync` fails here, at a line
/// naming `Orchestration`, instead of at a distant `thread::spawn`.
fn static_assertion(names: &[WorkerNames]) -> String {
    let workers: String = names.iter().map(|n| format!(" w::<{}>();", n.worker)).collect();
    format!(
        "const _: () = {{ fn s<T: Send + Sync>() {{}} fn w<T: Send>() {{}} fn a() {{ s::<{ORCHESTRATION_STRUCT_NAME}>();{workers} }} }};"
    )
}

impl<'m> TransformRclOrchestration<'m> {
    /// `density_module_name` and `orchestration_module_name` name the two generated modules,
    /// which import each other. `pools` must be the ones the device codegen was built with.
    pub fn new(
        device_codegen: Box<dyn DeviceCodegen + 'm>,
        arena: &'m bumpalo::Bump,
        pools: Pools,
        density_module_name: &str,
        orchestration_module_name: &str,
    ) -> Self {
        Self {
            device_codegen,
            arena,
            pools,
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

    fn default_impl(&self, type_name: &str, default: rcl::Function<'m>) -> rcl::Impl<'m> {
        let mut imp = rcl::Impl::new(type_name);
        imp.trait_name = Some("Default".into());
        imp.methods.push(self.intern(default));
        imp
    }
}

impl<'a, 'm> OrchestrationCodegen<'a> for TransformRclOrchestration<'m> {
    /// Generates the kernels on the device side, then the structs, `Orchestration` and the
    /// workers. Calling it again replaces the orchestration model. `interface` must be the one the
    /// device codegen was built with.
    fn convert_program(&mut self, program: &'a OptimizedProgram, interface: &Interface) {
        // Device code first: one kernel per scheduled instance (repeats are no-ops).
        for wave in interface.waves() {
            for inst in wave {
                self.device_codegen.generate_kernel_of(vec![inst.cu]);
            }
        }

        self.model = rcl::RCL::new();
        self.model
            .add_import(format!("super::{}::*", self.density_module_name));

        let names = rust_worker_names(program, &NamingRules::rust());
        let specs = auto_const_specs(program, interface);
        let (hosts, hosts_default) =
            host_struct(HOST_CONSTS_STRUCT_NAME, &static_hosts(program, interface, None), INTERNAL);
        let mut structs = vec![
            hosts,
            auto_consts_struct(&specs),
            internal_buffers_struct(&self.pools),
            shared_struct(),
            orchestration_struct(),
        ];
        let mut impls = vec![self.default_impl(HOST_CONSTS_STRUCT_NAME, hosts_default)];
        for (e, n) in names.iter().enumerate() {
            let own = static_hosts(program, interface, Some(interface.entry_hosts(e)));
            let (s, default) = host_struct(&n.host_consts, &own, "pub ");
            structs.push(s);
            impls.push(self.default_impl(&n.host_consts, default));
            structs.push(worker_struct(n));
        }
        for s in structs {
            let s = self.struct_ref(s);
            self.model.structs.push(s);
        }

        let mut orch = rcl::Impl::new(ORCHESTRATION_STRUCT_NAME);
        orch.methods.push(self.intern(new_fn(program, interface, &specs)));
        for (e, n) in names.iter().enumerate() {
            orch.methods.push(self.intern(factory_fn(interface, &self.pools, e, n)));
        }
        self.model.impls.push(orch);
        for (e, n) in names.iter().enumerate() {
            let target = &program.entry_points[e];
            let (x, y, z) = target.dimensions;
            let mut worker = rcl::Impl::new(n.worker.clone());
            worker.consts.push((
                "LEN".into(),
                rcl::Type::Usize,
                rcl::Expression::UsizeLiteral(cells(target.dimensions)),
            ));
            worker.consts.push((
                "DIMS".into(),
                rcl::Type::Tuple(vec![rcl::Type::I32; 3]),
                rcl::Expression::TupleInit(vec![
                    rcl::Expression::I32Literal(x),
                    rcl::Expression::I32Literal(y),
                    rcl::Expression::I32Literal(z),
                ]),
            ));
            worker.methods.push(self.intern(run_fn(program, interface, &self.pools, e, n)));
            self.model.impls.push(worker);
        }
        self.model.impls.extend(impls);
        self.model.raw_items.push(static_assertion(&names));
    }

    /// Density module followed by the orchestration module.
    fn output(&self) -> String {
        self.density_module() + "\n\n" + &self.orchestration_module()
    }
}
