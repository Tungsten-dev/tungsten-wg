//! Tokens, lexed with `logos`. Whitespace and `//` comments are skipped. A pass after lexing sets
//! `joint` (the next token follows with no gap, which the parser uses to build `>>`, `>>>` and
//! `>=`) and marks a `-` that is glued to a number or `inf`.

use logos::Logos;

use crate::ast::{NumKind, RawNum};
use crate::span::{Diagnostic, FileId, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Suffix {
    None,
    /// `name#N`
    Hash(u64),
    /// `name##N`
    HashHash(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// An identifier or quoted identifier with its label suffix.
    Name {
        text: String,
        quoted: bool,
        suffix: Suffix,
    },
    /// `%N`
    Anon(u64),
    Kw(Kw),
    /// `@name`
    Attr(String),
    Str(String),
    Num(RawNum),
    Minus {
        glued: bool,
    },
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    ColonColon,
    Dot,
    Arrow,
    Assign,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Shl,
    Plus,
    Star,
    Slash,
    Amp,
    AmpAmp,
    Pipe,
    PipePipe,
    Caret,
    Bang,
    Eof,
}

macro_rules! keywords {
    ($($variant:ident = $text:literal),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Kw { $($variant),* }

        impl Kw {
            pub fn from_word(s: &str) -> Option<Kw> {
                match s { $($text => Some(Kw::$variant),)* _ => None }
            }
            pub fn as_str(self) -> &'static str {
                match self { $(Kw::$variant => $text),* }
            }
        }
    };
}

keywords! {
    Program = "program", Fn = "fn", Unit = "unit", Main = "main", Input = "input", Entry = "entry",
    Host = "host", Table = "table", Const = "const", Use = "use", Let = "let", Vars = "vars",
    Return = "return", Break = "break", If = "if", Else = "else", While = "while",
    Repeat = "repeat", True = "true", False = "false", As = "as", New = "new", Extern = "extern",
    Ext = "ext", Mut = "mut", Dynamic = "dynamic", None = "none", Perlin = "perlin",
    Base3d = "base3d", Density = "density", Ambient = "ambient", Helpers = "helpers",
    Tables = "tables", Hosts = "hosts", Consts = "consts", Inf = "inf", Nan = "nan",
    InfF32 = "inf_f32", NanF32 = "nan_f32",
    Bool = "bool", I8 = "i8", I32 = "i32", I64 = "i64", F32 = "f32", F64 = "f64", Vec3 = "vec3",
    Pos3 = "pos3", PermTable = "perm_table",
    InterpolatedNoiseSampler = "interpolated_noise_sampler",
}

pub fn is_keyword(s: &str) -> bool {
    Kw::from_word(s).is_some()
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// `[A-Za-z_][A-Za-z0-9_]*`
pub fn is_ident(s: &str) -> bool {
    s.bytes()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && s.bytes().all(is_ident_char)
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
    pub joint: bool,
}

/// A token that failed. Without a message it is a stray character. `end` (a byte offset) moves
/// the end of the reported span, which is the token's span otherwise.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LexError {
    msg: Option<String>,
    end: Option<usize>,
}

fn fail<T>(msg: impl Into<String>) -> Result<T, LexError> {
    Err(LexError {
        msg: Some(msg.into()),
        end: None,
    })
}

type Lexer<'s> = logos::Lexer<'s, Raw>;

#[derive(Logos)]
#[logos(error = LexError)]
#[logos(skip r"[ \t\r\n]+")]
#[logos(skip(r"//[^\n]*", allow_greedy = true))]
enum Raw {
    #[token("{", |_| Tok::LBrace)]
    #[token("}", |_| Tok::RBrace)]
    #[token("(", |_| Tok::LParen)]
    #[token(")", |_| Tok::RParen)]
    #[token("[", |_| Tok::LBracket)]
    #[token("]", |_| Tok::RBracket)]
    #[token(",", |_| Tok::Comma)]
    #[token(";", |_| Tok::Semi)]
    #[token(":", |_| Tok::Colon)]
    #[token("::", |_| Tok::ColonColon)]
    #[token(".", |_| Tok::Dot)]
    #[token("->", |_| Tok::Arrow)]
    #[token("=", |_| Tok::Assign)]
    #[token("==", |_| Tok::EqEq)]
    #[token("!=", |_| Tok::NotEq)]
    #[token("<", |_| Tok::Lt)]
    #[token("<=", |_| Tok::Le)]
    #[token("<<", |_| Tok::Shl)]
    #[token(">", |_| Tok::Gt)]
    #[token("+", |_| Tok::Plus)]
    #[token("-", |_| Tok::Minus { glued: false })]
    #[token("*", |_| Tok::Star)]
    #[token("/", |_| Tok::Slash)]
    #[token("&", |_| Tok::Amp)]
    #[token("&&", |_| Tok::AmpAmp)]
    #[token("|", |_| Tok::Pipe)]
    #[token("||", |_| Tok::PipePipe)]
    #[token("^", |_| Tok::Caret)]
    #[token("!", |_| Tok::Bang)]
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*", word)]
    #[token("`", quoted)]
    #[token("\"", string)]
    #[regex(r"%[0-9]*", anon)]
    #[regex(r"@[A-Za-z0-9_]*", attr)]
    #[regex(
        r"[0-9]+(_[0-9]+)*(\.[0-9]+(_[0-9]+)*)?([eE][+-]?[0-9]+(_[0-9]+)*)?",
        decimal
    )]
    #[regex(r"0x[0-9a-fA-F]+(_[0-9a-fA-F]+)*", |l| radix(l, 16))]
    #[regex(r"0b[01]+(_[01]+)*", |l| radix(l, 2))]
    T(Tok),
}

