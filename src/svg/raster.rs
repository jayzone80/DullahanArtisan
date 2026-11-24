/// High-quality zero-dependency SVG software rasterizer:
/// - 4-subscanline analytical horizontal coverage anti-aliasing (nonzero & evenodd fill rules)
/// - Linear & radial gradients (objectBoundingBox & userSpaceOnUse, pad/reflect/repeat)
/// - Clip paths, group opacity isolation, separable Gaussian blur & drop-shadow filters
/// - GPU-friendly transparent edge alpha-bleed + checkerboard/9-slice/grid QA preview mode

use crate::common::font::Canvas;
use crate::common::png::encode_rgba;
use crate::svg::parser::{
    CoordUnits, DrawGroup, DrawShape, FillRule, FilterKind, Gradient, GradientKind, Paint,
    RenderItem, SpreadMethod, SvgDocument,
};
use crate::svg::path::{expand_stroke, Affine, BBox, PathData, Pt};

#[derive(Debug, Clone)]
pub struct SvgRenderOptions {
    pub width: u32,
    pub height: u32,
    pub bg: Option<[u8; 4]>, // None = transparent RGBA
    pub checker: bool,       // render transparency checkerboard (used by --preview / --bg checker)
    pub bleed: bool,         // RGB alpha-bleed into 0-alpha pixels for clean GPU bilinear filtering
    pub hud: bool,
    pub grid: bool,
    pub nine_slice: Option<[f64; 4]>, // [top, right, bottom, left] in SVG viewBox units
}

impl SvgRenderOptions {
    pub fn for_doc(doc: &SvgDocument) -> Self {
        Self {
            width: doc.width.round().max(1.0) as u32,
            height: doc.height.round().max(1.0) as u32,
            bg: None,
            checker: false,
            bleed: true,
            hud: false,
            grid: false,
            nine_slice: None,
        }
    }
}

/// Premultiplied linear-ish float RGBA buffer `[r, g, b, a]` in `0..=1`.
struct Surface {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Surface {
    fn new(w: usize, h: usize) -> Self {
        Self { w, h, px: vec![[0.0; 4]; w * h] }
    }

    fn fill_bg(&mut self, c: [u8; 4]) {
        let a = c[3] as f32 / 255.0;
        let p = [
            (c[0] as f32 / 255.0) * a,
            (c[1] as f32 / 255.0) * a,
            (c[2] as f32 / 255.0) * a,
            a,
        ];
        self.px.fill(p);
    }

    fn fill_checker(&mut self, cell: usize) {
        let cell = cell.max(4);
        let c0 = [0.16f32, 0.17, 0.20, 1.0];
        let c1 = [0.21f32, 0.22, 0.26, 1.0];
        for y in 0..self.h {
            for x in 0..self.w {
                let even = ((x / cell) + (y / cell)) % 2 == 0;
                self.px[y * self.w + x] = if even { c0 } else { c1 };
            }
        }
    }

    #[inline]
    fn blend_over(&mut self, idx: usize, sr: f32, sg: f32, sb: f32, sa: f32) {
        if sa <= 1e-6 {
            return;
        }
        let d = self.px[idx];
        let inv = 1.0 - sa;
        self.px[idx] = [
            sr + d[0] * inv,
            sg + d[1] * inv,
            sb + d[2] * inv,
            sa + d[3] * inv,
        ];
    }

    fn composite_surface(&mut self, src: &Surface, opacity: f32, mask: Option<&[f32]>) {
        if opacity <= 1e-6 {
            return;
        }
        for i in 0..self.px.len() {
            let m = mask.map_or(1.0, |mk| mk[i]) * opacity;
            if m <= 1e-6 {
                continue;
            }
            let s = src.px[i];
            if s[3] <= 1e-6 {
                continue;
            }
            self.blend_over(i, s[0] * m, s[1] * m, s[2] * m, s[3] * m);
        }
    }

