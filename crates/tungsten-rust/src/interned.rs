use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::ops::Deref;

/// Handle to an arena-allocated value, compared and hashed by address.
///
/// Mirrors `Interned` in `tungsten-legacy` (the orphan rule keeps the two apart for now).
pub struct Interned<'m, T: ?Sized>(&'m T);

impl<'m, T> Interned<'m, T> {
    pub fn new(value: &'m T) -> Self {
        Interned(value)
    }
}

impl<'m, T> PartialEq for Interned<'m, T> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl<'m, T> Eq for Interned<'m, T> {}

impl<'m, T> Hash for Interned<'m, T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(self.0, state);
    }
}

impl<'m, T> Deref for Interned<'m, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<'m, T> Debug for Interned<'m, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Interned({:p})", self.0)
    }
}

impl<'m, T> Clone for Interned<'m, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'m, T> Copy for Interned<'m, T> {}

impl<T> tungsten_codegen::Addr for Interned<'_, T> {
    fn addr(&self) -> *const () {
        self.0 as *const T as *const ()
    }
}
