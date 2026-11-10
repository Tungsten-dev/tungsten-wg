//! `use lib::{…}`: libraries are `.tw` files of `fn`, `const` and `use` only. Importing copies the
//! named items plus everything they reach.

use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;

use super::{Ctx, R};
use crate::ast::*;
use crate::parser::parse;
use crate::span::Diagnostic;

/// A library with the items it imported itself already merged in front of its own.
pub(crate) struct Lib {
    pub name: String,
    pub fns: Vec<LibItem<FnItem>>,
    pub consts: Vec<LibItem<ConstItem>>,
}

#[derive(Clone)]
pub(crate) struct LibItem<T> {
    pub item: T,
    /// The library that defines it, for deduplication across imports.
    pub origin: String,
}

/// What the `use` items of the file being elaborated import: per library, in order of first
/// use, the indices of its fns and consts.
#[derive(Default)]
pub(crate) struct Imports {
    libs: Vec<(Rc<Lib>, BTreeSet<usize>, BTreeSet<usize>)>,
}

fn load(cx: &mut Ctx, name: &str, at: &UseItem) -> R<Rc<Lib>> {
    if let Some(l) = cx.libs.get(name) {
        return Ok(l.clone());
    }
    if cx.lib_stack.iter().any(|n| n == name) {
        let cycle = cx.lib_stack.join(" -> ");
        return Err(Diagnostic::elab(
            at.library_span,
            format!("library `{name}` uses itself ({cycle} -> {name})"),
        ));
    }
    let Some((file, src)) = cx.env.library(name) else {
        return Err(Diagnostic::elab(
            at.library_span,
            format!("unknown library `{name}`"),
        ));
    };
    if let Some(other) = cx.files.name(file) {
        return Err(Diagnostic::elab(
            at.library_span,
            format!("library `{name}` has {file:?}, which `{other}` already uses"),
        ));
    }
    let file_name = cx.env.file_name(file).unwrap_or_else(|| name.to_string());
    cx.files.add(file, Some(file_name), src);
    let ast = parse(src, file)?;
    cx.lib_stack.push(name.to_string());
    let r = build(cx, name, &ast);
    cx.lib_stack.pop();
    let lib = Rc::new(r?);
    cx.libs.insert(name.to_string(), lib.clone());
    Ok(lib)
}

fn build(cx: &mut Ctx, name: &str, ast: &File) -> R<Lib> {
    let mut lib = Lib {
        name: name.to_string(),
        fns: vec![],
        consts: vec![],
    };
    fn own<T>(item: &T, origin: &str) -> LibItem<T>
    where
        T: Clone,
    {
        LibItem {
            item: item.clone(),
            origin: origin.to_string(),
        }
    }
    for item in &ast.program.items {
        match item {
            Item::Use(u) => {
                let other = load(cx, &u.library, u)?;
                let (fns, consts) = closure(&other, &u.names)?;
                for i in fns {
                    add(&mut lib.fns, other.fns[i].clone(), |f| &f.label)?;
                }
                for i in consts {
                    add(&mut lib.consts, other.consts[i].clone(), |c| &c.label)?;
                }
            }
            Item::Fn(FnItem {
                hosts: Some((_, span)),
                ..
            }) => {
                return Err(Diagnostic::elab(
                    *span,
                    "library functions can't have `@hosts`: library code can't use host inputs",
                ));
            }
            Item::Fn(f) => add(&mut lib.fns, own(f, name), |f| &f.label)?,
            Item::Const(c) => add(&mut lib.consts, own(c, name), |c| &c.label)?,
            Item::Host(HostItem { span, .. })
            | Item::Table(TableItem { span, .. })
            | Item::Unit(UnitItem { span, .. }) => {
                return Err(Diagnostic::elab(
                    *span,
                    "a library may only contain `fn`, `const` and `use`",
                ));
            }
            Item::Entry(e) => {
                return Err(Diagnostic::elab(
                    e.span,
                    "a library may only contain `fn`, `const` and `use`",
                ));
            }
        }
    }
    Ok(lib)
}

/// Appends unless the same item (same origin, same label) is already there.
fn add<T>(list: &mut Vec<LibItem<T>>, new: LibItem<T>, label: impl Fn(&T) -> &Label) -> R<()> {
    let key = label(&new.item).key();
    if let Some(old) = list.iter().find(|o| label(&o.item).key() == key) {
        if old.origin == new.origin {
            return Ok(());
        }
        return Err(Diagnostic::elab(
            label(&new.item).span,
            format!(
                "library label `{}` is defined by both `{}` and `{}`",
                label(&new.item).display(),
                old.origin,
                new.origin
            ),
        ));
    }
    list.push(new);
    Ok(())
}