    fn to_rgba_u8(&self, bleed: bool) -> Vec<u8> {
        let mut out = vec![0u8; self.w * self.h * 4];
        for (i, &p) in self.px.iter().enumerate() {
            let a = p[3].clamp(0.0, 1.0);
            if a > 1e-5 {
                let inv_a = 1.0 / a;
                out[i * 4] = (p[0] * inv_a * 255.0).round().clamp(0.0, 255.0) as u8;
                out[i * 4 + 1] = (p[1] * inv_a * 255.0).round().clamp(0.0, 255.0) as u8;
                out[i * 4 + 2] = (p[2] * inv_a * 255.0).round().clamp(0.0, 255.0) as u8;
                out[i * 4 + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        if bleed {
            alpha_bleed(&mut out, self.w, self.h);
        }
        out
    }
}

/// Propagate RGB from opaque/semi-opaque neighbors into 0-alpha transparent border pixels
/// so GPU bilinear filtering never introduces dark fringe artifacts around UI sprites.
fn alpha_bleed(rgba: &mut [u8], w: usize, h: usize) {
    let orig = rgba.to_vec();
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            if orig[i + 3] > 0 {
                continue;
            }
            let mut r_sum = 0u32;
            let mut g_sum = 0u32;
            let mut b_sum = 0u32;
            let mut w_sum = 0u32;
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(h - 1);
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(w - 1);
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    let ni = (ny * w + nx) * 4;
                    let na = orig[ni + 3] as u32;
                    if na > 0 {
                        r_sum += orig[ni] as u32 * na;
                        g_sum += orig[ni + 1] as u32 * na;
                        b_sum += orig[ni + 2] as u32 * na;
                        w_sum += na;
                    }
                }
            }
            if w_sum > 0 {
                rgba[i] = (r_sum / w_sum) as u8;
                rgba[i + 1] = (g_sum / w_sum) as u8;
                rgba[i + 2] = (b_sum / w_sum) as u8;
            }
        }
    }
}