fn word(l: &mut Lexer) -> Result<Tok, LexError> {
    let text = l.slice();
    if let Some(kw) = Kw::from_word(text) {
        // keywords take no label suffix
        return Ok(Tok::Kw(kw));
    }
    Ok(Tok::Name {
        text: text.to_string(),
        quoted: false,
        suffix: label_suffix(l)?,
    })
}

fn quoted(l: &mut Lexer) -> Result<Tok, LexError> {
    let text = delimited(l, '`', "quoted identifier")?;
    Ok(Tok::Name {
        text,
        quoted: true,
        suffix: label_suffix(l)?,
    })
}

fn string(l: &mut Lexer) -> Result<Tok, LexError> {
    Ok(Tok::Str(delimited(l, '"', "string")?))
}

fn slot(digits: &str) -> Result<u64, LexError> {
    digits.parse().or_else(|_| fail("slot number out of range"))
}

/// `#N` or `##N` right after a name.
fn label_suffix(l: &mut Lexer) -> Result<Suffix, LexError> {
    let rest = l.remainder();
    let hashes = rest.bytes().take(2).take_while(|b| *b == b'#').count();
    if hashes == 0 {
        return Ok(Suffix::None);
    }
    let digits = &rest[hashes..];
    let digits = &digits[..digits.bytes().take_while(u8::is_ascii_digit).count()];
    l.bump(hashes + digits.len());
    if digits.is_empty() {
        return fail("`#` must be followed by a slot number");
    }
    let n = slot(digits)?;
    Ok(if hashes == 2 {
        Suffix::HashHash(n)
    } else {
        Suffix::Hash(n)
    })
}

fn anon(l: &mut Lexer) -> Result<Tok, LexError> {
    match &l.slice()[1..] {
        "" => fail("`%` must be followed by a slot number"),
        digits => Ok(Tok::Anon(slot(digits)?)),
    }
}

fn attr(l: &mut Lexer) -> Result<Tok, LexError> {
    match &l.slice()[1..] {
        "" => fail("expected a directive or attribute name after `@`"),
        name => Ok(Tok::Attr(name.to_string())),
    }
}

/// The body of a quoted identifier or string, after the opening `quote`. Quoted identifiers end
/// at a newline.
fn delimited(l: &mut Lexer, quote: char, what: &str) -> Result<String, LexError> {
    let rest = l.remainder();
    let mut out = String::new();
    let mut i = 0;
    let r = loop {
        let Some(c) = rest[i..].chars().next() else {
            break Err(format!("unterminated {what}"));
        };
        if c == quote {
            i += 1;
            break Ok(());
        }
        if c == '\n' && quote == '`' {
            break Err(format!("unterminated {what}"));
        }
        i += c.len_utf8();
        if c != '\\' {
            out.push(c);
            continue;
        }
        match escape(&rest[i..], quote) {
            Ok((ch, n)) => {
                out.push(ch);
                i += n;
            }
            Err((msg, n)) => {
                i += n;
                break Err(msg.to_string());
            }
        }
    };
    l.bump(i);
    r.map(|()| out).or_else(fail)
}

