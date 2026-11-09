//! Wire format between the harness and a generated driver (SPEC §7.4).
//!
//! Little-endian. A request is `origin:[u64;3] param* host*`. A response is `0x00 value*` or
//! `0x01 kind:u8 len:u32 utf8[len]`. Values are positional and typed by the program, with no tags.

use tungsten_check::Value;
use tungsten_core::{F64x3, I32x3};
use tungsten_ir::model::{ComputeUnit, HostInput, Name, OptimizedProgram};
use tungsten_ir::VariableType;

use crate::env::TestEnv;

pub const KIND_DIV_BY_ZERO: u8 = 0;
pub const KIND_OUT_OF_BOUNDS: u8 = 1;
pub const KIND_EXTERN_DOMAIN: u8 = 2;
pub const KIND_OTHER: u8 = 3;

pub fn kind_name(kind: u8) -> &'static str {
    match kind {
        KIND_DIV_BY_ZERO => "DivByZero",
        KIND_OUT_OF_BOUNDS => "OutOfBounds",
        KIND_EXTERN_DOMAIN => "ExternDomain",
        _ => "other",
    }
}

/// Types a driver can encode: no tables, anywhere.
pub fn has_table_type(t: &VariableType) -> bool {
    match t {
        VariableType::Handle(_) => true,
        VariableType::Array(e, _) => has_table_type(e),
        _ => false,
    }
}

fn record_fields<'a>(env: &'a TestEnv, name: &str) -> Result<&'a [(&'static str, VariableType)], String> {
    env.spec()
        .records
        .iter()
        .find(|r| r.name == name)
        .map(|r| r.fields.as_slice())
        .ok_or_else(|| format!("unknown record `{name}`"))
}

/// Encoded size in bytes.
pub fn size_of(t: &VariableType, env: &TestEnv) -> Result<usize, String> {
    Ok(match t {
        VariableType::Bool | VariableType::I8 => 1,
        VariableType::I32 | VariableType::F32 => 4,
        VariableType::I64 | VariableType::F64 => 8,
        VariableType::Vec3 => 24,
        VariableType::Pos3 => 12,
        VariableType::Array(e, n) => size_of(e, env)? * n,
        VariableType::Extern(name) => {
            let mut s = 0;
            for (_, ft) in record_fields(env, name)? {
                s += size_of(ft, env)?;
            }
            s
        }
        other => return Err(format!("type {other:?} can't be sent to a backend")),
    })
}

pub fn encode_value(v: &Value, t: &VariableType, env: &TestEnv, out: &mut Vec<u8>) -> Result<(), String> {
    match (v, t) {
        (Value::Bool(b), VariableType::Bool) => out.push(*b as u8),
        (Value::I8(x), VariableType::I8) => out.push(*x as u8),
        (Value::I32(x), VariableType::I32) => out.extend(x.to_le_bytes()),
        (Value::I64(x), VariableType::I64) => out.extend(x.to_le_bytes()),
        (Value::F32(b), VariableType::F32) => out.extend(b.to_le_bytes()),
        (Value::F64(b), VariableType::F64) => out.extend(b.to_le_bytes()),
        (Value::Vec3(p), VariableType::Vec3) => {
            for b in [p.x, p.y, p.z] {
                out.extend(b.to_le_bytes());
            }
        }
        (Value::Pos3(p), VariableType::Pos3) => {
            for x in [p.x, p.y, p.z] {
                out.extend(x.to_le_bytes());
            }
        }
        (Value::Array(vs), VariableType::Array(e, n)) if vs.len() == *n => {
            for v in vs {
                encode_value(v, e, env, out)?;
            }
        }
        (Value::Record(_, vs), VariableType::Extern(name)) => {
            let fields = record_fields(env, name)?;
            if fields.len() != vs.len() {
                return Err(format!("record `{name}`: wrong field count"));
            }
            for (v, (_, ft)) in vs.iter().zip(fields) {
                encode_value(v, ft, env, out)?;
            }
        }
        _ => return Err(format!("value {v:?} doesn't fit type {t:?}")),
    }
    Ok(())
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let end = self.pos + N;
        let s = self.buf.get(self.pos..end).ok_or("response is shorter than its types predict")?;
        self.pos = end;
        Ok(s.try_into().unwrap())
    }
    pub fn rest(&self) -> usize {
        self.buf.len() - self.pos
    }
}

