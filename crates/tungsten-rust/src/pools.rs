/*
Program-wide facts the device and orchestration sides both need (`specs/workers.md` §5.2): the
fields of `InternalBuffers`, one `Box<[T]>` per pool element type, and the generated zero value of
each type, which replaces `make_boxed`'s zeroed memory.
*/

use std::collections::HashSet;

use tungsten_interface::naming::{NamingRules, sanitize_name, unique};
use tungsten_interface::{BufferBounds, Interface};
use tungsten_ir::model::{ComputeUnit, OptimizedProgram, Ref, VariableType};

use crate::model as rcl;
use crate::types::convert_type;

/// The pools of `InternalBuffers`, and how reads of them are checked.
#[derive(Debug, Clone)]
pub struct Pools {
    /// (element type, field name), in first-use order over the entries' workers.
    pub fields: Vec<(VariableType, String)>,
    pub bounds: BufferBounds,
}

impl Pools {
    /// One field per element type used by any worker or read by any density input, so dead code
    /// still compiles.
    pub fn new(program: &OptimizedProgram, interface: &Interface, bounds: BufferBounds) -> Self {
        let mut types: Vec<VariableType> = vec![];
        let mut add = |t: &VariableType| {
            if !types.contains(t) {
                types.push(t.clone());
            }
        };
        for e in 0..program.entry_points.len() {
            for pool in &interface.worker(e).pools {
                add(&pool.element);
            }
        }
        for (_, d) in program.density_inputs.iter() {
            add(unit_return_type(program, d.density_function));
        }
        let mut used: HashSet<String> = NamingRules::rust().used_set();
        let fields = types
            .into_iter()
            .map(|t| {
                let name = unique(&mut used, format!("{}s", type_stem(&t)));
                (t, name)
            })
            .collect();
        Self { fields, bounds }
    }

    /// The `InternalBuffers` field holding elements of type `t`.
    pub fn field(&self, t: &VariableType) -> &str {
        self.fields
            .iter()
            .find(|(e, _)| e == t)
            .map(|(_, n)| n.as_str())
            .unwrap_or_else(|| panic!("no pool for element type {t:?}"))
    }
}

/// Element type of an instance's buffer: its unit's return type.
pub fn unit_return_type(program: &OptimizedProgram, cu: Ref<ComputeUnit>) -> &VariableType {
    let ComputeUnit::DensityFunction { function, .. } = &program.compute_units[cu];
    &program.functions[*function].return_type
}

/// `f64`, `vec3`, `pair`, `f64x33`: a lower-case stem for a pool field name.
pub fn type_stem(t: &VariableType) -> String {
    match t {
        VariableType::Vec3 => "vec3".into(),
        VariableType::Pos3 => "pos3".into(),
        VariableType::F32 => "f32".into(),
        VariableType::F64 => "f64".into(),
        VariableType::I8 => "i8".into(),
        VariableType::I32 => "i32".into(),
        VariableType::I64 => "i64".into(),
        VariableType::Bool => "bool".into(),
        VariableType::Extern(n) | VariableType::Handle(n) => sanitize_name(n).to_ascii_lowercase(),
        VariableType::Array(e, n) => format!("{}x{n}", type_stem(e)),
    }
}

/// The generated zero value of `t`. A record's is `R::default()`: the runtime contract makes
/// every record used as a unit output or host `Copy + Default`.
pub fn zero<'m>(t: &VariableType) -> rcl::Expression<'m> {
    use rcl::Expression as E;
    let new = |name: &str, t: rcl::Type, args: Vec<E<'m>>| E::LateBoundCall {
        function_name: format!("{name}::new"),
        argument_types: vec![t; args.len()],
        return_type: rcl::Type::Struct(name.into()),
        arguments: args,
    };
    match t {
        VariableType::F32 => E::F32Literal(0.0),
        VariableType::F64 => E::F64Literal(0.0),
        VariableType::I8 => E::I8Literal(0),
        VariableType::I32 => E::I32Literal(0),
        VariableType::I64 => E::I64Literal(0),
        VariableType::Bool => E::BoolLiteral(false),
        VariableType::Vec3 => new("Vec3", rcl::Type::F64, vec![E::F64Literal(0.0); 3]),
        VariableType::Pos3 => new("Pos3", rcl::Type::I32, vec![E::I32Literal(0); 3]),
        VariableType::Array(e, n) => E::ArrayInit {
            element_type: convert_type(e),
            element: Box::new(zero(e)),
            count: *n,
        },
        // Through the trait, so a record without `Default` fails with an unsatisfied bound.
        VariableType::Extern(_) => E::LateBoundCall {
            function_name: format!("<{} as Default>::default", convert_type(t)),
            argument_types: vec![],
            return_type: convert_type(t),
            arguments: vec![],
        },
        VariableType::Handle(n) => panic!("a handle ({n}) has no zero value"),
    }
}

/// Number of cells of a grid. Dims below 1 occur on the unchecked path, and count as empty.
pub fn cells((x, y, z): (i32, i32, i32)) -> usize {
    [x, y, z].into_iter().fold(1, |n: usize, d| n.saturating_mul(d.max(0) as usize))
}
