use std::rc::Rc;

const INDENT: &str = "    ";

pub struct Printer {
    out: String,
    pub line: String,
    indent: usize,
    name_cache: std::collections::HashMap<*const (), String>,
    anon_counter: usize,
}

impl Printer {
    pub fn new() -> Self {
        Self {
            out: String::new(),
            line: String::new(),
            indent: 0,
            anon_counter: 0,
            name_cache: std::collections::HashMap::new(),
        }
    }

    pub fn new_with_name_cache(name_cache: std::collections::HashMap<*const (), String>) -> Self {
        Self {
            out: String::new(),
            line: String::new(),
            indent: 0,
            anon_counter: 0,
            name_cache: name_cache,
        }
    }

    pub fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str(INDENT);
        }
        self.out.push_str(&self.line);
        self.line.clear();
        self.out.push_str(s);
        self.out.push('\n');
    }

    pub fn push(&mut self, s: &str) {
        self.line.push_str(s);
    }

    pub fn indent(&mut self) {
        self.indent += 1;
    }

    pub fn dedent(&mut self) {
        self.indent -= 1;
    }

    pub fn finish(self) -> String {
        self.finish_with_name_cache().0
    }

    pub fn finish_with_name_cache(self) -> (String, std::collections::HashMap<*const (), String>) {
        (self.out + &self.line, self.name_cache)
    }

    pub fn anon_name<T: Addr>(&mut self, entity: T, prefix: &str) -> String {
        if let Some(name) = self.name_cache.get(&(entity.addr())) {
            return name.clone();
        }

        let name = format!("<{}-{}>", prefix, self.anon_counter);
        self.anon_counter += 1;
        self.name_cache.insert(entity.addr(), name.clone());
        name
    }
}

pub trait Addr {
    fn addr(&self) -> *const ();
}

impl<T> Addr for &T {
    fn addr(&self) -> *const () {
        *self as *const T as *const ()
    }
}

impl<T> Addr for std::rc::Rc<T> {
    fn addr(&self) -> *const () {
        let ptr: *const T = Rc::as_ptr(self);
        ptr as *const ()
    }
}
