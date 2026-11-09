//! A small, stable 128-bit hash for cache keys and module names (two FNV-1a lanes). It only has
//! to tell inputs apart and give the same answer on every run, not resist attacks.

use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Digest(u128);

impl fmt::LowerHex for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

pub struct Context {
    a: u64,
    b: u64,
}

impl Context {
    pub fn new() -> Self {
        Context { a: 0xcbf2_9ce4_8422_2325, b: 0x8422_2325_cbf2_9ce4 }
    }

    pub fn consume(&mut self, bytes: &[u8]) {
        for &x in bytes {
            self.a = (self.a ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3);
            self.b = (self.b ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3).rotate_left(5);
        }
        // Separate consecutive inputs, so ("ab", "c") and ("a", "bc") differ.
        self.a = (self.a ^ 0xff).wrapping_mul(0x0000_0100_0000_01b3);
        self.b = (self.b ^ 0xff).wrapping_mul(0x0000_0100_0000_01b3).rotate_left(5);
    }

    pub fn finalize(self) -> Digest {
        Digest(((self.a as u128) << 64) | self.b as u128)
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

pub fn compute(bytes: &[u8]) -> Digest {
    let mut c = Context::new();
    c.consume(bytes);
    c.finalize()
}
