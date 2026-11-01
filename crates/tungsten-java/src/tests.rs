//! Smoke test: a small hand-built program exercising the Java-specific lowerings.
//! Set `TUNGSTEN_JAVA_OUT=<dir>` to also write the generated Java (and RCL) there, e.g. to run javac on it.

use bumpalo::Bump;

use tungsten_interface::{Backend, Interface};
use tungsten_legacy::builder::{Builder, SPMTBuilder, floor, sin};
use tungsten_legacy::conversion::convert;
use tungsten_legacy::model::*;
use tungsten_rust::backend::{RustBackend, RustConfig};

use crate::backend::{JavaBackend, JavaConfig};

fn var<'m>(v: Var<'m>) -> Expression<'m> {
    Expression::Variable(v)
}

fn build_program<'m>(arena: &'m Bump) -> SPMT<'m> {
    // Helper `int` (a Java keyword) with a parameter and a loop with a break.
    let mut h = Builder::new(arena).named("int").returns(VariableType::F64);
    let t = h.param("t", VariableType::F64);
    let acc = h.local("acc", VariableType::F64);
    h.set_body(vec![
        Statement::Assign { target: acc, value: 0.0f64.into() },
        Statement::Repeat {
            count: 3,
            body: vec![
                Statement::Assign { target: acc, value: acc + t },
                Statement::If { condition: acc.gt(10.0f64), then_branch: vec![Statement::Break], else_branch: vec![] },
            ],
        },
        Statement::Return(var(acc)),
    ]);
    let helper = h.finish_function();

    // A 1x1x1 unit reading a host input.
    let mut s = Builder::new(arena).named("minecraft:scalar").returns(VariableType::I32);
    let sea = s.alloc_var(Name::Named("sea_level".into()), VariableType::I32);
    let flag = s.local("flag", VariableType::Bool);
    s.set_body(vec![
        Statement::Assign { target: flag, value: var(sea).gt(62i32) },
        // i32 + bool, `!` on an integer, logical shift.
        Statement::Return(((var(sea) + var(flag)).bitxor(!var(sea))).ushr(1i32)),
    ]);
    let mut scalar = s.build_compute_unit();
    scalar.host_inputs = vec![HostInput::Static(sea)];
    let scalar: DensityFunctionRef = Interned::new(arena.alloc(scalar));

    // A 4x4x4 noise unit: constants (one depending on another), permutation table, pos3 binding.
    let n = Builder::new(arena).named("minecraft:noise").returns(VariableType::F64);
    let base = n.constant("base", VariableType::F64, 0.25f64);
    let scale = n.constant("scale", VariableType::F64, Expression::Variable(base) * 2.0f64);
    let points = n.constant(
        "points",
        VariableType::Array(Box::new(VariableType::F64), 3),
        Expression::ArrayLiteral(vec![(-1.0f64).into(), 0.0f64.into(), f64::NAN.into()]),
    );
    let pos3 = n.alloc_var(Name::Named("pos3".into()), VariableType::Pos3);
    let v = n.local("v", VariableType::F64);
    let bits = n.local("bits", VariableType::I64);
    let mut n = n;
    let x = Expression::Field {
        base: Box::new(var(pos3)),
        field: "x".into(),
        type_of_field: VariableType::F64,
        known_idnex: Some(0),
    };
    let perm = Expression::PermutationTable(PermutationTableInput::PerlinNoise {
        ident: "minecraft:temperature".into(),
        subident: None,
        subident_index: 0,
    });
    let sample = Expression::ExternCall {
        function_name: "sample".into(),
        parameters: vec![perm, x.clone()],
        parameter_types: vec![VariableType::PermutationTable, VariableType::F64],
        return_type: VariableType::F64,
    };
    n.set_body(vec![
        Statement::Assign { target: v, value: floor(x * var(scale)) + sin(sample) },
        Statement::Assign {
            target: bits,
            value: Expression::ExplicitCast { to: VariableType::I64, expr: Box::new(var(v)) } * (-5i64),
        },
        // `&mut v` on a primitive becomes a DoubleRef with a write-back.
        Statement::Assign {
            target: bits,
            value: Expression::ExternCall {
                function_name: "nudge".into(),
                parameters: vec![v.as_mut_ref()],
                parameter_types: vec![VariableType::F64],
                return_type: VariableType::I64,
            },
        },
        Statement::Return(
            var(v) + Expression::ArrayAccess { array: Box::new(var(points)), index: Box::new(var(bits)) },
        ),
    ]);
    let noise = n.finish_compute_unit();

    // The 4x4x4 entry: reads both units, calls the helper, compares vectors.
    let f = Builder::new(arena).named("minecraft:final").returns(VariableType::F64);
    let noise_in = f.density(noise, (1.0, 1.0, 1.0), (0.25, 0.25, 0.25), (4, 4, 4));
    let scalar_in = f.density(scalar, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), (1, 1, 1));
    let origin = f.alloc_var(Name::Named("origin".into()), VariableType::Vec3);
    let same = f.local("same", VariableType::Bool);
    let mut f = f;
    f.set_body(vec![
        Statement::Assign {
            target: same,
            value: var(origin).eq_expr(Expression::Construct {
                t: VariableType::Vec3,
                args: vec![0.0f64.into(), 0.0f64.into(), 0.0f64.into()],
            }),
        },
        Statement::If {
            condition: var(same),
            then_branch: vec![Statement::Return(noise_in.at(0i32))],
            else_branch: vec![],
        },
        Statement::Return(
            helper.call([noise_in.get()])
                + Expression::ExplicitCast { to: VariableType::F64, expr: Box::new(scalar_in.get()) },
        ),
    ]);
    let entry = f.finish_compute_unit();

    SPMT {
        density_functions: vec![scalar, noise, entry],
        functions: vec![helper],
        main_density_functions: vec![MainDensityFunction {
            density_function: entry,
            dimensions: (4, 4, 4),
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (0.25, 0.25, 0.25),
        }],
    }
}

