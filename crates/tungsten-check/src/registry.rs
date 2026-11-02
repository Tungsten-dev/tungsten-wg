//! The registry: names and declarations of externs and records, which [`crate::lower`] resolves
//! against.

use std::collections::HashMap;

use tungsten_core::{ExternDecl, ExternId, RecordDecl, RecordId};
use tungsten_ir::VariableType;

use crate::lower::types::{TypeFail, lower_type};

/// Extern and record declarations with their names. Build one with [`RegistryBuilder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    pub(crate) extern_names: Vec<String>,
    pub(crate) externs: Vec<ExternDecl>,
    pub(crate) extern_ids: HashMap<String, ExternId>,
    pub(crate) record_names: Vec<String>,
    pub(crate) records: Vec<RecordDecl>,
    pub(crate) record_ids: HashMap<String, RecordId>,
    pub(crate) field_names: Vec<Vec<String>>,
}

impl Registry {
    pub fn extern_id(&self, name: &str) -> Option<ExternId> {
        self.extern_ids.get(name).copied()
    }

    pub fn record_id(&self, name: &str) -> Option<RecordId> {
        self.record_ids.get(name).copied()
    }

    pub fn field_index(&self, r: RecordId, field: &str) -> Option<usize> {
        self.field_names.get(r.0)?.iter().position(|f| f == field)
    }

    /// Panics if `e` is not an id of this registry.
    pub fn extern_name(&self, e: ExternId) -> &str {
        &self.extern_names[e.0]
    }

    /// Panics if `r` is not an id of this registry.
    pub fn record_name(&self, r: RecordId) -> &str {
        &self.record_names[r.0]
    }

    /// Panics if `r` or `field` is out of range.
    pub fn field_name(&self, r: RecordId, field: usize) -> &str {
        &self.field_names[r.0][field]
    }

    pub fn extern_decl(&self, e: ExternId) -> Option<&ExternDecl> {
        self.externs.get(e.0)
    }

    pub fn record_decl(&self, r: RecordId) -> Option<&RecordDecl> {
        self.records.get(r.0)
    }

    pub fn decls(&self) -> (Vec<ExternDecl>, Vec<RecordDecl>) {
        (self.externs.clone(), self.records.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("extern `{0}` is declared twice")]
    DuplicateExtern(String),
    #[error("record `{0}` is declared twice")]
    DuplicateRecord(String),
    #[error("record `{record}` has field `{field}` twice")]
    DuplicateField { record: String, field: String },
    /// `context` names the extern or `record.field` whose type mentions it.
    #[error("unknown record `{name}` in the type of `{context}`")]
    UnknownRecord { context: String, name: String },
    #[error("`density` used as a type in `{context}`")]
    DensityInputType { context: String },
    /// The records on the cycle, starting and ending with the same name.
    #[error("records contain themselves by value: {}", .0.join(" → "))]
    RecordCycle(Vec<String>),
}

struct ExternEntry {
    name: String,
    params: Vec<VariableType>,
    ret: VariableType,
    domain_error: bool,
}

struct RecordEntry {
    name: String,
    fields: Vec<(String, VariableType)>,
}

/// Declares externs and records. Ids are assigned in declaration order. Records may refer to
/// each other by name in any order; [`build`](Self::build) resolves them.
#[derive(Default)]
pub struct RegistryBuilder {
    externs: Vec<ExternEntry>,
    records: Vec<RecordEntry>,
}

impl RegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn extern_fn(self, name: &str, params: &[VariableType], ret: VariableType) -> Self {
        self.push_extern(name, params, ret, false)
    }

    pub fn extern_fn_with_domain_error(
        self,
        name: &str,
        params: &[VariableType],
        ret: VariableType,
    ) -> Self {
        self.push_extern(name, params, ret, true)
    }

    fn push_extern(
        mut self,
        name: &str,
        params: &[VariableType],
        ret: VariableType,
        domain_error: bool,
    ) -> Self {
        self.externs.push(ExternEntry {
            name: name.to_string(),
            params: params.to_vec(),
            ret,
            domain_error,
        });
        self
    }

    pub fn record(mut self, name: &str, fields: &[(&str, VariableType)]) -> Self {
        self.records.push(RecordEntry {
            name: name.to_string(),
            fields: fields
                .iter()
                .map(|(n, t)| (n.to_string(), t.clone()))
                .collect(),
        });
        self
    }

