//! Converting literals as written (`ast::LitKind`) to values.

use crate::ast::{LitKind, NumKind, RawNum};
pub use crate::tables::Num;

pub const NAN_F64: u64 = 0x7ff8_0000_0000_0000;
pub const NAN_F32: u32 = 0x7fc0_0000;

/// Converts a numeric literal. With `want`, the literal must be acceptable where that kind is
/// expected: the same kind, or an unsuffixed int for a float, or an unsuffixed float for an f32.
/// Without it, the literal's own kind decides. Bool literals are not numeric.
pub fn convert(lit: &LitKind, want: Option<NumKind>) -> Result<Num, String> {
    match lit {
        LitKind::Bool(_) => Err("expected a number, found a bool".into()),
        LitKind::Num { raw, neg } => {
            let k = want.unwrap_or(raw.kind);
            check_compatible(raw, k)?;
            if k.is_float() {
                float(raw, *neg, k)
            } else {
                int(raw, *neg, k)
            }
        }
        LitKind::Inf { neg, f32 } => {
            let k = float_target(want, *f32, "inf")?;
            let sign = if *neg { -1.0 } else { 1.0 };
            Ok(match k {
                NumKind::F32 => Num::Float(sign as f32 * f32::INFINITY),
                _ => Num::Double(sign * f64::INFINITY),
            })
        }
        LitKind::Nan {
            bits,
            f32,
            explicit,
        } => {
            let k = float_target(want, *f32, "nan")?;
            let is32 = k == NumKind::F32;
            if *explicit && is32 != *f32 {
                return Err("nan payload doesn't fit the expected float type".into());
            }
            Ok(match (is32, *explicit) {
                (true, true) => Num::Float(f32::from_bits(*bits as u32)),
                (true, false) => Num::Float(f32::from_bits(NAN_F32)),
                (false, true) => Num::Double(f64::from_bits(*bits)),
                (false, false) => Num::Double(f64::from_bits(NAN_F64)),
            })
        }
    }
}

fn float_target(want: Option<NumKind>, natural_f32: bool, what: &str) -> Result<NumKind, String> {
    match want {
        None if natural_f32 => Ok(NumKind::F32),
        None => Ok(NumKind::F64),
        Some(NumKind::F64) if natural_f32 => Err(format!("`{what}_f32` where f64 is expected")),
        Some(k) if k.is_float() => Ok(k),
        Some(k) => Err(format!("`{what}` where {} is expected", k.name())),
    }
}

fn check_compatible(raw: &RawNum, want: NumKind) -> Result<(), String> {
    let ok = raw.kind == want
        || (want.is_float() && !raw.kind.is_float() && !raw.suffixed)
        || (want == NumKind::F32 && raw.kind == NumKind::F64 && !raw.suffixed);
    if ok {
        Ok(())
    } else {
        Err(format!(
            "expected a {} literal, found {}",
            want.name(),
            raw.kind.name()
        ))
    }
}

fn int(raw: &RawNum, neg: bool, k: NumKind) -> Result<Num, String> {
    let bits = k.width();
    let v: i128 = if raw.radix == 10 {
        let out_of_range = || format!("integer literal out of range for {}", k.name());
        let v: i128 = raw.digits.parse().map_err(|_| out_of_range())?;
        let v = if neg { -v } else { v };
        let (lo, hi) = (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1);
        if v < lo || v > hi {
            return Err(out_of_range());
        }
        v
    } else {
        if neg {
            return Err("a sign can't be glued to a hex or binary literal".into());
        }
        let v = u128::from_str_radix(&raw.digits, raw.radix)
            .ok()
            .filter(|v| *v < 1u128 << bits)
            .ok_or_else(|| {
                format!(
                    "literal has more significant bits than the width of {}",
                    k.name()
                )
            })?;
        // reinterpret the bit pattern
        let shift = 128 - bits;
        ((v << shift) as i128) >> shift
    };
    Ok(match k {
        NumKind::Byte => Num::Byte(v as i8),
        NumKind::Int => Num::Int(v as i32),
        _ => Num::Long(v as i64),
    })
}