/// Render an SVG document to PNG bytes (transparent RGBA or solid background).
pub fn render_svg_png(doc: &SvgDocument, opts: &SvgRenderOptions) -> Vec<u8> {
    let w = opts.width.max(1) as usize;
    let h = opts.height.max(1) as usize;

    let (vb_x, vb_y, vb_w, vb_h) = doc.view_box;
    // Uniform fit centered inside (w, h) preserving aspect ratio
    let scale = (w as f64 / vb_w).min(h as f64 / vb_h);
    let draw_w = vb_w * scale;
    let draw_h = vb_h * scale;
    let off_x = (w as f64 - draw_w) * 0.5;
    let off_y = (h as f64 - draw_h) * 0.5;
    let root_tf = Affine::translate(off_x, off_y)
        .mul(Affine::scale(scale, scale))
        .mul(Affine::translate(-vb_x, -vb_y));

    let mut surf = Surface::new(w, h);
    if opts.checker {
        surf.fill_checker(10);
    } else if let Some(bg) = opts.bg {
        surf.fill_bg(bg);
    }

    render_items(doc, &doc.items, root_tf, scale, &mut surf);

    let do_bleed = opts.bleed && opts.bg.is_none() && !opts.checker;
    let rgba = surf.to_rgba_u8(do_bleed);

    if !opts.hud && !opts.grid && opts.nine_slice.is_none() {
        return encode_rgba(w as u32, h as u32, &rgba);
    }

    // Visual QA overlay pass (grid, viewBox border, 9-slice guides, HUD bar)
    let hud_h = if opts.hud { 24usize } else { 0 };
    let pad = if opts.hud { 12usize } else { 0 };
    let total_w = w + pad * 2;
    let total_h = h + pad * 2 + hud_h;
    let mut canvas = Canvas::new(total_w, total_h, [24, 26, 32, 255]);

    // Blit rendered image into padded frame
    for y in 0..h {
        let src_row = &rgba[y * w * 4..(y + 1) * w * 4];
        let dst_off = ((y + pad) * total_w + pad) * 4;
        canvas.rgba[dst_off..dst_off + w * 4].copy_from_slice(src_row);
    }

    let vx0 = pad as i32 + off_x.round() as i32;
    let vy0 = pad as i32 + off_y.round() as i32;
    let vw_i = draw_w.round() as i32;
    let vh_i = draw_h.round() as i32;

    if opts.grid {
        let step_vb = choose_grid_step(vb_w.max(vb_h));
        let mut gx = (vb_x / step_vb).ceil() * step_vb;
        while gx < vb_x + vb_w {
            let px = pad as i32 + ((gx - vb_x) * scale + off_x).round() as i32;
            canvas.fill_rect(px, vy0, 1, vh_i, [140, 180, 255, 42]);
            gx += step_vb;
        }
        let mut gy = (vb_y / step_vb).ceil() * step_vb;
        while gy < vb_y + vb_h {
            let py = pad as i32 + ((gy - vb_y) * scale + off_y).round() as i32;
            canvas.fill_rect(vx0, py, vw_i, 1, [140, 180, 255, 42]);
            gy += step_vb;
        }
    }

    // Crisp viewBox frame border
    canvas.stroke_rect(vx0 - 1, vy0 - 1, vw_i + 2, vh_i + 2, [96, 210, 255, 160]);

    // Optional 9-slice guides [top, right, bottom, left]
    if let Some([top, right, bottom, left]) = opts.nine_slice {
        let guide = [255, 184, 76, 210];
        let yt = vy0 + (top * scale).round() as i32;
        let yb = vy0 + vh_i - (bottom * scale).round() as i32;
        let xl = vx0 + (left * scale).round() as i32;
        let xr = vx0 + vw_i - (right * scale).round() as i32;
        draw_dashed_h(&mut canvas, vx0, vx0 + vw_i, yt, guide);
        draw_dashed_h(&mut canvas, vx0, vx0 + vw_i, yb, guide);
        draw_dashed_v(&mut canvas, xl, vy0, vy0 + vh_i, guide);
        draw_dashed_v(&mut canvas, xr, vy0, vy0 + vh_i, guide);
    }

    if opts.hud {
        let bar_y = (h + pad * 2) as i32;
        canvas.fill_rect(0, bar_y, total_w as i32, hud_h as i32, [16, 18, 22, 255]);
        let status = if !doc.is_ok() {
            "ERR"
        } else if !doc.warnings.is_empty() {
            "WARN"
        } else {
            "PASS"
        };
        let label = format!(
            "SVG {}  VB:{}x{}  OUT:{}x{}  SHAPES:{}  GRADS:{}  {}",
            doc.name,
            vb_w.round() as i32,
            vb_h.round() as i32,
            w,
            h,
            doc.stats.shapes,
            doc.stats.gradients,
            status
        );
        let col = if status == "PASS" {
            [130, 235, 160, 255]
        } else if status == "WARN" {
            [255, 200, 90, 255]
        } else {
            [255, 110, 110, 255]
        };
        canvas.draw_text(8, bar_y + 8, &label, col, 1);
    }

    encode_rgba(total_w as u32, total_h as u32, &canvas.rgba)
}

fn choose_grid_step(max_dim: f64) -> f64 {
    if max_dim <= 32.0 {
        4.0
    } else if max_dim <= 96.0 {
        8.0
    } else if max_dim <= 256.0 {
        16.0
    } else if max_dim <= 640.0 {
        32.0
    } else {
        64.0
    }
}

fn draw_dashed_h(c: &mut Canvas, x0: i32, x1: i32, y: i32, col: [u8; 4]) {
    let mut x = x0;
    while x < x1 {
        c.fill_rect(x, y, (x1 - x).min(4), 1, col);
        x += 7;
    }
}

fn draw_dashed_v(c: &mut Canvas, x: i32, y0: i32, y1: i32, col: [u8; 4]) {
    let mut y = y0;
    while y < y1 {
        c.fill_rect(x, y, 1, (y1 - y).min(4), col);
        y += 7;
    }
}

fn render_items(
    doc: &SvgDocument,
    items: &[RenderItem],
    root_tf: Affine,
    root_scale: f64,
    dst: &mut Surface,
) {
    for item in items {
        match item {
            RenderItem::Shape(sh) => render_shape(doc, sh, root_tf, root_scale, dst),
            RenderItem::Group(grp) => render_group(doc, grp, root_tf, root_scale, dst),
        }
    }
}