    /// Checks, in this order: duplicate externs, duplicate records, duplicate fields, record
    /// field types, extern types, record cycles. Returns the first error.
    pub fn build(self) -> Result<Registry, RegistryError> {
        let mut extern_ids = HashMap::new();
        for (i, e) in self.externs.iter().enumerate() {
            if extern_ids.insert(e.name.clone(), ExternId(i)).is_some() {
                return Err(RegistryError::DuplicateExtern(e.name.clone()));
            }
        }
        let mut record_ids = HashMap::new();
        for (i, r) in self.records.iter().enumerate() {
            if record_ids.insert(r.name.clone(), RecordId(i)).is_some() {
                return Err(RegistryError::DuplicateRecord(r.name.clone()));
            }
        }
        for r in &self.records {
            for (i, (f, _)) in r.fields.iter().enumerate() {
                if r.fields[..i].iter().any(|(g, _)| g == f) {
                    return Err(RegistryError::DuplicateField {
                        record: r.name.clone(),
                        field: f.clone(),
                    });
                }
            }
        }

        let resolve = |t: &VariableType, context: &dyn Fn() -> String| {
            lower_type(t, &mut |n| record_ids.get(n).copied()).map_err(|e| match e {
                TypeFail::DensityInput => RegistryError::DensityInputType { context: context() },
                TypeFail::UnknownRecord(name) => RegistryError::UnknownRecord {
                    context: context(),
                    name: name.to_string(),
                },
            })
        };

        let mut records = Vec::with_capacity(self.records.len());
        for r in &self.records {
            let mut fields = Vec::with_capacity(r.fields.len());
            for (f, t) in &r.fields {
                fields.push(resolve(t, &|| format!("{}.{}", r.name, f))?);
            }
            records.push(RecordDecl { fields });
        }
        let mut externs = Vec::with_capacity(self.externs.len());
        for e in &self.externs {
            let ctx = || e.name.clone();
            let params = e
                .params
                .iter()
                .map(|t| resolve(t, &ctx))
                .collect::<Result<Vec<_>, _>>()?;
            let ret = resolve(&e.ret, &ctx)?;
            externs.push(ExternDecl {
                params,
                ret,
                domain_error: e.domain_error,
            });
        }

        let names: Vec<String> = self.records.iter().map(|r| r.name.clone()).collect();
        if let Some(cycle) = find_record_cycle(&records) {
            return Err(RegistryError::RecordCycle(
                cycle.into_iter().map(|r| names[r].clone()).collect(),
            ));
        }

        Ok(Registry {
            extern_names: self.externs.iter().map(|e| e.name.clone()).collect(),
            externs,
            extern_ids,
            record_names: names,
            records,
            record_ids,
            field_names: self
                .records
                .iter()
                .map(|r| r.fields.iter().map(|(f, _)| f.clone()).collect())
                .collect(),
        })
    }
}

/// A record that contains itself by value (directly, through other records, or through
/// arrays), as the list of record indices on the cycle, first == last.
fn find_record_cycle(records: &[RecordDecl]) -> Option<Vec<usize>> {
    fn edges(t: &tungsten_core::Type, out: &mut Vec<usize>) {
        match t {
            tungsten_core::Type::Record(r) => out.push(r.0),
            tungsten_core::Type::Array(e, _) => edges(e, out),
            _ => {}
        }
    }
    let succ: Vec<Vec<usize>> = records
        .iter()
        .map(|r| {
            let mut v = vec![];
            for t in &r.fields {
                edges(t, &mut v);
            }
            v
        })
        .collect();

    // 0 = unvisited, 1 = on stack, 2 = done. Iterative DFS.
    let mut state = vec![0u8; records.len()];
    for start in 0..records.len() {
        if state[start] != 0 {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        state[start] = 1;
        while let Some(&mut (node, ref mut next)) = stack.last_mut() {
            if let Some(&s) = succ[node].get(*next) {
                *next += 1;
                match state[s] {
                    0 => {
                        state[s] = 1;
                        stack.push((s, 0));
                    }
                    1 => {
                        let from = stack.iter().position(|&(n, _)| n == s).unwrap_or(0);
                        let mut cycle: Vec<usize> = stack[from..].iter().map(|&(n, _)| n).collect();
                        cycle.push(s);
                        return Some(cycle);
                    }
                    _ => {}
                }
            } else {
                state[node] = 2;
                stack.pop();
            }
        }
    }
    None
}
