//! Runtime math and iteration for generated Rust code (`use crate::mathf64::*`).
//!
//! Vendored from the verified runtime in the sister project, `tungsten-frontends/tungsten-mc`
//! (`src/mathf64.rs` at commit 88b88ae, `src/utilsf64.rs` for `Config`), which the generated
//! `density.rs` there compiles against. Copied verbatim: `Vec3`, `Pos3` and their operators,
//! `as_index`, `index`, `Iter3D`, `PositionIterator`, `iter_3d`, `Config`.
//! Left out: everything Minecraft-specific (Perlin, Xoroshiro, `corner*`, structure weights,
//! `preliminary_surface_*`) and `pow/floor/ceil`, which the registry externs replace with
//! Java-semantics versions (see `api.rs`).
//! Added here (not upstream): `Neg for Vec3`, `Default` on `Vec3` and `Pos3` (so records can
//! derive it), and the re-exports at the bottom.
#![allow(dead_code)]

#[derive(Copy, Clone, Debug, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn dot(&self, other: Vec3) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }
    pub fn fract(&self) -> Vec3 {
        Vec3::new(self.x.fract(), self.y.fract(), self.z.fract())
    }
    pub fn floor(&self) -> Vec3 {
        Vec3::new(self.x.floor(), self.y.floor(), self.z.floor())
    }
}

impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl std::ops::Mul for Vec3 {
    type Output = Self;
    fn mul(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x * other.x, self.y * other.y, self.z * other.z)
    }
}

impl std::ops::Mul<f64> for Vec3 {
    type Output = Self;
    fn mul(self, scalar: f64) -> Vec3 {
        Vec3::new(self.x * scalar, self.y * scalar, self.z * scalar)
    }
}

impl std::ops::Mul<i32> for Vec3 {
    type Output = Self;
    fn mul(self, scalar: i32) -> Vec3 {
        Vec3::new(
            self.x * scalar as f64,
            self.y * scalar as f64,
            self.z * scalar as f64,
        )
    }
}

impl std::ops::Div<f64> for Vec3 {
    type Output = Self;
    fn div(self, scalar: f64) -> Vec3 {
        Vec3::new(self.x / scalar, self.y / scalar, self.z / scalar)
    }
}