fn float(raw: &RawNum, neg: bool, k: NumKind) -> Result<Num, String> {
    if raw.radix != 10 {
        return Err("hex and binary literals can't be floats".into());
    }
    let text = if neg {
        format!("-{}", raw.digits)
    } else {
        raw.digits.clone()
    };
    // Parsed directly in the target type, so f32 literals don't round twice.
    let invalid = |_| "invalid float literal".to_string();
    let (n, finite, zero) = if k == NumKind::F32 {
        let v: f32 = text.parse().map_err(invalid)?;
        (Num::Float(v), v.is_finite(), v == 0.0)
    } else {
        let v: f64 = text.parse().map_err(invalid)?;
        (Num::Double(v), v.is_finite(), v == 0.0)
    };
    if !finite {
        return Err("float literal overflows; spell infinity as `inf`".into());
    }
    let mantissa = raw.digits.split(['e', 'E']).next().unwrap_or("");
    if zero && mantissa.bytes().any(|b| (b'1'..=b'9').contains(&b)) {
        return Err(format!(
            "float literal underflows to zero in {}; write `0.0` if that is meant",
            k.name()
        ));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(digits: &str, radix: u32, kind: NumKind, suffixed: bool) -> LitKind {
        LitKind::Num {
            raw: RawNum {
                digits: digits.into(),
                radix,
                kind,
                suffixed,
            },
            neg: false,
        }
    }
    fn neg(l: LitKind) -> LitKind {
        match l {
            LitKind::Num { raw, .. } => LitKind::Num { raw, neg: true },
            l => l,
        }
    }

    #[test]
    fn decimal_ranges_include_the_sign() {
        let i = |d| raw(d, 10, NumKind::Int, false);
        assert_eq!(convert(&neg(i("2147483648")), None), Ok(Num::Int(i32::MIN)));
        assert!(convert(&i("2147483648"), None).is_err());
        assert_eq!(
            convert(&neg(raw("128", 10, NumKind::Byte, true)), None),
            Ok(Num::Byte(-128))
        );
        assert!(convert(&raw("128", 10, NumKind::Byte, true), None).is_err());
        assert_eq!(
            convert(
                &neg(raw("9223372036854775808", 10, NumKind::Long, true)),
                None
            ),
            Ok(Num::Long(i64::MIN))
        );
    }

    #[test]
    fn hex_is_a_bit_pattern() {
        assert_eq!(
            convert(&raw("FFFFFFFF", 16, NumKind::Int, false), None),
            Ok(Num::Int(-1))
        );
        assert_eq!(
            convert(&raw("80", 16, NumKind::Byte, true), None),
            Ok(Num::Byte(-128))
        );
        assert_eq!(
            convert(&raw("8000000000000000", 16, NumKind::Long, true), None),
            Ok(Num::Long(i64::MIN))
        );
        assert!(convert(&raw("100000000", 16, NumKind::Int, false), None).is_err());
        assert!(convert(&raw("100", 16, NumKind::Byte, true), None).is_err());
        assert!(convert(&neg(raw("1", 16, NumKind::Int, false)), None).is_err());
        assert_eq!(
            convert(&raw("101", 2, NumKind::Int, false), None),
            Ok(Num::Int(5))
        );
    }

    #[test]
    fn floats_parse_in_their_own_type() {
        // a known double-rounding case: parsing through f64 gives a different f32
        let s = "1.00000005960464477539063";
        let direct: f32 = s.parse().unwrap();
        let via_f64 = s.parse::<f64>().unwrap() as f32;
        assert_ne!(direct, via_f64);
        assert_eq!(
            convert(&raw(s, 10, NumKind::F32, true), None),
            Ok(Num::Float(direct))
        );
        assert!(convert(&raw("1e999", 10, NumKind::F64, false), None).is_err());
        assert!(convert(&raw("1e50", 10, NumKind::F32, true), None).is_err());
    }

    #[test]
    fn underflow_is_an_error_but_zero_is_fine() {
        let f = |d| convert(&raw(d, 10, NumKind::F64, false), None);
        assert!(f("1e-400").unwrap_err().contains("underflows"));
        assert!(convert(&raw("1e-50", 10, NumKind::F32, true), None).is_err());
        assert_eq!(f("0.0e-400"), Ok(Num::Double(0.0)));
        assert_eq!(f("0.000"), Ok(Num::Double(0.0)));
        // the smallest subnormal is still representable
        assert_eq!(f("5e-324"), Ok(Num::Double(5e-324)));
    }

    #[test]
    fn attribute_position_conversions() {
        let f64k = Some(NumKind::F64);
        assert_eq!(
            convert(&raw("3", 10, NumKind::Int, false), f64k),
            Ok(Num::Double(3.0))
        );
        assert_eq!(
            convert(&neg(raw("3", 10, NumKind::Int, false)), f64k),
            Ok(Num::Double(-3.0))
        );
        // checked against f64, not against the literal's own i32
        assert_eq!(
            convert(&raw("3000000000", 10, NumKind::Int, false), f64k),
            Ok(Num::Double(3e9))
        );
        assert!(convert(&raw("3", 10, NumKind::Long, true), f64k).is_err());
        assert!(convert(&raw("1.5", 10, NumKind::F32, true), f64k).is_err());
        assert_eq!(
            convert(&raw("0.5", 10, NumKind::F64, false), Some(NumKind::F32)),
            Ok(Num::Float(0.5))
        );
        assert!(convert(&raw("0.5", 10, NumKind::F64, true), Some(NumKind::F32)).is_err());
        assert!(convert(&raw("1.5", 10, NumKind::F64, false), Some(NumKind::Int)).is_err());
    }

    #[test]
    fn specials() {
        let nan = |bits, f32, explicit| LitKind::Nan {
            bits,
            f32,
            explicit,
        };
        match convert(&nan(0, false, false), None).unwrap() {
            Num::Double(d) => assert_eq!(d.to_bits(), NAN_F64),
            n => panic!("{n:?}"),
        }
        match convert(&nan(0xfff8_0000_0000_0001, false, true), None).unwrap() {
            Num::Double(d) => assert_eq!(d.to_bits(), 0xfff8_0000_0000_0001),
            n => panic!("{n:?}"),
        }
        assert!(convert(&nan(0x7fc0_0001, true, true), Some(NumKind::F64)).is_err());
        assert_eq!(
            convert(
                &LitKind::Inf {
                    neg: true,
                    f32: true
                },
                None
            ),
            Ok(Num::Float(f32::NEG_INFINITY))
        );
    }
}
