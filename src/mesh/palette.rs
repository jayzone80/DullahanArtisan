use std::collections::HashMap;
use std::path::Path;

use crate::common::font::Canvas;
use crate::common::math::{parse_hex_rgba, Vec2};
use crate::common::png::{encode_rgb, encode_rgba};

#[derive(Debug, Clone)]
pub struct Swatch {
    pub name: String,
    pub col: u32,
    pub row: u32,
    pub basecolor: [u8; 4], // RGBA sRGB
    pub roughness: f32,     // 0..1
    pub metallic: f32,      // 0..1
    pub ao: f32,            // 0..1
    pub emissive: [u8; 3],  // RGB sRGB
    pub uv: Vec2,           // exact pixel center UV
}

impl Swatch {
    #[inline]
    pub fn is_translucent(&self) -> bool {
        self.basecolor[3] < 255
    }
    #[inline]
    pub fn is_emissive(&self) -> bool {
        self.emissive[0] > 0 || self.emissive[1] > 0 || self.emissive[2] > 0
    }
}

#[derive(Debug, Clone)]
pub struct Palette {
    pub cols: u32,
    pub rows: u32,
    pub tile_px: u32,
    pub width: u32,
    pub height: u32,
    pub bg: [u8; 4],
    pub swatches: Vec<Swatch>,
    name_map: HashMap<String, usize>,
    pub basecolor_rgba: Vec<u8>,
    pub orm_rgb: Vec<u8>,
    pub emissive_rgb: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedMat {
    pub uv: Vec2,
    pub is_translucent: bool,
}

impl Palette {
    pub fn load_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read palette '{}': {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut cols = 8u32;
        let mut rows = 8u32;
        let mut tile_px = 16u32;
        let mut bg = [0x1Eu8, 0x1E, 0x22, 255];
        let mut saw_header = false;

        let mut swatches: Vec<Swatch> = Vec::new();
        let mut name_map: HashMap<String, usize> = HashMap::new();
        let mut occupied: Vec<bool> = Vec::new();
        let mut next_slot = 0usize;

        for (line_idx, raw_line) in text.lines().enumerate() {
            let lineno = line_idx + 1;
            let line = strip_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if tokens[0].eq_ignore_ascii_case("grid") {
                if saw_header {
                    return Err(format!("palette line {lineno}: duplicate 'grid' header"));
                }
                if tokens.len() < 2 {
                    return Err(format!("palette line {lineno}: expected 'grid CxR@T' or 'grid N@T'"));
                }
                let (c, r, t) = parse_grid_spec(tokens[1])
                    .map_err(|e| format!("palette line {lineno}: {e}"))?;
                cols = c;
                rows = r;
                tile_px = t;
                for &tok in &tokens[2..] {
                    if let Some(hex) = tok.strip_prefix("bg=") {
                        bg = parse_hex_rgba(hex)
                            .map_err(|e| format!("palette line {lineno}: {e}"))?;
                    } else if let Ok(t2) = tok.parse::<u32>() {
                        tile_px = t2.max(1);
                    } else {
                        return Err(format!("palette line {lineno}: unknown grid option '{tok}'"));
                    }
                }
                occupied = vec![false; (cols * rows) as usize];
                saw_header = true;
                continue;
            }

            if !saw_header {
                // Allow shorthand header like "8x8@16" or "8x16" on first non-comment line
                if let Ok((c, r, t)) = parse_grid_spec(tokens[0]) {
                    cols = c;
                    rows = r;
                    tile_px = t;
                    occupied = vec![false; (cols * rows) as usize];
                    saw_header = true;
                    continue;
                }
                occupied = vec![false; (cols * rows) as usize];
                saw_header = true;
            }

            if tokens.len() < 2 {
                return Err(format!(
                    "palette line {lineno}: expected '<name> <#hex> [rough] [metal] [emissive] [ao] [at=c,r]'"
                ));
            }
            let name = tokens[0].to_string();
            let key = name.to_ascii_lowercase();
            if name_map.contains_key(&key) {
                return Err(format!("palette line {lineno}: duplicate swatch name '{name}'"));
            }
            let basecolor = parse_hex_rgba(tokens[1])
                .map_err(|e| format!("palette line {lineno}: {e}"))?;

            let mut roughness = 0.88f32;
            let mut metallic = 0.0f32;
            let mut emissive = [0u8, 0, 0];
            let mut ao = 1.0f32;
            let mut explicit_cell: Option<(u32, u32)> = None;

            let mut pos_idx = 0usize;
            for &tok in &tokens[2..] {
                if let Some(at) = tok.strip_prefix("at=") {
                    let parts: Vec<&str> = at.split(',').collect();
                    if parts.len() != 2 {
                        return Err(format!("palette line {lineno}: invalid at=c,r '{tok}'"));
                    }
                    let c: u32 = parts[0]
                        .parse()
                        .map_err(|_| format!("palette line {lineno}: bad col in '{tok}'"))?;
                    let r: u32 = parts[1]
                        .parse()
                        .map_err(|_| format!("palette line {lineno}: bad row in '{tok}'"))?;
                    explicit_cell = Some((c, r));
                } else if let Some(v) = tok.strip_prefix("r=") {
                    roughness = parse_unit(v, lineno, "roughness")?;
                } else if let Some(v) = tok.strip_prefix("m=") {
                    metallic = parse_unit(v, lineno, "metallic")?;
                } else if let Some(v) = tok.strip_prefix("e=") {
                    emissive = parse_emissive(v, basecolor, lineno)?;
                } else if let Some(v) = tok.strip_prefix("ao=") {
                    ao = parse_unit(v, lineno, "ao")?;
                } else {
                    match pos_idx {
                        0 => roughness = parse_unit(tok, lineno, "roughness")?,
                        1 => metallic = parse_unit(tok, lineno, "metallic")?,
                        2 => emissive = parse_emissive(tok, basecolor, lineno)?,
                        3 => ao = parse_unit(tok, lineno, "ao")?,
                        _ => {
                            return Err(format!(
                                "palette line {lineno}: unexpected extra token '{tok}'"
                            ))
                        }
                    }
                    pos_idx += 1;
                }
            }

            let capacity = (cols * rows) as usize;
            let (col, row) = if let Some((c, r)) = explicit_cell {
                if c >= cols || r >= rows {
                    return Err(format!(
                        "palette line {lineno}: cell ({c},{r}) out of bounds for {cols}x{rows} grid"
                    ));
                }
                let idx = (r * cols + c) as usize;
                if occupied[idx] {
                    return Err(format!("palette line {lineno}: cell ({c},{r}) already occupied"));
                }
                occupied[idx] = true;
                (c, r)
            } else {
                while next_slot < capacity && occupied[next_slot] {
                    next_slot += 1;
                }
                if next_slot >= capacity {
                    return Err(format!(
                        "palette line {lineno}: grid capacity {capacity} ({cols}x{rows}) exceeded"
                    ));
                }
                occupied[next_slot] = true;
                let c = (next_slot as u32) % cols;
                let r = (next_slot as u32) / cols;
                next_slot += 1;
                (c, r)
            };

            let width = cols * tile_px;
            let height = rows * tile_px;
            let uv = compute_cell_uv_center(col, row, cols, rows, width, height);
            let idx = swatches.len();
            swatches.push(Swatch {
                name,
                col,
                row,
                basecolor,
                roughness,
                metallic,
                ao,
                emissive,
                uv,
            });
            name_map.insert(key, idx);
        }

        if swatches.is_empty() {
            return Err("palette contains no color swatches".into());
        }

        let width = cols * tile_px;
        let height = rows * tile_px;
        let mut pal = Self {
            cols,
            rows,
            tile_px,
            width,
            height,
            bg,
            swatches,
            name_map,
            basecolor_rgba: Vec::new(),
            orm_rgb: Vec::new(),
            emissive_rgb: Vec::new(),
        };
        pal.rebuild_atlases();
        Ok(pal)
    }