pub fn decode_value(r: &mut Reader, t: &VariableType, env: &TestEnv) -> Result<Value, String> {
    Ok(match t {
        VariableType::Bool => Value::Bool(r.take::<1>()?[0] != 0),
        VariableType::I8 => Value::I8(r.take::<1>()?[0] as i8),
        VariableType::I32 => Value::I32(i32::from_le_bytes(r.take()?)),
        VariableType::I64 => Value::I64(i64::from_le_bytes(r.take()?)),
        VariableType::F32 => Value::F32(u32::from_le_bytes(r.take()?)),
        VariableType::F64 => Value::F64(u64::from_le_bytes(r.take()?)),
        VariableType::Vec3 => Value::Vec3(F64x3 {
            x: u64::from_le_bytes(r.take()?),
            y: u64::from_le_bytes(r.take()?),
            z: u64::from_le_bytes(r.take()?),
        }),
        VariableType::Pos3 => Value::Pos3(I32x3 {
            x: i32::from_le_bytes(r.take()?),
            y: i32::from_le_bytes(r.take()?),
            z: i32::from_le_bytes(r.take()?),
        }),
        VariableType::Array(e, n) => {
            Value::Array((0..*n).map(|_| decode_value(r, e, env)).collect::<Result<_, _>>()?)
        }
        VariableType::Extern(name) => {
            let fields = record_fields(env, name)?;
            let id = env.registry.record_id(name).ok_or_else(|| format!("unknown record `{name}`"))?;
            let mut vs = vec![];
            for (_, ft) in fields {
                vs.push(decode_value(r, ft, env)?);
            }
            Value::Record(id, vs)
        }
        other => return Err(format!("type {other:?} can't be received from a backend")),
    })
}

/// What a program looks like on the wire: the auto params and static hosts (arena order) and
/// the entries.
#[derive(Debug, Clone)]
pub struct TestIo {
    /// Name and type of every auto param, in `auto_params` arena order.
    pub params: Vec<(String, VariableType)>,
    /// Name and type of every static host, in `host_inputs` arena order.
    pub hosts: Vec<(String, VariableType)>,
    pub entries: Vec<EntryIo>,
}

#[derive(Debug, Clone)]
pub struct EntryIo {
    pub ty: VariableType,
    pub dims: (i32, i32, i32),
}

impl EntryIo {
    pub fn cells(&self) -> usize {
        (self.dims.0 * self.dims.1 * self.dims.2) as usize
    }
}

impl TestIo {
    pub fn from_program(p: &OptimizedProgram) -> Result<TestIo, String> {
        let params = p.auto_params.values().map(|a| (a.name.clone(), a.t.clone())).collect();
        let mut hosts = vec![];
        for (_, h) in p.host_inputs.iter() {
            match h {
                HostInput::Static { name: Name::Named(n), t } => hosts.push((n.clone(), t.clone())),
                other => return Err(format!("host {other:?} has no name to bind")),
            }
        }
        let entries = p
            .entry_points
            .iter()
            .map(|e| {
                let ComputeUnit::DensityFunction { function, .. } = &p.compute_units[e.density_function];
                EntryIo { ty: p.functions[*function].return_type.clone(), dims: e.dimensions }
            })
            .collect();
        Ok(TestIo { params, hosts, entries })
    }

    /// Entries that return tables can't be encoded (SPEC §3.3).
    pub fn check_encodable(&self) -> Result<(), String> {
        for (i, e) in self.entries.iter().enumerate() {
            if has_table_type(&e.ty) {
                return Err(format!(
                    "entry {i} returns {:?}, which holds a table and can't be compared on a backend",
                    e.ty
                ));
            }
        }
        Ok(())
    }

    /// Bytes of the host part of a request.
    pub fn param_bytes(&self, env: &TestEnv) -> Result<usize, String> {
        self.params.iter().map(|(_, t)| size_of(t, env)).sum()
    }

    pub fn host_bytes(&self, env: &TestEnv) -> Result<usize, String> {
        self.hosts.iter().map(|(_, t)| size_of(t, env)).sum()
    }

    /// Bytes of the value part of an `Ok` response.
    pub fn response_bytes(&self, env: &TestEnv) -> Result<usize, String> {
        let mut s = 0;
        for e in &self.entries {
            s += size_of(&e.ty, env)? * e.cells();
        }
        Ok(s)
    }

    /// `hosts` are matched by name, so the same case works on an inlined program.
    pub fn encode_request(
        &self,
        env: &TestEnv,
        origin: [f64; 3],
        params: &[(String, Value)],
        hosts: &[(String, Value)],
    ) -> Result<Vec<u8>, String> {
        let mut out = vec![];
        for o in origin {
            out.extend(o.to_bits().to_le_bytes());
        }
        for (name, t) in &self.params {
            let (_, v) = params
                .iter()
                .find(|(n, _)| n == name)
                .ok_or_else(|| format!("the case gives no value for param `{name}`"))?;
            encode_value(v, t, env, &mut out)?;
        }
        for (name, t) in &self.hosts {
            let (_, v) = hosts
                .iter()
                .find(|(n, _)| n == name)
                .ok_or_else(|| format!("the case gives no value for host `{name}`"))?;
            encode_value(v, t, env, &mut out)?;
        }
        Ok(out)
    }

