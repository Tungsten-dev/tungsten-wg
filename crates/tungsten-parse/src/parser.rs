//! Recursive descent with precedence climbing. Never panics: every failure is a `Diagnostic`.

use tungsten_ir::{BinaryOperator, UnaryOperator, VariableType};

use crate::ast::*;
use crate::lexer::{Kw, Suffix, Tok, Token, lex};
use crate::numlit;
use crate::span::{Diagnostic, FileId, SourceFiles, Span};
use crate::stack;
use crate::tables::{self, OpClass};

type R<T> = Result<T, Diagnostic>;

pub fn parse(src: &str, file: FileId) -> Result<File, Diagnostic> {
    let attach = |d: Diagnostic| d.with_files(SourceFiles::single(file, None, src));
    let toks = lex(src, file).map_err(attach)?;
    let mut p = Parser {
        toks,
        pos: 0,
        rec: 0,
        depth: [0; 3],
    };
    let (directives, program) = p.file().map_err(attach)?;
    Ok(File {
        file,
        src: src.to_string(),
        directives,
        program,
    })
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// Recursion depth, only a stack guard (`MAX_RECURSION`).
    rec: u32,
    /// Nesting depth per `Nest` kind (`MAX_DEPTH` each). Expressions count theirs in `Expr.depth`.
    depth: [u32; 3],
}

#[derive(Clone, Copy)]
enum Nest {
    Block,
    Type,
    Value,
}

impl Nest {
    fn what(self) -> &'static str {
        match self {
            Nest::Block => "blocks",
            Nest::Type => "types",
            Nest::Value => "values",
        }
    }
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Name { text, .. } => format!("`{text}`"),
        Tok::Anon(n) => format!("`%{n}`"),
        Tok::Kw(k) => format!("`{}`", k.as_str()),
        Tok::Attr(a) => format!("`@{a}`"),
        Tok::Str(_) => "a string".into(),
        Tok::Num(_) => "a number".into(),
        Tok::Eof => "end of file".into(),
        t => format!("`{}`", punct_text(t)),
    }
}

fn punct_text(t: &Tok) -> &'static str {
    match t {
        Tok::LBrace => "{",
        Tok::RBrace => "}",
        Tok::LParen => "(",
        Tok::RParen => ")",
        Tok::LBracket => "[",
        Tok::RBracket => "]",
        Tok::Comma => ",",
        Tok::Semi => ";",
        Tok::Colon => ":",
        Tok::ColonColon => "::",
        Tok::Dot => ".",
        Tok::Arrow => "->",
        Tok::Assign => "=",
        Tok::EqEq => "==",
        Tok::NotEq => "!=",
        Tok::Lt => "<",
        Tok::Le => "<=",
        Tok::Gt => ">",
        Tok::Shl => "<<",
        Tok::Plus => "+",
        Tok::Minus { .. } => "-",
        Tok::Star => "*",
        Tok::Slash => "/",
        Tok::Amp => "&",
        Tok::AmpAmp => "&&",
        Tok::Pipe => "|",
        Tok::PipePipe => "||",
        Tok::Caret => "^",
        Tok::Bang => "!",
        _ => "?",
    }
}

impl Parser {
    // ----- token plumbing -----

