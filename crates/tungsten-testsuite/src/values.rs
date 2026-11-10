//! `tungsten_parse::Value` ↔ `tungsten_core::Value`, and value equality (crate README, "Rules and contracts").

use tungsten_check::{Registry, Value as Core};
use tungsten_core::{F64x3, I32x3};
use tungsten_parse::Value as Parsed;

/// Parse value → core value. Records resolve through the registry; floats keep their bits.
pub fn to_core(v: &Parsed, reg: &Registry) -> Result<Core, String> {
    Ok(match v {
        Parsed::Bool(b) => Core::Bool(*b),
        Parsed::I8(x) => Core::I8(*x),
        Parsed::I32(x) => Core::I32(*x),
        Parsed::I64(x) => Core::I64(*x),
        Parsed::F32(x) => Core::F32(x.to_bits()),
        Parsed::F64(x) => Core::F64(x.to_bits()),
        Parsed::Vec3([x, y, z]) => {
            Core::Vec3(F64x3 { x: x.to_bits(), y: y.to_bits(), z: z.to_bits() })
        }
        Parsed::Pos3([x, y, z]) => Core::Pos3(I32x3 { x: *x, y: *y, z: *z }),
        Parsed::Array(vs) => {
            Core::Array(vs.iter().map(|v| to_core(v, reg)).collect::<Result<_, _>>()?)
        }
        Parsed::Record(name, fields) => {
            let id = reg.record_id(name).ok_or_else(|| format!("unknown record `{name}`"))?;
            let mut out = vec![];
            for (i, (fname, fv)) in fields.iter().enumerate() {
                let idx = reg
                    .field_index(id, fname)
                    .ok_or_else(|| format!("record `{name}` has no field `{fname}`"))?;
                if idx != i {
                    return Err(format!("record `{name}`: field `{fname}` is out of order"));
                }
                out.push(to_core(fv, reg)?);
            }
            Core::Record(id, out)
        }
    })
}

fn nan32(b: u32) -> bool {
    f32::from_bits(b).is_nan()
}
fn nan64(b: u64) -> bool {
    f64::from_bits(b).is_nan()
}

/// Core value equality: floats by bits, any NaN equals any NaN of the same width.
pub fn core_eq(a: &Core, b: &Core) -> bool {
    match (a, b) {
        (Core::F32(x), Core::F32(y)) => x == y || (nan32(*x) && nan32(*y)),
        (Core::F64(x), Core::F64(y)) => x == y || (nan64(*x) && nan64(*y)),
        (Core::Vec3(p), Core::Vec3(q)) => {
            core_eq(&Core::F64(p.x), &Core::F64(q.x))
                && core_eq(&Core::F64(p.y), &Core::F64(q.y))
                && core_eq(&Core::F64(p.z), &Core::F64(q.z))
        }
        (Core::Array(x), Core::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| core_eq(a, b))
        }
        (Core::Record(r, x), Core::Record(s, y)) => {
            r == s && x.len() == y.len() && x.iter().zip(y).all(|(a, b)| core_eq(a, b))
        }
        _ => a == b,
    }
}

/// Hex and decimal rendering for reports.
pub fn show(v: &Core) -> String {
    match v {
        Core::F32(b) => format!("{} (0x{b:08x})", f32::from_bits(*b)),
        Core::F64(b) => format!("{} (0x{b:016x})", f64::from_bits(*b)),
        Core::Vec3(p) => format!(
            "vec3({}, {}, {})",
            show(&Core::F64(p.x)),
            show(&Core::F64(p.y)),
            show(&Core::F64(p.z))
        ),
        Core::Array(vs) => format!("[{}]", vs.iter().map(show).collect::<Vec<_>>().join(", ")),
        Core::Record(r, vs) => {
            format!("record#{}{{{}}}", r.0, vs.iter().map(show).collect::<Vec<_>>().join(", "))
        }
        other => format!("{other:?}"),
    }
}

/// True if `v` contains a handle, which a test can't expect or encode.
pub fn has_table(v: &Core) -> bool {
    match v {
        Core::Handle(_) => true,
        Core::Array(vs) | Core::Record(_, vs) => vs.iter().any(has_table),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nan_matches_any_nan() {
        let a = Core::F64(0x7ff8_0000_0000_0000);
        let b = Core::F64(0x7ff8_0000_0000_0001);
        assert!(core_eq(&a, &b));
        assert!(!core_eq(&Core::F64(0.0f64.to_bits()), &Core::F64((-0.0f64).to_bits())));
        assert!(!core_eq(&Core::F32(0x7fc0_0000), &Core::F64(0x7ff8_0000_0000_0000)));
    }
}
