use crate::common::font::Canvas;
use crate::common::math::Vec3;
use crate::common::png::encode_rgba;
use crate::tex::bake::BakedTexture;
use crate::tex::parser::{NormalConvention, WrapMode};

#[inline]
fn linear_to_srgb_u8(x: f32) -> u8 {
    let c = (x / (1.0 + x * 0.14)).clamp(0.0, 1.0).powf(1.0 / 2.2);
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Evaluate studio PBR shading (GGX specular + diffuse + hemisphere fill + HBAO) at a UV point.
fn shade_pbr(
    albedo: [f32; 3],
    n: Vec3,
    v: Vec3,
    rough: f32,
    metal: f32,
    ao: f32,
    key_dir: Vec3,
    fill_dir: Vec3,
) -> [u8; 4] {
    let n = n.normalized();
    let v = v.normalized();
    let ndotv = n.dot(v).max(0.001);

    let alb = Vec3::new(albedo[0], albedo[1], albedo[2]);
    let f0 = Vec3::splat(0.04).lerp(alb, metal);
    let diff_col = alb * (1.0 - metal);

    let r = rough.clamp(0.06, 1.0);
    let alpha = r * r;
    let a2 = alpha * alpha;

    let eval_light = |l_dir: Vec3, l_col: Vec3| -> Vec3 {
        let ndotl = n.dot(l_dir).max(0.0);
        if ndotl <= 0.0 {
            return Vec3::ZERO;
        }
        let h = (l_dir + v).normalized();
        let ndoth = n.dot(h).max(0.0);
        let vdoth = v.dot(h).max(0.0);
        let denom = ndoth * ndoth * (a2 - 1.0) + 1.0;
        let d = a2 / (std::f32::consts::PI * denom * denom).max(1e-5);
        let k = (r + 1.0) * (r + 1.0) * 0.125;
        let g1v = ndotv / (ndotv * (1.0 - k) + k);
        let g1l = ndotl / (ndotl * (1.0 - k) + k);
        let g = g1v * g1l;
        let fres = (1.0 - vdoth).powi(5);
        let f = Vec3::new(
            f0.x + (1.0 - f0.x) * fres,
            f0.y + (1.0 - f0.y) * fres,
            f0.z + (1.0 - f0.z) * fres,
        );
        let spec = f * (d * g / (4.0 * ndotv * ndotl).max(1e-4));
        let kd = (Vec3::ONE - f) * (1.0 - metal);
        let diff = Vec3::new(kd.x * diff_col.x, kd.y * diff_col.y, kd.z * diff_col.z)
            * (1.0 / std::f32::consts::PI);
        let sum = diff + spec;
        Vec3::new(sum.x * l_col.x, sum.y * l_col.y, sum.z * l_col.z) * ndotl
    };

    let key = eval_light(key_dir, Vec3::new(2.75, 2.62, 2.38));
    let fill = eval_light(fill_dir, Vec3::new(0.70, 0.80, 0.96));

    // Hemisphere ambient modulated strongly by baked HBAO
    let hemi_t = n.y * 0.5 + 0.5;
    let sky = Vec3::new(0.28, 0.34, 0.44);
    let gnd = Vec3::new(0.14, 0.13, 0.11);
    let amb_light = gnd.lerp(sky, hemi_t);
    let amb = Vec3::new(
        diff_col.x * amb_light.x,
        diff_col.y * amb_light.y,
        diff_col.z * amb_light.z,
    ) * ao;

    // Specular environment bounce so metals/bevels gleam naturally
    let refl = (n * (2.0 * ndotv) - v).normalized();
    let env_t = refl.y * 0.5 + 0.5;
    let env_col = Vec3::new(0.16, 0.17, 0.19).lerp(Vec3::new(0.55, 0.62, 0.72), env_t);
    let env_spec = Vec3::new(f0.x * env_col.x, f0.y * env_col.y, f0.z * env_col.z)
        * ((1.0 - r * 0.78) * ao);

    let col = (key + fill) * (0.35 + 0.65 * ao) + amb + env_spec;
    [
        linear_to_srgb_u8(col.x),
        linear_to_srgb_u8(col.y),
        linear_to_srgb_u8(col.z),
        255,
    ]
}

fn sample_tex_uv(tex: &BakedTexture, u: f32, v: f32) -> ([f32; 3], Vec3, f32, f32, f32, f32) {
    let uu = if tex.wrap.wrap_x() { u.rem_euclid(1.0) } else { u.clamp(0.0, 0.9999) };
    let vv = if tex.wrap.wrap_y() { v.rem_euclid(1.0) } else { v.clamp(0.0, 0.9999) };
    let x = ((uu * tex.width as f32) as usize).min(tex.width - 1);
    let y = ((vv * tex.height as f32) as usize).min(tex.height - 1);
    let i = y * tex.width + x;
    let n = tex.normal[i];
    (
        tex.albedo[i],
        Vec3::new(n[0], n[1], n[2]),
        tex.height_norm[i],
        tex.ao[i],
        tex.roughness[i],
        tex.metallic[i],
    )
}

/// Render a 2x2 studio PBR & tiling QA contact sheet PNG.
pub fn render_tex_preview_png(tex: &BakedTexture, show_grid: bool) -> Vec<u8> {
    let cell = 440usize;
    let footer_h = 26usize;
    let total_w = cell * 2;
    let total_h = cell * 2 + footer_h;
    let mut canvas = Canvas::new(total_w, total_h, [20, 22, 28, 255]);

    let key_dir = Vec3::new(0.46, 0.58, 0.67).normalized();
    let fill_dir = Vec3::new(-0.62, 0.24, 0.75).normalized();

    let (rep_x, rep_y, tile_lbl) = match tex.wrap {
        WrapMode::XY => (2.0f32, 2.0f32, "TILED PBR 2X2"),
        WrapMode::X => (2.0f32, 1.0f32, "TILED TRIM 2X1"),
        WrapMode::Y => (1.0f32, 2.0f32, "TILED PBR 1X2"),
        WrapMode::None => (1.0f32, 1.0f32, "LIT PBR 1X1"),
    };

    // Panel 1 (Top-Left): Tiled Lit PBR View
    for py in 0..cell {
        let v = (py as f32 + 0.5) / cell as f32 * rep_y;
        for px in 0..cell {
            let u = (px as f32 + 0.5) / cell as f32 * rep_x;
            let (alb, n, _h, ao, rgh, met) = sample_tex_uv(tex, u, v);
            let col = shade_pbr(alb, n, Vec3::Z, rgh, met, ao, key_dir, fill_dir);
            canvas.set_px(px as i32, py as i32, col);
        }
    }
    if show_grid {
        let seam_col = [90, 230, 255, 165];
        if tex.wrap.wrap_x() {
            let sx = (cell / 2) as i32;
            for py in (0..cell as i32).step_by(6) {
                canvas.fill_rect(sx, py, 1, 3, seam_col);
            }
        }
        if tex.wrap.wrap_y() {
            let sy = (cell / 2) as i32;
            for px in (0..cell as i32).step_by(6) {
                canvas.fill_rect(px, sy, 3, 1, seam_col);
            }
        }
    }

    // Panel 2 (Top-Right): 3D Lit PBR Sphere (for XY tiles) or 3D Lit Column (for X trim sheets)
    let is_trim = tex.wrap == WrapMode::X;
    let p2_lbl = if is_trim { "3D PBR PILLAR (TRIM)" } else { "3D PBR SPHERE" };
    let ox2 = cell;
    for py in 0..cell {
        let ny_ndc = 1.0 - 2.0 * ((py as f32 + 0.5) / cell as f32);
        // Studio vignette background
        let bg_t = py as f32 / cell as f32;
        let bg = [
            (36.0 - bg_t * 14.0) as u8,
            (40.0 - bg_t * 14.0) as u8,
            (48.0 - bg_t * 14.0) as u8,
            255,
        ];
        for px in 0..cell {
            let nx_ndc = 2.0 * ((px as f32 + 0.5) / cell as f32) - 1.0;
            let mut out_col = bg;
            if is_trim {
                // Lit 3D cylinder wrapping the horizontal trim seamlessly!
                let rad = 0.72f32;
                let half_h = 0.86f32;
                if nx_ndc.abs() <= rad && ny_ndc.abs() <= half_h {
                    let cx = nx_ndc / rad;
                    let cz = (1.0 - cx * cx).max(0.0).sqrt();
                    let geo_n = Vec3::new(cx, 0.0, cz).normalized();
                    let tang = Vec3::new(cz, 0.0, -cx).normalized();
                    let bitang = Vec3::Y;
                    let theta = cx.asin(); // -pi/2 .. +pi/2
                    let u = (theta / std::f32::consts::PI) * 1.2 + 0.5;
                    let v = 0.5 - ny_ndc / (2.0 * half_h);
                    let (alb, tn, _h, ao, rgh, met) = sample_tex_uv(tex, u, v);
                    let world_n = (tang * tn.x + bitang * tn.y + geo_n * tn.z).normalized();
                    out_col = shade_pbr(alb, world_n, Vec3::Z, rgh, met, ao, key_dir, fill_dir);
                }
            } else {
                // Lit 3D sphere with tangent-space normal mapping (2x1 UV wrap for natural rock scale)
                let r = 0.82f32;
                let d2 = nx_ndc * nx_ndc + ny_ndc * ny_ndc;
                if d2 <= r * r {
                    let sx = nx_ndc / r;
                    let sy = ny_ndc / r;
                    let sz = (1.0 - sx * sx - sy * sy).max(0.0).sqrt();
                    let geo_n = Vec3::new(sx, sy, sz).normalized();
                    let u = (sx.atan2(sz) / std::f32::consts::PI) * 1.15 + 0.5;
                    let v = (0.5 - sy.asin() / std::f32::consts::PI) * 1.5;
                    let tang = Vec3::new(sz, 0.0, -sx).normalized();
                    let bitang = geo_n.cross(tang).normalized();
                    let (alb, tn, _h, ao, rgh, met) = sample_tex_uv(tex, u, v);
                    let world_n = (tang * tn.x + bitang * tn.y + geo_n * tn.z).normalized();
                    out_col = shade_pbr(alb, world_n, Vec3::Z, rgh, met, ao, key_dir, fill_dir);
                }
            }
            canvas.set_px((ox2 + px) as i32, py as i32, out_col);
        }
    }

    // Panel 3 (Bottom-Left): Tangent-Space Normal Map
    let oy3 = cell;
    let flip_y = tex.normal_conv == NormalConvention::DirectX;
    for py in 0..cell {
        let v = (py as f32 + 0.5) / cell as f32;
        for px in 0..cell {
            let u = (px as f32 + 0.5) / cell as f32;
            let (_alb, n, _h, _ao, _rgh, _met) = sample_tex_uv(tex, u, v);
            let ny = if flip_y { -n.y } else { n.y };
            let enc = |val: f32| ((val.clamp(-1.0, 1.0) * 0.5 + 0.5) * 255.0).round() as u8;
            canvas.set_px(px as i32, (oy3 + py) as i32, [enc(n.x), enc(ny), enc(n.z), 255]);
        }
    }

    // Panel 4 (Bottom-Right): Split Displacement Height (Left) | Baked HBAO (Right)
    let ox4 = cell;
    let oy4 = cell;
    let split = cell / 2;
    for py in 0..cell {
        let v = (py as f32 + 0.5) / cell as f32;
        for px in 0..cell {
            let u = (px as f32 + 0.5) / cell as f32;
            let (_alb, _n, h_n, ao, _rgh, _met) = sample_tex_uv(tex, u, v);
            let col = if px == split {
                [245, 200, 80, 255]
            } else if px < split {
                let g = (h_n.clamp(0.0, 1.0) * 255.0).round() as u8;
                [g, g, g, 255]
            } else {
                let g = (ao.clamp(0.0, 1.0) * 255.0).round() as u8;
                [g, g, g, 255]
            };
            canvas.set_px((ox4 + px) as i32, (oy4 + py) as i32, col);
        }
    }

    // Crisp panel dividers & labels
    canvas.fill_rect(cell as i32 - 1, 0, 2, (cell * 2) as i32, [14, 16, 20, 255]);
    canvas.fill_rect(0, cell as i32 - 1, total_w as i32, 2, [14, 16, 20, 255]);

    let draw_badge = |c: &mut Canvas, x: i32, y: i32, text: &str| {
        c.fill_rect(x, y, text.len() as i32 * 6 + 10, 15, [12, 14, 18, 200]);
        c.draw_text(x + 5, y + 4, text, [235, 240, 248, 255], 1);
    };
    draw_badge(&mut canvas, 8, 8, tile_lbl);
    draw_badge(&mut canvas, ox2 as i32 + 8, 8, p2_lbl);
    let n_lbl = if flip_y { "NORMAL MAP (DX -Y)" } else { "NORMAL MAP (GL +Y)" };
    draw_badge(&mut canvas, 8, oy3 as i32 + 8, n_lbl);
    draw_badge(&mut canvas, ox4 as i32 + 8, oy4 as i32 + 8, "HEIGHT16 | BAKED AO");

    // HUD footer strip
    let fy = (cell * 2) as i32;
    canvas.fill_rect(0, fy, total_w as i32, footer_h as i32, [14, 16, 22, 255]);
    let foot = format!(
        "TEX: {} {}X{} WRAP:{} | SEAM A:{:.3} N:{:.3} H:{:.3} | AO:{:.2}..{:.2} | TRIS:{}",
        tex.name.to_uppercase(),
        tex.width,
        tex.height,
        tex.wrap.label().to_uppercase(),
        tex.seam_albedo,
        tex.seam_normal,
        tex.seam_height,
        tex.ao_min,
        tex.ao_max,
        tex.tri_count
    );
    canvas.draw_text(10, fy + 9, &foot, [215, 225, 240, 255], 1);

    encode_rgba(total_w as u32, total_h as u32, &canvas.rgba)
}