    fn cur(&self) -> &Token {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }
    fn tok(&self) -> &Tok {
        &self.cur().tok
    }
    fn at(&self, n: usize) -> &Token {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)]
    }
    fn span(&self) -> Span {
        self.cur().span
    }
    fn prev_span(&self) -> Span {
        self.toks[self.pos.saturating_sub(1).min(self.toks.len() - 1)].span
    }
    fn bump(&mut self) -> Span {
        let span = self.span();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        span
    }
    /// Moves the current token out (there is no backtracking) and advances.
    fn take(&mut self) -> Tok {
        if self.pos == self.toks.len() - 1 {
            return Tok::Eof;
        }
        self.pos += 1;
        std::mem::replace(&mut self.toks[self.pos - 1].tok, Tok::Eof)
    }
    fn is(&self, t: &Tok) -> bool {
        self.tok() == t
    }
    fn is_kw(&self, k: Kw) -> bool {
        matches!(self.tok(), Tok::Kw(x) if *x == k)
    }
    fn eat(&mut self, t: &Tok) -> bool {
        let hit = self.is(t);
        if hit {
            self.bump();
        }
        hit
    }
    fn eat_kw(&mut self, k: Kw) -> bool {
        let hit = self.is_kw(k);
        if hit {
            self.bump();
        }
        hit
    }

    fn fail<T>(&self, span: Span, msg: impl Into<String>) -> R<T> {
        Err(Diagnostic::parse(span, msg))
    }

    fn unexpected<T>(&self, expected: &str) -> R<T> {
        self.fail(
            self.span(),
            format!("expected {expected}, found {}", describe(self.tok())),
        )
    }

    fn expect(&mut self, t: Tok) -> R<Span> {
        if self.is(&t) {
            Ok(self.bump())
        } else {
            self.unexpected(&format!("`{}`", punct_text(&t)))
        }
    }
    fn expect_kw(&mut self, k: Kw) -> R<Span> {
        if self.is_kw(k) {
            Ok(self.bump())
        } else {
            self.unexpected(&format!("`{}`", k.as_str()))
        }
    }

    /// Runs a recursive step on a stack that can grow, within `MAX_RECURSION`.
    fn guarded<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        if self.rec >= MAX_RECURSION {
            return self.fail(
                self.span(),
                format!("nesting is too deep (more than {MAX_RECURSION} levels of recursion)"),
            );
        }
        self.rec += 1;
        let r = stack::grow(|| f(self));
        self.rec -= 1;
        r
    }

    /// One more level of `n`, within `MAX_DEPTH`.
    fn nest<T>(&mut self, n: Nest, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        let i = n as usize;
        if self.depth[i] >= MAX_DEPTH {
            return self.fail(
                self.span(),
                format!(
                    "{} are nested too deeply (the limit is {MAX_DEPTH})",
                    n.what()
                ),
            );
        }
        self.depth[i] += 1;
        let r = self.guarded(f);
        self.depth[i] -= 1;
        r
    }

    /// `item ("," item)* ","?`, then `close`.
    fn comma_list<T>(&mut self, close: Tok, mut item: impl FnMut(&mut Self) -> R<T>) -> R<Vec<T>> {
        let mut out = vec![];
        while !self.is(&close) {
            out.push(item(self)?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(close)?;
        Ok(out)
    }

    // ----- names -----

    fn label(&mut self) -> R<Label> {
        let span = self.span();
        let kind = match self.tok() {
            Tok::Name { suffix, .. } => match *suffix {
                Suffix::None => LabelKind::Plain,
                Suffix::Hash(n) => LabelKind::Hash(n),
                Suffix::HashHash(n) => LabelKind::HashHash(n),
            },
            Tok::Anon(n) => LabelKind::Anon(*n),
            _ => return self.unexpected("a label"),
        };
        let (name, quoted) = match self.take() {
            Tok::Name { text, quoted, .. } => (text, quoted),
            _ => (String::new(), false),
        };
        Ok(Label {
            name,
            quoted,
            kind,
            span,
        })
    }

    fn at_label(&self) -> bool {
        matches!(self.tok(), Tok::Name { .. } | Tok::Anon(_))
    }

    /// A name with no label suffix: plain or quoted.
    fn plain_name(&mut self, what: &str) -> R<(String, Span)> {
        let span = self.span();
        if !matches!(
            self.tok(),
            Tok::Name {
                suffix: Suffix::None,
                ..
            }
        ) {
            return self.unexpected(what);
        }
        let Tok::Name { text, .. } = self.take() else {
            unreachable!()
        };
        Ok((text, span))
    }

    /// A plain, unquoted identifier equal to `word` (a contextual keyword).
    fn at_word(&self, word: &str) -> bool {
        matches!(self.tok(), Tok::Name { text, quoted: false, suffix: Suffix::None } if text == word)
    }
    fn expect_word(&mut self, word: &str) -> R<Span> {
        if self.at_word(word) {
            Ok(self.bump())
        } else {
            self.unexpected(&format!("`{word}`"))
        }
    }

    fn labels(&mut self, close: Tok) -> R<Vec<Label>> {
        self.comma_list(close, Self::label)
    }

    /// The digits of an unsuffixed integer literal at the cursor, with their radix.
    fn int_digits(&self, radix: Option<u32>) -> Option<(&str, u32)> {
        match self.tok() {
            Tok::Num(RawNum {
                digits,
                radix: r,
                kind: NumKind::Int,
                suffixed: false,
            }) if radix.is_none_or(|x| x == *r) => Some((digits, *r)),
            _ => None,
        }
    }

    fn uint(&mut self) -> R<u64> {
        let span = self.span();
        let Some((digits, radix)) = self.int_digits(None) else {
            return self.unexpected("an unsigned integer");
        };
        let v = u64::from_str_radix(digits, radix);
        self.bump();
        v.or_else(|_| self.fail(span, "number out of range for u64"))
    }

    fn usize_(&mut self) -> R<usize> {
        let span = self.span();
        let v = self.uint()?;
        usize::try_from(v).or_else(|_| self.fail(span, "number out of range"))
    }

    /// A signed decimal integer literal, for coordinates and dims.
    fn int(&mut self) -> R<i64> {
        let span = self.span();
        let neg = matches!(self.tok(), Tok::Minus { glued: true });
        if neg {
            self.bump();
        }
        let Some((digits, _)) = self.int_digits(Some(10)) else {
            return self.unexpected("a decimal integer");
        };
        let v = format!("{}{digits}", if neg { "-" } else { "" }).parse::<i64>();
        self.bump();
        v.or_else(|_| self.fail(span.to(self.prev_span()), "number out of range"))
    }

    fn string(&mut self) -> R<String> {
        match self.tok() {
            Tok::Str(_) => match self.take() {
                Tok::Str(s) => Ok(s),
                _ => unreachable!(),
            },
            _ => self.unexpected("a string"),
        }
    }

    // ----- literals -----

    fn at_literal(&self) -> bool {
        match self.tok() {
            Tok::Num(_) | Tok::Minus { glued: true } => true,
            Tok::Kw(k) => matches!(
                k,
                Kw::True | Kw::False | Kw::Inf | Kw::InfF32 | Kw::Nan | Kw::NanF32
            ),
            _ => false,
        }
    }

    /// With `natural`, the literal is checked against its own kind here. Attribute positions
    /// check it later, against the kind they expect.
    fn literal(&mut self, natural: bool) -> R<Lit> {
        let start = self.span();
        let neg = matches!(self.tok(), Tok::Minus { glued: true });
        if neg {
            self.bump();
        }
        let kind = match self.tok() {
            Tok::Num(_) => match self.take() {
                Tok::Num(raw) => LitKind::Num { raw, neg },
                _ => unreachable!(),
            },
            Tok::Kw(k @ (Kw::Inf | Kw::InfF32)) => {
                let f32 = *k == Kw::InfF32;
                self.bump();
                LitKind::Inf { neg, f32 }
            }
            Tok::Kw(k @ (Kw::True | Kw::False)) if !neg => {
                let b = *k == Kw::True;
                self.bump();
                LitKind::Bool(b)
            }
            Tok::Kw(k @ (Kw::Nan | Kw::NanF32)) if !neg => {
                let f32 = *k == Kw::NanF32;
                self.bump();
                self.nan(f32)?
            }
            _ => return self.unexpected("a literal"),
        };
        let span = start.to(self.prev_span());
        if natural && !matches!(kind, LitKind::Bool(_)) {
            numlit::convert(&kind, None).or_else(|m| self.fail(span, m))?;
        }
        Ok(Lit { span, kind })
    }

    fn nan(&mut self, f32: bool) -> R<LitKind> {
        if !self.eat(&Tok::LParen) {
            return Ok(LitKind::Nan {
                bits: 0,
                f32,
                explicit: false,
            });
        }
        let span = self.span();
        let Some((digits, _)) = self.int_digits(Some(16)) else {
            return self.unexpected("hex NaN bits like `0x7ff8000000000001`");
        };
        let max_digits = if f32 { 8 } else { 16 };
        let bits = u64::from_str_radix(digits, 16)
            .ok()
            .filter(|_| digits.len() <= max_digits);
        self.bump();
        let Some(bits) = bits else {
            return self.fail(span, format!("at most {max_digits} hex digits"));
        };
        let is_nan = if f32 {
            bits & 0x7f80_0000 == 0x7f80_0000 && bits & 0x007f_ffff != 0
        } else {
            bits & 0x7ff0_0000_0000_0000 == 0x7ff0_0000_0000_0000
                && bits & 0x000f_ffff_ffff_ffff != 0
        };
        if !is_nan {
            return self.fail(span, "these bits don't encode a NaN");
        }
        self.expect(Tok::RParen)?;
        Ok(LitKind::Nan {
            bits,
            f32,
            explicit: true,
        })
    }

    /// An attribute-position number: any non-bool literal.
    fn num(&mut self) -> R<Lit> {
        let l = self.literal(false)?;
        if matches!(l.kind, LitKind::Bool(_)) {
            return self.fail(l.span, "expected a number");
        }
        Ok(l)
    }

    fn triple(&mut self) -> R<[Lit; 3]> {
        self.expect(Tok::LParen)?;
        let a = self.num()?;
        self.expect(Tok::Comma)?;
        let b = self.num()?;
        self.expect(Tok::Comma)?;
        let c = self.num()?;
        self.expect(Tok::RParen)?;
        Ok([a, b, c])
    }

    // ----- types -----

    fn ty(&mut self) -> R<Ty> {
        self.nest(Nest::Type, |p| {
            let span = p.span();
            let kind = match p.tok() {
                Tok::Kw(Kw::Extern) => {
                    p.bump();
                    TyKind::Extern(p.plain_name("an extern type name")?.0)
                }
                Tok::Kw(k) => {
                    let Some(t) = tables::prim_type(*k) else {
                        return p.unexpected("a type");
                    };
                    p.bump();
                    TyKind::Prim(t)
                }
                Tok::LBracket => {
                    p.bump();
                    let elem = p.ty()?;
                    p.expect(Tok::Semi)?;
                    let n = p.usize_()?;
                    p.expect(Tok::RBracket)?;
                    TyKind::Array(Box::new(elem), n)
                }
                _ => return p.unexpected("a type"),
            };
            Ok(Ty {
                span: span.to(p.prev_span()),
                kind,
            })
        })
    }

    // ----- file -----

    fn file(&mut self) -> R<(Vec<Directive>, Program)> {
        let mut directives = vec![];
        let mut program = None;
        loop {
            match self.tok() {
                Tok::Eof => break,
                Tok::Attr(_) => directives.push(self.directive()?),
                Tok::Kw(Kw::Program) => {
                    if program.is_some() {
                        return self.fail(self.span(), "a file has one `program`");
                    }
                    program = Some(self.program()?);
                }
                _ => return self.unexpected("`program` or a directive"),
            }
        }
        match program {
            Some(p) => Ok((directives, p)),
            None => self.fail(self.span(), "missing `program { … }`"),
        }
    }

    fn program(&mut self) -> R<Program> {
        let start = self.expect_kw(Kw::Program)?;
        self.expect(Tok::LBrace)?;
        let mut items = vec![];
        while !self.is(&Tok::RBrace) {
            items.push(self.item()?);
        }
        let end = self.expect(Tok::RBrace)?;
        Ok(Program {
            span: start.to(end),
            items,
        })
    }

    fn item(&mut self) -> R<Item> {
        let start = self.span();
        match self.tok() {
            Tok::Kw(Kw::Use) => {
                self.bump();
                let (library, library_span) = self.plain_name("a library name")?;
                self.expect(Tok::ColonColon)?;
                self.expect(Tok::LBrace)?;
                let names = self.labels(Tok::RBrace)?;
                self.expect(Tok::Semi)?;
                Ok(Item::Use(UseItem {
                    span: start.to(self.prev_span()),
                    library,
                    library_span,
                    names,
                }))
            }
            Tok::Kw(Kw::Host) => {
                self.bump();
                let label = self.label()?;
                let ty = if self.eat_kw(Kw::Dynamic) {
                    None
                } else {
                    self.expect(Tok::Colon)?;
                    Some(self.ty()?)
                };
                self.expect(Tok::Semi)?;
                Ok(Item::Host(HostItem {
                    span: start.to(self.prev_span()),
                    label,
                    ty,
                }))
            }
            Tok::Kw(Kw::Table) => {
                self.bump();
                let label = self.label()?;
                self.expect(Tok::Assign)?;
                let def = if self.eat_kw(Kw::Base3d) {
                    TableDef::Base3d
                } else {
                    self.expect_kw(Kw::Perlin)?;
                    self.expect(Tok::LParen)?;
                    let ident = self.string()?;
                    self.expect(Tok::Comma)?;
                    let subident = if self.eat_kw(Kw::None) {
                        None
                    } else {
                        Some(self.string()?)
                    };
                    self.expect(Tok::Comma)?;
                    let index = self.uint()?;
                    self.expect(Tok::RParen)?;
                    TableDef::Perlin {
                        ident,
                        subident,
                        index,
                    }
                };
                self.expect(Tok::Semi)?;
                Ok(Item::Table(TableItem {
                    span: start.to(self.prev_span()),
                    label,
                    def,
                }))
            }
            Tok::Kw(Kw::Const) => {
                self.bump();
                let label = self.label()?;
                self.expect(Tok::Assign)?;
                let value = self.expr()?;
                self.expect(Tok::Semi)?;
                Ok(Item::Const(ConstItem {
                    span: start.to(self.prev_span()),
                    label,
                    value,
                }))
            }
            Tok::Kw(Kw::Fn) => Ok(Item::Fn(self.fn_item()?)),
            Tok::Kw(Kw::Unit) => Ok(Item::Unit(self.unit_item()?)),
            Tok::Kw(Kw::Entry) => {
                self.bump();
                let label = if self.at_label() && self.at(1).tok == Tok::Assign {
                    let l = self.label()?;
                    self.bump();
                    Some(l)
                } else {
                    None
                };
                let unit = self.label()?;
                let (grid, _) = self.grid_attrs(false)?;
                self.expect(Tok::Semi)?;
                Ok(Item::Entry(Box::new(EntryItem {
                    span: start.to(self.prev_span()),
                    label,
                    unit,
                    grid,
                })))
            }
            _ => {
                self.unexpected("an item (`fn`, `unit`, `entry`, `const`, `host`, `table`, `use`)")
            }
        }
    }

    /// The `@name` at the cursor, consumed: its name and span.
    fn attr(&mut self) -> Option<(String, Span)> {
        if !matches!(self.tok(), Tok::Attr(_)) {
            return None;
        }
        let span = self.span();
        match self.take() {
            Tok::Attr(name) => Some((name, span)),
            _ => unreachable!(),
        }
    }

    fn fn_item(&mut self) -> R<FnItem> {
        let start = self.expect_kw(Kw::Fn)?;
        let label = self.label()?;
        self.expect(Tok::LParen)?;
        let params = self.comma_list(Tok::RParen, |p| {
            let label = p.label()?;
            let ty = if p.eat(&Tok::Colon) {
                Some(p.ty()?)
            } else {
                None
            };
            Ok(Param { label, ty })
        })?;
        self.expect(Tok::Arrow)?;
        let ret = self.ty()?;
        let mut hash = None;
        let mut hosts = None;
        while let Some((name, span)) = self.attr() {
            match name.as_str() {
                "hash" if hash.is_none() => {
                    self.expect(Tok::LParen)?;
                    hash = Some(self.uint()?);
                    self.expect(Tok::RParen)?;
                }
                "hosts" if hosts.is_none() => {
                    self.expect(Tok::LParen)?;
                    let list = self.labels(Tok::RParen)?;
                    hosts = Some((list, span.to(self.prev_span())));
                }
                "hash" | "hosts" => return self.fail(span, format!("duplicate `@{name}`")),
                _ => return self.fail(span, format!("unknown fn attribute `@{name}`")),
            }
        }
        self.expect(Tok::LBrace)?;
        let vars = if self.eat_kw(Kw::Vars) {
            self.expect(Tok::LBrace)?;
            self.comma_list(Tok::RBrace, |p| {
                let l = p.label()?;
                p.expect(Tok::Colon)?;
                Ok((l, p.ty()?))
            })?
        } else {
            vec![]
        };
        let body = self.stmts_until_rbrace()?;
        Ok(FnItem {
            span: start.to(self.prev_span()),
            label,
            params,
            ret,
            hash,
            hosts,
            vars,
            body,
        })
    }

    fn unit_item(&mut self) -> R<UnitItem> {
        let start = self.expect_kw(Kw::Unit)?;
        let label = self.label()?;
        let mut u = UnitItem {
            span: start,
            label,
            hash: None,
            main: None,
            helpers: None,
            tables: None,
            hosts: None,
            consts: None,
            inputs: vec![],
        };
        if let Some((a, span)) = self.attr() {
            if a != "hash" {
                return self.fail(span, format!("unknown unit attribute `@{a}`"));
            }
            self.expect(Tok::LParen)?;
            u.hash = Some(self.uint()?);
            self.expect(Tok::RParen)?;
        }
        self.expect(Tok::LBrace)?;
        while !self.is(&Tok::RBrace) {
            let span = self.span();
            match *self.tok() {
                Tok::Kw(Kw::Main) => {
                    self.bump();
                    if u.main.is_some() {
                        return self.fail(span, "duplicate `main`");
                    }
                    u.main = Some(self.label()?);
                    self.expect(Tok::Semi)?;
                }
                Tok::Kw(k @ (Kw::Helpers | Kw::Tables | Kw::Hosts | Kw::Consts)) => {
                    self.bump();
                    self.expect(Tok::LBracket)?;
                    let list = self.labels(Tok::RBracket)?;
                    self.expect(Tok::Semi)?;
                    let slot = match k {
                        Kw::Helpers => &mut u.helpers,
                        Kw::Tables => &mut u.tables,
                        Kw::Hosts => &mut u.hosts,
                        _ => &mut u.consts,
                    };
                    if slot.replace(list).is_some() {
                        return self.fail(span, format!("duplicate `{}` list", k.as_str()));
                    }
                }
                Tok::Kw(Kw::Input) => {
                    self.bump();
                    let label = self.label()?;
                    self.expect(Tok::Assign)?;
                    let target = self.label()?;
                    let (mut grid, var) = self.grid_attrs(true)?;
                    self.expect(Tok::Semi)?;
                    grid.span = span.to(self.prev_span());
                    u.inputs.push(InputItem {
                        span: span.to(self.prev_span()),
                        label,
                        target,
                        grid,
                        var,
                    });
                }
                _ => return self.unexpected("`main`, a list, or `input`"),
            }
        }
        self.expect(Tok::RBrace)?;
        u.span = start.to(self.prev_span());
        Ok(u)
    }

    /// `@dims(..) @origin_scale(..) @position_scale(..)` in any order, plus `@var(f.v)` among
    /// them when `allow_var`.
    fn grid_attrs(&mut self, allow_var: bool) -> R<(GridAttrs, Option<(Label, Label)>)> {
        let mut g = GridAttrs {
            span: self.span(),
            dims: None,
            origin_scale: None,
            position_scale: None,
        };
        let mut var = None;
        while let Some((name, span)) = self.attr() {
            let dup = format!("duplicate `@{name}`");
            match name.as_str() {
                "dims" => {
                    self.expect(Tok::LParen)?;
                    let x = self.int()?;
                    self.expect(Tok::Comma)?;
                    let y = self.int()?;
                    self.expect(Tok::Comma)?;
                    let z = self.int()?;
                    self.expect(Tok::RParen)?;
                    if g.dims
                        .replace(([x, y, z], span.to(self.prev_span())))
                        .is_some()
                    {
                        return self.fail(span, dup);
                    }
                }
                "origin_scale" | "position_scale" => {
                    let t = (self.triple()?, span.to(self.prev_span()));
                    let slot = if name == "origin_scale" {
                        &mut g.origin_scale
                    } else {
                        &mut g.position_scale
                    };
                    if slot.replace(t).is_some() {
                        return self.fail(span, dup);
                    }
                }
                "var" if allow_var => {
                    self.expect(Tok::LParen)?;
                    let f = self.label()?;
                    self.expect(Tok::Dot)?;
                    let v = self.label()?;
                    self.expect(Tok::RParen)?;
                    if var.replace((f, v)).is_some() {
                        return self.fail(span, dup);
                    }
                }
                _ => return self.fail(span, format!("unknown attribute `@{name}` here")),
            }
        }
        g.span = g.span.to(self.prev_span());
        Ok((g, var))
    }

    // ----- statements -----

    fn stmts_until_rbrace(&mut self) -> R<Vec<Stmt>> {
        let mut out = vec![];
        while !self.is(&Tok::RBrace) {
            out.push(self.stmt()?);
        }
        self.expect(Tok::RBrace)?;
        Ok(out)
    }

    fn block(&mut self) -> R<Vec<Stmt>> {
        self.expect(Tok::LBrace)?;
        self.nest(Nest::Block, |p| p.stmts_until_rbrace())
    }

    fn stmt(&mut self) -> R<Stmt> {
        let start = self.span();
        let kind = match self.tok() {
            Tok::Kw(Kw::Let) => {
                self.bump();
                let label = self.label()?;
                if !self.eat(&Tok::Colon) {
                    return self.fail(
                        self.span(),
                        "a `let` needs a type: `let x: T = …;` (there is no inference)",
                    );
                }
                let ty = self.ty()?;
                let init = if self.eat(&Tok::Assign) {
                    Some(self.expr()?)
                } else {
                    None
                };
                self.expect(Tok::Semi)?;
                StmtKind::Let { label, ty, init }
            }
            Tok::Kw(Kw::Return) => {
                self.bump();
                let e = self.expr()?;
                self.expect(Tok::Semi)?;
                StmtKind::Return(e)
            }
            Tok::Kw(Kw::Break) => {
                self.bump();
                self.expect(Tok::Semi)?;
                StmtKind::Break
            }
            Tok::Kw(Kw::If) => return self.if_stmt(),
            Tok::Kw(Kw::While) => {
                self.bump();
                let cond = self.expr()?;
                let body = self.block()?;
                StmtKind::While { cond, body }
            }
            Tok::Kw(Kw::Repeat) => {
                self.bump();
                let count = self.usize_()?;
                let body = self.block()?;
                StmtKind::Repeat { count, body }
            }
            Tok::Name { .. } | Tok::Anon(_) => {
                let target = self.label()?;
                self.expect(Tok::Assign)?;
                let value = self.expr()?;
                self.expect(Tok::Semi)?;
                StmtKind::Assign { target, value }
            }
            _ => return self.unexpected("a statement"),
        };
        Ok(Stmt {
            span: start.to(self.prev_span()),
            kind,
        })
    }

    fn if_stmt(&mut self) -> R<Stmt> {
        let start = self.expect_kw(Kw::If)?;
        let cond = self.expr()?;
        let then = self.block()?;
        let els = if !self.eat_kw(Kw::Else) {
            None
        } else if self.is_kw(Kw::If) {
            // `else if` is an `else` block holding one `if`, one level deeper
            Some(vec![self.nest(Nest::Block, |p| p.if_stmt())?])
        } else {
            Some(self.block()?)
        };
        Ok(Stmt {
            span: start.to(self.prev_span()),
            kind: StmtKind::If { cond, then, els },
        })
    }

    // ----- expressions -----

    fn mk(&self, kind: ExprKind, span: Span) -> R<Expr> {
        let mut depth = 0;
        kind.for_each_child(|c| depth = depth.max(c.depth));
        let depth = depth + 1;
        if depth > MAX_DEPTH {
            return self.fail(
                span,
                format!("expressions are nested too deeply (the limit is {MAX_DEPTH})"),
            );
        }
        Ok(Expr {
            span,
            kind,
            paren: false,
            depth,
        })
    }

    pub fn expr(&mut self) -> R<Expr> {
        self.guarded(|p| p.binary(0))
    }

    /// The binary operator at the cursor and its number of tokens. `>>`, `>>>` and `>=` are
    /// built from joint tokens.
    fn peek_binop(&self) -> Option<(BinaryOperator, usize)> {
        let t = self.cur();
        let (text, len) = match t.tok {
            Tok::Gt if t.joint && self.at(1).tok == Tok::Gt => {
                if self.at(1).joint && self.at(2).tok == Tok::Gt {
                    (">>>", 3)
                } else {
                    (">>", 2)
                }
            }
            Tok::Gt if t.joint && self.at(1).tok == Tok::Assign => (">=", 2),
            ref tok => (punct_text(tok), 1),
        };
        tables::op_from_text(text).map(|op| (op, len))
    }

    fn binary(&mut self, min_prec: u8) -> R<Expr> {
        let mut lhs = self.cast()?;
        while let Some((op, len)) = self.peek_binop() {
            let prec = tables::op_prec(op);
            if prec < min_prec {
                break;
            }
            for _ in 0..len {
                self.bump();
            }
            let rhs = self.binary(prec + 1)?;
            let span = lhs.span.to(rhs.span);
            for child in [&lhs, &rhs] {
                if let ExprKind::Binary(cop, ..) = child.kind
                    && !child.paren
                    && cop != op
                    && (tables::op_class(op).restricted() || tables::op_class(cop).restricted())
                {
                    return self.fail(
                        child.span,
                        format!(
                            "`{}` and `{}` can't be mixed without parentheses",
                            tables::op_text(cop),
                            tables::op_text(op)
                        ),
                    );
                }
            }
            lhs = self.mk(ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), span)?;
            if tables::op_class(op) == OpClass::Cmp
                && let Some((op2, _)) = self.peek_binop()
                && tables::op_class(op2) == OpClass::Cmp
            {
                return self.fail(
                    self.span(),
                    format!(
                        "comparisons don't associate: `{}` after `{}`; add parentheses",
                        tables::op_text(op2),
                        tables::op_text(op)
                    ),
                );
            }
        }
        Ok(lhs)
    }

    fn cast(&mut self) -> R<Expr> {
        let mut e = self.unary()?;
        while self.eat_kw(Kw::As) {
            let ty = self.ty()?;
            let span = e.span.to(ty.span);
            e = self.mk(ExprKind::Cast(Box::new(e), ty), span)?;
        }
        Ok(e)
    }

    fn unary(&mut self) -> R<Expr> {
        let start = self.span();
        let ops: &[UnaryOperator] = match self.tok() {
            Tok::Minus { glued: false } => {
                if self.cur().joint && matches!(self.at(1).tok, Tok::Kw(Kw::Nan | Kw::NanF32)) {
                    return self.fail(
                        start,
                        "`-nan` isn't allowed; spell the bits: `nan(0xfff8000000000000)`",
                    );
                }
                &[UnaryOperator::Negate]
            }
            Tok::Bang => &[UnaryOperator::Not],
            Tok::Amp | Tok::AmpAmp => {
                // `&&e` in operand position is two `&`
                let two = *self.tok() == Tok::AmpAmp;
                self.bump();
                let inner = UnaryOperator::Ref(self.eat_kw(Kw::Mut));
                let ops = [inner, UnaryOperator::Ref(false)];
                return self.unary_operand(start, if two { &ops[..] } else { &ops[..1] });
            }
            _ => return self.postfix(),
        };
        self.bump();
        self.unary_operand(start, ops)
    }

    /// The operand of the prefix operators `ops`, innermost first.
    fn unary_operand(&mut self, start: Span, ops: &[UnaryOperator]) -> R<Expr> {
        self.guarded(|p| {
            let mut e = p.unary()?;
            for op in ops {
                e = p.mk(ExprKind::Unary(*op, Box::new(e)), start.to(p.prev_span()))?;
            }
            Ok(e)
        })
    }

    fn postfix(&mut self) -> R<Expr> {
        let start = self.span();
        let mut e = self.primary()?;
        loop {
            let kind = match self.tok() {
                Tok::Dot => {
                    self.bump();
                    let (name, name_span) = self.plain_name("a field name")?;
                    let ty = if self.eat(&Tok::ColonColon) {
                        self.expect(Tok::Lt)?;
                        let t = self.ty()?;
                        let base = if self.eat(&Tok::Semi) {
                            Some(self.ty()?)
                        } else {
                            None
                        };
                        self.expect(Tok::Gt)?;
                        Some((t, base))
                    } else {
                        None
                    };
                    ExprKind::Field {
                        base: Box::new(e),
                        name,
                        name_span,
                        ty,
                    }
                }
                Tok::LBracket => {
                    self.bump();
                    let index = self.expr()?;
                    self.expect(Tok::RBracket)?;
                    ExprKind::Index {
                        base: Box::new(e),
                        index: Box::new(index),
                    }
                }
                _ => return Ok(e),
            };
            let span = start.to(self.prev_span());
            e = self.mk(kind, span)?;
        }
    }

    fn args(&mut self) -> R<Vec<Expr>> {
        self.expect(Tok::LParen)?;
        self.comma_list(Tok::RParen, Self::expr)
    }

    fn paren_label(&mut self) -> R<Label> {
        self.expect(Tok::LParen)?;
        let l = self.label()?;
        self.expect(Tok::RParen)?;
        Ok(l)
    }

    fn primary(&mut self) -> R<Expr> {
        let start = self.span();
        if self.at_literal() {
            let lit = self.literal(true)?;
            return self.mk(ExprKind::Lit(lit), start.to(self.prev_span()));
        }
        let kind = match *self.tok() {
            Tok::Name { .. } | Tok::Anon(_) => {
                let label = self.label()?;
                if self.is(&Tok::LParen) {
                    ExprKind::Call {
                        callee: label,
                        args: self.args()?,
                    }
                } else {
                    ExprKind::Name(label)
                }
            }
            Tok::Kw(k @ (Kw::Const | Kw::Host | Kw::Table)) => {
                self.bump();
                let l = self.paren_label()?;
                match k {
                    Kw::Const => ExprKind::Const(l),
                    Kw::Host => ExprKind::Host(l),
                    _ => ExprKind::Table(l),
                }
            }
            Tok::Kw(Kw::Density) => {
                self.bump();
                self.expect(Tok::LParen)?;
                let first = self.label()?;
                let (unit, input) = if self.eat(&Tok::Dot) {
                    (Some(first), self.label()?)
                } else {
                    (None, first)
                };
                let index = if self.eat(&Tok::Comma) {
                    Some(Box::new(self.expr()?))
                } else {
                    None
                };
                self.expect(Tok::RParen)?;
                ExprKind::Density { unit, input, index }
            }
            Tok::Kw(Kw::Ambient) => {
                self.bump();
                self.expect(Tok::LParen)?;
                let kind = match self.tok() {
                    Tok::Kw(k) => tables::ambient_from_text(k.as_str()),
                    Tok::Name {
                        text,
                        quoted: false,
                        suffix: Suffix::None,
                    } => tables::ambient_from_text(text),
                    _ => None,
                };
                let Some(kind) = kind else {
                    return self.unexpected(tables::AMBIENT_EXPECTED);
                };
                self.bump();
                self.expect(Tok::RParen)?;
                ExprKind::Ambient(kind)
            }
            Tok::Kw(Kw::Ext) => {
                self.bump();
                let (name, _) = self.plain_name("an extern function name")?;
                let sig = if self.eat(&Tok::ColonColon) {
                    self.expect(Tok::Lt)?;
                    self.expect(Tok::LParen)?;
                    let params = self.comma_list(Tok::RParen, Self::ty)?;
                    self.expect(Tok::Arrow)?;
                    let ret = self.ty()?;
                    self.expect(Tok::Gt)?;
                    Some((params, ret))
                } else {
                    None
                };
                ExprKind::ExtCall {
                    name,
                    sig,
                    args: self.args()?,
                }
            }
            Tok::Kw(k @ (Kw::Vec3 | Kw::Pos3)) => {
                self.bump();
                let ty = Ty {
                    span: start,
                    kind: TyKind::Prim(if k == Kw::Vec3 {
                        VariableType::Vec3
                    } else {
                        VariableType::Pos3
                    }),
                };
                ExprKind::Construct {
                    ty,
                    args: self.args()?,
                }
            }
            Tok::Kw(Kw::New) => {
                self.bump();
                let ty = self.new_ty()?;
                if self.eat(&Tok::LBrace) {
                    let fields = self.comma_list(Tok::RBrace, |p| {
                        let (n, ns) = p.plain_name("a field name")?;
                        p.expect(Tok::Colon)?;
                        Ok((n, ns, p.expr()?))
                    })?;
                    ExprKind::ConstructExtern { ty, fields }
                } else {
                    ExprKind::Construct {
                        ty,
                        args: self.args()?,
                    }
                }
            }
            Tok::LBracket => {
                self.bump();
                ExprKind::Array(self.comma_list(Tok::RBracket, Self::expr)?)
            }
            Tok::LParen => {
                self.bump();
                let mut e = self.expr()?;
                self.expect(Tok::RParen)?;
                e.paren = true;
                e.span = start.to(self.prev_span());
                return Ok(e);
            }
            _ => return self.unexpected("an expression"),
        };
        self.mk(kind, start.to(self.prev_span()))
    }

    /// The type after `new`. A bare name is an extern type.
    fn new_ty(&mut self) -> R<Ty> {
        if matches!(
            self.tok(),
            Tok::Name {
                suffix: Suffix::None,
                ..
            }
        ) {
            let (name, span) = self.plain_name("a type")?;
            return Ok(Ty {
                span,
                kind: TyKind::Extern(name),
            });
        }
        self.ty()
    }

    // ----- envelope -----

    fn directive(&mut self) -> R<Directive> {
        let start = self.span();
        let Some((name, _)) = self.attr() else {
            unreachable!()
        };
        let kind = match name.as_str() {
            "knownfail" => {
                self.expect(Tok::LParen)?;
                let (backend, _) = self.plain_name("a backend name")?;
                self.expect(Tok::Comma)?;
                let reason = self.string()?;
                let mut case = None;
                let mut stage = None;
                while self.eat(&Tok::Comma) {
                    if self.at_word("case") && case.is_none() {
                        self.bump();
                        self.expect(Tok::Assign)?;
                        case = Some(self.string()?);
                    } else if self.at_word("stage") && stage.is_none() {
                        self.bump();
                        self.expect(Tok::Assign)?;
                        let (s, sp) = self.plain_name("`original` or `inlined`")?;
                        if s != "original" && s != "inlined" {
                            return self.fail(sp, "stage must be `original` or `inlined`");
                        }
                        stage = Some(s);
                    } else {
                        return self.unexpected("`case = \"…\"` or `stage = …`");
                    }
                }
                self.expect(Tok::RParen)?;
                DirectiveKind::KnownFail {
                    backend,
                    reason,
                    case,
                    stage,
                }
            }
            "fuel" => {
                self.expect(Tok::LParen)?;
                let n = self.uint()?;
                self.expect(Tok::RParen)?;
                DirectiveKind::Fuel(n)
            }
            "tier" => {
                self.expect(Tok::LParen)?;
                let (t, _) = self.plain_name("a tier name")?;
                self.expect(Tok::RParen)?;
                DirectiveKind::Tier(t)
            }
            "cells" => {
                self.expect(Tok::LParen)?;
                self.expect_word("sample")?;
                self.expect(Tok::Assign)?;
                let sample = self.uint()?;
                self.expect(Tok::Comma)?;
                self.expect_word("seed")?;
                self.expect(Tok::Assign)?;
                let seed = self.uint()?;
                self.expect(Tok::RParen)?;
                DirectiveKind::Cells { sample, seed }
            }
            "case" => {
                let name = self.string()?;
                self.expect_word("origin")?;
                let origin = self.triple()?;
                self.expect_word("seed")?;
                self.expect(Tok::LParen)?;
                let seed = self.uint()?;
                self.expect(Tok::RParen)?;
                let mut hosts = vec![];
                while self.eat_kw(Kw::Host) {
                    self.expect(Tok::LParen)?;
                    hosts.extend(self.comma_list(Tok::RParen, |p| {
                        let l = p.label()?;
                        p.expect(Tok::Assign)?;
                        Ok((l, p.value()?))
                    })?);
                }
                DirectiveKind::Case {
                    name,
                    origin,
                    seed,
                    hosts,
                }
            }
            "expect" if self.at_word("reject") => {
                self.bump();
                let kind = if self.eat(&Tok::LParen) {
                    let (k, _) = self.plain_name("an error kind")?;
                    self.expect(Tok::RParen)?;
                    Some(k)
                } else {
                    None
                };
                DirectiveKind::ExpectReject(kind)
            }
            "expect" => {
                let case_span = self.span();
                let case = self.string()?;
                let expectation = self.expectation()?;
                DirectiveKind::Expect {
                    case,
                    case_span,
                    expectation,
                }
            }
            other => {
                return self.fail(start, format!("unknown directive `@{other}`"));
            }
        };
        Ok(Directive {
            span: start.to(self.prev_span()),
            kind,
        })
    }

    fn expectation(&mut self) -> R<Expectation> {
        if self.at_word("error") && self.at(1).tok == Tok::LParen {
            self.bump();
            self.bump();
            let (k, _) = self.plain_name("an error kind")?;
            self.expect(Tok::RParen)?;
            return Ok(Expectation::Error(k));
        }
        let entry = self.label()?;
        if self.eat(&Tok::LBracket) {
            if self.eat(&Tok::Star) {
                self.expect(Tok::RBracket)?;
                self.expect(Tok::Assign)?;
                let value = self.value()?;
                return Ok(Expectation::All { entry, value });
            }
            let x = self.int()?;
            self.expect(Tok::Comma)?;
            let y = self.int()?;
            self.expect(Tok::Comma)?;
            let z = self.int()?;
            self.expect(Tok::RBracket)?;
            self.expect(Tok::Assign)?;
            let value = self.value()?;
            return Ok(Expectation::Cell {
                entry,
                pos: [x, y, z],
                value,
            });
        }
        self.expect(Tok::Assign)?;
        self.expect(Tok::LBracket)?;
        let values = self.comma_list(Tok::RBracket, Self::value)?;
        Ok(Expectation::Grid { entry, values })
    }

    fn value(&mut self) -> R<AstValue> {
        self.nest(Nest::Value, |p| {
            let start = p.span();
            let kind = if p.at_literal() {
                AstValueKind::Lit(p.literal(false)?)
            } else {
                match *p.tok() {
                    Tok::Kw(k @ (Kw::Vec3 | Kw::Pos3)) => {
                        p.bump();
                        p.expect(Tok::LParen)?;
                        let mut vs = vec![p.value()?];
                        for _ in 0..2 {
                            p.expect(Tok::Comma)?;
                            vs.push(p.value()?);
                        }
                        p.expect(Tok::RParen)?;
                        if k == Kw::Vec3 {
                            AstValueKind::Vec3(vs)
                        } else {
                            AstValueKind::Pos3(vs)
                        }
                    }
                    Tok::LBracket => {
                        p.bump();
                        AstValueKind::Array(p.comma_list(Tok::RBracket, Self::value)?)
                    }
                    Tok::Kw(Kw::New) => {
                        p.bump();
                        let ty = p.new_ty()?;
                        p.expect(Tok::LBrace)?;
                        let fields = p.comma_list(Tok::RBrace, |p| {
                            let (n, ns) = p.plain_name("a field name")?;
                            p.expect(Tok::Colon)?;
                            Ok((n, ns, p.value()?))
                        })?;
                        AstValueKind::Record(ty, fields)
                    }
                    _ => return p.unexpected("a value"),
                }
            };
            Ok(AstValue {
                span: start.to(p.prev_span()),
                kind,
            })
        })
    }
}
