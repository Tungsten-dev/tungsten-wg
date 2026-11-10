//! Directives: `@case`, `@expect`, `@knownfail`, … and the typed values they carry.

use std::collections::{HashMap, HashSet};

use tungsten_ir::*;

use super::{Case, CellSample, Ctx, Expect, ExpectKind, KnownFail, R, TestFile, Value, err};
use crate::ast::{
    self, AstValue, AstValueKind, DirectiveKind, Expectation, LitKind, NumKind, TyKind,
};
use crate::numlit::{self, Num};
use crate::printer::type_text;
use crate::span::{Diagnostic, Span};
use crate::stack;

/// An f64 in an attribute position: an unsuffixed int is accepted.
pub(super) fn attr_f64(l: &ast::Lit) -> R<f64> {
    match numlit::convert(&l.kind, Some(NumKind::F64)) {
        Ok(Num::Double(d)) => Ok(d),
        Ok(_) => unreachable!(),
        Err(m) => err(l.span, m),
    }
}

impl<'a> Ctx<'a> {
    pub(super) fn envelope(&mut self, file: &ast::File) -> R<TestFile> {
        let mut t = TestFile {
            program: OptimizedProgram {
                compute_units: Arena::new(),
                functions: Arena::new(),
                entry_points: vec![],
                constants: Arena::new(),
                host_inputs: Arena::new(),
                permutation_tables: Arena::new(),
            },
            source_map: Default::default(),
            entry_labels: std::mem::take(&mut self.entry_labels),
            cases: vec![],
            expects: vec![],
            reject: None,
            knownfails: vec![],
            fuel: None,
            tier: None,
            cells: None,
        };

        // cases first, so expectations can name a case declared after them
        let mut case_index: HashMap<String, usize> = HashMap::new();
        for d in &file.directives {
            if let DirectiveKind::Case {
                name,
                origin,
                seed,
                hosts,
            } = &d.kind
            {
                if case_index.insert(name.clone(), t.cases.len()).is_some() {
                    return err(d.span, format!("duplicate case \"{name}\""));
                }
                t.cases.push(self.case(d.span, name, origin, *seed, hosts)?);
            }
        }

        let mut covered: HashSet<(usize, usize, usize)> = HashSet::new();
        let mut covered_all: HashSet<(usize, usize)> = HashSet::new();
        let mut errors: HashSet<usize> = HashSet::new();
        let mut expect_any: HashSet<usize> = HashSet::new();
        let mut reject_span = None;
        let mut other_span = None;

        for d in &file.directives {
            match &d.kind {
                DirectiveKind::Case { .. } => other_span = Some(d.span),
                DirectiveKind::KnownFail {
                    backend,
                    reason,
                    case,
                    stage,
                } => {
                    if let Some(c) = case
                        && !case_index.contains_key(c)
                    {
                        return err(d.span, format!("`case = \"{c}\"` names no case"));
                    }
                    t.knownfails.push(KnownFail {
                        backend: backend.clone(),
                        reason: reason.clone(),
                        case: case.clone(),
                        stage: stage.clone(),
                    });
                }
                DirectiveKind::Fuel(n) => {
                    if t.fuel.replace(*n).is_some() {
                        return err(d.span, "duplicate `@fuel`");
                    }
                }
                DirectiveKind::Tier(s) => {
                    if t.tier.replace(s.clone()).is_some() {
                        return err(d.span, "duplicate `@tier`");
                    }
                }
                DirectiveKind::Cells { sample, seed } => {
                    let c = CellSample {
                        sample: *sample,
                        seed: *seed,
                    };
                    if t.cells.replace(c).is_some() {
                        return err(d.span, "duplicate `@cells`");
                    }
                }
                DirectiveKind::ExpectReject(k) => {
                    if t.reject.replace(k.clone()).is_some() {
                        return err(d.span, "duplicate `@expect reject`");
                    }
                    reject_span = Some(d.span);
                }
                DirectiveKind::Expect {
                    case,
                    case_span,
                    expectation,
                } => {
                    other_span = Some(d.span);
                    let Some(&ci) = case_index.get(case) else {
                        return err(*case_span, format!("no case named \"{case}\""));
                    };
                    let exp = self.expectation(
                        d.span,
                        ci,
                        expectation,
                        &mut covered,
                        &mut covered_all,
                        &mut errors,
                        &mut expect_any,
                    )?;
                    t.expects.push(exp);
                }
            }
        }
        if let (Some(r), Some(o)) = (reject_span, other_span) {
            return Err(Diagnostic::elab(
                o,
                "a file with `@expect reject` can't have `@case` or other `@expect` directives",
            )
            .note(r, "rejected here"));
        }

        t.program = std::mem::replace(&mut self.prog, t.program);
        t.source_map = std::mem::take(&mut self.map);
        Ok(t)
    }

