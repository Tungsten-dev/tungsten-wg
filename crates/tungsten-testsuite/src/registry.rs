//! `registry.toml` loader: the externs and records of the test environment.

use serde::Deserialize;
use tungsten_check::{AutoArgType, Registry, RegistryBuilder};
use tungsten_ir::VariableType;
use tungsten_parse::intern;

pub const REGISTRY_TOML: &str = include_str!("../registry.toml");

#[derive(Debug, Deserialize)]
struct Raw {
    #[serde(default)]
    record: Vec<RawRecord>,
    #[serde(default, rename = "extern")]
    externs: Vec<RawExtern>,
    #[serde(default)]
    auto_initializer: Vec<RawInitializer>,
}

#[derive(Debug, Deserialize)]
struct RawInitializer {
    name: String,
    params: Vec<String>,
    ret: String,
}

#[derive(Debug, Deserialize)]
struct RawRecord {
    name: String,
    fields: Vec<RawField>,
}

#[derive(Debug, Deserialize)]
struct RawField {
    name: String,
    #[serde(rename = "type")]
    ty: String,
}

#[derive(Debug, Deserialize)]
struct RawExtern {
    name: String,
    params: Vec<String>,
    ret: String,
    domain_error: bool,
}

#[derive(Debug, Clone)]
pub struct ExternSpec {
    pub name: String,
    pub params: Vec<VariableType>,
    pub ret: VariableType,
    pub domain_error: bool,
}

#[derive(Debug, Clone)]
pub struct InitializerSpec {
    pub name: String,
    pub params: Vec<AutoArgType>,
    pub ret: VariableType,
}

#[derive(Debug, Clone)]
pub struct RecordSpec {
    pub name: String,
    pub fields: Vec<(&'static str, VariableType)>,
}

/// The parsed `registry.toml`.
#[derive(Debug, Clone)]
pub struct RegistrySpec {
    pub externs: Vec<ExternSpec>,
    pub records: Vec<RecordSpec>,
    pub initializers: Vec<InitializerSpec>,
}

fn parse_arg_type(s: &str) -> Result<AutoArgType, String> {
    Ok(match s.trim() {
        "i64" => AutoArgType::I64,
        "f64" => AutoArgType::F64,
        "bool" => AutoArgType::Bool,
        "str" => AutoArgType::Str,
        other => return Err(format!("unknown argument type `{other}`")),
    })
}

/// Parses a type spelling of tungsten-parse §5.4: `f64`, `[f64; 3]`, `extern Pair`, ...
pub fn parse_type(s: &str) -> Result<VariableType, String> {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        let (elem, n) = inner.rsplit_once(';').ok_or_else(|| format!("bad array type `{s}`"))?;
        let n: usize = n.trim().parse().map_err(|_| format!("bad array length in `{s}`"))?;
        return Ok(VariableType::Array(Box::new(parse_type(elem)?), n));
    }
    if let Some(name) = s.strip_prefix("extern ") {
        return Ok(VariableType::Extern(intern(name.trim())));
    }
    if let Some(name) = s.strip_prefix("handle ") {
        return Ok(VariableType::Handle(intern(name.trim())));
    }
    Ok(match s {
        "bool" => VariableType::Bool,
        "i8" => VariableType::I8,
        "i32" => VariableType::I32,
        "i64" => VariableType::I64,
        "f32" => VariableType::F32,
        "f64" => VariableType::F64,
        "vec3" => VariableType::Vec3,
        "pos3" => VariableType::Pos3,
        _ => return Err(format!("unknown type `{s}`")),
    })
}

impl RegistrySpec {
    pub fn parse(text: &str) -> Result<RegistrySpec, String> {
        let raw: Raw = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut externs = vec![];
        for e in raw.externs {
            let params = e
                .params
                .iter()
                .map(|p| parse_type(p))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|m| format!("extern `{}`: {m}", e.name))?;
            let ret = parse_type(&e.ret).map_err(|m| format!("extern `{}`: {m}", e.name))?;
            externs.push(ExternSpec { name: e.name, params, ret, domain_error: e.domain_error });
        }
        let mut records = vec![];
        for r in raw.record {
            let fields = r
                .fields
                .iter()
                .map(|f| Ok((intern(&f.name), parse_type(&f.ty)?)))
                .collect::<Result<Vec<_>, String>>()
                .map_err(|m| format!("record `{}`: {m}", r.name))?;
            records.push(RecordSpec { name: r.name, fields });
        }
        let mut initializers = vec![];
        for i in raw.auto_initializer {
            let params = i
                .params
                .iter()
                .map(|p| parse_arg_type(p))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|m| format!("auto initializer `{}`: {m}", i.name))?;
            let ret = parse_type(&i.ret).map_err(|m| format!("auto initializer `{}`: {m}", i.name))?;
            initializers.push(InitializerSpec { name: i.name, params, ret });
        }
        Ok(RegistrySpec { externs, records, initializers })
    }

    pub fn builtin() -> RegistrySpec {
        RegistrySpec::parse(REGISTRY_TOML).expect("registry.toml is valid")
    }

    pub fn build(&self) -> Result<Registry, String> {
        let mut b = RegistryBuilder::new();
        for e in &self.externs {
            b = if e.domain_error {
                b.extern_fn_with_domain_error(&e.name, &e.params, e.ret.clone())
            } else {
                b.extern_fn(&e.name, &e.params, e.ret.clone())
            };
        }
        for r in &self.records {
            b = b.record(&r.name, &r.fields);
        }
        for i in &self.initializers {
            b = b.auto_initializer(&i.name, &i.params, i.ret.clone());
        }
        b.build().map_err(|e| format!("{e:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_builds() {
        let spec = RegistrySpec::builtin();
        spec.build().unwrap();
        assert!(spec.externs.iter().any(|e| e.name == "test_domain" && e.domain_error));
    }

    #[test]
    fn types() {
        assert_eq!(parse_type("[f64; 3]").unwrap(), VariableType::Array(Box::new(VariableType::F64), 3));
        assert!(parse_type("nope").is_err());
    }
}
