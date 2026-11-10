//! Float and name spelling shared by the printer.

use crate::lexer::{is_ident, is_keyword};

fn finite(debug: String, suffix: &str) -> String {
    // `{:?}` is the shortest representation that round-trips. `1e16` needs a `.0` to lex as a
    // float literal.
    let s = match debug.find('e') {
        Some(i) if !debug[..i].contains('.') => format!("{}.0{}", &debug[..i], &debug[i..]),
        _ => debug,
    };
    format!("{s}{suffix}")
}

pub fn f64_lit(v: f64) -> String {
    if v.is_nan() {
        format!("nan(0x{:016x})", v.to_bits())
    } else if v.is_infinite() {
        if v < 0.0 { "-inf" } else { "inf" }.to_string()
    } else {
        finite(format!("{v:?}"), "")
    }
}

pub fn f32_lit(v: f32) -> String {
    if v.is_nan() {
        format!("nan_f32(0x{:08x})", v.to_bits())
    } else if v.is_infinite() {
        if v < 0.0 { "-inf_f32" } else { "inf_f32" }.to_string()
    } else {
        finite(format!("{v:?}"), "f")
    }
}

/// A name as an identifier, or quoted when it isn't one (or is a keyword).
pub fn quote_name(s: &str) -> String {
    if is_ident(s) && !is_keyword(s) {
        return s.to_string();
    }
    let mut out = String::from("`");
    for c in s.chars() {
        match c {
            '`' => out.push_str("\\`"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('`');
    out
}

pub fn quote_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellings() {
        assert_eq!(f64_lit(1e16), "1.0e16");
        assert_eq!(f64_lit(1.5e-7), "1.5e-7");
        assert_eq!(f64_lit(0.1), "0.1");
        assert_eq!(f64_lit(-0.0), "-0.0");
        assert_eq!(f64_lit(1.0), "1.0");
        assert_eq!(f32_lit(0.1), "0.1f");
        assert_eq!(f32_lit(-0.0), "-0.0f");
        assert_eq!(f32_lit(1e-7), "1.0e-7f");
        assert_eq!(f64_lit(f64::INFINITY), "inf");
        assert_eq!(f64_lit(f64::NEG_INFINITY), "-inf");
        assert_eq!(f32_lit(f32::NEG_INFINITY), "-inf_f32");
        assert_eq!(
            f64_lit(f64::from_bits(0xfff8_0000_0000_0001)),
            "nan(0xfff8000000000001)"
        );
        assert_eq!(f32_lit(f32::from_bits(0x7fc0_0000)), "nan_f32(0x7fc00000)");
    }

    #[test]
    fn names() {
        assert_eq!(quote_name("abc"), "abc");
        assert_eq!(quote_name("let"), "`let`");
        assert_eq!(
            quote_name("minecraft:overworld/x"),
            "`minecraft:overworld/x`"
        );
        assert_eq!(quote_name(""), "``");
        assert_eq!(quote_name("a`b\\c\n"), "`a\\`b\\\\c\\n`");
        assert_eq!(quote_name("9x"), "`9x`");
    }
}
