//! `tungsten_check::Externs` over the shared runtime in `runtime/rust/`, plus the per-case toy
//! tables (SPEC §5.3).

use tungsten_check::{AutoArgValue, AutoCoreId, ExternFail, Externs, LowerMap, ResolvedAutoConst, Value};
use tungsten_core::{ExternId, F64x3};

#[path = "../runtime/rust/externs.rs"]
pub mod rt_externs;
#[path = "../runtime/rust/utilsf64.rs"]
pub mod rt_utils;

enum Table {
    Perm(Box<[u8; 256]>),
    Sampler(u64),
}

/// The interpreter's externs for one case: the registry names (by `ExternId`) and the tables
/// derived from the seed.
pub struct CaseExterns {
    names: Vec<String>,
    tables: Vec<Table>,
}

impl CaseExterns {
    /// Builds the toy runtime objects of every handle autoconst (`inits` are in arena order).
    /// The testsuite acts as a small frontend: it knows what its own initializers mean.
    pub fn new(map: &LowerMap, inits: &[ResolvedAutoConst]) -> Result<Self, String> {
        let mut tables: Vec<Option<Table>> = vec![];
        for a in inits {
            let AutoCoreId::Table(id) = a.core;
            let i64s = a
                .args
                .iter()
                .map(|v| match v {
                    AutoArgValue::I64(x) => Ok(*x),
                    other => Err(format!("autoconst `{}`: unexpected argument {other:?}", a.name)),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let table = match (a.initializer, i64s.as_slice()) {
                ("make_permutation_table", [seed, id_lo, id_hi, idx, sub_lo, sub_hi]) => {
                    let st = rt_utils::perlin_state(*seed as u64, *id_lo, *id_hi, *idx, *sub_lo, *sub_hi);
                    Table::Perm(Box::new(rt_utils::make_perm(st)))
                }
                ("make_base3d_perm_table", [seed]) => Table::Sampler(rt_utils::base3d_state(*seed as u64)),
                (name, args) => {
                    return Err(format!("unknown initializer `{name}` with {} arguments", args.len()));
                }
            };
            if tables.len() <= id.0 {
                tables.resize_with(id.0 + 1, || None);
            }
            tables[id.0] = Some(table);
        }
        let tables = tables
            .into_iter()
            .map(|t| t.ok_or_else(|| "a table has no initializer".to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CaseExterns { names: map.externs.clone(), tables })
    }
}

fn f64v(v: &Value) -> f64 {
    match v {
        Value::F64(b) => f64::from_bits(*b),
        _ => panic!("expected f64, got {v:?}"),
    }
}
fn f32v(v: &Value) -> f32 {
    match v {
        Value::F32(b) => f32::from_bits(*b),
        _ => panic!("expected f32, got {v:?}"),
    }
}
fn i32v(v: &Value) -> i32 {
    match v {
        Value::I32(x) => *x,
        _ => panic!("expected i32, got {v:?}"),
    }
}
fn i64v(v: &Value) -> i64 {
    match v {
        Value::I64(x) => *x,
        _ => panic!("expected i64, got {v:?}"),
    }
}
fn mk64(x: f64) -> Value {
    Value::F64(x.to_bits())
}
fn mk32(x: f32) -> Value {
    Value::F32(x.to_bits())
}

impl Externs for CaseExterns {
    fn call(&self, ext: ExternId, a: &[Value]) -> Result<Value, ExternFail> {
        use rt_externs::*;
        let name = self.names[ext.0].as_str();
        Ok(match name {
            "sqrt" => mk64(f64v(&a[0]).sqrt()),
            "floor" => mk64(f64v(&a[0]).floor()),
            "ceil" => mk64(f64v(&a[0]).ceil()),
            "trunc" => mk64(f64v(&a[0]).trunc()),
            "abs" => mk64(f64v(&a[0]).abs()),
            "min" => mk64(min_f64(f64v(&a[0]), f64v(&a[1]))),
            "max" => mk64(max_f64(f64v(&a[0]), f64v(&a[1]))),
            "copysign" => mk64(f64v(&a[0]).copysign(f64v(&a[1]))),
            "fma" => mk64(f64v(&a[0]).mul_add(f64v(&a[1]), f64v(&a[2]))),
            "sqrt_f32" => mk32(f32v(&a[0]).sqrt()),
            "floor_f32" => mk32(f32v(&a[0]).floor()),
            "ceil_f32" => mk32(f32v(&a[0]).ceil()),
            "trunc_f32" => mk32(f32v(&a[0]).trunc()),
            "abs_f32" => mk32(f32v(&a[0]).abs()),
            "min_f32" => mk32(min_f32(f32v(&a[0]), f32v(&a[1]))),
            "max_f32" => mk32(max_f32(f32v(&a[0]), f32v(&a[1]))),
            "copysign_f32" => mk32(f32v(&a[0]).copysign(f32v(&a[1]))),
            "fma_f32" => mk32(f32v(&a[0]).mul_add(f32v(&a[1]), f32v(&a[2]))),
            "float_to_bits" => Value::I32(float_to_bits(f32v(&a[0]))),
            "double_to_bits" => Value::I64(double_to_bits(f64v(&a[0]))),
            "bits_to_float" => mk32(bits_to_float(i32v(&a[0]))),
            "bits_to_double" => mk64(bits_to_double(i64v(&a[0]))),
            "test_domain" => Value::I32(test_domain(i32v(&a[0])).ok_or(ExternFail::Domain)?),
            "perm_get" => match &a[0] {
                Value::Handle(id) => match &self.tables[id.0] {
                    Table::Perm(p) => Value::I32(rt_utils::perm_lookup(p, i32v(&a[1]))),
                    Table::Sampler(_) => panic!("perm_get on a sampler"),
                },
                v => panic!("perm_get: expected a perm table, got {v:?}"),
            },
            "sampler_sample" => match (&a[0], &a[1]) {
                (Value::Handle(id), Value::Vec3(F64x3 { x, y, z })) => match &self.tables[id.0] {
                    Table::Sampler(st) => mk64(rt_utils::sample_state(
                        *st,
                        [f64::from_bits(*x), f64::from_bits(*y), f64::from_bits(*z)],
                    )),
                    Table::Perm(_) => panic!("sampler_sample on a perm table"),
                },
                v => panic!("sampler_sample: bad arguments {v:?}"),
            },
            other => panic!("extern `{other}` has no implementation in runtime/rust"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::rt_externs::*;
    use super::rt_utils::*;

    #[test]
    fn java_min_max() {
        assert!(min_f64(0.0, -0.0).is_sign_negative());
        assert!(max_f64(-0.0, 0.0).is_sign_positive());
        assert!(min_f64(f64::NAN, 1.0).is_nan());
        assert_eq!(max_f64(1.0, 2.0), 2.0);
    }

    #[test]
    fn canonical_nan_bits() {
        assert_eq!(double_to_bits(f64::from_bits(0x7ff8_0000_0000_0001)), 0x7ff8_0000_0000_0000);
        assert_eq!(float_to_bits(f32::NAN), 0x7fc0_0000);
        assert_eq!(double_to_bits(-0.0), i64::MIN);
    }

    #[test]
    fn domain() {
        assert_eq!(test_domain(-1), None);
        assert_eq!(test_domain(3), Some(3));
    }

    #[test]
    fn perm_is_permutation() {
        let p = make_perm(perlin_state(1, 2, 3, 0, 0, 0));
        let mut seen = [false; 256];
        for x in p {
            seen[x as usize] = true;
        }
        assert!(seen.iter().all(|b| *b));
        assert_eq!(perm_lookup(&p, 256 + 5), perm_lookup(&p, 5));
        assert_eq!(perm_lookup(&p, -1), p[255] as i32);
    }

    #[test]
    fn splitmix_golden() {
        // splitmix64 with state 0: first output of the reference generator.
        assert_eq!(splitmix64(0), 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn sampler_in_unit_interval() {
        let s = base3d_state(7);
        let v = sample_state(s, [1.5, -2.0, 3.0]);
        assert!((0.0..1.0).contains(&v));
        // Same cell, same value.
        assert_eq!(v, sample_state(s, [1.9, -1.1, 3.9]));
    }
}

#[cfg(test)]
mod golden {
    use super::rt_utils::*;

    /// Toy-table golden values for fixed seeds (SPEC §5.3, §12). Any port must reproduce these.
    /// Cross-checked against an independent Python implementation of the definitions.
    #[test]
    fn toy_tables() {
        assert_eq!(splitmix64(0), 0xE220_A839_7B1D_CDAF);
        let st = perlin_state(1, 2, 3, 4, 5, 6);
        assert_eq!(st, 0x0c2b_5815_eea8_0ebf);
        assert_eq!(&make_perm(st)[..8], &[216, 28, 120, 37, 55, 25, 49, 241]);
        let b = base3d_state(7);
        assert_eq!(b, 0x63cb_e1e4_5932_0dd7);
        assert_eq!(sample_state(b, [1.5, -2.0, 3.0]).to_bits(), 0x3fed_e235_7dfd_f669);
    }
}