    fn case(
        &self,
        span: Span,
        name: &str,
        origin: &[ast::Lit; 3],
        seed: u64,
        bindings: &[(ast::Label, AstValue)],
    ) -> R<Case> {
        let [x, y, z] = origin.each_ref().map(attr_f64);
        let o = [x?, y?, z?];
        let mut bound: HashMap<Ref<HostInput>, Value> = HashMap::new();
        let mut hosts = vec![];
        for (label, v) in bindings {
            let h = self.lookup_host(label)?;
            let ty = match &self.prog.host_inputs[h] {
                HostInput::Static { t, .. } => t.clone(),
                HostInput::Dynamic => {
                    return err(label.span, "a dynamic host input can't be bound in a case");
                }
            };
            let value = self.value(v, &ty)?;
            if bound.insert(h, value.clone()).is_some() {
                return err(label.span, format!("`{}` is bound twice", label.display()));
            }
            hosts.push((h, value));
        }
        for (h, input) in self.prog.host_inputs.iter() {
            if matches!(input, HostInput::Static { .. }) && !bound.contains_key(&h) {
                let label = self
                    .hosts
                    .iter()
                    .find(|(_, r)| **r == h)
                    .map(|(k, _)| k.display());
                return err(
                    span,
                    format!(
                        "case \"{name}\" doesn't bind the host input `{}`",
                        label.unwrap_or_default()
                    ),
                );
            }
        }
        Ok(Case {
            name: name.to_string(),
            origin: o,
            seed,
            hosts,
            span,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn expectation(
        &self,
        span: Span,
        case: usize,
        ex: &Expectation,
        covered: &mut HashSet<(usize, usize, usize)>,
        covered_all: &mut HashSet<(usize, usize)>,
        errors: &mut HashSet<usize>,
        any: &mut HashSet<usize>,
    ) -> R<Expect> {
        let entry_of = |label: &ast::Label| -> R<(usize, VariableType, (i32, i32, i32))> {
            let Some(&i) = self.entry_keys.get(&label.key()) else {
                return err(label.span, format!("unknown entry `{}`", label.display()));
            };
            let e = &self.prog.entry_points[i];
            let ComputeUnit::DensityFunction { function, .. } =
                &self.prog.compute_units[e.density_function];
            Ok((
                i,
                self.prog.functions[*function].return_type.clone(),
                e.dimensions,
            ))
        };
        let dup =
            |what: &str| -> R<()> { err(span, format!("two expectations cover the same {what}")) };
        match ex {
            Expectation::Error(kind) => {
                if any.contains(&case) || !errors.insert(case) {
                    dup("case")?;
                }
                Ok(Expect {
                    case,
                    entry: None,
                    kind: ExpectKind::Error(kind.clone()),
                    span,
                })
            }
            Expectation::Cell { entry, pos, value } => {
                let (i, ty, (dx, dy, dz)) = entry_of(entry)?;
                if errors.contains(&case) {
                    dup("case")?;
                }
                any.insert(case);
                for (c, d) in pos.iter().zip([dx, dy, dz]) {
                    if *c < 0 || *c >= d as i64 {
                        return err(
                            span,
                            format!(
                                "cell [{}, {}, {}] is outside dims ({dx}, {dy}, {dz})",
                                pos[0], pos[1], pos[2]
                            ),
                        );
                    }
                }
                let idx = pos[0] as usize
                    + dx as usize * (pos[1] as usize + dy as usize * pos[2] as usize);
                if covered_all.contains(&(case, i)) || !covered.insert((case, i, idx)) {
                    dup("cell")?;
                }
                Ok(Expect {
                    case,
                    entry: Some(i),
                    kind: ExpectKind::Cell(
                        [pos[0] as i32, pos[1] as i32, pos[2] as i32],
                        self.value(value, &ty)?,
                    ),
                    span,
                })
            }
            Expectation::All { entry, value } => {
                let (i, ty, _) = entry_of(entry)?;
                if errors.contains(&case) {
                    dup("case")?;
                }
                any.insert(case);
                if covered.iter().any(|(c, e, _)| (*c, *e) == (case, i))
                    || !covered_all.insert((case, i))
                {
                    dup("cell")?;
                }
                Ok(Expect {
                    case,
                    entry: Some(i),
                    kind: ExpectKind::All(self.value(value, &ty)?),
                    span,
                })
            }
            Expectation::Grid { entry, values } => {
                let (i, ty, (dx, dy, dz)) = entry_of(entry)?;
                if errors.contains(&case) {
                    dup("case")?;
                }
                any.insert(case);
                let cells = (dx.max(0) as i64) * (dy.max(0) as i64) * (dz.max(0) as i64);
                if values.len() as i64 != cells {
                    return err(
                        span,
                        format!(
                            "a grid lists every cell: dims ({dx}, {dy}, {dz}) need {cells} values, found {}",
                            values.len()
                        ),
                    );
                }
                if covered.iter().any(|(c, e, _)| (*c, *e) == (case, i))
                    || !covered_all.insert((case, i))
                {
                    dup("cell")?;
                }
                let vs = values
                    .iter()
                    .map(|v| self.value(v, &ty))
                    .collect::<R<Vec<_>>>()?;
                Ok(Expect {
                    case,
                    entry: Some(i),
                    kind: ExpectKind::Grid(vs),
                    span,
                })
            }
        }
    }

    /// A value, typed by the type it is expected to have.
    pub(super) fn value(&self, v: &AstValue, ty: &VariableType) -> R<Value> {
        stack::grow(|| self.value_inner(v, ty))
    }

    fn value_inner(&self, v: &AstValue, ty: &VariableType) -> R<Value> {
        let mismatch = || -> R<Value> {
            err(
                v.span,
                format!("expected a value of type {}", type_text(ty)),
            )
        };
        if let Some(want) = NumKind::of_type(ty) {
            return match &v.kind {
                AstValueKind::Lit(l) => numlit::convert(&l.kind, Some(want))
                    .map(Num::into_value)
                    .or_else(|m| err(l.span, m)),
                _ => err(v.span, format!("expected a {} literal", want.name())),
            };
        }
        let each = |vs: &[AstValue], t: &VariableType| -> R<Vec<Value>> {
            vs.iter().map(|x| self.value(x, t)).collect()
        };
        Ok(match (ty, &v.kind) {
            (
                VariableType::Bool,
                AstValueKind::Lit(ast::Lit {
                    kind: LitKind::Bool(b),
                    ..
                }),
            ) => Value::Bool(*b),
            (VariableType::Vec3, AstValueKind::Vec3(vs)) => {
                let xs = each(vs, &VariableType::F64)?;
                Value::Vec3(std::array::from_fn(|i| match xs[i] {
                    Value::F64(d) => d,
                    _ => unreachable!(),
                }))
            }
            (VariableType::Pos3, AstValueKind::Pos3(vs)) => {
                let xs = each(vs, &VariableType::I32)?;
                Value::Pos3(std::array::from_fn(|i| match xs[i] {
                    Value::I32(d) => d,
                    _ => unreachable!(),
                }))
            }
            (VariableType::Array(elem, n), AstValueKind::Array(vs)) => {
                if vs.len() != *n {
                    return err(v.span, format!("expected {n} elements, found {}", vs.len()));
                }
                Value::Array(each(vs, elem)?)
            }
            (VariableType::Extern(name), AstValueKind::Record(rty, fields)) => {
                if !matches!(&rty.kind, TyKind::Extern(n) if n == name) {
                    return mismatch();
                }
                self.record_value(v, name, fields)?
            }
            (
                VariableType::DensityInput
                | VariableType::PermutationTable
                | VariableType::InterpolatedNoiseSampler,
                _,
            ) => {
                return err(v.span, format!("{} has no literal values", type_text(ty)));
            }
            _ => return mismatch(),
        })
    }

    /// A record of the extern type `name`. With the registry's fields, values are typed by them
    /// and listed in registry order. Without (`NoEnv`), they keep the written order and are typed
    /// by their literals, which canonical text spells unambiguously.
    fn record_value(
        &self,
        v: &AstValue,
        name: &str,
        fields: &[(String, Span, AstValue)],
    ) -> R<Value> {
        for (i, (n, ns, _)) in fields.iter().enumerate() {
            if fields[..i].iter().any(|(m, _, _)| m == n) {
                return err(*ns, format!("field `{n}` given twice"));
            }
        }
        let name = super::Ctx::intern_name(name);
        let Some(decl) = self.env.extern_type(name) else {
            let out = fields
                .iter()
                .map(|(n, _, x)| Ok((super::Ctx::intern_name(n), self.untyped(x)?)))
                .collect::<R<_>>()?;
            return Ok(Value::Record(name, out));
        };
        for (n, ns, _) in fields {
            if !decl.iter().any(|(d, _)| d == n) {
                return err(*ns, format!("`{name}` has no field `{n}`"));
            }
        }
        let mut out = vec![];
        for (fname, fty) in &decl {
            match fields.iter().find(|(n, _, _)| n == fname) {
                Some((_, _, x)) => out.push((*fname, self.value(x, fty)?)),
                None => return err(v.span, format!("missing field `{fname}`")),
            }
        }
        Ok(Value::Record(name, out))
    }

    /// A value with no expected type: typed by how it is written.
    fn untyped(&self, v: &AstValue) -> R<Value> {
        stack::grow(|| {
            let each =
                |vs: &[AstValue]| -> R<Vec<Value>> { vs.iter().map(|x| self.untyped(x)).collect() };
            Ok(match &v.kind {
                AstValueKind::Lit(ast::Lit {
                    kind: LitKind::Bool(b),
                    ..
                }) => Value::Bool(*b),
                AstValueKind::Lit(l) => numlit::convert(&l.kind, None)
                    .map(Num::into_value)
                    .or_else(|m| err(l.span, m))?,
                AstValueKind::Vec3(_) => self.value(v, &VariableType::Vec3)?,
                AstValueKind::Pos3(_) => self.value(v, &VariableType::Pos3)?,
                AstValueKind::Array(vs) => Value::Array(each(vs)?),
                AstValueKind::Record(rty, fields) => match &rty.kind {
                    TyKind::Extern(n) => self.record_value(v, n, fields)?,
                    _ => return err(rty.span, "a record needs an extern type"),
                },
            })
        })
    }
}