    /// Decodes the body of an `Ok` response: per entry, per cell in flat order.
    pub fn decode_response(&self, env: &TestEnv, body: &[u8]) -> Result<Vec<Vec<Value>>, String> {
        let mut r = Reader::new(body);
        let mut out = vec![];
        for e in &self.entries {
            out.push((0..e.cells()).map(|_| decode_value(&mut r, &e.ty, env)).collect::<Result<_, _>>()?);
        }
        if r.rest() != 0 {
            return Err(format!("response is longer than its types predict ({} extra bytes)", r.rest()));
        }
        Ok(out)
    }

    /// Encodes a response body (used by golden bytes and stub drivers in tests).
    pub fn encode_response(&self, env: &TestEnv, cells: &[Vec<Value>]) -> Result<Vec<u8>, String> {
        let mut out = vec![0u8];
        for (e, vs) in self.entries.iter().zip(cells) {
            for v in vs {
                encode_value(v, &e.ty, env, &mut out)?;
            }
        }
        Ok(out)
    }
}

/// Frames a failure response, as a driver does.
pub fn encode_failure(kind: u8, msg: &str) -> Vec<u8> {
    let mut out = vec![1u8, kind];
    out.extend((msg.len() as u32).to_le_bytes());
    out.extend(msg.as_bytes());
    out
}