#[test]
fn generates_java_for_small_program() {
    let arena = Bump::new();
    let program = build_program(&arena);
    let program = convert(&program).expect("conversion");
    let mut backend = JavaBackend::new(JavaConfig::default());
    let interface = Interface::build(&program, backend.naming_rules());
    backend.generate(&program, &interface).unwrap();
    let java = backend.output().expect("java output").source.clone();

    if let Ok(dir) = std::env::var("TUNGSTEN_JAVA_OUT") {
        let dir = std::path::Path::new(&dir).join("tungsten/generated");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("GeneratedWorldgen.java"), &java).unwrap();
        // The Rust output for the same program, to compare against.
        let mut rust = RustBackend::new(RustConfig::default());
        let rust_interface = Interface::build(&program, rust.naming_rules());
        rust.generate(&program, &rust_interface).unwrap();
        std::fs::write(dir.join("../../rcl_new.rs"), &rust.output().unwrap().full_source).unwrap();
    }

    for expected in [
        "package tungsten.generated;",
        "import tungsten.runtime.*;",
        "public final class GeneratedWorldgen {",
        // keyword-named helper is renamed
        "static double int_1(",
        // constants are qualified and ordered by dependency
        "static final double base = 0.25;",
        "static final double scale = GeneratedWorldgen.base * 2.0;",
        "Double.NaN",
        // Java-specific lowerings
        ">>>",
        "(~host_consts.sea_level)",
        "(flag ? 1 : 0)",
        "final DoubleRef v_ref = new DoubleRef(v);",
        "v = v_ref.value;",
        "origin.equals(new Vec3(0.0, 0.0, 0.0))",
        "Helpers.sample(auto_constants.",
        "host_consts.sea_level",
        "buffers.minecraft_noise_d4x4x4_output[p.index]",
        "public double[] orchestrate_minecraft_final(double x, double y, double z, HostConsts host_consts)",
    ] {
        assert!(java.contains(expected), "missing `{}` in:\n{}", expected, java);
    }
    assert!(
        java.find("static final double base").unwrap() < java.find("static final double scale").unwrap(),
        "constants must be emitted in dependency order"
    );
}

/// A 4x4x4 unit returning `I8`: byte constants, wrapping arithmetic, bit operations and casts.
fn build_byte_program<'m>(arena: &'m Bump) -> SPMT<'m> {
    let b = Builder::new(arena).named("minecraft:bytes").returns(VariableType::I8);
    let table = b.constant(
        "table",
        VariableType::Array(Box::new(VariableType::I8), 3),
        Expression::ArrayLiteral(vec![1i8.into(), (-2i8).into(), 127i8.into()]),
    );
    let a = b.local("a", VariableType::I8);
    let wide = b.local("wide", VariableType::I32);
    let d = b.local("d", VariableType::F64);
    let mut b = b;
    b.set_body(vec![
        // 127 + 127 wraps
        Statement::Assign { target: a, value: var(table).index(2i32) + 127i8 },
        Statement::Assign { target: a, value: (-var(a)).bitand(var(a).shl(1i32)) },
        Statement::Assign { target: wide, value: var(a).cast(VariableType::I32).bitand(255i32) },
        Statement::Assign { target: d, value: 300.5f64.into() },
        Statement::Assign { target: a, value: var(d).cast(VariableType::I8) },
        Statement::Return(var(a) + var(wide).cast(VariableType::I8)),
    ]);
    let unit = b.finish_compute_unit();
    SPMT {
        density_functions: vec![unit],
        functions: vec![],
        main_density_functions: vec![MainDensityFunction {
            density_function: unit,
            dimensions: (4, 4, 4),
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (1.0, 1.0, 1.0),
        }],
    }
}

#[test]
fn bytes() {
    let arena = Bump::new();
    let program = convert(&build_byte_program(&arena)).expect("conversion");
    let mut backend = JavaBackend::new(JavaConfig::default());
    let interface = Interface::build(&program, backend.naming_rules());
    backend.generate(&program, &interface).unwrap();
    let java = &backend.output().expect("java output").source;
    for e in [
        "static final byte[] table = new byte[]{(byte) 1, (byte) (-2), (byte) 127};",
        "final byte[] minecraft_bytes_d4x4x4_output = new byte[64];",
        "static byte minecraft_bytes_d4x4x4(",
        // Java promotes bytes to int; results are narrowed back.
        "a = (byte) (GeneratedWorldgen.table[2] + ((byte) 127));",
        "(((byte) (-a)) & ((byte) (a << 1)))",
        "wide = (int) (((int) a) & 255);",
        // Saturating like Rust `as i8`, not wrapping like `(byte)`.
        "a = (byte) Math.max((-128), Math.min(127, (int) d));",
        "return (byte) (a + ((byte) wide));",
    ] {
        assert!(java.contains(e), "missing `{}` in:\n{}", e, java);
    }
}
