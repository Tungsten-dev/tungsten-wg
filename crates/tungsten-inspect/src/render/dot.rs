//! A small Graphviz DOT writer: quoting, attribute lists, clusters.

const INDENT: &str = "    ";

pub(crate) type Attrs = Vec<(&'static str, String)>;

pub(crate) struct Dot {
    out: String,
    depth: usize,
}

impl Dot {
    /// Opens `digraph "<name>" {`.
    pub fn digraph(name: &str) -> Self {
        Self {
            out: format!("digraph {} {{\n", quote(name)),
            depth: 1,
        }
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.depth {
            self.out.push_str(INDENT);
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    /// `key=value` at graph (or current cluster) level.
    pub fn attr(&mut self, key: &str, value: &str) {
        self.line(&format!("{key}={}", quote(value)));
    }

    /// Defaults for `node` or `edge`.
    pub fn defaults(&mut self, target: &str, attrs: Attrs) {
        self.line(&format!("{target}{}", attr_list(&attrs)));
    }

    pub fn node(&mut self, id: &str, attrs: Attrs) {
        self.line(&format!("{}{}", quote(id), attr_list(&attrs)));
    }

    pub fn edge(&mut self, from: &str, to: &str, attrs: Attrs) {
        self.line(&format!(
            "{} -> {}{}",
            quote(from),
            quote(to),
            attr_list(&attrs)
        ));
    }

    /// Opens `subgraph "<name>" {`. Prefix the name with `cluster` for a drawn cluster.
    pub fn open(&mut self, name: &str) {
        self.line(&format!("subgraph {} {{", quote(name)));
        self.depth += 1;
    }

    pub fn close(&mut self) {
        self.depth -= 1;
        self.line("}");
    }

    pub fn finish(mut self) -> String {
        assert_eq!(self.depth, 1, "unclosed subgraph");
        self.out.push_str("}\n");
        self.out
    }
}

/// A quoted DOT string: `"` and `\` escaped, newlines as `\n` (a centered line break).
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn attr_list(attrs: &Attrs) -> String {
    if attrs.is_empty() {
        return String::new();
    }
    let items: Vec<String> = attrs
        .iter()
        .map(|(k, v)| format!("{k}={}", quote(v)))
        .collect();
    format!(" [{}]", items.join(", "))
}