/// Indices of the fns and consts reached from `names`.
fn closure(lib: &Lib, names: &[Label]) -> R<(BTreeSet<usize>, BTreeSet<usize>)> {
    let fn_index = |l: &Label| lib.fns.iter().position(|f| f.item.label.key() == l.key());
    let const_index = |l: &Label| {
        lib.consts
            .iter()
            .position(|c| c.item.label.key() == l.key())
    };
    let missing = |l: &Label| {
        Diagnostic::elab(
            l.span,
            format!("`{}` is not defined in library `{}`", l.display(), lib.name),
        )
    };

    let mut fns = BTreeSet::new();
    let mut consts = BTreeSet::new();
    let mut work: Vec<(bool, usize)> = vec![];
    for n in names {
        let (f, c) = (fn_index(n), const_index(n));
        if f.is_none() && c.is_none() {
            return Err(Diagnostic::elab(
                n.span,
                format!("library `{}` has no `{}`", lib.name, n.display()),
            ));
        }
        work.extend(f.map(|i| (true, i)));
        work.extend(c.map(|i| (false, i)));
    }
    while let Some((is_fn, i)) = work.pop() {
        let new = if is_fn {
            fns.insert(i)
        } else {
            consts.insert(i)
        };
        if !new {
            continue;
        }
        let refs = if is_fn {
            fn_refs(&lib.fns[i].item)?
        } else {
            let mut refs = Refs::default();
            walk_expr(&lib.consts[i].item.value, &mut refs)?;
            refs
        };
        for l in refs.calls {
            work.push((true, fn_index(l).ok_or_else(|| missing(l))?));
        }
        for l in refs.consts {
            work.push((false, const_index(l).ok_or_else(|| missing(l))?));
        }
    }
    Ok((fns, consts))
}

/// What a library item refers to.
#[derive(Default)]
struct Refs<'a> {
    /// The fn's variables, so a bare name that isn't one is a const.
    vars: HashSet<LabelKey>,
    calls: Vec<&'a Label>,
    consts: Vec<&'a Label>,
}

fn fn_refs(f: &FnItem) -> R<Refs<'_>> {
    let mut refs = Refs::default();
    let declared = f.params.iter().map(|p| &p.label);
    let declared = declared.chain(f.vars.iter().map(|(l, _)| l));
    let declared = declared.chain(lets(&f.body).into_iter().map(|(l, _)| l));
    refs.vars = declared.map(Label::key).collect();
    try_for_each_stmt(&f.body, &mut |s| {
        s.exprs().try_for_each(|e| walk_expr(e, &mut refs))
    })?;
    Ok(refs)
}

fn walk_expr<'a>(e: &'a Expr, refs: &mut Refs<'a>) -> R<()> {
    let mut visit = |e: &'a Expr| {
        match &e.kind {
            ExprKind::Call { callee, .. } => refs.calls.push(callee),
            ExprKind::Const(l) => refs.consts.push(l),
            ExprKind::Name(l) if !refs.vars.contains(&l.key()) => refs.consts.push(l),
            ExprKind::Host(_) | ExprKind::Table(_) | ExprKind::Density { .. } => {
                return Err(Diagnostic::elab(
                    e.span,
                    "library code can't use host inputs, tables or density inputs",
                ));
            }
            _ => {}
        }
        Ok(())
    };
    visit(e)?;
    e.kind.try_for_each_descendant(&mut visit)
}

pub(super) fn import(cx: &mut Ctx, u: &UseItem) -> R<()> {
    let lib = load(cx, &u.library, u)?;
    let (fns, consts) = closure(&lib, &u.names)?;
    let libs = &mut cx.imports.libs;
    let i = match libs.iter().position(|(l, ..)| Rc::ptr_eq(l, &lib)) {
        Some(i) => i,
        None => {
            libs.push((lib, BTreeSet::new(), BTreeSet::new()));
            libs.len() - 1
        }
    };
    libs[i].1.extend(fns);
    libs[i].2.extend(consts);
    Ok(())
}

/// Everything the `use` items import, deduplicated, per library in library textual order.
pub(super) fn imported(cx: &mut Ctx) -> R<(Vec<FnItem>, Vec<ConstItem>)> {
    let mut fns: Vec<LibItem<FnItem>> = vec![];
    let mut consts: Vec<LibItem<ConstItem>> = vec![];
    for (lib, fi, ci) in std::mem::take(&mut cx.imports.libs) {
        for &i in &fi {
            add(&mut fns, lib.fns[i].clone(), |f| &f.label)?;
        }
        for &i in &ci {
            add(&mut consts, lib.consts[i].clone(), |c| &c.label)?;
        }
    }
    for f in &fns {
        let l = &f.item.label;
        cx.imported_from
            .insert((true, l.key()), (f.origin.clone(), l.span));
    }
    for c in &consts {
        let l = &c.item.label;
        cx.imported_from
            .insert((false, l.key()), (c.origin.clone(), l.span));
    }
    Ok((
        fns.into_iter().map(|f| f.item).collect(),
        consts.into_iter().map(|c| c.item).collect(),
    ))
}
