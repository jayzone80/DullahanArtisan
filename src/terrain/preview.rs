/// 2x2 Terrain Diagnostic QA Sheet renderer:
/// - Panel 0: 3D-lit Hillshade Relief + Contour Lines
/// - Panel 1: Elevation Heatmap (hypsometric ramp + sea level)
/// - Panel 2: Hydraulic Flow & Erosion/Deposition Map
/// - Panel 3: 4-Channel Biome Weightmap / Splat Preview

use crate::common::font::Canvas;
use crate::common::math::Vec3;
use crate::common::png::encode_rgba;
use crate::mesh::palette::Palette;
use crate::terrain::sim::TerrainData;

pub fn render_terrain_preview_png(terrain: &TerrainData, palette: Option<&Palette>) -> Vec<u8> {
    let cell = 340usize;
    let total_w = cell * 2 + 2;
    let footer_h = 24usize;
    let total_h = cell * 2 + 2 + footer_h;
    let mut canvas = Canvas::new(total_w, total_h, [18, 20, 26, 255]);

    // Precompute sun hillshade across terrain grid
    let sun = Vec3::new(-0.55, 0.68, -0.48).normalized();
    let dx_world = terrain.world[0] / (terrain.width.saturating_sub(1).max(1)) as f32;
    let dz_world = terrain.world[2] / (terrain.height.saturating_sub(1).max(1)) as f32;

    // Resolve preview colors for the 4 biome splat channels
    let fallback_cols: [[f32; 3]; 4] = [
        [0.84, 0.74, 0.56], // R: sand
        [0.36, 0.58, 0.28], // G: grass
        [0.48, 0.51, 0.56], // B: rock
        [0.92, 0.93, 0.96], // A: snow/peak
    ];
    let mut biome_cols = fallback_cols;
    if let Some(pal) = palette {
        for ch in 0..4 {
            if let Ok(rm) = pal.resolve_mat(&terrain.channel_mats[ch]) {
                let c = pal.sample_basecolor_nearest(rm.uv);
                biome_cols[ch] = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0];
            }
        }
    }

    let max_erode = terrain
        .erosion_delta
        .iter()
        .map(|v| v.abs())
        .fold(1e-5f32, f32::max);

    let oxs = [0usize, cell + 2, 0usize, cell + 2];
    let oys = [0usize, 0usize, cell + 2, cell + 2];

    for py in 0..cell {
        let v = py as f32 / (cell - 1) as f32;
        let ty = ((v * (terrain.height - 1) as f32).round() as usize).min(terrain.height - 1);
        let ym = ty.saturating_sub(1);
        let yp = (ty + 1).min(terrain.height - 1);

        for px in 0..cell {
            let u = px as f32 / (cell - 1) as f32;
            let tx = ((u * (terrain.width - 1) as f32).round() as usize).min(terrain.width - 1);
            let xm = tx.saturating_sub(1);
            let xp = (tx + 1).min(terrain.width - 1);
            let i = ty * terrain.width + tx;

            let h = terrain.heights[i];
            let hl = terrain.heights[ty * terrain.width + xm] * terrain.world[1];
            let hr = terrain.heights[ty * terrain.width + xp] * terrain.world[1];
            let hu = terrain.heights[ym * terrain.width + tx] * terrain.world[1];
            let hd = terrain.heights[yp * terrain.width + tx] * terrain.world[1];
            let n = Vec3::new(-(hr - hl) / (2.0 * dx_world), 1.0, -(hd - hu) / (2.0 * dz_world))
                .normalized();
            let shade = (n.dot(sun).max(0.0) * 0.78 + 0.26).clamp(0.18, 1.08);

            // Panel 0: Shaded Relief + Contours
            let sp = terrain.splats[i];
            let mut base = [0.0f32; 3];
            for ch in 0..4 {
                for k in 0..3 {
                    base[k] += biome_cols[ch][k] * sp[ch];
                }
            }
            if terrain.sea_level > 0.005 && h <= terrain.sea_level {
                let depth = ((terrain.sea_level - h) / terrain.sea_level.max(0.01)).clamp(0.0, 1.0);
                base = [
                    0.16 * (1.0 - depth * 0.5),
                    0.42 * (1.0 - depth * 0.35),
                    0.58 * (1.0 - depth * 0.2),
                ];
            }
            // Subtle contour lines every 10% height
            let contour_band = (h * 10.0).fract();
            let contour = if contour_band < 0.035 || contour_band > 0.965 { 0.84 } else { 1.0 };
            let c0 = to_rgba([base[0] * shade * contour, base[1] * shade * contour, base[2] * shade * contour]);
            canvas.set_px((oxs[0] + px) as i32, (oys[0] + py) as i32, c0);

            // Panel 1: Elevation Heatmap
            let hm = elevation_heatmap(h, terrain.sea_level);
            let h_shade = 0.75 + 0.30 * shade;
            let c1 = to_rgba([hm[0] * h_shade, hm[1] * h_shade, hm[2] * h_shade]);
            canvas.set_px((oxs[1] + px) as i32, (oys[1] + py) as i32, c1);

            // Panel 2: Hydraulic Flow & Erosion/Deposition
            let fl = terrain.flow[i].powf(1.35);
            let ed = (terrain.erosion_delta[i] / max_erode).clamp(-1.0, 1.0);
            let bg = 0.14 + 0.24 * shade;
            let mut er = bg;
            let mut eg = bg * 1.04;
            let mut eb = bg * 1.12;
            // Deposited sediment = warm gold; carved channels = deep crimson/cyan flow
            if ed > 0.0 {
                er += ed * 0.72;
                eg += ed * 0.48;
                eb += ed * 0.10;
            } else {
                let cut = -ed;
                er += cut * 0.55;
                eg += cut * 0.12;
                eb += cut * 0.28;
            }
            // Glowing cyan river flow overlay
            er = er * (1.0 - fl * 0.65) + 0.18 * fl;
            eg = eg * (1.0 - fl * 0.45) + 0.88 * fl;
            eb = eb * (1.0 - fl * 0.35) + 0.98 * fl;
            canvas.set_px((oxs[2] + px) as i32, (oys[2] + py) as i32, to_rgba([er, eg, eb]));

            // Panel 3: Biome Splatmap (crisp unshaded biome weights + subtle slope lines)
            let c3 = to_rgba([
                (biome_cols[0][0] * sp[0] + biome_cols[1][0] * sp[1] + biome_cols[2][0] * sp[2] + biome_cols[3][0] * sp[3]) * (0.82 + 0.22 * shade),
                (biome_cols[0][1] * sp[0] + biome_cols[1][1] * sp[1] + biome_cols[2][1] * sp[2] + biome_cols[3][1] * sp[3]) * (0.82 + 0.22 * shade),
                (biome_cols[0][2] * sp[0] + biome_cols[1][2] * sp[1] + biome_cols[2][2] * sp[2] + biome_cols[3][2] * sp[3]) * (0.82 + 0.22 * shade),
            ]);
            canvas.set_px((oxs[3] + px) as i32, (oys[3] + py) as i32, c3);
        }
    }

    // Panel header badges
    let labels = [
        "1. HILLSHADE + CONTOURS",
        "2. ELEVATION HEATMAP",
        "3. HYDRAULIC FLOW + EROSION",
        "4. BIOME SPLATMAP (RGBA)",
    ];
    for p in 0..4 {
        let lx = oxs[p] as i32 + 6;
        let ly = oys[p] as i32 + 6;
        let bw = (labels[p].len() as i32) * 6 + 10;
        canvas.fill_rect(lx, ly, bw, 15, [12, 14, 18, 200]);
        canvas.draw_text(lx + 5, ly + 4, labels[p], [232, 238, 248, 255], 1);
    }

    // Mini legend bar on Biome panel
    let lx = oxs[3] as i32 + 6;
    let ly = oys[3] as i32 + cell as i32 - 20;
    canvas.fill_rect(lx, ly, cell as i32 - 12, 15, [12, 14, 18, 200]);
    let ch_tags = ["R", "G", "B", "A"];
    let mut cx = lx + 5;
    for ch in 0..4 {
        canvas.fill_rect(cx, ly + 3, 9, 9, to_rgba(biome_cols[ch]));
        let short: String = terrain.channel_mats[ch].chars().take(10).collect();
        let txt = format!("{}:{}", ch_tags[ch], short);
        canvas.draw_text(cx + 12, ly + 4, &txt, [224, 232, 242, 255], 1);
        cx += 80;
    }

    // Bottom HUD footer
    let fy = (cell * 2 + 2) as i32;
    canvas.fill_rect(0, fy, total_w as i32, footer_h as i32, [14, 16, 22, 255]);
    let (lo, hi) = terrain.min_max_height();
    let footer = format!(
        "TERRAIN {}  {}x{}  WORLD:{:.0}x{:.0}x{:.0}  H:[{:.2}..{:.2}]  MAX_SLOPE:{:.0}DEG  DROPS:{}",
        terrain.name,
        terrain.width,
        terrain.height,
        terrain.world[0],
        terrain.world[1],
        terrain.world[2],
        lo,
        hi,
        terrain.max_slope_deg(),
        terrain.drops_simulated
    );
    canvas.draw_text(8, fy + 8, &footer, [140, 235, 170, 255], 1);

    encode_rgba(total_w as u32, total_h as u32, &canvas.rgba)
}

