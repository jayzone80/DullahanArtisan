use std::ops::{Add, Div, Mul, Neg, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };
    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    #[inline]
    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y
    }
    #[inline]
    pub fn perp_dot(self, o: Self) -> f32 {
        self.x * o.y - self.y * o.x
    }
    #[inline]
    pub fn len(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    #[inline]
    pub fn normalized(self) -> Self {
        let l = self.len();
        if l > 1e-12 {
            Self::new(self.x / l, self.y / l)
        } else {
            Self::ZERO
        }
    }
}

impl Add for Vec2 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
}
impl Sub for Vec2 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f32> for Vec2 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0 };
    pub const ONE: Self = Self { x: 1.0, y: 1.0, z: 1.0 };
    pub const X: Self = Self { x: 1.0, y: 0.0, z: 0.0 };
    pub const Y: Self = Self { x: 0.0, y: 1.0, z: 0.0 };
    pub const Z: Self = Self { x: 0.0, y: 0.0, z: 1.0 };

    #[inline]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub const fn splat(v: f32) -> Self {
        Self { x: v, y: v, z: v }
    }
    #[inline]
    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    #[inline]
    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    #[inline]
    pub fn len_sq(self) -> f32 {
        self.dot(self)
    }
    #[inline]
    pub fn len(self) -> f32 {
        self.len_sq().sqrt()
    }
    #[inline]
    pub fn normalized(self) -> Self {
        let l = self.len();
        if l > 1e-12 {
            self / l
        } else {
            Self::ZERO
        }
    }
    #[inline]
    pub fn min(self, o: Self) -> Self {
        Self::new(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }
    #[inline]
    pub fn max(self, o: Self) -> Self {
        Self::new(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }
    #[inline]
    pub fn lerp(self, o: Self, t: f32) -> Self {
        Self::new(
            self.x + (o.x - self.x) * t,
            self.y + (o.y - self.y) * t,
            self.z + (o.z - self.z) * t,
        )
    }
    #[inline]
    pub fn mul_elem(self, o: Self) -> Self {
        Self::new(self.x * o.x, self.y * o.y, self.z * o.z)
    }
    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
    #[inline]
    pub fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
}

impl Add for Vec3 {
    type Output = Self;
    #[inline]
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Self;
    #[inline]
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Self;
    #[inline]
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}
impl Mul<Vec3> for f32 {
    type Output = Vec3;
    #[inline]
    fn mul(self, v: Vec3) -> Vec3 {
        Vec3::new(self * v.x, self * v.y, self * v.z)
    }
}
impl Div<f32> for Vec3 {
    type Output = Self;
    #[inline]
    fn div(self, s: f32) -> Self {
        let inv = 1.0 / s;
        Self::new(self.x * inv, self.y * inv, self.z * inv)
    }
}
impl Neg for Vec3 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

/// Column-major 4x4 matrix (m[col][row]), matching OpenGL / glTF 2.0 conventions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat4 {
    pub m: [[f32; 4]; 4],
}

impl Mat4 {
    pub const IDENTITY: Self = Self {
        m: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    };

    pub fn translation(t: Vec3) -> Self {
        let mut r = Self::IDENTITY;
        r.m[3][0] = t.x;
        r.m[3][1] = t.y;
        r.m[3][2] = t.z;
        r
    }

    pub fn scale(s: Vec3) -> Self {
        let mut r = Self::IDENTITY;
        r.m[0][0] = s.x;
        r.m[1][1] = s.y;
        r.m[2][2] = s.z;
        r
    }

    pub fn rot_x(deg: f32) -> Self {
        let a = deg.to_radians();
        let (s, c) = a.sin_cos();
        let mut r = Self::IDENTITY;
        r.m[1][1] = c;
        r.m[1][2] = s;
        r.m[2][1] = -s;
        r.m[2][2] = c;
        r
    }

    pub fn rot_y(deg: f32) -> Self {
        let a = deg.to_radians();
        let (s, c) = a.sin_cos();
        let mut r = Self::IDENTITY;
        r.m[0][0] = c;
        r.m[0][2] = -s;
        r.m[2][0] = s;
        r.m[2][2] = c;
        r
    }

    pub fn rot_z(deg: f32) -> Self {
        let a = deg.to_radians();
        let (s, c) = a.sin_cos();
        let mut r = Self::IDENTITY;
        r.m[0][0] = c;
        r.m[0][1] = s;
        r.m[1][0] = -s;
        r.m[1][1] = c;
        r
    }

    /// Standard TRS composition: T * Rz * Ry * Rx * S (Euler XYZ local rotations applied X then Y then Z).
    pub fn from_trs(pos: Vec3, rot_deg: Vec3, scale: Vec3) -> Self {
        let mut mat = Self::translation(pos);
        if rot_deg.z.abs() > 1e-7 {
            mat = mat.mul(Self::rot_z(rot_deg.z));
        }
        if rot_deg.y.abs() > 1e-7 {
            mat = mat.mul(Self::rot_y(rot_deg.y));
        }
        if rot_deg.x.abs() > 1e-7 {
            mat = mat.mul(Self::rot_x(rot_deg.x));
        }
        if (scale.x - 1.0).abs() > 1e-7
            || (scale.y - 1.0).abs() > 1e-7
            || (scale.z - 1.0).abs() > 1e-7
        {
            mat = mat.mul(Self::scale(scale));
        }
        mat
    }

