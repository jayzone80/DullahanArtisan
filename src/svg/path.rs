/// SVG 2D geometry, path command parser (M/L/H/V/C/S/Q/T/A/Z), adaptive Bezier flattening,
/// 2D affine transforms, and stroke-to-polygon expansion (butt/round/square caps, miter/round/bevel joins, dashes).

use std::f64::consts::{FRAC_PI_2, PI, TAU};

#[derive(Debug, Clone, Copy, Default)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }

    #[inline]
    pub fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }

    #[inline]
    pub fn scale(self, s: f64) -> Self {
        Self::new(self.x * s, self.y * s)
    }

    #[inline]
    pub fn len(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    #[inline]
    pub fn norm(self) -> Self {
        let l = self.len();
        if l > 1e-12 {
            Self::new(self.x / l, self.y / l)
        } else {
            Self::new(0.0, 0.0)
        }
    }

    #[inline]
    pub fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }

    #[inline]
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y
    }

    #[inline]
    pub fn cross(self, o: Self) -> f64 {
        self.x * o.y - self.y * o.x
    }
}

/// 2D affine transform matrix:
/// [ a c e ]
/// [ b d f ]
/// [ 0 0 1 ]
#[derive(Debug, Clone, Copy)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Affine {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    #[inline]
    pub fn translate(tx: f64, ty: f64) -> Self {
        Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: tx, f: ty }
    }

    #[inline]
    pub fn scale(sx: f64, sy: f64) -> Self {
        Self { a: sx, b: 0.0, c: 0.0, d: sy, e: 0.0, f: 0.0 }
    }

    #[inline]
    pub fn rotate_deg(deg: f64) -> Self {
        let rad = deg.to_radians();
        let (s, c) = rad.sin_cos();
        Self { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
    }

    #[inline]
    pub fn skew_x_deg(deg: f64) -> Self {
        Self { a: 1.0, b: 0.0, c: deg.to_radians().tan(), d: 1.0, e: 0.0, f: 0.0 }
    }

    #[inline]
    pub fn skew_y_deg(deg: f64) -> Self {
        Self { a: 1.0, b: deg.to_radians().tan(), c: 0.0, d: 1.0, e: 0.0, f: 0.0 }
    }

    /// Matrix multiplication `self * rhs` (apply `rhs` first, then `self`).
    #[inline]
    pub fn mul(self, r: Self) -> Self {
        Self {
            a: self.a * r.a + self.c * r.b,
            b: self.b * r.a + self.d * r.b,
            c: self.a * r.c + self.c * r.d,
            d: self.b * r.c + self.d * r.d,
            e: self.a * r.e + self.c * r.f + self.e,
            f: self.b * r.e + self.d * r.f + self.f,
        }
    }

    #[inline]
    pub fn apply(self, p: Pt) -> Pt {
        Pt::new(self.a * p.x + self.c * p.y + self.e, self.b * p.x + self.d * p.y + self.f)
    }

    pub fn inverse(self) -> Option<Self> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        Some(Self {
            a: self.d * inv,
            b: -self.b * inv,
            c: -self.c * inv,
            d: self.a * inv,
            e: (self.c * self.f - self.d * self.e) * inv,
            f: (self.b * self.e - self.a * self.f) * inv,
        })
    }

    /// Approximate linear scale factor (used for flatness tolerance and stroke width heuristics).
    pub fn avg_scale(self) -> f64 {
        let sx = (self.a * self.a + self.b * self.b).sqrt();
        let sy = (self.c * self.c + self.d * self.d).sqrt();
        0.5 * (sx + sy)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BBox {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl BBox {
    pub fn empty() -> Self {
        Self {
            min_x: f64::INFINITY,
            min_y: f64::INFINITY,
            max_x: f64::NEG_INFINITY,
            max_y: f64::NEG_INFINITY,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.min_x <= self.max_x && self.min_y <= self.max_y
    }

    pub fn include(&mut self, p: Pt) {
        self.min_x = self.min_x.min(p.x);
        self.min_y = self.min_y.min(p.y);
        self.max_x = self.max_x.max(p.x);
        self.max_y = self.max_y.max(p.y);
    }

    pub fn union(&mut self, o: BBox) {
        if o.is_valid() {
            self.min_x = self.min_x.min(o.min_x);
            self.min_y = self.min_y.min(o.min_y);
            self.max_x = self.max_x.max(o.max_x);
            self.max_y = self.max_y.max(o.max_y);
        }
    }

    pub fn expand(&mut self, pad: f64) {
        if self.is_valid() {
            self.min_x -= pad;
            self.min_y -= pad;
            self.max_x += pad;
            self.max_y += pad;
        }
    }

    pub fn width(&self) -> f64 {
        if self.is_valid() { (self.max_x - self.min_x).max(0.0) } else { 0.0 }
    }

    pub fn height(&self) -> f64 {
        if self.is_valid() { (self.max_y - self.min_y).max(0.0) } else { 0.0 }
    }
}

#[derive(Debug, Clone)]
pub struct Subpath {
    pub pts: Vec<Pt>,
    pub closed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PathData {
    pub subpaths: Vec<Subpath>,
}

impl PathData {
    pub fn transform(&self, m: Affine) -> Self {
        Self {
            subpaths: self
                .subpaths
                .iter()
                .map(|sp| Subpath {
                    pts: sp.pts.iter().map(|&p| m.apply(p)).collect(),
                    closed: sp.closed,
                })
                .collect(),
        }
    }

    pub fn bbox(&self) -> BBox {
        let mut bb = BBox::empty();
        for sp in &self.subpaths {
            for &p in &sp.pts {
                bb.include(p);
            }
        }
        bb
    }
}

/// Parse an SVG `transform="..."` attribute string.
pub fn parse_transform(s: &str) -> Result<Affine, String> {
    let mut m = Affine::IDENTITY;
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let name_start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        let name = &s[name_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'(' {
            return Err(format!("invalid transform '{s}' near '{name}'"));
        }
        i += 1;
        let args_start = i;
        while i < bytes.len() && bytes[i] != b')' {
            i += 1;
        }
        if i >= bytes.len() {
            return Err(format!("unclosed transform '{name}(...)'"));
        }
        let args = parse_number_list(&s[args_start..i])?;
        i += 1;

        let step = match name {
            "translate" => match args.len() {
                1 => Affine::translate(args[0], 0.0),
                2 => Affine::translate(args[0], args[1]),
                _ => return Err(format!("translate expects 1 or 2 args in '{s}'")),
            },
            "scale" => match args.len() {
                1 => Affine::scale(args[0], args[0]),
                2 => Affine::scale(args[0], args[1]),
                _ => return Err(format!("scale expects 1 or 2 args in '{s}'")),
            },
            "rotate" => match args.len() {
                1 => Affine::rotate_deg(args[0]),
                3 => Affine::translate(args[1], args[2])
                    .mul(Affine::rotate_deg(args[0]))
                    .mul(Affine::translate(-args[1], -args[2])),
                _ => return Err(format!("rotate expects 1 or 3 args in '{s}'")),
            },
            "skewX" => {
                if args.len() != 1 {
                    return Err(format!("skewX expects 1 arg in '{s}'"));
                }
                Affine::skew_x_deg(args[0])
            }
            "skewY" => {
                if args.len() != 1 {
                    return Err(format!("skewY expects 1 arg in '{s}'"));
                }
                Affine::skew_y_deg(args[0])
            }
            "matrix" => {
                if args.len() != 6 {
                    return Err(format!("matrix expects 6 args in '{s}'"));
                }
                Affine {
                    a: args[0],
                    b: args[1],
                    c: args[2],
                    d: args[3],
                    e: args[4],
                    f: args[5],
                }
            }
            _ => return Err(format!("unsupported transform '{name}'")),
        };
        m = m.mul(step);
    }
    Ok(m)
}

pub fn parse_number_list(s: &str) -> Result<Vec<f64>, String> {
    let mut out = Vec::new();
    let mut p = PathScanner::new(s);
    while p.has_number() {
        out.push(p.next_number()?);
    }
    p.skip_sep();
    if !p.eof() {
        return Err(format!("unexpected token in number list '{s}'"));
    }
    Ok(out)
}

struct PathScanner<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
}

impl<'a> PathScanner<'a> {
    fn new(s: &'a str) -> Self {
        Self { s, b: s.as_bytes(), i: 0 }
    }

    fn eof(&self) -> bool {
        self.i >= self.b.len()
    }

    fn skip_sep(&mut self) {
        while self.i < self.b.len() && (self.b[self.i].is_ascii_whitespace() || self.b[self.i] == b',') {
            self.i += 1;
        }
    }

    fn has_number(&mut self) -> bool {
        self.skip_sep();
        if let Some(&c) = self.b.get(self.i) {
            c.is_ascii_digit() || c == b'+' || c == b'-' || c == b'.'
        } else {
            false
        }
    }

    fn next_flag(&mut self) -> Result<bool, String> {
        self.skip_sep();
        match self.b.get(self.i).copied() {
            Some(b'0') => {
                self.i += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.i += 1;
                Ok(true)
            }
            _ => Err(format!("expected SVG arc flag 0|1 near byte {}", self.i)),
        }
    }

    fn next_number(&mut self) -> Result<f64, String> {
        self.skip_sep();
        let start = self.i;
        if matches!(self.b.get(self.i), Some(b'+') | Some(b'-')) {
            self.i += 1;
        }
        let mut saw_digit = false;
        while let Some(&c) = self.b.get(self.i) {
            if c.is_ascii_digit() {
                saw_digit = true;
                self.i += 1;
            } else {
                break;
            }
        }
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            while let Some(&c) = self.b.get(self.i) {
                if c.is_ascii_digit() {
                    saw_digit = true;
                    self.i += 1;
                } else {
                    break;
                }
            }
        }
        if !saw_digit {
            return Err(format!("expected number in SVG path near byte {start}"));
        }
        if matches!(self.b.get(self.i), Some(b'e') | Some(b'E')) {
            let save = self.i;
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+') | Some(b'-')) {
                self.i += 1;
            }
            let exp_start = self.i;
            while let Some(&c) = self.b.get(self.i) {
                if c.is_ascii_digit() {
                    self.i += 1;
                } else {
                    break;
                }
            }
            if self.i == exp_start {
                self.i = save;
            }
        }
        self.s[start..self.i]
            .parse::<f64>()
            .map_err(|_| format!("invalid float '{}'", &self.s[start..self.i]))
    }
}

/// Parse an SVG `d="..."` path string and flatten curves with the given tolerance.
pub fn parse_svg_path(d: &str, tol: f64) -> Result<PathData, String> {
    let tol = tol.max(0.05);
    let mut sc = PathScanner::new(d);
    let mut subpaths: Vec<Subpath> = Vec::new();
    let mut cur = Pt::new(0.0, 0.0);
    let mut sub_start = Pt::new(0.0, 0.0);
    let mut cur_pts: Vec<Pt> = Vec::new();
    let mut last_ctrl: Option<Pt> = None;
    let mut last_cmd: u8 = 0;

    let flush_open = |pts: &mut Vec<Pt>, out: &mut Vec<Subpath>| {
        if pts.len() >= 2 {
            out.push(Subpath { pts: std::mem::take(pts), closed: false });
        } else {
            pts.clear();
        }
    };

    while !sc.eof() {
        sc.skip_sep();
        if sc.eof() {
            break;
        }
        let ch = sc.b[sc.i];
        let cmd = if ch.is_ascii_alphabetic() {
            sc.i += 1;
            ch
        } else if last_cmd != 0 {
            match last_cmd {
                b'M' => b'L',
                b'm' => b'l',
                other => other,
            }
        } else {
            return Err(format!("SVG path must start with a command, got '{}'", ch as char));
        };

        match cmd {
            b'M' | b'm' => {
                flush_open(&mut cur_pts, &mut subpaths);
                let x = sc.next_number()?;
                let y = sc.next_number()?;
                cur = if cmd == b'm' { Pt::new(cur.x + x, cur.y + y) } else { Pt::new(x, y) };
                sub_start = cur;
                cur_pts.push(cur);
                last_ctrl = None;
                last_cmd = cmd;
                while sc.has_number() {
                    let lx = sc.next_number()?;
                    let ly = sc.next_number()?;
                    cur = if cmd == b'm' { Pt::new(cur.x + lx, cur.y + ly) } else { Pt::new(lx, ly) };
                    push_unique(&mut cur_pts, cur);
                    last_cmd = if cmd == b'm' { b'l' } else { b'L' };
                }
            }
            b'L' | b'l' => {
                let x = sc.next_number()?;
                let y = sc.next_number()?;
                cur = if cmd == b'l' { Pt::new(cur.x + x, cur.y + y) } else { Pt::new(x, y) };
                if cur_pts.is_empty() {
                    cur_pts.push(cur);
                } else {
                    push_unique(&mut cur_pts, cur);
                }
                last_ctrl = None;
                last_cmd = cmd;
            }
            b'H' | b'h' => {
                let x = sc.next_number()?;
                cur.x = if cmd == b'h' { cur.x + x } else { x };
                push_unique(&mut cur_pts, cur);
                last_ctrl = None;
                last_cmd = cmd;
            }
            b'V' | b'v' => {
                let y = sc.next_number()?;
                cur.y = if cmd == b'v' { cur.y + y } else { y };
                push_unique(&mut cur_pts, cur);
                last_ctrl = None;
                last_cmd = cmd;
            }
            b'C' | b'c' => {
                let (x1, y1, x2, y2, x, y) = (
                    sc.next_number()?,
                    sc.next_number()?,
                    sc.next_number()?,
                    sc.next_number()?,
                    sc.next_number()?,
                    sc.next_number()?,
                );
                let (p1, p2, p3) = if cmd == b'c' {
                    (
                        Pt::new(cur.x + x1, cur.y + y1),
                        Pt::new(cur.x + x2, cur.y + y2),
                        Pt::new(cur.x + x, cur.y + y),
                    )
                } else {
                    (Pt::new(x1, y1), Pt::new(x2, y2), Pt::new(x, y))
                };
                flatten_cubic(cur, p1, p2, p3, tol, &mut cur_pts, 0);
                cur = p3;
                last_ctrl = Some(p2);
                last_cmd = cmd;
            }
            b'S' | b's' => {
                let (x2, y2, x, y) = (
                    sc.next_number()?,
                    sc.next_number()?,
                    sc.next_number()?,
                    sc.next_number()?,
                );
                let p1 = if matches!(last_cmd, b'C' | b'c' | b'S' | b's') {
                    if let Some(lc) = last_ctrl {
                        Pt::new(2.0 * cur.x - lc.x, 2.0 * cur.y - lc.y)
                    } else {
                        cur
                    }
                } else {
                    cur
                };
                let (p2, p3) = if cmd == b's' {
                    (Pt::new(cur.x + x2, cur.y + y2), Pt::new(cur.x + x, cur.y + y))
                } else {
                    (Pt::new(x2, y2), Pt::new(x, y))
                };
                flatten_cubic(cur, p1, p2, p3, tol, &mut cur_pts, 0);
                cur = p3;
                last_ctrl = Some(p2);
                last_cmd = cmd;
            }
            b'Q' | b'q' => {
                let (x1, y1, x, y) = (
                    sc.next_number()?,
                    sc.next_number()?,
                    sc.next_number()?,
                    sc.next_number()?,
                );
                let (p1, p2) = if cmd == b'q' {
                    (Pt::new(cur.x + x1, cur.y + y1), Pt::new(cur.x + x, cur.y + y))
                } else {
                    (Pt::new(x1, y1), Pt::new(x, y))
                };
                flatten_quad(cur, p1, p2, tol, &mut cur_pts, 0);
                cur = p2;
                last_ctrl = Some(p1);
                last_cmd = cmd;
            }
            b'T' | b't' => {
                let (x, y) = (sc.next_number()?, sc.next_number()?);
                let p1 = if matches!(last_cmd, b'Q' | b'q' | b'T' | b't') {
                    if let Some(lc) = last_ctrl {
                        Pt::new(2.0 * cur.x - lc.x, 2.0 * cur.y - lc.y)
                    } else {
                        cur
                    }
                } else {
                    cur
                };
                let p2 = if cmd == b't' { Pt::new(cur.x + x, cur.y + y) } else { Pt::new(x, y) };
                flatten_quad(cur, p1, p2, tol, &mut cur_pts, 0);
                cur = p2;
                last_ctrl = Some(p1);
                last_cmd = cmd;
            }
            b'A' | b'a' => {
                let rx = sc.next_number()?;
                let ry = sc.next_number()?;
                let x_rot = sc.next_number()?;
                let large_arc = sc.next_flag()?;
                let sweep = sc.next_flag()?;
                let x = sc.next_number()?;
                let y = sc.next_number()?;
                let end = if cmd == b'a' { Pt::new(cur.x + x, cur.y + y) } else { Pt::new(x, y) };
                flatten_arc(cur, rx, ry, x_rot, large_arc, sweep, end, tol, &mut cur_pts);
                cur = end;
                last_ctrl = None;
                last_cmd = cmd;
            }
            b'Z' | b'z' => {
                if !cur_pts.is_empty() {
                    if cur_pts.len() >= 2 {
                        // Drop duplicate closing point if identical to start
                        if cur_pts.last().unwrap().sub(sub_start).len() < 1e-6 && cur_pts.len() > 2 {
                            cur_pts.pop();
                        }
                        subpaths.push(Subpath {
                            pts: std::mem::take(&mut cur_pts),
                            closed: true,
                        });
                    } else {
                        cur_pts.clear();
                    }
                }
                cur = sub_start;
                last_ctrl = None;
                last_cmd = cmd;
            }
            other => {
                return Err(format!("unknown SVG path command '{}'", other as char));
            }
        }
    }
    flush_open(&mut cur_pts, &mut subpaths);
    Ok(PathData { subpaths })
}

fn push_unique(pts: &mut Vec<Pt>, p: Pt) {
    if let Some(&last) = pts.last() {
        if (p.x - last.x).abs() < 1e-7 && (p.y - last.y).abs() < 1e-7 {
            return;
        }
    }
    pts.push(p);
}

fn flatten_quad(p0: Pt, p1: Pt, p2: Pt, tol: f64, out: &mut Vec<Pt>, depth: u32) {
    // Midpoint deviation of quadratic Bezier from chord p0-p2 is 0.5 * dist(p1, mid(p0,p2))
    let mx = 0.5 * (p0.x + p2.x);
    let my = 0.5 * (p0.y + p2.y);
    let err = ((p1.x - mx).powi(2) + (p1.y - my).powi(2)).sqrt() * 0.5;
    if err <= tol || depth >= 10 {
        push_unique(out, p2);
        return;
    }
    let q0 = Pt::new(0.5 * (p0.x + p1.x), 0.5 * (p0.y + p1.y));
    let q1 = Pt::new(0.5 * (p1.x + p2.x), 0.5 * (p1.y + p2.y));
    let r = Pt::new(0.5 * (q0.x + q1.x), 0.5 * (q0.y + q1.y));
    flatten_quad(p0, q0, r, tol, out, depth + 1);
    flatten_quad(r, q1, p2, tol, out, depth + 1);
}

fn flatten_cubic(p0: Pt, p1: Pt, p2: Pt, p3: Pt, tol: f64, out: &mut Vec<Pt>, depth: u32) {
    let dx = p3.x - p0.x;
    let dy = p3.y - p0.y;
    let d1 = ((p1.x - p3.x) * dy - (p1.y - p3.y) * dx).abs();
    let d2 = ((p2.x - p3.x) * dy - (p2.y - p3.y) * dx).abs();
    let chord_sq = dx * dx + dy * dy;
    let flat = if chord_sq > 1e-12 {
        (d1 + d2) * (d1 + d2) <= tol * tol * chord_sq * 16.0
    } else {
        p1.sub(p0).len() + p2.sub(p0).len() <= tol * 2.0
    };
    if flat || depth >= 10 {
        push_unique(out, p3);
        return;
    }
    let p01 = Pt::new(0.5 * (p0.x + p1.x), 0.5 * (p0.y + p1.y));
    let p12 = Pt::new(0.5 * (p1.x + p2.x), 0.5 * (p1.y + p2.y));
    let p23 = Pt::new(0.5 * (p2.x + p3.x), 0.5 * (p2.y + p3.y));
    let p012 = Pt::new(0.5 * (p01.x + p12.x), 0.5 * (p01.y + p12.y));
    let p123 = Pt::new(0.5 * (p12.x + p23.x), 0.5 * (p12.y + p23.y));
    let mid = Pt::new(0.5 * (p012.x + p123.x), 0.5 * (p012.y + p123.y));
    flatten_cubic(p0, p01, p012, mid, tol, out, depth + 1);
    flatten_cubic(mid, p123, p23, p3, tol, out, depth + 1);
}

/// SVG 1.1 F.6.5 endpoint-to-center arc conversion + adaptive sampling.
fn flatten_arc(
    p0: Pt,
    rx_in: f64,
    ry_in: f64,
    x_rot_deg: f64,
    large_arc: bool,
    sweep: bool,
    p1: Pt,
    tol: f64,
    out: &mut Vec<Pt>,
) {
    if p1.sub(p0).len() < 1e-9 {
        return;
    }
    let mut rx = rx_in.abs();
    let mut ry = ry_in.abs();
    if rx < 1e-9 || ry < 1e-9 {
        push_unique(out, p1);
        return;
    }
    let phi = x_rot_deg.to_radians();
    let (sin_p, cos_p) = phi.sin_cos();

    let dx2 = 0.5 * (p0.x - p1.x);
    let dy2 = 0.5 * (p0.y - p1.y);
    let x1p = cos_p * dx2 + sin_p * dy2;
    let y1p = -sin_p * dx2 + cos_p * dy2;

    let lam = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lam > 1.0 {
        let s = lam.sqrt();
        rx *= s;
        ry *= s;
    }

    let rx2 = rx * rx;
    let ry2 = ry * ry;
    let x1p2 = x1p * x1p;
    let y1p2 = y1p * y1p;
    let num = (rx2 * ry2 - rx2 * y1p2 - ry2 * x1p2).max(0.0);
    let den = (rx2 * y1p2 + ry2 * x1p2).max(1e-12);
    let sign = if large_arc == sweep { -1.0 } else { 1.0 };
    let coef = sign * (num / den).sqrt();
    let cxp = coef * (rx * y1p / ry);
    let cyp = coef * -(ry * x1p / rx);

    let cx = cos_p * cxp - sin_p * cyp + 0.5 * (p0.x + p1.x);
    let cy = sin_p * cxp + cos_p * cyp + 0.5 * (p0.y + p1.y);

    let vec_angle = |ux: f64, uy: f64, vx: f64, vy: f64| -> f64 {
        let dot = ux * vx + uy * vy;
        let len = ((ux * ux + uy * uy) * (vx * vx + vy * vy)).sqrt().max(1e-12);
        let a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 { -a } else { a }
    };

    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = vec_angle(1.0, 0.0, ux, uy);
    let mut dtheta = vec_angle(ux, uy, vx, vy);
    if !sweep && dtheta > 0.0 {
        dtheta -= TAU;
    } else if sweep && dtheta < 0.0 {
        dtheta += TAU;
    }

    let r_max = rx.max(ry);
    let step_angle = (2.0 * (1.0 - (tol / r_max).min(0.5)).acos()).clamp(0.03, 0.25);
    let steps = ((dtheta.abs() / step_angle).ceil() as usize).clamp(4, 256);
    for k in 1..=steps {
        let t = theta1 + dtheta * (k as f64 / steps as f64);
        let (st, ct) = t.sin_cos();
        let ex = rx * ct;
        let ey = ry * st;
        let px = cos_p * ex - sin_p * ey + cx;
        let py = sin_p * ex + cos_p * ey + cy;
        push_unique(out, Pt::new(px, py));
    }
}

/// Build a `<rect>` (with optional rounded corners `rx`, `ry`) as a PathData.
pub fn rect_path(x: f64, y: f64, w: f64, h: f64, rx_in: f64, ry_in: f64, tol: f64) -> PathData {
    if w <= 0.0 || h <= 0.0 {
        return PathData::default();
    }
    let rx = rx_in.max(0.0).min(w * 0.5);
    let ry = ry_in.max(0.0).min(h * 0.5);
    if rx < 1e-6 || ry < 1e-6 {
        return PathData {
            subpaths: vec![Subpath {
                pts: vec![
                    Pt::new(x, y),
                    Pt::new(x + w, y),
                    Pt::new(x + w, y + h),
                    Pt::new(x, y + h),
                ],
                closed: true,
            }],
        };
    }
    let mut pts = Vec::new();
    let corners = [
        (x + w - rx, y + ry, -FRAC_PI_2, 0.0),
        (x + w - rx, y + h - ry, 0.0, FRAC_PI_2),
        (x + rx, y + h - ry, FRAC_PI_2, PI),
        (x + rx, y + ry, PI, 3.0 * FRAC_PI_2),
    ];
    let r_max = rx.max(ry);
    let segs = ((FRAC_PI_2 / (2.0 * (1.0 - (tol / r_max).min(0.5)).acos()).clamp(0.04, 0.3)).ceil()
        as usize)
        .clamp(4, 32);
    for (cx, cy, a0, a1) in corners {
        for k in 0..=segs {
            let a = a0 + (a1 - a0) * (k as f64 / segs as f64);
            push_unique(&mut pts, Pt::new(cx + rx * a.cos(), cy + ry * a.sin()));
        }
    }
    PathData {
        subpaths: vec![Subpath { pts, closed: true }],
    }
}

pub fn ellipse_path(cx: f64, cy: f64, rx: f64, ry: f64, tol: f64) -> PathData {
    if rx <= 0.0 || ry <= 0.0 {
        return PathData::default();
    }
    let r_max = rx.max(ry);
    let step = (2.0 * (1.0 - (tol / r_max).min(0.5)).acos()).clamp(0.03, 0.2);
    let segs = ((TAU / step).ceil() as usize).clamp(16, 256);
    let mut pts = Vec::with_capacity(segs);
    for k in 0..segs {
        let a = TAU * (k as f64 / segs as f64);
        pts.push(Pt::new(cx + rx * a.cos(), cy + ry * a.sin()));
    }
    PathData {
        subpaths: vec![Subpath { pts, closed: true }],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineCap {
    Butt,
    Round,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineJoin {
    Miter,
    Round,
    Bevel,
}

#[derive(Debug, Clone)]
pub struct StrokeStyle {
    pub width: f64,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f64,
    pub dasharray: Vec<f64>,
    pub dashoffset: f64,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 4.0,
            dasharray: Vec::new(),
            dashoffset: 0.0,
        }
    }
}

/// Expand a PathData into filled polygons representing its stroke.
pub fn expand_stroke(path: &PathData, style: &StrokeStyle) -> PathData {
    let hw = style.width * 0.5;
    if hw <= 1e-6 {
        return PathData::default();
    }
    let dashed = if !style.dasharray.is_empty() && style.dasharray.iter().any(|&d| d > 0.0) {
        apply_dash(path, &style.dasharray, style.dashoffset)
    } else {
        path.clone()
    };

    let mut out_subpaths = Vec::new();
    for sp in &dashed.subpaths {
        if sp.pts.len() < 2 {
            continue;
        }
        // Clean consecutive duplicate vertices
        let mut pts: Vec<Pt> = Vec::with_capacity(sp.pts.len());
        for &p in &sp.pts {
            push_unique(&mut pts, p);
        }
        if sp.closed && pts.len() >= 2 && pts.last().unwrap().sub(pts[0]).len() < 1e-6 {
            pts.pop();
        }
        if pts.len() < 2 {
            continue;
        }

        let n = pts.len();
        let seg_count = if sp.closed { n } else { n - 1 };

        // Emit each segment quad + joins + caps (union via nonzero fill rule)
        for s in 0..seg_count {
            let a = pts[s];
            let b = pts[(s + 1) % n];
            let d = b.sub(a);
            let l = d.len();
            if l < 1e-9 {
                continue;
            }
            let dir = Pt::new(d.x / l, d.y / l);
            let nrm = dir.perp().scale(hw);

            let mut p0 = a;
            let mut p1 = b;
            if !sp.closed && style.cap == LineCap::Square {
                if s == 0 {
                    p0 = p0.sub(dir.scale(hw));
                }
                if s + 1 == seg_count {
                    p1 = p1.add(dir.scale(hw));
                }
            }

            out_subpaths.push(Subpath {
                pts: vec![p0.add(nrm), p1.add(nrm), p1.sub(nrm), p0.sub(nrm)],
                closed: true,
            });
        }

        // Round caps for open subpaths
        if !sp.closed && style.cap == LineCap::Round {
            out_subpaths.push(disc_subpath(pts[0], hw));
            out_subpaths.push(disc_subpath(*pts.last().unwrap(), hw));
        }

        // Line joins
        let join_start = if sp.closed { 0 } else { 1 };
        let join_end = if sp.closed { n } else { n - 1 };
        for j in join_start..join_end {
            let prev = pts[(j + n - 1) % n];
            let curr = pts[j];
            let next = pts[(j + 1) % n];
            let d0 = curr.sub(prev).norm();
            let d1 = next.sub(curr).norm();
            if d0.len() < 0.5 || d1.len() < 0.5 {
                continue;
            }
            let cross = d0.cross(d1);
            if cross.abs() < 1e-5 {
                continue;
            }
            match style.join {
                LineJoin::Round => {
                    out_subpaths.push(disc_subpath(curr, hw));
                }
                LineJoin::Bevel => {
                    let sign = if cross > 0.0 { -1.0 } else { 1.0 };
                    let n0 = d0.perp().scale(hw * sign);
                    let n1 = d1.perp().scale(hw * sign);
                    out_subpaths.push(Subpath {
                        pts: vec![curr, curr.add(n0), curr.add(n1)],
                        closed: true,
                    });
                }
                LineJoin::Miter => {
                    let sign = if cross > 0.0 { -1.0 } else { 1.0 };
                    let n0 = d0.perp().scale(hw * sign);
                    let n1 = d1.perp().scale(hw * sign);
                    let a0 = curr.add(n0);
                    let a1 = curr.add(n1);
                    // Intersect a0 + t*d0 with a1 + u*d1
                    let denom = d0.cross(d1);
                    let mut use_miter = false;
                    let mut miter_pt = curr;
                    if denom.abs() > 1e-9 {
                        let t = a1.sub(a0).cross(d1) / denom;
                        let cand = a0.add(d0.scale(t));
                        let ratio = cand.sub(curr).len() / hw;
                        if ratio <= style.miter_limit {
                            miter_pt = cand;
                            use_miter = true;
                        }
                    }
                    if use_miter {
                        out_subpaths.push(Subpath {
                            pts: vec![curr, a0, miter_pt, a1],
                            closed: true,
                        });
                    } else {
                        out_subpaths.push(Subpath {
                            pts: vec![curr, a0, a1],
                            closed: true,
                        });
                    }
                }
            }
        }
    }

    for sp in &mut out_subpaths {
        ensure_positive_winding(&mut sp.pts);
    }

    PathData { subpaths: out_subpaths }
}

fn ensure_positive_winding(pts: &mut [Pt]) {
    if pts.len() < 3 {
        return;
    }
    let mut area2 = 0.0;
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        area2 += a.x * b.y - b.x * a.y;
    }
    if area2 < 0.0 {
        pts.reverse();
    }
}

fn disc_subpath(c: Pt, r: f64) -> Subpath {
    let segs = ((r * 3.0).ceil() as usize).clamp(8, 32);
    let mut pts = Vec::with_capacity(segs);
    for k in 0..segs {
        let a = TAU * (k as f64 / segs as f64);
        pts.push(Pt::new(c.x + r * a.cos(), c.y + r * a.sin()));
    }
    Subpath { pts, closed: true }
}

fn apply_dash(path: &PathData, raw_dash: &[f64], dashoffset: f64) -> PathData {
    let mut pattern: Vec<f64> = raw_dash.iter().copied().map(|d| d.max(0.0)).collect();
    if pattern.len() % 2 == 1 {
        let copy = pattern.clone();
        pattern.extend_from_slice(&copy);
    }
    let period: f64 = pattern.iter().sum();
    if period <= 1e-9 {
        return path.clone();
    }

    let mut out = Vec::new();
    for sp in &path.subpaths {
        if sp.pts.len() < 2 {
            continue;
        }
        let mut poly = sp.pts.clone();
        if sp.closed {
            poly.push(sp.pts[0]);
        }

        let mut offset = dashoffset.rem_euclid(period);
        let mut pat_idx = 0usize;
        while offset >= pattern[pat_idx] {
            offset -= pattern[pat_idx];
            pat_idx = (pat_idx + 1) % pattern.len();
        }
        let mut rem = pattern[pat_idx] - offset;
        let mut draw = pat_idx % 2 == 0;
        let mut cur_dash: Vec<Pt> = if draw { vec![poly[0]] } else { Vec::new() };

        for seg in poly.windows(2) {
            let mut a = seg[0];
            let b = seg[1];
            let mut seg_len = b.sub(a).len();
            if seg_len < 1e-9 {
                continue;
            }
            let dir = b.sub(a).scale(1.0 / seg_len);
            while seg_len > 1e-9 {
                if seg_len < rem {
                    rem -= seg_len;
                    if draw {
                        push_unique(&mut cur_dash, b);
                    }
                    seg_len = 0.0;
                } else {
                    let split = a.add(dir.scale(rem));
                    if draw {
                        push_unique(&mut cur_dash, split);
                        if cur_dash.len() >= 2 {
                            out.push(Subpath {
                                pts: std::mem::take(&mut cur_dash),
                                closed: false,
                            });
                        } else {
                            cur_dash.clear();
                        }
                    }
                    a = split;
                    seg_len -= rem;
                    pat_idx = (pat_idx + 1) % pattern.len();
                    rem = pattern[pat_idx];
                    draw = pat_idx % 2 == 0;
                    if draw {
                        cur_dash.push(a);
                    }
                }
            }
        }
        if draw && cur_dash.len() >= 2 {
            out.push(Subpath { pts: cur_dash, closed: false });
        }
    }
    PathData { subpaths: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_arc_and_transform_parse() {
        let p = parse_svg_path("M10 10 L30 10 A10 10 0 0140 20 Q40 30 30 30 C20 30 10 25 10 10 Z", 0.25).unwrap();
        assert_eq!(p.subpaths.len(), 1);
        assert!(p.subpaths[0].closed);
        assert!(p.subpaths[0].pts.len() > 8);

        let m = parse_transform("translate(10, 20) rotate(90) scale(2)").unwrap();
        let q = m.apply(Pt::new(1.0, 0.0));
        assert!((q.x - 10.0).abs() < 1e-6);
        assert!((q.y - 22.0).abs() < 1e-6);
    }
}
