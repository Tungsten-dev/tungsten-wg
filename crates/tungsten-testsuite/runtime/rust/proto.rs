//! Driver side of the wire protocol (SPEC §7.4) and the request loop. Generated drivers decode a
//! request with `Reader` and encode the result with `Writer`; `serve` does the framing, catches
//! panics and maps them to failure responses.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;

pub struct Reader {
    buf: Vec<u8>,
    pos: usize,
}

impl Reader {
    pub fn new(buf: Vec<u8>) -> Self {
        Reader { buf, pos: 0 }
    }
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let s = &self.buf[self.pos..self.pos + N];
        self.pos += N;
        s.try_into().unwrap()
    }
    pub fn bool(&mut self) -> bool {
        self.take::<1>()[0] != 0
    }
    pub fn i8(&mut self) -> i8 {
        self.take::<1>()[0] as i8
    }
    pub fn i32(&mut self) -> i32 {
        i32::from_le_bytes(self.take())
    }
    pub fn i64(&mut self) -> i64 {
        i64::from_le_bytes(self.take())
    }
    pub fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take())
    }
    pub fn f32(&mut self) -> f32 {
        f32::from_bits(u32::from_le_bytes(self.take()))
    }
    pub fn f64(&mut self) -> f64 {
        f64::from_bits(u64::from_le_bytes(self.take()))
    }
}

/// Starts with the `0x00` tag of an `Ok` response.
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer { buf: vec![0] }
    }
    pub fn bool(&mut self, x: bool) {
        self.buf.push(x as u8)
    }
    pub fn i8(&mut self, x: i8) {
        self.buf.push(x as u8)
    }
    pub fn i32(&mut self, x: i32) {
        self.buf.extend(x.to_le_bytes())
    }
    pub fn i64(&mut self, x: i64) {
        self.buf.extend(x.to_le_bytes())
    }
    pub fn f32(&mut self, x: f32) {
        self.buf.extend(x.to_bits().to_le_bytes())
    }
    pub fn f64(&mut self, x: f64) {
        self.buf.extend(x.to_bits().to_le_bytes())
    }
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

static LAST_PANIC: Mutex<String> = Mutex::new(String::new());

fn classify(msg: &str) -> u8 {
    if msg.contains("divide by zero") || msg.contains("divisor of zero") {
        0
    } else if msg.contains("out of bounds") {
        1
    } else if msg.contains("tw:ExternDomain") {
        2
    } else {
        3
    }
}

fn failure(msg: &str) -> Vec<u8> {
    let mut out = vec![1u8, classify(msg)];
    out.extend((msg.len() as u32).to_le_bytes());
    out.extend(msg.as_bytes());
    out
}

/// Reads requests of `request_len` bytes from stdin until EOF, and answers each on stdout.
pub fn serve(request_len: usize, run: fn(&mut Reader) -> Vec<u8>) {
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let msg = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".to_string());
        *LAST_PANIC.lock().unwrap() = msg;
    }));
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    loop {
        let mut buf = vec![0u8; request_len];
        match stdin.read_exact(&mut buf) {
            Ok(()) => {}
            Err(_) => return,
        }
        let mut r = Reader::new(buf);
        let reply = match catch_unwind(AssertUnwindSafe(|| run(&mut r))) {
            Ok(bytes) => bytes,
            Err(_) => failure(&LAST_PANIC.lock().unwrap().clone()),
        };
        if stdout.write_all(&reply).and_then(|_| stdout.flush()).is_err() {
            return;
        }
    }
}