    #[inline]
    pub fn mul(self, rhs: Self) -> Self {
        let mut out = [[0.0f32; 4]; 4];
        for c in 0..4 {
            for r in 0..4 {
                out[c][r] = self.m[0][r] * rhs.m[c][0]
                    + self.m[1][r] * rhs.m[c][1]
                    + self.m[2][r] * rhs.m[c][2]
                    + self.m[3][r] * rhs.m[c][3];
            }
        }
        Self { m: out }
    }

    #[inline]
    pub fn transform_point(self, p: Vec3) -> Vec3 {
        Vec3::new(
            self.m[0][0] * p.x + self.m[1][0] * p.y + self.m[2][0] * p.z + self.m[3][0],
            self.m[0][1] * p.x + self.m[1][1] * p.y + self.m[2][1] * p.z + self.m[3][1],
            self.m[0][2] * p.x + self.m[1][2] * p.y + self.m[2][2] * p.z + self.m[3][2],
        )
    }

    /// Transform a normal vector using the inverse transpose of the upper 3x3 submatrix.
    pub fn transform_normal(self, n: Vec3) -> Vec3 {
        let a = Vec3::new(self.m[0][0], self.m[0][1], self.m[0][2]);
        let b = Vec3::new(self.m[1][0], self.m[1][1], self.m[1][2]);
        let c = Vec3::new(self.m[2][0], self.m[2][1], self.m[2][2]);
        let c0 = b.cross(c);
        let c1 = c.cross(a);
        let c2 = a.cross(b);
        let det = a.dot(c0);
        if det.abs() < 1e-12 {
            return n.normalized();
        }
        let inv_det = 1.0 / det;
        // Cofactor matrix divided by det is the exact inverse transpose M^{-T}.
        // For reflections (det < 0), M^{-T} already reflects normals outward to match flipped winding.
        let res = (c0 * n.x + c1 * n.y + c2 * n.z) * inv_det;
        res.normalized()
    }

    /// Determinant of the upper 3x3 linear submatrix (negative means odd-axis reflection).
    #[inline]
    pub fn det3(self) -> f32 {
        let a = Vec3::new(self.m[0][0], self.m[0][1], self.m[0][2]);
        let b = Vec3::new(self.m[1][0], self.m[1][1], self.m[1][2]);
        let c = Vec3::new(self.m[2][0], self.m[2][1], self.m[2][2]);
        a.dot(b.cross(c))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn empty() -> Self {
        Self {
            min: Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY),
            max: Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY),
        }
    }
    pub fn is_valid(&self) -> bool {
        self.min.x <= self.max.x && self.min.y <= self.max.y && self.min.z <= self.max.z
    }
    pub fn expand(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    pub fn expand_aabb(&mut self, o: &Aabb) {
        if o.is_valid() {
            self.expand(o.min);
            self.expand(o.max);
        }
    }
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }
}

/// Deterministic fast PRNG (SplitMix64) for geometry jitter, noise, and audio scatter.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut r = Self {
            state: seed.wrapping_add(0x9E37_79B9_7F4A_7C15),
        };
        let _ = r.next_u64();
        r
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform float in [0.0, 1.0).
    #[inline]
    pub fn f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) * (1.0 / 16777216.0)
    }

    /// Uniform f64 in [0.0, 1.0).
    #[inline]
    pub fn f64(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64) * (1.0 / 9007199254740992.0)
    }

    /// Uniform float in [lo, hi).
    #[inline]
    pub fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }

    /// Uniform f64 in [lo, hi).
    #[inline]
    pub fn range_f64(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f64()
    }

    /// Uniform signed float in [-1.0, 1.0).
    #[inline]
    pub fn signed_f32(&mut self) -> f32 {
        self.f32() * 2.0 - 1.0
    }

    /// Uniform usize in [0, bound).
    #[inline]
    pub fn usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() as usize) % bound
        }
    }
}

/// Parse `#RGB`, `#RRGGBB`, or `#RRGGBBAA` into `[r, g, b, a]`.
pub fn parse_hex_rgba(s: &str) -> Result<[u8; 4], String> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).map_err(|_| format!("invalid hex '{s}'"))?;
            let g = u8::from_str_radix(&hex[1..2], 16).map_err(|_| format!("invalid hex '{s}'"))?;
            let b = u8::from_str_radix(&hex[2..3], 16).map_err(|_| format!("invalid hex '{s}'"))?;
            Ok([r * 17, g * 17, b * 17, 255])
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).map_err(|_| format!("invalid hex '{s}'"))?;
            let g = u8::from_str_radix(&hex[2..4], 16).map_err(|_| format!("invalid hex '{s}'"))?;
            let b = u8::from_str_radix(&hex[4..6], 16).map_err(|_| format!("invalid hex '{s}'"))?;
            Ok([r, g, b, 255])
        }
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).map_err(|_| format!("invalid hex '{s}'"))?;
            let g = u8::from_str_radix(&hex[2..4], 16).map_err(|_| format!("invalid hex '{s}'"))?;
            let b = u8::from_str_radix(&hex[4..6], 16).map_err(|_| format!("invalid hex '{s}'"))?;
            let a = u8::from_str_radix(&hex[6..8], 16).map_err(|_| format!("invalid hex '{s}'"))?;
            Ok([r, g, b, a])
        }
        _ => Err(format!("invalid hex color '{s}' (expected #RGB, #RRGGBB, or #RRGGBBAA)")),
    }
}

#[inline]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}
