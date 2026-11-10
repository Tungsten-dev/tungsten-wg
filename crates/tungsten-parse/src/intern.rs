//! A global interner for the `&'static str`s the IR wants (extern type and field names).

use std::collections::HashSet;
use std::sync::Mutex;

static POOL: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

/// One leaked copy per distinct string.
pub fn intern(s: &str) -> &'static str {
    let mut guard = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let pool = guard.get_or_insert_with(HashSet::new);
    if let Some(&existing) = pool.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    pool.insert(leaked);
    leaked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_copy_per_string() {
        let a = intern("hello");
        let b = intern(&(String::from("hel") + "lo"));
        assert!(std::ptr::eq(a, b));
        assert!(!std::ptr::eq(a, intern("world")));
    }
}