impl std::ops::Div for Vec3 {
    type Output = Self;
    fn div(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x / other.x, self.y / other.y, self.z / other.z)
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct Pos3 {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Pos3 {
    #[inline(always)]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

impl std::ops::Mul<i32> for Pos3 {
    type Output = Self;
    #[inline(always)]
    fn mul(self, scalar: i32) -> Pos3 {
        Pos3::new(self.x * scalar, self.y * scalar, self.z * scalar)
    }
}

impl std::ops::Add for Pos3 {
    type Output = Self;
    #[inline(always)]
    fn add(self, other: Pos3) -> Pos3 {
        Pos3::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl std::ops::Sub for Pos3 {
    type Output = Self;
    #[inline(always)]
    fn sub(self, other: Pos3) -> Pos3 {
        Pos3::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl std::ops::Mul for Pos3 {
    type Output = Self;
    #[inline(always)]
    fn mul(self, other: Pos3) -> Pos3 {
        Pos3::new(self.x * other.x, self.y * other.y, self.z * other.z)
    }
}

impl std::ops::Add<i32> for Pos3 {
    type Output = Self;
    #[inline(always)]
    fn add(self, scalar: i32) -> Pos3 {
        Pos3::new(self.x + scalar, self.y + scalar, self.z + scalar)
    }
}

impl std::ops::Add<Pos3> for Vec3 {
    type Output = Self;
    #[inline(always)]
    fn add(self, other: Pos3) -> Vec3 {
        Vec3::new(
            self.x + other.x as f64,
            self.y + other.y as f64,
            self.z + other.z as f64,
        )
    }
}

impl std::ops::Sub<Pos3> for Vec3 {
    type Output = Self;
    #[inline(always)]
    fn sub(self, other: Pos3) -> Vec3 {
        Vec3::new(
            self.x - other.x as f64,
            self.y - other.y as f64,
            self.z - other.z as f64,
        )
    }
}

impl std::ops::Mul<Pos3> for Vec3 {
    type Output = Self;
    #[inline(always)]
    fn mul(self, other: Pos3) -> Vec3 {
        Vec3::new(
            self.x * other.x as f64,
            self.y * other.y as f64,
            self.z * other.z as f64,
        )
    }
}

impl std::ops::Mul<Vec3> for Pos3 {
    type Output = Vec3;
    #[inline(always)]
    fn mul(self, other: Vec3) -> Vec3 {
        Vec3::new(
            self.x as f64 * other.x,
            self.y as f64 * other.y,
            self.z as f64 * other.z,
        )
    }
}

#[inline(always)]
pub fn as_index(pos: Pos3, size_x: i32, size_y: i32, _size_z: i32) -> usize {
    (pos.z * size_y * size_x + pos.y * size_x + pos.x) as usize
}

#[inline(always)]
pub fn index(pos: PositionIterator) -> usize {
    pos.idx
}

pub struct Iter3D {
    idx: usize,
    x: i32,
    y: i32,
    z: i32,
    pub mx: i32,
    pub my: i32,
    pub mz: i32,
}

#[derive(Copy, Clone, Debug)]
pub struct PositionIterator {
    pub pos: Pos3,
    pub idx: usize,
}

impl Iterator for Iter3D {
    type Item = PositionIterator;
    #[inline(always)]
    fn next(&mut self) -> Option<PositionIterator> {
        if self.z >= self.mz {
            return None;
        }
        let current = self.always_next();
        Some(current)
    }
}

pub struct ComplexIter3D {
    inner: Iter3D,
}

impl Iterator for ComplexIter3D {
    type Item = (usize, [Pos3; 4]);
    #[inline(always)]
    fn next(&mut self) -> Option<(usize, [Pos3; 4])> {
        if self.inner.z >= self.inner.mz {
            return None;
        }

        // the logic is: simply do 4 iterations of the inner iterator and collect the results into an array
        let mut results = [Pos3::new(0, 0, 0); 4];
        for i in 0..4 {
            results[i] = self.inner.always_next().pos;
        }
        Some((self.inner.idx - 4, results))
    }
}

impl Iter3D {
    pub fn always_next(&mut self) -> PositionIterator {
        let current = PositionIterator {
            pos: Pos3::new(self.x, self.y, self.z),
            idx: self.idx,
        };
        self.x += 1;
        self.idx += 1;
        if self.x >= self.mx {
            self.x = 0;
            self.y += 1;
            if self.y >= self.my {
                self.y = 0;
                self.z += 1;
            }
        }
        current
    }

    pub fn chunk(&self) -> ComplexIter3D {
        ComplexIter3D {
            inner: Iter3D {
                idx: self.idx,
                x: self.x,
                y: self.y,
                z: self.z,
                mx: self.mx,
                my: self.my,
                mz: self.mz,
            },
        }
    }
}

impl ExactSizeIterator for Iter3D {
    fn len(&self) -> usize {
        (self.mx * self.my * self.mz) as usize
    }
}

#[inline(always)]
pub fn iter_usize(i: PositionIterator) -> usize {
    i.idx
}

impl Into<Pos3> for PositionIterator {
    #[inline(always)]
    fn into(self) -> Pos3 {
        self.pos
    }
}

#[inline(always)]
pub fn iter_3d(config: &Config) -> Iter3D {
    let mx = config.dimensions.0;
    let my = config.dimensions.1;
    let mz = config.dimensions.2;
    Iter3D {
        idx: 0,
        x: 0,
        y: 0,
        z: 0,
        mx,
        my,
        mz,
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Config {
    pub origin: Vec3,
    pub origin_scale: Vec3,
    pub position_scale: Vec3,
    pub dimensions: (i32, i32, i32),
}

impl std::ops::Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

// Names the generated code reaches through `use crate::mathf64::*`.
pub use crate::api::*;
pub use crate::records::*;