    pub fn rebuild_atlases(&mut self) {
        let w = self.width as usize;
        let h = self.height as usize;
        self.basecolor_rgba = vec![0u8; w * h * 4];
        self.orm_rgb = vec![0u8; w * h * 3];
        self.emissive_rgb = vec![0u8; w * h * 3];

        // Fill background defaults
        for i in 0..(w * h) {
            self.basecolor_rgba[i * 4..i * 4 + 4].copy_from_slice(&self.bg);
            // Default ORM: AO=255, Roughness=224 (0.88), Metallic=0
            self.orm_rgb[i * 3] = 255;
            self.orm_rgb[i * 3 + 1] = 224;
            self.orm_rgb[i * 3 + 2] = 0;
        }

        for sw in &self.swatches {
            let x0 = (sw.col * self.width / self.cols) as usize;
            let x1 = ((sw.col + 1) * self.width / self.cols) as usize;
            let y0 = (sw.row * self.height / self.rows) as usize;
            let y1 = ((sw.row + 1) * self.height / self.rows) as usize;
            let ao_b = (sw.ao.clamp(0.0, 1.0) * 255.0).round() as u8;
            let r_b = (sw.roughness.clamp(0.0, 1.0) * 255.0).round() as u8;
            let m_b = (sw.metallic.clamp(0.0, 1.0) * 255.0).round() as u8;

            for y in y0..y1 {
                for x in x0..x1 {
                    let p = y * w + x;
                    self.basecolor_rgba[p * 4..p * 4 + 4].copy_from_slice(&sw.basecolor);
                    self.orm_rgb[p * 3] = ao_b;
                    self.orm_rgb[p * 3 + 1] = r_b;
                    self.orm_rgb[p * 3 + 2] = m_b;
                    self.emissive_rgb[p * 3..p * 3 + 3].copy_from_slice(&sw.emissive);
                }
            }
        }
    }