fn render_group(
    doc: &SvgDocument,
    grp: &DrawGroup,
    root_tf: Affine,
    root_scale: f64,
    dst: &mut Surface,
) {
    let mut tmp = Surface::new(dst.w, dst.h);
    render_items(doc, &grp.items, root_tf, root_scale, &mut tmp);
    if let Some(ref fid) = grp.filter {
        if let Some(fk) = doc.filters.get(fid) {
            apply_filter(&mut tmp, fk, root_scale);
        }
    }
    let mask = grp.clip_path.as_deref().and_then(|cid| {
        build_clip_mask(doc, cid, root_tf.mul(grp.transform), None, dst.w, dst.h)
    });
    dst.composite_surface(&tmp, grp.opacity as f32, mask.as_deref());
}

fn render_shape(
    doc: &SvgDocument,
    sh: &DrawShape,
    root_tf: Affine,
    root_scale: f64,
    dst: &mut Surface,
) {
    let full_tf = root_tf.mul(sh.transform);
    let has_filter = sh.filter.is_some();
    let has_clip = sh.clip_path.is_some();

    let mut local_surf = if has_filter { Some(Surface::new(dst.w, dst.h)) } else { None };
    let target = local_surf.as_mut().unwrap_or(dst);

    let obj_bbox = sh.path.bbox();
    let clip_mask = if !has_filter && has_clip {
        sh.clip_path
            .as_deref()
            .and_then(|cid| build_clip_mask(doc, cid, full_tf, Some(obj_bbox), target.w, target.h))
    } else {
        None
    };

    // 1. Fill pass
    if !matches!(sh.fill, Paint::None) && sh.fill_opacity > 0.0 {
        let dev_path = sh.path.transform(full_tf);
        let alpha = (if has_filter { sh.fill_opacity } else { sh.fill_opacity * sh.opacity }) as f32;
        rasterize_fill(
            target,
            &dev_path,
            sh.fill_rule,
            &sh.fill,
            alpha,
            doc,
            full_tf,
            obj_bbox,
            clip_mask.as_deref(),
        );
    }

    // 2. Stroke pass
    if !matches!(sh.stroke, Paint::None) && sh.stroke_opacity > 0.0 && sh.stroke_style.width > 0.0 {
        let stroke_local = expand_stroke(&sh.path, &sh.stroke_style);
        let dev_stroke = stroke_local.transform(full_tf);
        let alpha =
            (if has_filter { sh.stroke_opacity } else { sh.stroke_opacity * sh.opacity }) as f32;
        rasterize_fill(
            target,
            &dev_stroke,
            FillRule::NonZero,
            &sh.stroke,
            alpha,
            doc,
            full_tf,
            obj_bbox,
            clip_mask.as_deref(),
        );
    }

    if let Some(mut tmp) = local_surf {
        if let Some(ref fid) = sh.filter {
            if let Some(fk) = doc.filters.get(fid) {
                apply_filter(&mut tmp, fk, root_scale * sh.transform.avg_scale());
            }
        }
        let mask = sh
            .clip_path
            .as_deref()
            .and_then(|cid| build_clip_mask(doc, cid, full_tf, Some(obj_bbox), dst.w, dst.h));
        dst.composite_surface(&tmp, sh.opacity as f32, mask.as_deref());
    }
}

fn build_clip_mask(
    doc: &SvgDocument,
    clip_id: &str,
    elem_dev_tf: Affine,
    obj_bbox: Option<BBox>,
    w: usize,
    h: usize,
) -> Option<Vec<f32>> {
    let cdef = doc.clip_paths.get(clip_id)?;
    let tf = match cdef.units {
        CoordUnits::UserSpaceOnUse => elem_dev_tf,
        CoordUnits::ObjectBoundingBox => {
            let bb = obj_bbox?;
            if !bb.is_valid() {
                return None;
            }
            elem_dev_tf
                .mul(Affine::translate(bb.min_x, bb.min_y))
                .mul(Affine::scale(bb.width(), bb.height()))
        }
    };
    let mut mask = vec![0.0f32; w * h];
    for (p, rule) in &cdef.shapes {
        let dev = p.transform(tf);
        rasterize_coverage(&dev, *rule, w, h, &mut mask);
    }
    Some(mask)
}