// The driver-side reader and writer of generated Rust drivers, to check them against the host side.
#[cfg(test)]
#[path = "../../runtime/rust/proto.rs"]
mod rt_proto;

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn leaf_types() -> Vec<VariableType> {
        use VariableType::*;
        vec![Bool, I8, I32, I64, F32, F64, Vec3, Pos3, Extern("Pair"), Extern("Wrap")]
    }

    fn any_type() -> impl Strategy<Value = VariableType> {
        let leaf = proptest::sample::select(leaf_types());
        leaf.prop_recursive(2, 6, 3, |inner| {
            (inner, 1usize..4).prop_map(|(t, n)| VariableType::Array(Box::new(t), n))
        })
    }

    /// A value of type `t` from a stream of random bits (floats keep arbitrary bit patterns).
    fn value_of(t: &VariableType, bits: &mut impl Iterator<Item = u64>, env: &TestEnv) -> Value {
        let mut next = || bits.next().unwrap_or(0);
        match t {
            VariableType::Bool => Value::Bool(next() & 1 == 1),
            VariableType::I8 => Value::I8(next() as i8),
            VariableType::I32 => Value::I32(next() as i32),
            VariableType::I64 => Value::I64(next() as i64),
            VariableType::F32 => Value::F32(next() as u32),
            VariableType::F64 => Value::F64(next()),
            VariableType::Vec3 => Value::Vec3(F64x3 { x: next(), y: next(), z: next() }),
            VariableType::Pos3 => {
                Value::Pos3(I32x3 { x: next() as i32, y: next() as i32, z: next() as i32 })
            }
            VariableType::Array(e, n) => Value::Array((0..*n).map(|_| value_of(e, bits, env)).collect()),
            VariableType::Extern(name) => {
                let id = env.registry.record_id(name).unwrap();
                let fields: Vec<VariableType> = record_fields(env, name).unwrap().iter().map(|f| f.1.clone()).collect();
                Value::Record(id, fields.iter().map(|ft| value_of(ft, bits, env)).collect())
            }
            _ => unreachable!(),
        }
    }

    proptest! {
        #[test]
        fn round_trip(t in any_type(), seed in proptest::collection::vec(any::<u64>(), 64)) {
            let env = TestEnv::new();
            let v = value_of(&t, &mut seed.iter().copied().cycle(), &env);
            let mut bytes = vec![];
            encode_value(&v, &t, &env, &mut bytes).unwrap();
            prop_assert_eq!(bytes.len(), size_of(&t, &env).unwrap());
            let mut r = Reader::new(&bytes);
            let back = decode_value(&mut r, &t, &env).unwrap();
            prop_assert_eq!(r.rest(), 0);
            prop_assert_eq!(back, v);
        }

        #[test]
        fn short_input_is_an_error(t in any_type(), cut in 1usize..8) {
            let env = TestEnv::new();
            let v = value_of(&t, &mut std::iter::repeat(7u64), &env);
            let mut bytes = vec![];
            encode_value(&v, &t, &env, &mut bytes).unwrap();
            bytes.truncate(bytes.len().saturating_sub(cut));
            prop_assert!(decode_value(&mut Reader::new(&bytes), &t, &env).is_err());
        }
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join("")
    }

    /// Golden bytes, shared with the Java driver in milestone 3 (`tests/golden/protocol.txt`).
    #[test]
    fn golden_bytes() {
        let env = TestEnv::new();
        let io = TestIo {
            params: vec![("seed".into(), VariableType::I64)],
            hosts: vec![("w".into(), VariableType::Extern("Wrap")), ("n".into(), VariableType::I8)],
            entries: vec![
                EntryIo { ty: VariableType::Array(Box::new(VariableType::I32), 2), dims: (1, 1, 1) },
                EntryIo { ty: VariableType::Pos3, dims: (2, 1, 1) },
            ],
        };
        let wrap = Value::Record(
            env.registry.record_id("Wrap").unwrap(),
            vec![
                Value::Record(
                    env.registry.record_id("Pair").unwrap(),
                    vec![Value::F64((-0.0f64).to_bits()), Value::I32(-2)],
                ),
                Value::Vec3(F64x3 { x: 1.5f64.to_bits(), y: f64::NAN.to_bits(), z: 0 }),
            ],
        );
        let req = io
            .encode_request(&env, [1.5, -0.0, 2.0], &[("seed".into(), Value::I64(0x0102_0304_0506_0708))], &[("n".into(), Value::I8(-3)), ("w".into(), wrap)])
            .unwrap();
        let resp = io
            .encode_response(
                &env,
                &[
                    vec![Value::Array(vec![Value::I32(1), Value::I32(-1)])],
                    vec![Value::Pos3(I32x3 { x: 1, y: 2, z: 3 }), Value::Pos3(I32x3 { x: -1, y: 0, z: 7 })],
                ],
            )
            .unwrap();
        let fail = encode_failure(KIND_DIV_BY_ZERO, "boom");
        let got = format!("request {}\nresponse {}\nfailure {}\n", hex(&req), hex(&resp), hex(&fail));
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/protocol.txt");
        let want = std::fs::read_to_string(path).unwrap_or_default();
        assert_eq!(got, want, "golden bytes changed; if intended, write this to {path}:\n{got}");
        // The response part decodes back.
        let cells = io.decode_response(&env, &resp[1..]).unwrap();
        assert_eq!(cells[1][1], Value::Pos3(I32x3 { x: -1, y: 0, z: 7 }));
    }

    /// An entry returning a table can't be encoded: a test error on every backend (SPEC §3.3).
    #[test]
    fn table_entries_are_a_test_error() {
        let env = TestEnv::new();
        let src = r#"
program {
    param seed: i64;
    auto p: handle PerlinNoiseSampler = make_permutation_table(param(seed), 1L, 2L, 3L, 4L, 5L);
    fn run() -> handle PerlinNoiseSampler { return auto(p); }
    unit u { main run; autos [p]; }
    entry u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
@case "c" origin(0, 0, 0) param(seed = 1)
"#;
        let tf = crate::corpus::load_source(src, &env).unwrap();
        let io = TestIo::from_program(&tf.program).unwrap();
        let err = io.check_encodable().unwrap_err();
        assert!(err.contains("holds a table"), "{err}");
        // And a plain program is fine.
        let ok = TestIo { params: vec![], hosts: vec![], entries: vec![EntryIo { ty: VariableType::F64, dims: (1, 1, 1) }] };
        ok.check_encodable().unwrap();
    }

    /// The driver-side `Writer` and `Reader` agree with the host side byte for byte.
    #[test]
    fn driver_side_matches_host_side() {
        let env = TestEnv::new();
        let mut w = rt_proto::Writer::new();
        w.bool(true);
        w.i8(-3);
        w.i32(-2);
        w.i64(i64::MIN);
        w.f32(f32::from_bits(0x7fc0_0001));
        w.f64(-0.0);
        let bytes = w.finish();
        let mut host = vec![0u8];
        for (v, t) in [
            (Value::Bool(true), VariableType::Bool),
            (Value::I8(-3), VariableType::I8),
            (Value::I32(-2), VariableType::I32),
            (Value::I64(i64::MIN), VariableType::I64),
            (Value::F32(0x7fc0_0001), VariableType::F32),
            (Value::F64((-0.0f64).to_bits()), VariableType::F64),
        ] {
            encode_value(&v, &t, &env, &mut host).unwrap();
        }
        assert_eq!(bytes, host);
        let mut r = rt_proto::Reader::new(bytes[1..].to_vec());
        assert!(r.bool());
        assert_eq!(r.i8(), -3);
        assert_eq!(r.i32(), -2);
        assert_eq!(r.i64(), i64::MIN);
        assert_eq!(r.f32().to_bits(), 0x7fc0_0001);
        assert_eq!(r.f64().to_bits(), (-0.0f64).to_bits());
    }
}