/// The escape after a backslash: the character and the bytes it took, or an error and the bytes
/// to skip.
fn escape(s: &str, quote: char) -> Result<(char, usize), (&'static str, usize)> {
    let Some(c) = s.chars().next() else {
        return Err(("unterminated escape", 0));
    };
    match c {
        '\\' => return Ok(('\\', 1)),
        'n' => return Ok(('\n', 1)),
        't' => return Ok(('\t', 1)),
        'r' => return Ok(('\r', 1)),
        c if c == quote => return Ok((quote, 1)),
        'u' => {}
        c => return Err(("unknown escape", c.len_utf8())),
    }
    let Some(body) = s[1..].strip_prefix('{') else {
        return Err(("expected `{` after `\\u`", 1));
    };
    let end = body.find(['}', '\n']).unwrap_or(body.len());
    if !body[end..].starts_with('}') {
        return Err(("unterminated `\\u{…}` escape", 2 + end));
    }
    let hex = &body[..end];
    let n = 2 + end + 1;
    let valid = !hex.is_empty() && hex.len() <= 6 && hex.bytes().all(|b| b.is_ascii_hexdigit());
    u32::from_str_radix(hex, 16)
        .ok()
        .filter(|_| valid)
        .and_then(char::from_u32)
        .map(|ch| (ch, n))
        .ok_or(("invalid unicode escape", n))
}

/// Identifier characters right after a numeric literal: its suffix, if it has one. Consumed.
fn suffix<'s>(l: &mut Lexer<'s>) -> &'s str {
    let rest = l.remainder();
    let s = &rest[..rest.bytes().take_while(|b| is_ident_char(*b)).count()];
    l.bump(s.len());
    s
}

fn decimal(l: &mut Lexer) -> Result<Tok, LexError> {
    let body = l.slice();
    let is_float = body.contains(['.', 'e', 'E']);
    let sfx = suffix(l);
    let kind = match sfx {
        "" if is_float => NumKind::F64,
        "" => NumKind::Int,
        s => match NumKind::from_suffix(s) {
            Some((k, _)) => k,
            None if body == "0" && s.starts_with('x') => {
                return fail("expected digits after the radix prefix");
            }
            None if s.starts_with('_') => return fail("misplaced `_` in numeric literal"),
            None => return fail("invalid numeric literal suffix"),
        },
    };
    let suffixed = !sfx.is_empty();
    if suffixed && kind.is_float() != is_float {
        return fail(if is_float {
            "an integer suffix on a float literal"
        } else {
            "a float literal needs a `.` or an exponent"
        });
    }
    // `1.` isn't a literal, but in `1.x` and `1..` the int is followed by a `.`
    if let Some(after) = l.remainder().strip_prefix('.')
        && !suffixed
        && !after
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_' || b == b'.')
    {
        return Err(LexError {
            msg: Some("a float needs digits on both sides of `.`".into()),
            end: Some(l.span().end + 1),
        });
    }
    Ok(Tok::Num(RawNum {
        digits: body.replace('_', ""),
        radix: 10,
        kind,
        suffixed,
    }))
}

fn radix(l: &mut Lexer, radix: u32) -> Result<Tok, LexError> {
    let digits = l.slice()[2..].replace('_', "");
    let sfx = suffix(l);
    let kind = match NumKind::from_suffix(sfx) {
        _ if sfx.is_empty() => NumKind::Int,
        Some((k, false)) if !k.is_float() => k,
        _ => return fail("hex and binary literals only take the `_i8` and `_i64` suffixes"),
    };
    let rest = l.remainder();
    if sfx.is_empty()
        && rest.starts_with('.')
        && rest[1..].starts_with(|c: char| c.is_ascii_digit())
    {
        return Err(LexError {
            msg: Some("hex and binary literals are integers".into()),
            end: Some(l.span().end + 1),
        });
    }
    Ok(Tok::Num(RawNum {
        digits,
        radix,
        kind,
        suffixed: !sfx.is_empty(),
    }))
}