fn to_rgba(rgb: [f32; 3]) -> [u8; 4] {
    [
        (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        255,
    ]
}

fn elevation_heatmap(h: f32, sea: f32) -> [f32; 3] {
    if sea > 0.005 && h < sea {
        let t = (h / sea).clamp(0.0, 1.0);
        return [0.06 + 0.08 * t, 0.16 + 0.28 * t, 0.36 + 0.32 * t];
    }
    let stops: [(f32, [f32; 3]); 6] = [
        (0.00, [0.12, 0.34, 0.56]),
        (0.18, [0.22, 0.62, 0.42]),
        (0.42, [0.64, 0.78, 0.30]),
        (0.66, [0.88, 0.56, 0.22]),
        (0.84, [0.76, 0.24, 0.22]),
        (1.00, [0.96, 0.96, 0.98]),
    ];
    let v = h.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        if v >= w[0].0 && v <= w[1].0 {
            let f = (v - w[0].0) / (w[1].0 - w[0].0).max(1e-6);
            return [
                w[0].1[0] + (w[1].1[0] - w[0].1[0]) * f,
                w[0].1[1] + (w[1].1[1] - w[0].1[1]) * f,
                w[0].1[2] + (w[1].1[2] - w[0].1[2]) * f,
            ];
        }
    }
    stops.last().unwrap().1
}