    pub fn encode_basecolor_png(&self) -> Vec<u8> {
        encode_rgba(self.width, self.height, &self.basecolor_rgba)
    }

    pub fn encode_orm_png(&self) -> Vec<u8> {
        encode_rgb(self.width, self.height, &self.orm_rgb)
    }

    pub fn encode_emissive_png(&self) -> Vec<u8> {
        encode_rgb(self.width, self.height, &self.emissive_rgb)
    }

    /// Resolve a material specifier: semantic name, `cell:c,r`, or `uv:u,v`.
    pub fn resolve_mat(&self, spec: &str) -> Result<ResolvedMat, String> {
        if let Some(rest) = spec.strip_prefix("uv:") {
            let parts: Vec<&str> = rest.split(',').collect();
            if parts.len() != 2 {
                return Err(format!("invalid uv specifier '{spec}'"));
            }
            let u: f32 = parts[0].parse().map_err(|_| format!("bad u in '{spec}'"))?;
            let v: f32 = parts[1].parse().map_err(|_| format!("bad v in '{spec}'"))?;
            let uv = Vec2::new(u.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
            let bc = self.sample_basecolor_nearest(uv);
            return Ok(ResolvedMat {
                uv,
                is_translucent: bc[3] < 255,
            });
        }
        if let Some(rest) = spec.strip_prefix("cell:") {
            let parts: Vec<&str> = rest.split(',').collect();
            if parts.len() != 2 {
                return Err(format!("invalid cell specifier '{spec}'"));
            }
            let c: u32 = parts[0].parse().map_err(|_| format!("bad col in '{spec}'"))?;
            let r: u32 = parts[1].parse().map_err(|_| format!("bad row in '{spec}'"))?;
            if c >= self.cols || r >= self.rows {
                return Err(format!("cell ({c},{r}) out of bounds"));
            }
            let uv = compute_cell_uv_center(c, r, self.cols, self.rows, self.width, self.height);
            let bc = self.sample_basecolor_nearest(uv);
            return Ok(ResolvedMat {
                uv,
                is_translucent: bc[3] < 255,
            });
        }
        let key = spec.to_ascii_lowercase();
        if let Some(&idx) = self.name_map.get(&key) {
            let sw = &self.swatches[idx];
            return Ok(ResolvedMat {
                uv: sw.uv,
                is_translucent: sw.is_translucent(),
            });
        }
        if let Ok(idx) = spec.parse::<usize>() {
            if idx < self.swatches.len() {
                let sw = &self.swatches[idx];
                return Ok(ResolvedMat {
                    uv: sw.uv,
                    is_translucent: sw.is_translucent(),
                });
            }
        }
        let avail: Vec<&str> = self.swatches.iter().map(|s| s.name.as_str()).collect();
        Err(format!(
            "unknown palette material '{spec}' (available: {})",
            avail.join(", ")
        ))
    }

    #[inline]
    fn uv_to_pixel(&self, uv: Vec2) -> usize {
        let x = ((uv.x.clamp(0.0, 0.999_999) * self.width as f32) as usize)
            .min(self.width as usize - 1);
        let y = ((uv.y.clamp(0.0, 0.999_999) * self.height as f32) as usize)
            .min(self.height as usize - 1);
        y * self.width as usize + x
    }

    #[inline]
    pub fn sample_basecolor_nearest(&self, uv: Vec2) -> [u8; 4] {
        let p = self.uv_to_pixel(uv) * 4;
        [
            self.basecolor_rgba[p],
            self.basecolor_rgba[p + 1],
            self.basecolor_rgba[p + 2],
            self.basecolor_rgba[p + 3],
        ]
    }

    #[inline]
    pub fn sample_orm_nearest(&self, uv: Vec2) -> [f32; 3] {
        let p = self.uv_to_pixel(uv) * 3;
        [
            self.orm_rgb[p] as f32 / 255.0,
            self.orm_rgb[p + 1] as f32 / 255.0,
            self.orm_rgb[p + 2] as f32 / 255.0,
        ]
    }

    #[inline]
    pub fn sample_emissive_nearest(&self, uv: Vec2) -> [f32; 3] {
        let p = self.uv_to_pixel(uv) * 3;
        [
            self.emissive_rgb[p] as f32 / 255.0,
            self.emissive_rgb[p + 1] as f32 / 255.0,
            self.emissive_rgb[p + 2] as f32 / 255.0,
        ]
    }

    /// Render a labeled visual contact sheet PNG showing every tile's color, name, and R/M/E tags.
    pub fn render_swatch_sheet_png(&self) -> Vec<u8> {
        let cell_w = 96i32;
        let cell_h = 54i32;
        let pad = 6i32;
        let header_h = 26i32;
        let w = (self.cols as i32 * (cell_w + pad) + pad) as usize;
        let h = (header_h + self.rows as i32 * (cell_h + pad) + pad) as usize;
        let mut c = Canvas::new(w, h, [24, 24, 28, 255]);

        let title = format!(
            "PALETTE {}x{}@{} ({}x{}px) USED:{}/{}",
            self.cols,
            self.rows,
            self.tile_px,
            self.width,
            self.height,
            self.swatches.len(),
            self.cols * self.rows
        );
        c.draw_text(8, 8, &title, [230, 232, 240, 255], 1);

        // Draw empty slot outlines
        for r in 0..self.rows as i32 {
            for col in 0..self.cols as i32 {
                let x = pad + col * (cell_w + pad);
                let y = header_h + pad + r * (cell_h + pad);
                c.fill_rect(x, y, cell_w, cell_h, [34, 35, 40, 255]);
                c.stroke_rect(x, y, cell_w, cell_h, [54, 56, 64, 255]);
            }
        }

        for sw in &self.swatches {
            let x = pad + sw.col as i32 * (cell_w + pad);
            let y = header_h + pad + sw.row as i32 * (cell_h + pad);
            // Checkerboard under translucent swatches
            if sw.is_translucent() {
                for cy in (0..30).step_by(6) {
                    for cx in (0..cell_w).step_by(6) {
                        let chk = if ((cx / 6) + (cy / 6)) % 2 == 0 { 160 } else { 95 };
                        c.fill_rect(x + cx, y + cy, 6.min(cell_w - cx), 6.min(30 - cy), [chk, chk, chk, 255]);
                    }
                }
            }
            c.fill_rect(x, y, cell_w, 30, sw.basecolor);
            c.fill_rect(x, y + 30, cell_w, cell_h - 30, [20, 21, 25, 255]);
            c.stroke_rect(x, y, cell_w, cell_h, [75, 78, 88, 255]);

            // Emissive indicator dot
            if sw.is_emissive() {
                c.fill_rect(x + cell_w - 11, y + 3, 8, 8, [sw.emissive[0], sw.emissive[1], sw.emissive[2], 255]);
                c.stroke_rect(x + cell_w - 11, y + 3, 8, 8, [255, 255, 255, 255]);
            }

            let short_name: String = sw.name.chars().take(14).collect();
            c.draw_text(x + 3, y + 33, &short_name, [235, 235, 240, 255], 1);
            let meta = format!(
                "R{:.1} M{:.1}{}",
                sw.roughness,
                sw.metallic,
                if sw.is_translucent() { " A" } else if sw.is_emissive() { " E" } else { "" }
            );
            c.draw_text(x + 3, y + 44, &meta, [160, 166, 180, 255], 1);
        }

        encode_rgba(w as u32, h as u32, &c.rgba)
    }
}

pub fn compute_cell_uv_center(
    col: u32,
    row: u32,
    cols: u32,
    rows: u32,
    width: u32,
    height: u32,
) -> Vec2 {
    let x0 = (col * width / cols) as f32;
    let x1 = ((col + 1) * width / cols) as f32;
    let y0 = (row * height / rows) as f32;
    let y1 = ((row + 1) * height / rows) as f32;
    Vec2::new((x0 + x1) * 0.5 / width as f32, (y0 + y1) * 0.5 / height as f32)
}

fn strip_comment(line: &str) -> &str {
    // Only treat `#` as a comment when preceded by start-of-line or whitespace,
    // so hex colors like `#56823a` are preserved!
    let bytes = line.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'#' {
            if i == 0 || bytes[i - 1].is_ascii_whitespace() {
                // Check if next char is hex color (preceded by whitespace AND followed by 3/6/8 hex + ws/end)
                let rest = &line[i + 1..];
                let tok = rest.split_whitespace().next().unwrap_or("");
                let looks_like_hex = (tok.len() == 3 || tok.len() == 6 || tok.len() == 8)
                    && tok.bytes().all(|b| b.is_ascii_hexdigit());
                if i == 0 || !looks_like_hex {
                    return &line[..i];
                }
            }
        }
    }
    line
}

fn parse_grid_spec(s: &str) -> Result<(u32, u32, u32), String> {
    // Formats: "8x8@16", "8x4@16", "8@16", or "8x16" (8x8 tiles of 16px)
    if let Some((dim, px_str)) = s.split_once('@') {
        let tile_px: u32 = px_str
            .parse()
            .map_err(|_| format!("invalid tile size in '{s}'"))?;
        let (c, r) = if let Some((cs, rs)) = dim.split_once('x').or_else(|| dim.split_once('X')) {
            (
                cs.parse::<u32>().map_err(|_| format!("bad cols in '{s}'"))?,
                rs.parse::<u32>().map_err(|_| format!("bad rows in '{s}'"))?,
            )
        } else {
            let n: u32 = dim.parse().map_err(|_| format!("bad grid dim in '{s}'"))?;
            (n, n)
        };
        if c == 0 || r == 0 || tile_px == 0 {
            return Err("grid dimensions and tile size must be > 0".into());
        }
        Ok((c, r, tile_px))
    } else if let Some((cs, ts)) = s.split_once('x').or_else(|| s.split_once('X')) {
        // User shorthand: "8x16" => 8x8 tiles, 16px wide
        let n: u32 = cs.parse().map_err(|_| format!("bad grid dim in '{s}'"))?;
        let t: u32 = ts.parse().map_err(|_| format!("bad tile px in '{s}'"))?;
        if n == 0 || t == 0 {
            return Err("grid dimensions and tile size must be > 0".into());
        }
        Ok((n, n, t))
    } else {
        Err(format!("invalid grid spec '{s}' (expected CxR@T, N@T, or NxT)"))
    }
}

fn parse_unit(s: &str, lineno: usize, field: &str) -> Result<f32, String> {
    let v: f32 = s
        .parse()
        .map_err(|_| format!("palette line {lineno}: invalid {field} '{s}'"))?;
    Ok(v.clamp(0.0, 1.0))
}

fn parse_emissive(s: &str, basecolor: [u8; 4], lineno: usize) -> Result<[u8; 3], String> {
    if s == "-" || s == "0" || s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("false") {
        Ok([0, 0, 0])
    } else if s == "1" || s.eq_ignore_ascii_case("true") {
        Ok([basecolor[0], basecolor[1], basecolor[2]])
    } else {
        let rgba = parse_hex_rgba(s).map_err(|e| format!("palette line {lineno}: {e}"))?;
        Ok([rgba[0], rgba[1], rgba[2]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::png::inspect_and_verify_png;

    #[test]
    fn palette_parse_and_atlases() {
        let src = r#"
grid 4x2@16 bg=#18181c
# name      hex        rough metal emissive
mat_a       #808080    .90   0     -
mat_metal   #c0c4c8    .25   1.0   -
mat_glass   #88bbdd80  .10   0     -
mat_glow    #ffeeaa    .35   0     #ffaa33
"#;
        let pal = Palette::parse(src).unwrap();
        assert_eq!(pal.width, 64);
        assert_eq!(pal.height, 32);
        assert_eq!(pal.swatches.len(), 4);

        let g = pal.resolve_mat("mat_glass").unwrap();
        assert!(g.is_translucent);
        assert_eq!(pal.sample_basecolor_nearest(g.uv), [0x88, 0xBB, 0xDD, 0x80]);

        let m = pal.resolve_mat("mat_metal").unwrap();
        let orm = pal.sample_orm_nearest(m.uv);
        assert!((orm[0] - 1.0).abs() < 0.01);
        assert!((orm[1] - 0.25).abs() < 0.02);
        assert!((orm[2] - 1.0).abs() < 0.01);

        let png = pal.encode_basecolor_png();
        let info = inspect_and_verify_png(&png).unwrap();
        assert_eq!((info.width, info.height), (64, 32));
        assert!(png.len() < 512, "flat atlas PNG should compress tiny, got {}", png.len());
    }
}
