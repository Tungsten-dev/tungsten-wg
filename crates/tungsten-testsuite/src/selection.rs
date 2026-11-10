//! `TW_BACKENDS`, `TW_REQUIRE`, `TW_LARGE` (crate README, "Running it"). Unknown names are
//! rejected so a typo can't pass silently.

pub const KNOWN_BACKENDS: &[&str] = &["rust", "java", "cuda"];

fn parse_list(var: &str, value: &str) -> Result<Vec<String>, String> {
    let mut out = vec![];
    for name in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if !KNOWN_BACKENDS.contains(&name) {
            return Err(format!("{var}: unknown backend `{name}` (known: rust, java, cuda)"));
        }
        out.push(name.to_string());
    }
    Ok(out)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// `None`: unset, meaning the default set.
    pub backends: Option<Vec<String>>,
    pub require: Vec<String>,
    pub large: bool,
}

impl Selection {
    pub fn parse(backends: Option<&str>, require: Option<&str>, large: Option<&str>) -> Result<Self, String> {
        Ok(Selection {
            backends: backends.map(|v| parse_list("TW_BACKENDS", v)).transpose()?,
            require: require.map(|v| parse_list("TW_REQUIRE", v)).transpose()?.unwrap_or_default(),
            large: large == Some("1"),
        })
    }

    pub fn from_env() -> Result<Self, String> {
        let get = |k: &str| std::env::var(k).ok();
        Selection::parse(
            get("TW_BACKENDS").as_deref(),
            get("TW_REQUIRE").as_deref(),
            get("TW_LARGE").as_deref(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_names_fail() {
        assert!(Selection::parse(Some("rust,jave"), None, None).is_err());
        assert!(Selection::parse(None, Some("nvcc"), None).is_err());
        let s = Selection::parse(Some("rust, java"), Some("rust"), Some("1")).unwrap();
        assert_eq!(s.backends.unwrap(), vec!["rust", "java"]);
        assert!(s.large);
    }
}
