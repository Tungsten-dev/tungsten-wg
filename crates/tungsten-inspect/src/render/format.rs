//! Float, size and name formatting shared by the renderers (SPEC.md §8).

use crate::model::Grid;

/// 4 significant digits, trailing zeros trimmed, `e` notation beyond 1e±5. For labels only;
/// tooltips and text outputs use the exact value.
pub(crate) fn round(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let exp = x.abs().log10().floor() as i32;
    // Round to 4 significant digits first; that can carry into the next power of ten.
    let factor = 10f64.powi(exp - 3);
    let x = (x / factor).round() * factor;
    let exp = x.abs().log10().floor() as i32;
    if !(-5..5).contains(&exp) {
        let s = format!("{x:.3e}");
        let (mantissa, e) = s.split_once('e').unwrap();
        return format!("{}e{e}", trim_zeros(mantissa));
    }
    let decimals = (3 - exp).max(0) as usize;
    trim_zeros(&format!("{x:.decimals$}")).to_string()
}

fn trim_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

pub(crate) fn round_triple(v: (f64, f64, f64)) -> String {
    format!("({}, {}, {})", round(v.0), round(v.1), round(v.2))
}

pub(crate) fn exact_triple(v: (f64, f64, f64)) -> String {
    format!("({}, {}, {})", v.0, v.1, v.2)
}

pub(crate) fn dims(d: (i32, i32, i32)) -> String {
    format!("{}x{}x{}", d.0, d.1, d.2)
}

/// `XxYxZ o(..) p(..)`, rounded.
pub(crate) fn grid_label(g: &Grid) -> String {
    format!(
        "{} o{} p{}",
        dims(g.dimensions),
        round_triple(g.scaled_origin),
        round_triple(g.scaled_position)
    )
}

/// `XxYxZ origin (..) pos (..)`, exact.
pub(crate) fn grid_exact(g: &Grid) -> String {
    format!(
        "{} origin {} pos {}",
        dims(g.dimensions),
        exact_triple(g.scaled_origin),
        exact_triple(g.scaled_position)
    )
}

/// Human units: B, KiB, MiB, GiB.
pub(crate) fn bytes(n: usize) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{} {}", trim_zeros(&format!("{v:.1}")), UNITS[unit])
    }
}

/// Escapes text for SVG/XML content and attribute values.
pub(crate) fn xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Escapes text for a Markdown table cell.
pub(crate) fn md(s: &str) -> String {
    s.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_to_four_significant_digits() {
        assert_eq!(round(0.25), "0.25");
        assert_eq!(round(1.0), "1");
        assert_eq!(round(3.14159265), "3.142");
        assert_eq!(round(12345.678), "12350");
        assert_eq!(round(123456.0), "1.235e5");
        assert_eq!(round(99999.9), "1e5");
        assert_eq!(round(1234.5678), "1235");
        assert_eq!(round(0.000123456), "0.0001235");
        assert_eq!(round(-0.5), "-0.5");
        assert_eq!(round(1.5e-7), "1.5e-7");
        assert_eq!(round(0.0), "0");
    }

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(2048), "2 KiB");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(3 * 1024 * 1024), "3 MiB");
    }
}