pub fn lex(src: &str, file: FileId) -> Result<Vec<Token>, Diagnostic> {
    let mut out: Vec<Token> = vec![];
    let mut l = Raw::lexer(src);
    while let Some(r) = l.next() {
        let span = l.span();
        match r {
            Ok(Raw::T(tok)) => out.push(Token {
                tok,
                span: Span::new(file, span.start, span.end),
                joint: false,
            }),
            Err(e) => {
                let msg = e.msg.unwrap_or_else(|| {
                    let ch = src[span.start..].chars().next().unwrap_or('?');
                    format!("unexpected character {ch:?}")
                });
                let end = e.end.unwrap_or(span.end).max(span.start + 1);
                return Err(Diagnostic::lex(Span::new(file, span.start, end), msg));
            }
        }
    }
    out.push(Token {
        tok: Tok::Eof,
        span: Span::new(file, src.len(), src.len()),
        joint: false,
    });
    for i in 1..out.len() {
        let joint = out[i - 1].span.end == out[i].span.start && out[i].tok != Tok::Eof;
        let glued = joint && matches!(out[i].tok, Tok::Num(_) | Tok::Kw(Kw::Inf | Kw::InfF32));
        let prev = &mut out[i - 1];
        prev.joint = joint;
        if let Tok::Minus { glued: g } = &mut prev.tok {
            *g = glued;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(s, FileId(0))
            .unwrap()
            .into_iter()
            .map(|t| t.tok)
            .collect()
    }
    fn num(s: &str) -> RawNum {
        match &toks(s)[0] {
            Tok::Num(n) => n.clone(),
            t => panic!("{t:?}"),
        }
    }
    fn err(s: &str) -> String {
        lex(s, FileId(0)).unwrap_err().message
    }

    #[test]
    fn numeric_forms() {
        assert_eq!(num("1_000").digits, "1000");
        assert_eq!(num("1_000").kind, NumKind::Int);
        assert_eq!(num("128b").kind, NumKind::Byte);
        assert_eq!(num("1L").kind, NumKind::Long);
        assert_eq!(num("1_i64").kind, NumKind::Long);
        assert_eq!(num("1.5").kind, NumKind::F64);
        assert_eq!(num("1e10").kind, NumKind::F64);
        assert_eq!(num("2.0e-7").digits, "2.0e-7");
        assert_eq!(num("1_000.5_0e1_0f").digits, "1000.50e10");
        assert_eq!(num("0.1f").kind, NumKind::F32);
        assert_eq!(num("0.1_f32").kind, NumKind::F32);
        assert_eq!(num("0.1_f64").kind, NumKind::F64);
        let h = num("0xFFFF_FFFF");
        assert_eq!(
            (h.radix, h.digits.as_str(), h.kind),
            (16, "FFFFFFFF", NumKind::Int)
        );
        assert_eq!(num("0x80_i8").kind, NumKind::Byte);
        assert_eq!(num("0b101_i64").kind, NumKind::Long);
        let zero_byte = num("0b");
        assert_eq!(
            (zero_byte.radix, zero_byte.digits.as_str(), zero_byte.kind),
            (10, "0", NumKind::Byte)
        );
        // 0x1b and 0x1f are hex digits, not suffixes
        assert_eq!(num("0x1b").digits, "1b");
        assert_eq!(num("0x1f").kind, NumKind::Int);
        assert_eq!(num("0x1_f32").digits, "1f32");
    }

    #[test]
    fn rejected_literals() {
        assert!(err("0x1L").contains("suffix"));
        assert!(err("0b1b").contains("suffix"));
        assert!(err("1.").contains("both sides"));
        assert!(err("1f").contains("`.` or an exponent"));
        assert!(err("1.5b").contains("float"));
        assert!(err("1x").contains("suffix"));
        assert!(err("0x").contains("digits"));
        assert!(err("0x.5").contains("digits"));
        assert!(err("0x1.5").contains("integers"));
        // `_` only goes between digits, or before a long suffix
        assert!(err("1__0").contains("misplaced"));
        assert!(err("1_").contains("misplaced"));
        assert!(err("0xFF_").contains("suffix"));
        assert!(err("0x1__2").contains("suffix"));
        // the radix prefixes are lowercase
        assert!(err("0X10").contains("suffix"));
        assert!(err("0B1").contains("suffix"));
    }

    #[test]
    fn a_dot_after_an_int_is_a_field_access() {
        assert_eq!(
            &toks("1.x")[..3],
            &[Tok::Num(num("1")), Tok::Dot, toks("x")[0].clone()]
        );
        assert_eq!(toks("1..")[1], Tok::Dot);
        assert_eq!(toks("1.e5")[1], Tok::Dot);
    }

    #[test]
    fn dot_five_is_not_a_literal() {
        let t = toks(".5");
        assert_eq!(t[0], Tok::Dot);
        assert!(matches!(t[1], Tok::Num(_)));
    }

    #[test]
    fn glued_minus() {
        let g = |s: &str| matches!(toks(s)[0], Tok::Minus { glued: true });
        assert!(g("-1"));
        assert!(g("-inf"));
        assert!(g("-inf_f32"));
        assert!(!g("- 1"));
        assert!(!g("-(1)"));
        assert!(!g("-info"));
        assert!(!g("-x"));
        assert!(!g("-//c\n1"));
        assert_eq!(toks("->")[0], Tok::Arrow);
    }

    #[test]
    fn labels() {
        let n = |text: &str, quoted, suffix| Tok::Name {
            text: text.into(),
            quoted,
            suffix,
        };
        assert_eq!(toks("a#3")[0], n("a", false, Suffix::Hash(3)));
        assert_eq!(toks("a##3")[0], n("a", false, Suffix::HashHash(3)));
        assert_eq!(toks("`a:b/c`#7")[0], n("a:b/c", true, Suffix::Hash(7)));
        assert_eq!(toks("%12")[0], Tok::Anon(12));
        assert_eq!(toks("`a\\`b\\\\`")[0], n("a`b\\", true, Suffix::None));
        assert_eq!(toks("`let`")[0], n("let", true, Suffix::None));
        assert!(matches!(toks("let")[0], Tok::Kw(Kw::Let)));
        assert!(err("a#").contains("slot number"));
        assert!(err("a##x").contains("slot number"));
        assert!(err("%").contains("slot number"));
        assert!(err("%99999999999999999999999").contains("out of range"));
        assert!(err("`abc").contains("unterminated"));
        assert!(err("`ab\ncd`").contains("unterminated"));
        assert!(err("@").contains("after `@`"));
    }

    #[test]
    fn strings_and_comments() {
        assert_eq!(
            toks("\"a\\n\\u{41}\\\"\" // c\n")[0],
            Tok::Str("a\nA\"".into())
        );
        assert_eq!(toks("\"multi\nline\"")[0], Tok::Str("multi\nline".into()));
        assert!(err("\"abc").contains("unterminated"));
        assert!(err("\"\\q\"").contains("escape"));
        assert!(err("\"\\u{110000}\"").contains("unicode"));
        assert!(err("\"\\u{+41}\"").contains("unicode"));
        assert!(err("\"\\u{41\"").contains("unterminated"));
        assert!(err("\"\\u41\"").contains("`{`"));
        assert!(err("\"\\").contains("unterminated"));
        assert_eq!(toks("// only a comment").len(), 1);
    }

    #[test]
    fn joint_flags() {
        let t = lex(">>= > =", FileId(0)).unwrap();
        assert!(t[0].joint && t[1].joint && !t[2].joint && !t[3].joint);
        let t = lex(">//c\n>", FileId(0)).unwrap();
        assert!(!t[0].joint);
    }

    #[test]
    fn spans() {
        let t = lex("ab  `c`#1 12L", FileId(0)).unwrap();
        let s: Vec<_> = t.iter().map(|t| (t.span.start, t.span.end)).collect();
        assert_eq!(s, [(0, 2), (4, 9), (10, 13), (13, 13)]);
        let d = lex("x 1.;", FileId(0)).unwrap_err();
        assert_eq!((d.span.start, d.span.end), (2, 4));
    }

    #[test]
    fn stray_bytes_are_errors_not_panics() {
        assert!(err("\u{0}").contains("unexpected"));
        assert!(err("é").contains("unexpected"));
        assert!(err("#").contains("unexpected"));
    }
}