#[derive(Clone, Copy)]
struct Edge {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    inv_dy: f64,
    dir: i32,
}

fn collect_edges(path: &PathData) -> (Vec<Edge>, f64, f64) {
    let mut edges = Vec::new();
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for sp in &path.subpaths {
        if sp.pts.len() < 2 {
            continue;
        }
        let n = sp.pts.len();
        for i in 0..n {
            let a = sp.pts[i];
            let b = sp.pts[(i + 1) % n];
            if (a.y - b.y).abs() < 1e-9 {
                continue;
            }
            let (p0, p1, dir) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
            min_y = min_y.min(p0.y);
            max_y = max_y.max(p1.y);
            edges.push(Edge {
                x0: p0.x,
                y0: p0.y,
                x1: p1.x,
                y1: p1.y,
                inv_dy: 1.0 / (p1.y - p0.y),
                dir,
            });
        }
    }
    (edges, min_y, max_y)
}

fn rasterize_coverage(path: &PathData, rule: FillRule, w: usize, h: usize, mask: &mut [f32]) {
    let (edges, min_y, max_y) = collect_edges(path);
    if edges.is_empty() || max_y <= 0.0 || min_y >= h as f64 {
        return;
    }
    let y_start = (min_y.floor() as i32).max(0) as usize;
    let y_end = (max_y.ceil() as i32).min(h as i32) as usize;
    let mut xings: Vec<(f64, i32)> = Vec::with_capacity(64);
    let mut row_cov = vec![0.0f32; w];

    for py in y_start..y_end {
        row_cov.fill(0.0);
        for s in 0..4 {
            let sy = py as f64 + (s as f64 + 0.5) * 0.25;
            xings.clear();
            for e in &edges {
                if sy >= e.y0 && sy < e.y1 {
                    let x = e.x0 + (sy - e.y0) * (e.x1 - e.x0) * e.inv_dy;
                    xings.push((x, e.dir));
                }
            }
            if xings.len() < 2 {
                continue;
            }
            xings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0i32;
            for pair in xings.windows(2) {
                wind += pair[0].1;
                let inside = match rule {
                    FillRule::NonZero => wind != 0,
                    FillRule::EvenOdd => (wind & 1) != 0,
                };
                if inside {
                    let x0 = pair[0].0.max(0.0);
                    let x1 = pair[1].0.min(w as f64);
                    if x1 > x0 {
                        let px0 = x0.floor() as usize;
                        let px1 = (x1.ceil() as usize).min(w);
                        for px in px0..px1 {
                            let cov = (x1.min(px as f64 + 1.0) - x0.max(px as f64)).max(0.0) as f32;
                            row_cov[px] += cov * 0.25;
                        }
                    }
                }
            }
        }
        let row_off = py * w;
        for px in 0..w {
            if row_cov[px] > 0.0 {
                mask[row_off + px] = (mask[row_off + px] + row_cov[px]).min(1.0);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn rasterize_fill(
    surf: &mut Surface,
    dev_path: &PathData,
    rule: FillRule,
    paint: &Paint,
    alpha: f32,
    doc: &SvgDocument,
    full_tf: Affine,
    obj_bbox: BBox,
    clip_mask: Option<&[f32]>,
) {
    if alpha <= 1e-6 {
        return;
    }
    let w = surf.w;
    let h = surf.h;
    let (edges, min_y, max_y) = collect_edges(dev_path);
    if edges.is_empty() || max_y <= 0.0 || min_y >= h as f64 {
        return;
    }

    let inv_full = full_tf.inverse();
    let grad = match paint {
        Paint::Gradient(id) => doc.gradients.get(id),
        _ => None,
    };
    let solid = match paint {
        Paint::Color(c) => Some(*c),
        _ => None,
    };
    if solid.is_none() && grad.is_none() {
        return;
    }

    let y_start = (min_y.floor() as i32).max(0) as usize;
    let y_end = (max_y.ceil() as i32).min(h as i32) as usize;
    let mut xings: Vec<(f64, i32)> = Vec::with_capacity(64);
    let mut row_cov = vec![0.0f32; w];

    for py in y_start..y_end {
        row_cov.fill(0.0);
        let mut active_x0 = w;
        let mut active_x1 = 0usize;

        for s in 0..4 {
            let sy = py as f64 + (s as f64 + 0.5) * 0.25;
            xings.clear();
            for e in &edges {
                if sy >= e.y0 && sy < e.y1 {
                    let x = e.x0 + (sy - e.y0) * (e.x1 - e.x0) * e.inv_dy;
                    xings.push((x, e.dir));
                }
            }
            if xings.len() < 2 {
                continue;
            }
            xings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0i32;
            for pair in xings.windows(2) {
                wind += pair[0].1;
                let inside = match rule {
                    FillRule::NonZero => wind != 0,
                    FillRule::EvenOdd => (wind & 1) != 0,
                };
                if inside {
                    let x0 = pair[0].0.max(0.0);
                    let x1 = pair[1].0.min(w as f64);
                    if x1 > x0 {
                        let px0 = x0.floor() as usize;
                        let px1 = (x1.ceil() as usize).min(w);
                        active_x0 = active_x0.min(px0);
                        active_x1 = active_x1.max(px1);
                        for px in px0..px1 {
                            let cov = (x1.min(px as f64 + 1.0) - x0.max(px as f64)).max(0.0) as f32;
                            row_cov[px] += cov * 0.25;
                        }
                    }
                }
            }
        }

        if active_x0 >= active_x1 {
            continue;
        }
        let row_off = py * w;
        for px in active_x0..active_x1 {
            let mut cov = row_cov[px];
            if cov <= 1e-5 {
                continue;
            }
            if let Some(mk) = clip_mask {
                cov *= mk[row_off + px];
                if cov <= 1e-5 {
                    continue;
                }
            }
            let c = if let Some(c) = solid {
                c
            } else {
                sample_gradient(
                    grad.unwrap(),
                    Pt::new(px as f64 + 0.5, py as f64 + 0.5),
                    inv_full,
                    obj_bbox,
                )
            };
            let sa = (c[3] as f32 / 255.0) * alpha * cov;
            if sa > 1e-6 {
                let sr = (c[0] as f32 / 255.0) * sa;
                let sg = (c[1] as f32 / 255.0) * sa;
                let sb = (c[2] as f32 / 255.0) * sa;
                surf.blend_over(row_off + px, sr, sg, sb, sa);
            }
        }
    }
}

fn sample_gradient(
    grad: &Gradient,
    dev_pt: Pt,
    inv_full: Option<Affine>,
    obj_bbox: BBox,
) -> [u8; 4] {
    if grad.stops.is_empty() {
        return [0, 0, 0, 0];
    }
    let local_pt = inv_full.map_or(dev_pt, |m| m.apply(dev_pt));
    let mut g_pt = match grad.units {
        CoordUnits::UserSpaceOnUse => local_pt,
        CoordUnits::ObjectBoundingBox => {
            let bw = obj_bbox.width().max(1e-6);
            let bh = obj_bbox.height().max(1e-6);
            Pt::new((local_pt.x - obj_bbox.min_x) / bw, (local_pt.y - obj_bbox.min_y) / bh)
        }
    };
    if let Some(inv_gt) = grad.transform.inverse() {
        g_pt = inv_gt.apply(g_pt);
    }

    let raw_t = match grad.kind {
        GradientKind::Linear { x1, y1, x2, y2 } => {
            let dx = x2 - x1;
            let dy = y2 - y1;
            let len2 = dx * dx + dy * dy;
            if len2 < 1e-12 {
                0.0
            } else {
                ((g_pt.x - x1) * dx + (g_pt.y - y1) * dy) / len2
            }
        }
        GradientKind::Radial { cx, cy, r, fx, fy } => {
            if r <= 1e-9 {
                1.0
            } else {
                // Exact focal-point radial gradient parameter t in [0, 1]
                let dx = g_pt.x - fx;
                let dy = g_pt.y - fy;
                let fcx = fx - cx;
                let fcy = fy - cy;
                let a = dx * dx + dy * dy;
                if a < 1e-12 {
                    0.0
                } else {
                    let b = 2.0 * (dx * fcx + dy * fcy);
                    let c = fcx * fcx + fcy * fcy - r * r;
                    let disc = (b * b - 4.0 * a * c).max(0.0).sqrt();
                    let s = (-b + disc) / (2.0 * a);
                    if s > 1e-9 { 1.0 / s } else { 1.0 }
                }
            }
        }
    };

    let t = match grad.spread {
        SpreadMethod::Pad => raw_t.clamp(0.0, 1.0),
        SpreadMethod::Repeat => raw_t.rem_euclid(1.0),
        SpreadMethod::Reflect => {
            let m = raw_t.rem_euclid(2.0);
            if m <= 1.0 { m } else { 2.0 - m }
        }
    };

    let stops = &grad.stops;
    if t <= stops[0].offset {
        return stops[0].color;
    }
    if t >= stops.last().unwrap().offset {
        return stops.last().unwrap().color;
    }
    for w in stops.windows(2) {
        if t >= w[0].offset && t <= w[1].offset {
            let span = (w[1].offset - w[0].offset).max(1e-9);
            let f = ((t - w[0].offset) / span) as f32;
            let c0 = w[0].color;
            let c1 = w[1].color;
            return [
                (c0[0] as f32 + (c1[0] as f32 - c0[0] as f32) * f).round() as u8,
                (c0[1] as f32 + (c1[1] as f32 - c0[1] as f32) * f).round() as u8,
                (c0[2] as f32 + (c1[2] as f32 - c0[2] as f32) * f).round() as u8,
                (c0[3] as f32 + (c1[3] as f32 - c0[3] as f32) * f).round() as u8,
            ];
        }
    }
    stops.last().unwrap().color
}

fn apply_filter(surf: &mut Surface, fk: &FilterKind, scale: f64) {
    match *fk {
        FilterKind::GaussianBlur { std_x, std_y, merge_source } => {
            let orig = if merge_source { Some(surf.px.clone()) } else { None };
            let rx = (std_x * scale * 1.6).round() as usize;
            let ry = (std_y * scale * 1.6).round() as usize;
            box_blur_3pass(surf, rx, ry);
            if let Some(src) = orig {
                for (i, s) in src.iter().enumerate() {
                    if s[3] > 1e-6 {
                        surf.blend_over(i, s[0], s[1], s[2], s[3]);
                    }
                }
            }
        }
        FilterKind::DropShadow { dx, dy, std_x, std_y, color } => {
            let orig = surf.px.clone();
            let sx = (dx * scale).round() as isize;
            let sy = (dy * scale).round() as isize;
            let cr = color[0] as f32 / 255.0;
            let cg = color[1] as f32 / 255.0;
            let cb = color[2] as f32 / 255.0;
            let ca = color[3] as f32 / 255.0;

            let mut shadow = Surface::new(surf.w, surf.h);
            for y in 0..surf.h {
                let ny = y as isize + sy;
                if ny < 0 || ny >= surf.h as isize {
                    continue;
                }
                for x in 0..surf.w {
                    let nx = x as isize + sx;
                    if nx < 0 || nx >= surf.w as isize {
                        continue;
                    }
                    let a = orig[y * surf.w + x][3] * ca;
                    if a > 1e-6 {
                        shadow.px[ny as usize * surf.w + nx as usize] = [cr * a, cg * a, cb * a, a];
                    }
                }
            }
            let rx = (std_x * scale * 1.6).round() as usize;
            let ry = (std_y * scale * 1.6).round() as usize;
            box_blur_3pass(&mut shadow, rx, ry);
            for (i, s) in orig.iter().enumerate() {
                if s[3] > 1e-6 {
                    shadow.blend_over(i, s[0], s[1], s[2], s[3]);
                }
            }
            surf.px = shadow.px;
        }
    }
}

/// 3-pass separable box blur approximating a true Gaussian kernel in O(W*H) time.
fn box_blur_3pass(surf: &mut Surface, rx: usize, ry: usize) {
    if rx == 0 && ry == 0 {
        return;
    }
    let w = surf.w;
    let h = surf.h;
    let mut tmp = vec![[0.0f32; 4]; w * h];
    let rx = rx.min(w.saturating_sub(1));
    let ry = ry.min(h.saturating_sub(1));

    for _ in 0..3 {
        if rx > 0 {
            let inv = 1.0 / (2 * rx + 1) as f32;
            for y in 0..h {
                let row = y * w;
                let mut acc = [0.0f32; 4];
                for dx in -(rx as isize)..=(rx as isize) {
                    let cx = dx.clamp(0, (w - 1) as isize) as usize;
                    let p = surf.px[row + cx];
                    for k in 0..4 {
                        acc[k] += p[k];
                    }
                }
                for x in 0..w {
                    tmp[row + x] = [acc[0] * inv, acc[1] * inv, acc[2] * inv, acc[3] * inv];
                    let add_x = (x + rx + 1).min(w - 1);
                    let sub_x = x.saturating_sub(rx);
                    let pa = surf.px[row + add_x];
                    let ps = surf.px[row + sub_x];
                    for k in 0..4 {
                        acc[k] += pa[k] - ps[k];
                    }
                }
            }
            std::mem::swap(&mut surf.px, &mut tmp);
        }
        if ry > 0 {
            let inv = 1.0 / (2 * ry + 1) as f32;
            for x in 0..w {
                let mut acc = [0.0f32; 4];
                for dy in -(ry as isize)..=(ry as isize) {
                    let cy = dy.clamp(0, (h - 1) as isize) as usize;
                    let p = surf.px[cy * w + x];
                    for k in 0..4 {
                        acc[k] += p[k];
                    }
                }
                for y in 0..h {
                    tmp[y * w + x] = [acc[0] * inv, acc[1] * inv, acc[2] * inv, acc[3] * inv];
                    let add_y = (y + ry + 1).min(h - 1);
                    let sub_y = y.saturating_sub(ry);
                    let pa = surf.px[add_y * w + x];
                    let ps = surf.px[sub_y * w + x];
                    for k in 0..4 {
                        acc[k] += pa[k] - ps[k];
                    }
                }
            }
            std::mem::swap(&mut surf.px, &mut tmp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::png::inspect_and_verify_png;
    use crate::svg::parser::parse_svg;

    #[test]
    fn svg_render_gradient_clip_and_preview() {
        let src = r##"
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#ff6622"/>
      <stop offset="100%" stop-color="#3388ff"/>
    </linearGradient>
    <clipPath id="c"><circle cx="32" cy="32" r="26"/></clipPath>
    <filter id="ds"><feDropShadow dx="0" dy="2" stdDeviation="2" flood-color="#000" flood-opacity="0.5"/></filter>
  </defs>
  <rect x="4" y="4" width="56" height="56" rx="10" fill="url(#g)" clip-path="url(#c)" filter="url(#ds)" stroke="#ffffff" stroke-width="2"/>
</svg>"##;
        let doc = parse_svg(src, "test_icon").unwrap();
        assert!(doc.is_ok(), "errors: {:?}", doc.errors);
        assert_eq!(doc.stats.shapes, 1);
        assert_eq!(doc.stats.gradients, 1);

        let opts = SvgRenderOptions::for_doc(&doc);
        let png = render_svg_png(&doc, &opts);
        let info = inspect_and_verify_png(&png).unwrap();
        assert_eq!((info.width, info.height), (64, 64));

        let mut prev_opts = opts.clone();
        prev_opts.checker = true;
        prev_opts.hud = true;
        prev_opts.grid = true;
        prev_opts.nine_slice = Some([8.0, 8.0, 8.0, 8.0]);
        let qa_png = render_svg_png(&doc, &prev_opts);
        assert!(inspect_and_verify_png(&qa_png).is_ok());
    }
}
