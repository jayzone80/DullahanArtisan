use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use dullahan_artisan::common::math::parse_hex_rgba;
use dullahan_artisan::mesh::gltf::{export_glb, export_gltf_json};
use dullahan_artisan::mesh::palette::Palette;
use dullahan_artisan::mesh::parser::load_and_compile_model_with_overrides;
use dullahan_artisan::mesh::render::{
    render_anim_paths_png, render_anim_sheet_png, render_to_png, render_turntable_png,
    RenderOptions,
};
use dullahan_artisan::mesh::validator::{validate_file as validate_gltf_file, validate_glb_bytes, validate_gltf_json_str};
use dullahan_artisan::sfx::diagnostics::{analyze_wav, render_diagnostic_png};
use dullahan_artisan::sfx::engine::render_sound;
use dullahan_artisan::sfx::parser::load_sound_file;
use dullahan_artisan::sfx::wav::{read_wav, write_pcm16};
use dullahan_artisan::svg::parser::load_svg_file;
use dullahan_artisan::svg::raster::{render_svg_png, SvgRenderOptions};
use dullahan_artisan::terrain::mesher::{build_terrain_mesh, TerrainMeshOptions};
use dullahan_artisan::terrain::parser::load_terrain_file;
use dullahan_artisan::terrain::preview::render_terrain_preview_png;
use dullahan_artisan::terrain::sim::{build_from_heightmap_png, simulate_terrain};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help" | "help") {
        print_usage();
        return ExitCode::SUCCESS;
    }
    let res = match args[0].as_str() {
        "pal" | "palette" => cmd_pal(&args[1..]),
        "mesh" | "model" => cmd_mesh(&args[1..]),
        "sfx" | "audio" => cmd_sfx(&args[1..]),
        "svg" | "ui" => cmd_svg(&args[1..]),
        "terrain" | "ter" => cmd_terrain(&args[1..]),
        "validate" | "val" => cmd_validate(&args[1..]),
        "spec" => cmd_spec(&args[1..]),
        other => Err(format!("unknown subcommand '{other}' (use: pal, mesh, sfx, svg, terrain, validate, spec)")),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ERR {e}");
            ExitCode::FAILURE
        }
    }
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    if let Some(p) = path.parent() {
        if !p.as_os_str().is_empty() {
            std::fs::create_dir_all(p)
                .map_err(|e| format!("failed to create directory '{}': {e}", p.display()))?;
        }
    }
    Ok(())
}

fn cmd_pal(args: &[String]) -> Result<(), String> {
    let mut input: Option<PathBuf> = None;
    let mut out_prefix: Option<String> = None;
    let mut sheet_out: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            "-o" | "--out" => {
                i += 1;
                out_prefix = Some(args.get(i).ok_or("-o requires output prefix")?.clone());
            }
            "--sheet" => {
                i += 1;
                sheet_out = Some(PathBuf::from(args.get(i).ok_or("--sheet requires png path")?));
            }
            a if !a.starts_with('-') && input.is_none() => input = Some(PathBuf::from(a)),
            other => return Err(format!("unknown pal arg '{other}'")),
        }
        i += 1;
    }

    let in_path = input.ok_or("pal requires <file.pal>")?;
    let pal = Palette::load_file(&in_path)?;
    let prefix = out_prefix.unwrap_or_else(|| {
        in_path.with_extension("").to_string_lossy().into_owned()
    });

    let bc_path = PathBuf::from(format!("{prefix}_basecolor.png"));
    let orm_path = PathBuf::from(format!("{prefix}_orm.png"));
    let em_path = PathBuf::from(format!("{prefix}_emissive.png"));
    ensure_parent(&bc_path)?;

    let bc_bytes = pal.encode_basecolor_png();
    let orm_bytes = pal.encode_orm_png();
    let em_bytes = pal.encode_emissive_png();
    std::fs::write(&bc_path, &bc_bytes).map_err(|e| format!("write basecolor: {e}"))?;
    std::fs::write(&orm_path, &orm_bytes).map_err(|e| format!("write orm: {e}"))?;
    std::fs::write(&em_path, &em_bytes).map_err(|e| format!("write emissive: {e}"))?;

    if let Some(sp) = sheet_out {
        ensure_parent(&sp)?;
        std::fs::write(&sp, pal.render_swatch_sheet_png())
            .map_err(|e| format!("write sheet: {e}"))?;
    }

    let blend_cnt = pal.swatches.iter().filter(|s| s.is_translucent()).count();
    let em_cnt = pal.swatches.iter().filter(|s| s.is_emissive()).count();
    let total_kb = (bc_bytes.len() + orm_bytes.len() + em_bytes.len()) as f64 / 1024.0;
    println!(
        "OK pal {}x{}@{} ({}x{}) used={}/{} opaque={} blend={} emissive={} pngs={:.2}KB -> {}_{{basecolor,orm,emissive}}.png",
        pal.cols,
        pal.rows,
        pal.tile_px,
        pal.width,
        pal.height,
        pal.swatches.len(),
        pal.cols * pal.rows,
        pal.swatches.len() - blend_cnt,
        blend_cnt,
        em_cnt,
        total_kb,
        prefix
    );
    Ok(())
}

fn cmd_mesh(args: &[String]) -> Result<(), String> {
    let mut input: Option<PathBuf> = None;
    let mut pal_override: Option<PathBuf> = None;
    let mut out_model: Option<PathBuf> = None;
    let mut ext_pal: Option<String> = None;
    let mut render_out: Option<PathBuf> = None;
    let mut turn_out: Option<PathBuf> = None;
    let mut paths_out: Option<PathBuf> = None;
    let mut sheet_out: Option<PathBuf> = None;
    let mut anim_name: Option<String> = None;
    let mut track_nodes: Vec<String> = Vec::new();
    let mut sheet_frames: usize = 8;
    let mut focus_id: Option<String> = None;
    let mut hud_override: Option<bool> = None;
    let mut cli_overrides: HashMap<String, String> = HashMap::new();
    let mut cli_morphs: Vec<(String, f32)> = Vec::new();
    let mut opts = RenderOptions::default();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            "-p" | "--pal" | "--palette" => {
                i += 1;
                pal_override = Some(PathBuf::from(args.get(i).ok_or("-p requires palette path")?));
            }
            "-o" | "--out" => {
                i += 1;
                out_model = Some(PathBuf::from(args.get(i).ok_or("-o requires output .glb/.gltf")?));
            }
            "--ext-pal" => {
                i += 1;
                ext_pal = Some(args.get(i).ok_or("--ext-pal requires URI prefix")?.clone());
            }
            "--ov" | "--override" => {
                i += 1;
                let spec = args.get(i).ok_or("--ov requires old:new[,old2:new2]")?;
                for pair in spec.split(',') {
                    let pair = pair.trim();
                    if pair.is_empty() {
                        continue;
                    }
                    let (old_m, new_m) = pair
                        .split_once(':')
                        .ok_or_else(|| format!("invalid --ov pair '{pair}' (expected old:new)"))?;
                    if old_m.trim().is_empty() || new_m.trim().is_empty() {
                        return Err(format!("invalid --ov pair '{pair}' (expected old:new)"));
                    }
                    cli_overrides.insert(old_m.trim().to_string(), new_m.trim().to_string());
                }
            }
            "--render" => {
                i += 1;
                render_out = Some(PathBuf::from(args.get(i).ok_or("--render requires png path")?));
            }
            "--turn" | "--turntable" => {
                i += 1;
                turn_out = Some(PathBuf::from(args.get(i).ok_or("--turn requires png path")?));
            }
            "--anim" | "--action" => {
                i += 1;
                anim_name = Some(args.get(i).ok_or("--anim requires action name")?.clone());
            }
            "--paths" | "--motion-paths" => {
                i += 1;
                paths_out = Some(PathBuf::from(args.get(i).ok_or("--paths requires png path")?));
            }
            "--track" => {
                i += 1;
                let list = args.get(i).ok_or("--track requires node1[,node2,...]")?;
                for item in list.split(',') {
                    let s = item.trim().strip_prefix('@').unwrap_or(item.trim());
                    if !s.is_empty() {
                        track_nodes.push(s.to_string());
                    }
                }
            }
            "--anim-sheet" | "--sheet" => {
                i += 1;
                sheet_out = Some(PathBuf::from(args.get(i).ok_or("--anim-sheet requires png path")?));
            }
            "--frames" => {
                i += 1;
                sheet_frames = args
                    .get(i)
                    .ok_or("--frames requires N")?
                    .parse()
                    .map_err(|_| "bad --frames")?;
            }
            "--focus" => {
                i += 1;
                let id = args.get(i).ok_or("--focus requires @id")?;
                focus_id = Some(id.strip_prefix('@').unwrap_or(id).to_string());
            }
            "--size" => {
                i += 1;
                let s = args.get(i).ok_or("--size requires WxH")?;
                let (w, h) = parse_size(s)?;
                opts.width = w;
                opts.height = h;
            }
            "--yaw" => {
                i += 1;
                opts.yaw_deg = args.get(i).ok_or("--yaw requires deg")?.parse().map_err(|_| "bad --yaw")?;
            }
            "--pitch" => {
                i += 1;
                opts.pitch_deg = args.get(i).ok_or("--pitch requires deg")?.parse().map_err(|_| "bad --pitch")?;
            }
            "--fov" => {
                i += 1;
                opts.fov_deg = args.get(i).ok_or("--fov requires deg")?.parse().map_err(|_| "bad --fov")?;
            }
            "--fill" => {
                i += 1;
                opts.fill = args.get(i).ok_or("--fill requires 0.1..0.96")?.parse().map_err(|_| "bad --fill")?;
            }
            "--bg" => {
                i += 1;
                opts.bg = parse_hex_rgba(args.get(i).ok_or("--bg requires #hex")?)?;
            }
            "--ortho" => opts.ortho = true,
            "--wire" | "--wireframe" => opts.wireframe = true,
            "--colliders" => opts.show_colliders = true,
            "--bones" | "--rig" => opts.show_bones = true,
            "--morph" | "--shape" => {
                i += 1;
                let spec = args
                    .get(i)
                    .ok_or("--morph requires name:weight[,name2:weight2]")?;
                for pair in spec.split(',') {
                    let pair = pair.trim();
                    if pair.is_empty() {
                        continue;
                    }
                    let (mname, w_str) = pair
                        .split_once(':')
                        .ok_or_else(|| format!("invalid --morph pair '{pair}' (expected name:weight)"))?;
                    let w: f32 = w_str
                        .trim()
                        .parse()
                        .map_err(|_| format!("invalid --morph weight '{w_str}'"))?;
                    cli_morphs.push((mname.trim().to_string(), w));
                }
            }
            "--hud" => {
                i += 1;
                let v = args.get(i).ok_or("--hud requires 0|1")?;
                hud_override = Some(v != "0" && !v.eq_ignore_ascii_case("false"));
            }
            a if !a.starts_with('-') && input.is_none() => input = Some(PathBuf::from(a)),
            other => return Err(format!("unknown mesh arg '{other}'")),
        }
        i += 1;
    }

    let in_path = input.ok_or("mesh requires <file.mdl>")?;
    let (mut parsed, palette, _pal_path) =
        load_and_compile_model_with_overrides(&in_path, pal_override.as_deref(), &cli_overrides)?;

    for (mname, w) in &cli_morphs {
        if let Some(m) = parsed.mesh.morphs.iter_mut().find(|m| m.name == *mname) {
            m.default_weight = *w;
        } else {
            let avail: Vec<&str> = parsed.mesh.morphs.iter().map(|m| m.name.as_str()).collect();
            return Err(format!(
                "unknown morph '{mname}' (available: {})",
                if avail.is_empty() { "none".to_string() } else { avail.join(", ") }
            ));
        }
    }

    if let Some(ref fid) = focus_id {
        if let Some(&bb) = parsed.mesh.part_bounds.get(fid) {
            opts.focus_bounds = Some(bb);
        } else {
            let mut tags: Vec<&str> = parsed.mesh.part_bounds.keys().map(|s| s.as_str()).collect();
            tags.sort();
            return Err(format!(
                "unknown focus tag '@{fid}' (available: {})",
                tags.join(", ")
            ));
        }
    }

    let mut export_info = String::new();
    let mut draws = if parsed.mesh.tris.iter().any(|t| t.is_translucent) { 2 } else { 1 };
    if let Some(ref out_p) = out_model {
        ensure_parent(out_p)?;
        let is_gltf = out_p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("gltf"))
            .unwrap_or(false);
        if is_gltf {
            let (json, stats) = export_gltf_json(&parsed.mesh, &palette, &parsed.name, ext_pal.as_deref())?;
            let rep = validate_gltf_json_str(&json)?;
            if !rep.is_ok() {
                return Err(format!("glTF validation failed: {}", rep.errors.join("; ")));
            }
            draws = stats.draw_calls;
            std::fs::write(out_p, json.as_bytes()).map_err(|e| format!("write gltf: {e}"))?;
            export_info = format!(" gltf={:.1}KB valid=PASS", json.len() as f64 / 1024.0);
        } else {
            let (glb, stats) = export_glb(&parsed.mesh, &palette, &parsed.name, ext_pal.as_deref())?;
            let rep = validate_glb_bytes(&glb)?;
            if !rep.is_ok() {
                return Err(format!("GLB validation failed: {}", rep.errors.join("; ")));
            }
            draws = stats.draw_calls;
            std::fs::write(out_p, &glb).map_err(|e| format!("write glb: {e}"))?;
            export_info = format!(" glb={:.1}KB valid=PASS", glb.len() as f64 / 1024.0);
        }
    }

    if let Some(ref rp) = render_out {
        ensure_parent(rp)?;
        let mut ropts = opts.clone();
        ropts.hud = hud_override.unwrap_or(false);
        let png = render_to_png(&parsed.mesh, &palette, &ropts);
        std::fs::write(rp, png).map_err(|e| format!("write render png: {e}"))?;
    }
    if let Some(ref tp) = turn_out {
        ensure_parent(tp)?;
        let mut topts = opts.clone();
        topts.hud = hud_override.unwrap_or(true);
        if topts.width == 640 && topts.height == 480 {
            topts.width = 400;
            topts.height = 300;
        }
        let png = render_turntable_png(&parsed.mesh, &palette, &topts);
        std::fs::write(tp, png).map_err(|e| format!("write turntable png: {e}"))?;
    }

    if paths_out.is_some() || sheet_out.is_some() {
        if parsed.mesh.actions.is_empty() {
            return Err("no `anim`/`action` blocks defined in model (required for --paths / --anim-sheet)".into());
        }
        let act = if let Some(ref an) = anim_name {
            parsed
                .mesh
                .actions
                .iter()
                .find(|a| a.name == *an)
                .ok_or_else(|| {
                    let avail: Vec<&str> = parsed.mesh.actions.iter().map(|a| a.name.as_str()).collect();
                    format!("unknown anim '{an}' (available: {})", avail.join(", "))
                })?
        } else {
            &parsed.mesh.actions[0]
        };

        if let Some(ref pp) = paths_out {
            ensure_parent(pp)?;
            let mut popts = opts.clone();
            if popts.width == 640 && popts.height == 480 {
                popts.width = 400;
                popts.height = 300;
            }
            let png = render_anim_paths_png(&parsed.mesh, &palette, act, &track_nodes, &popts);
            std::fs::write(pp, png).map_err(|e| format!("write motion-paths png: {e}"))?;
        }
        if let Some(ref sp) = sheet_out {
            ensure_parent(sp)?;
            let png = render_anim_sheet_png(&parsed.mesh, &palette, act, sheet_frames, &opts);
            std::fs::write(sp, png).map_err(|e| format!("write anim-sheet png: {e}"))?;
        }
    }

    let bb = parsed.mesh.compute_bounds();
    let sz = if bb.is_valid() { bb.size() } else { dullahan_artisan::common::math::Vec3::ZERO };
    let total_tris = parsed.mesh.total_visual_tris();
    let total_nodes = parsed.mesh.total_node_count();
    let node_info = if total_nodes == 0 {
        String::new()
    } else {
        format!(" nodes={total_nodes}")
    };
    let bone_info = if parsed.mesh.bones.is_empty() {
        String::new()
    } else {
        format!(" bones={}", parsed.mesh.bones.len())
    };
    let morph_info = if parsed.mesh.morphs.is_empty() {
        String::new()
    } else {
        format!(" morphs={}", parsed.mesh.morphs.len())
    };
    let anim_info = if parsed.mesh.actions.is_empty() {
        String::new()
    } else {
        format!(" anims={}", parsed.mesh.actions.len())
    };
    let col_info = if parsed.mesh.colliders.is_empty() {
        String::new()
    } else {
        format!(" cols={}", parsed.mesh.colliders.len())
    };
    println!(
        "OK mesh {} tris={} verts={} draws={} bbox=[{:.2}x{:.2}x{:.2}] y=[{:.2}..{:.2}] parts={}{}{}{}{}{}{}",
        parsed.name,
        total_tris,
        total_tris * 3,
        draws,
        sz.x,
        sz.y,
        sz.z,
        if bb.is_valid() { bb.min.y } else { 0.0 },
        if bb.is_valid() { bb.max.y } else { 0.0 },
        parsed.part_count,
        node_info,
        bone_info,
        morph_info,
        anim_info,
        col_info,
        export_info
    );
    Ok(())
}

fn cmd_sfx(args: &[String]) -> Result<(), String> {
    let mut input: Option<PathBuf> = None;
    let mut kit_override: Option<PathBuf> = None;
    let mut out_wav: Option<PathBuf> = None;
    let mut variations: usize = 0;
    let mut plot_out: Option<PathBuf> = None;
    let mut plot_w: u32 = 800;
    let mut plot_h: u32 = 520;
    let mut seed_override: Option<u64> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            "-k" | "--kit" => {
                i += 1;
                kit_override = Some(PathBuf::from(args.get(i).ok_or("-k requires kit path")?));
            }
            "-o" | "--out" => {
                i += 1;
                out_wav = Some(PathBuf::from(args.get(i).ok_or("-o requires wav path")?));
            }
            "--var" | "--variations" => {
                i += 1;
                variations = args.get(i).ok_or("--var requires count")?.parse().map_err(|_| "bad --var")?;
            }
            "--plot" => {
                i += 1;
                plot_out = Some(PathBuf::from(args.get(i).ok_or("--plot requires png path")?));
            }
            "--size" => {
                i += 1;
                let (w, h) = parse_size(args.get(i).ok_or("--size requires WxH")?)?;
                plot_w = w;
                plot_h = h;
            }
            "--seed" => {
                i += 1;
                seed_override = Some(args.get(i).ok_or("--seed requires integer")?.parse().map_err(|_| "bad --seed")?);
            }
            a if !a.starts_with('-') && input.is_none() => input = Some(PathBuf::from(a)),
            other => return Err(format!("unknown sfx arg '{other}'")),
        }
        i += 1;
    }

    let in_path = input.ok_or("sfx requires <file.sfx>")?;
    let doc = load_sound_file(&in_path, kit_override.as_deref())?;

    let count = if variations > 0 { variations } else { 1 };
    let mut primary_buf = None;
    let mut primary_metrics = None;

    for v in 0..count {
        let var_idx = if variations > 0 { v + 1 } else { 0 };
        let buf = render_sound(&doc, var_idx, seed_override)?;
        let metrics = analyze_wav(&buf, doc.loop_mode);
        if !metrics.is_ok() {
            return Err(format!("audio validation failed: {}", metrics.errors.join("; ")));
        }

        if let Some(ref base_out) = out_wav {
            let target = if variations > 0 {
                let stem = base_out.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
                let ext = base_out.extension().and_then(|s| s.to_str()).unwrap_or("wav");
                let parent = base_out.parent().unwrap_or_else(|| Path::new(""));
                parent.join(format!("{stem}_{:02}.{ext}", v + 1))
            } else {
                base_out.clone()
            };
            write_pcm16(&target, &buf)?;
        }

        if v == 0 {
            primary_buf = Some(buf);
            primary_metrics = Some(metrics);
        }
    }

    let buf = primary_buf.unwrap();
    let m = primary_metrics.unwrap();

    if let Some(ref pp) = plot_out {
        ensure_parent(pp)?;
        let png = render_diagnostic_png(&buf, &m, &doc.name, doc.loop_mode, plot_w, plot_h);
        std::fs::write(pp, png).map_err(|e| format!("write plot png: {e}"))?;
    }

    let seam_status = if !doc.loop_mode {
        "n/a".to_string()
    } else if m.loop_seam_delta <= 0.03 {
        format!("{:.4}(PASS)", m.loop_seam_delta)
    } else if m.loop_seam_delta <= 0.08 {
        format!("{:.4}(OK)", m.loop_seam_delta)
    } else {
        format!("{:.4}(WARN)", m.loop_seam_delta)
    };
    let var_tag = if variations > 0 { format!(" vars={variations}") } else { String::new() };
    println!(
        "OK sfx {} dur={:.2}s sr={} ch={} peak={:.1}dB rms={:.1}dB dc={:.3} clip={} seam={} bands=[sub:{:.0} bas:{:.0} lmd:{:.0} mid:{:.0} hmd:{:.0} air:{:.0}] cent={:.0}Hz{}",
        doc.name,
        m.duration_sec,
        m.sample_rate,
        m.channels,
        m.peak_db,
        m.rms_db,
        m.dc_offset,
        m.clipped_samples,
        seam_status,
        m.bands_db[0],
        m.bands_db[1],
        m.bands_db[2],
        m.bands_db[3],
        m.bands_db[4],
        m.bands_db[5],
        m.spectral_centroid_hz,
        var_tag
    );
    if !m.warnings.is_empty() {
        eprintln!("WARN {}", m.warnings.join("; "));
    }
    Ok(())
}

fn cmd_svg(args: &[String]) -> Result<(), String> {
    let mut input: Option<PathBuf> = None;
    let mut out_png: Option<PathBuf> = None;
    let mut preview_png: Option<PathBuf> = None;
    let mut size_override: Option<(u32, u32)> = None;
    let mut scale: f64 = 1.0;
    let mut bg: Option<[u8; 4]> = None;
    let mut bg_checker = false;
    let mut grid = false;
    let mut hud_override: Option<bool> = None;
    let mut bleed = true;
    let mut nine_slice: Option<[f64; 4]> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            "-o" | "--out" => {
                i += 1;
                out_png = Some(PathBuf::from(args.get(i).ok_or("-o requires png path")?));
            }
            "--preview" => {
                i += 1;
                preview_png = Some(PathBuf::from(args.get(i).ok_or("--preview requires png path")?));
            }
            "--size" => {
                i += 1;
                size_override = Some(parse_size(args.get(i).ok_or("--size requires WxH")?)?);
            }
            "--scale" => {
                i += 1;
                scale = args
                    .get(i)
                    .ok_or("--scale requires positive factor")?
                    .parse::<f64>()
                    .map_err(|_| "bad --scale")?;
                if !scale.is_finite() || scale <= 0.0 {
                    return Err("--scale must be > 0".into());
                }
            }
            "--bg" => {
                i += 1;
                let v = args.get(i).ok_or("--bg requires #hex or 'checker'")?;
                if v.eq_ignore_ascii_case("checker") {
                    bg_checker = true;
                    bg = None;
                } else {
                    bg = Some(parse_hex_rgba(v)?);
                    bg_checker = false;
                }
            }
            "--9slice" => {
                i += 1;
                let v = args.get(i).ok_or("--9slice requires T,R,B,L or N")?;
                let parts: Result<Vec<f64>, _> = v.split(',').map(|s| s.trim().parse::<f64>()).collect();
                let nums = parts.map_err(|_| format!("invalid --9slice '{v}'"))?;
                nine_slice = match nums.len() {
                    1 => Some([nums[0], nums[0], nums[0], nums[0]]),
                    2 => Some([nums[0], nums[1], nums[0], nums[1]]),
                    4 => Some([nums[0], nums[1], nums[2], nums[3]]),
                    _ => return Err("--9slice expects N, V,H, or T,R,B,L".into()),
                };
            }
            "--grid" => grid = true,
            "--hud" => {
                i += 1;
                let v = args.get(i).ok_or("--hud requires 0|1")?;
                hud_override = Some(v != "0" && !v.eq_ignore_ascii_case("false"));
            }
            "--bleed" => {
                i += 1;
                let v = args.get(i).ok_or("--bleed requires 0|1")?;
                bleed = v != "0" && !v.eq_ignore_ascii_case("false");
            }
            a if !a.starts_with('-') && input.is_none() => input = Some(PathBuf::from(a)),
            other => return Err(format!("unknown svg arg '{other}'")),
        }
        i += 1;
    }

    let in_path = input.ok_or("svg requires <file.svg>")?;
    let doc = load_svg_file(&in_path)?;
    if !doc.is_ok() {
        return Err(format!("SVG validation failed: {}", doc.errors.join("; ")));
    }

    let mut opts = SvgRenderOptions::for_doc(&doc);
    if let Some((w, h)) = size_override {
        opts.width = w.max(1);
        opts.height = h.max(1);
    } else {
        opts.width = (doc.width * scale).round().max(1.0) as u32;
        opts.height = (doc.height * scale).round().max(1.0) as u32;
    }
    opts.bg = bg;
    opts.checker = bg_checker;
    opts.bleed = bleed;

    let mut export_info = String::new();
    if let Some(ref op) = out_png {
        ensure_parent(op)?;
        let mut out_opts = opts.clone();
        out_opts.hud = hud_override.unwrap_or(false);
        out_opts.grid = false;
        out_opts.nine_slice = None;
        let png = render_svg_png(&doc, &out_opts);
        std::fs::write(op, &png).map_err(|e| format!("write svg png: {e}"))?;
        export_info = format!(" png={:.1}KB", png.len() as f64 / 1024.0);
    }

    if let Some(ref pp) = preview_png {
        ensure_parent(pp)?;
        let mut prev_opts = opts.clone();
        prev_opts.checker = bg.is_none();
        prev_opts.hud = hud_override.unwrap_or(true);
        prev_opts.grid = grid;
        prev_opts.nine_slice = nine_slice;
        let png = render_svg_png(&doc, &prev_opts);
        std::fs::write(pp, &png).map_err(|e| format!("write svg preview png: {e}"))?;
    }

    let (vx, vy, vw, vh) = doc.view_box;
    let bb = doc.stats.content_bbox;
    let bbox_str = if bb.is_valid() {
        format!("[{:.1},{:.1}..{:.1},{:.1}]", bb.min_x, bb.min_y, bb.max_x, bb.max_y)
    } else {
        "[empty]".to_string()
    };
    println!(
        "OK svg {} vb=[{:.0},{:.0} {:.0}x{:.0}] out={}x{} shapes={} grads={} clips={} filters={} bbox={}{} valid=PASS",
        doc.name,
        vx,
        vy,
        vw,
        vh,
        opts.width,
        opts.height,
        doc.stats.shapes,
        doc.stats.gradients,
        doc.stats.clip_paths,
        doc.stats.filters,
        bbox_str,
        export_info
    );
    if !doc.warnings.is_empty() {
        eprintln!("WARN {}", doc.warnings.join("; "));
    }
    Ok(())
}

fn cmd_terrain(args: &[String]) -> Result<(), String> {
    let mut input_ter: Option<PathBuf> = None;
    let mut from_hm: Option<PathBuf> = None;
    let mut weightmap: Option<PathBuf> = None;
    let mut pal_override: Option<PathBuf> = None;
    let mut out_prefix: Option<String> = None;
    let mut preview_png: Option<PathBuf> = None;
    let mut mesh_out: Option<PathBuf> = None;
    let mut render_out: Option<PathBuf> = None;
    let mut turn_out: Option<PathBuf> = None;
    let mut ext_pal: Option<String> = None;
    let mut seed_override: Option<u64> = None;
    let mut world_override: Option<[f32; 3]> = None;
    let mut mats_override: Option<[String; 4]> = None;
    let mut mesh_opts = TerrainMeshOptions::default();
    let mut render_opts = RenderOptions::default();
    render_opts.yaw_deg = 38.0;
    render_opts.pitch_deg = 28.0;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            "--from-heightmap" | "--heightmap" => {
                i += 1;
                from_hm = Some(PathBuf::from(args.get(i).ok_or("--from-heightmap requires png path")?));
            }
            "--weightmap" | "--splat" => {
                i += 1;
                weightmap = Some(PathBuf::from(args.get(i).ok_or("--weightmap requires png path")?));
            }
            "-p" | "--pal" | "--palette" => {
                i += 1;
                pal_override = Some(PathBuf::from(args.get(i).ok_or("-p requires palette path")?));
            }
            "-o" | "--out" => {
                i += 1;
                out_prefix = Some(args.get(i).ok_or("-o requires output prefix")?.clone());
            }
            "--preview" => {
                i += 1;
                preview_png = Some(PathBuf::from(args.get(i).ok_or("--preview requires png path")?));
            }
            "--mesh" => {
                i += 1;
                mesh_out = Some(PathBuf::from(args.get(i).ok_or("--mesh requires .glb/.gltf path")?));
            }
            "--ext-pal" => {
                i += 1;
                ext_pal = Some(args.get(i).ok_or("--ext-pal requires URI prefix")?.clone());
            }
            "--render" => {
                i += 1;
                render_out = Some(PathBuf::from(args.get(i).ok_or("--render requires png path")?));
            }
            "--turn" | "--turntable" => {
                i += 1;
                turn_out = Some(PathBuf::from(args.get(i).ok_or("--turn requires png path")?));
            }
            "--grid" => {
                i += 1;
                mesh_opts.grid = args.get(i).ok_or("--grid requires N")?.parse().map_err(|_| "bad --grid")?;
            }
            "--tris" => {
                i += 1;
                mesh_opts.target_tris = Some(args.get(i).ok_or("--tris requires N")?.parse().map_err(|_| "bad --tris")?);
            }
            "--skirt" => {
                i += 1;
                let v = args.get(i).ok_or("--skirt requires 0|1")?;
                mesh_opts.skirt = v != "0" && !v.eq_ignore_ascii_case("false");
            }
            "--smooth" => {
                i += 1;
                let v = args.get(i).ok_or("--smooth requires 0|1")?;
                mesh_opts.smooth = v != "0" && !v.eq_ignore_ascii_case("false");
            }
            "--col" => {
                i += 1;
                mesh_opts.collider_name = Some(args.get(i).ok_or("--col requires node name")?.clone());
            }
            "--world" => {
                i += 1;
                let s = args.get(i).ok_or("--world requires W,H,D")?;
                let parts: Result<Vec<f32>, _> = s.split(',').map(|x| x.trim().parse::<f32>()).collect();
                let nums = parts.map_err(|_| format!("invalid --world '{s}'"))?;
                if nums.len() != 3 {
                    return Err("--world expects W,H,D".into());
                }
                world_override = Some([nums[0].max(1.0), nums[1].max(0.1), nums[2].max(1.0)]);
            }
            "--mats" => {
                i += 1;
                let s = args.get(i).ok_or("--mats requires 4 comma-separated swatch names")?;
                let parts: Vec<String> = s.split(',').map(|x| x.trim().to_string()).collect();
                if parts.len() != 4 {
                    return Err("--mats expects 4 swatch names (r,g,b,a)".into());
                }
                mats_override = Some([parts[0].clone(), parts[1].clone(), parts[2].clone(), parts[3].clone()]);
            }
            "--seed" => {
                i += 1;
                seed_override = Some(args.get(i).ok_or("--seed requires integer")?.parse().map_err(|_| "bad --seed")?);
            }
            "--size" => {
                i += 1;
                let (w, h) = parse_size(args.get(i).ok_or("--size requires WxH")?)?;
                render_opts.width = w;
                render_opts.height = h;
            }
            "--yaw" => {
                i += 1;
                render_opts.yaw_deg = args.get(i).ok_or("--yaw requires deg")?.parse().map_err(|_| "bad --yaw")?;
            }
            "--pitch" => {
                i += 1;
                render_opts.pitch_deg = args.get(i).ok_or("--pitch requires deg")?.parse().map_err(|_| "bad --pitch")?;
            }
            "--wire" | "--wireframe" => render_opts.wireframe = true,
            a if !a.starts_with('-') && input_ter.is_none() => input_ter = Some(PathBuf::from(a)),
            other => return Err(format!("unknown terrain arg '{other}'")),
        }
        i += 1;
    }

    let (mut terrain, pal_path) = if let Some(ref hm_path) = from_hm {
        let h_bytes = std::fs::read(hm_path)
            .map_err(|e| format!("failed to read heightmap '{}': {e}", hm_path.display()))?;
        let w_bytes = match weightmap {
            Some(ref wp) => Some(
                std::fs::read(wp)
                    .map_err(|e| format!("failed to read weightmap '{}': {e}", wp.display()))?,
            ),
            None => None,
        };
        let stem = hm_path.file_stem().and_then(|s| s.to_str()).unwrap_or("terrain");
        let world = world_override.unwrap_or([100.0, 24.0, 100.0]);
        let mats = mats_override.clone().unwrap_or_else(|| {
            [
                "sand_warm".to_string(),
                "grass_mid".to_string(),
                "stone_mid".to_string(),
                "roof_cream".to_string(),
            ]
        });
        let td = build_from_heightmap_png(&h_bytes, w_bytes.as_deref(), world, stem, mats)?;
        (td, pal_override)
    } else {
        let ter_path = input_ter.ok_or("terrain requires <file.ter> or --from-heightmap <h.png>")?;
        let doc = load_terrain_file(&ter_path)?;
        let td = simulate_terrain(&doc, seed_override)?;
        let resolved_pal = pal_override.or_else(|| {
            doc.palette_rel
                .as_ref()
                .map(|rel| ter_path.parent().unwrap_or_else(|| Path::new(".")).join(rel))
        });
        (td, resolved_pal)
    };

    if let Some(w) = world_override {
        terrain.world = w;
    }
    if let Some(m) = mats_override {
        terrain.channel_mats = m;
    }

    let palette = match pal_path {
        Some(ref pp) => Some(Palette::load_file(pp)?),
        None => None,
    };

    let mut maps_info = String::new();
    if let Some(ref prefix) = out_prefix {
        let hm_p = PathBuf::from(format!("{prefix}_height16.png"));
        let sp_p = PathBuf::from(format!("{prefix}_splat.png"));
        let nm_p = PathBuf::from(format!("{prefix}_normal.png"));
        ensure_parent(&hm_p)?;
        let hm_b = terrain.encode_height16_png();
        let sp_b = terrain.encode_splat_rgba_png();
        let nm_b = terrain.encode_normal_rgb_png();
        std::fs::write(&hm_p, &hm_b).map_err(|e| format!("write height16: {e}"))?;
        std::fs::write(&sp_p, &sp_b).map_err(|e| format!("write splat: {e}"))?;
        std::fs::write(&nm_p, &nm_b).map_err(|e| format!("write normal: {e}"))?;
        let kb = (hm_b.len() + sp_b.len() + nm_b.len()) as f64 / 1024.0;
        maps_info = format!(" maps={:.1}KB->{prefix}_{{height16,splat,normal}}.png", kb);
    }

    if let Some(ref pp) = preview_png {
        ensure_parent(pp)?;
        let png = render_terrain_preview_png(&terrain, palette.as_ref());
        std::fs::write(pp, png).map_err(|e| format!("write terrain preview: {e}"))?;
    }

    let mut mesh_info = String::new();
    if mesh_out.is_some() || render_out.is_some() || turn_out.is_some() {
        let pal = palette.as_ref().ok_or(
            "terrain --mesh / --render requires a palette via 'pal <file.pal>' or -p <file.pal>",
        )?;
        let mesh = build_terrain_mesh(&terrain, pal, &mesh_opts);
        if let Some(ref mp) = mesh_out {
            ensure_parent(mp)?;
            let is_gltf = mp
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("gltf"))
                .unwrap_or(false);
            if is_gltf {
                let (json, stats) = export_gltf_json(&mesh, pal, &terrain.name, ext_pal.as_deref())?;
                let rep = validate_gltf_json_str(&json)?;
                if !rep.is_ok() {
                    return Err(format!("terrain glTF invalid: {}", rep.errors.join("; ")));
                }
                std::fs::write(mp, json.as_bytes()).map_err(|e| format!("write gltf: {e}"))?;
                mesh_info = format!(" tris={} draws={} gltf={:.1}KB valid=PASS", mesh.tris.len(), stats.draw_calls, json.len() as f64 / 1024.0);
            } else {
                let (glb, stats) = export_glb(&mesh, pal, &terrain.name, ext_pal.as_deref())?;
                let rep = validate_glb_bytes(&glb)?;
                if !rep.is_ok() {
                    return Err(format!("terrain GLB invalid: {}", rep.errors.join("; ")));
                }
                std::fs::write(mp, &glb).map_err(|e| format!("write glb: {e}"))?;
                mesh_info = format!(" tris={} draws={} glb={:.1}KB valid=PASS", mesh.tris.len(), stats.draw_calls, glb.len() as f64 / 1024.0);
            }
        }
        if let Some(ref rp) = render_out {
            ensure_parent(rp)?;
            let png = render_to_png(&mesh, pal, &render_opts);
            std::fs::write(rp, png).map_err(|e| format!("write terrain render: {e}"))?;
        }
        if let Some(ref tp) = turn_out {
            ensure_parent(tp)?;
            let mut topts = render_opts.clone();
            topts.hud = true;
            if topts.width == 640 && topts.height == 480 {
                topts.width = 400;
                topts.height = 300;
            }
            let png = render_turntable_png(&mesh, pal, &topts);
            std::fs::write(tp, png).map_err(|e| format!("write terrain turnaround: {e}"))?;
        }
    }

    let (lo, hi) = terrain.min_max_height();
    println!(
        "OK terrain {} {}x{} world=[{:.0}x{:.0}x{:.0}] h=[{:.2}..{:.2}] max_slope={:.0}deg drops={}{}{}",
        terrain.name,
        terrain.width,
        terrain.height,
        terrain.world[0],
        terrain.world[1],
        terrain.world[2],
        lo,
        hi,
        terrain.max_slope_deg(),
        terrain.drops_simulated,
        maps_info,
        mesh_info
    );
    Ok(())
}

fn cmd_validate(args: &[String]) -> Result<(), String> {
    let mut path: Option<PathBuf> = None;
    let mut expect_loop = false;
    for a in args {
        match a.as_str() {
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            "--loop" => expect_loop = true,
            s if !s.starts_with('-') && path.is_none() => path = Some(PathBuf::from(s)),
            other => return Err(format!("unknown validate arg '{other}'")),
        }
    }
    let p = path.ok_or("validate requires <file.glb|file.gltf|file.wav|file.svg>")?;
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "wav" {
        let buf = read_wav(&p)?;
        let m = analyze_wav(&buf, expect_loop);
        if !m.is_ok() {
            return Err(format!("WAV invalid: {}", m.errors.join("; ")));
        }
        println!(
            "OK validate WAV dur={:.2}s sr={} ch={} peak={:.1}dB rms={:.1}dB dc={:.3} clip={} seam={:.4}",
            m.duration_sec, m.sample_rate, m.channels, m.peak_db, m.rms_db, m.dc_offset, m.clipped_samples, m.loop_seam_delta
        );
        if !m.warnings.is_empty() {
            eprintln!("WARN {}", m.warnings.join("; "));
        }
    } else if ext == "svg" {
        let doc = load_svg_file(&p)?;
        if !doc.is_ok() {
            return Err(format!("SVG invalid: {}", doc.errors.join("; ")));
        }
        println!(
            "OK validate SVG vb={}x{} shapes={} grads={} clips={} filters={}",
            doc.view_box.2.round() as i32,
            doc.view_box.3.round() as i32,
            doc.stats.shapes,
            doc.stats.gradients,
            doc.stats.clip_paths,
            doc.stats.filters
        );
        if !doc.warnings.is_empty() {
            eprintln!("WARN {}", doc.warnings.join("; "));
        }
    } else {
        let rep = validate_gltf_file(&p)?;
        if !rep.is_ok() {
            return Err(format!("{} invalid: {}", rep.format, rep.errors.join("; ")));
        }
        println!(
            "OK validate {} prims={} tris={} verts={} pngs={}",
            rep.format, rep.primitives, rep.triangles, rep.vertices, rep.images_checked
        );
    }
    Ok(())
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (ws, hs) = s
        .split_once('x')
        .or_else(|| s.split_once('X'))
        .ok_or_else(|| format!("invalid size '{s}' (expected WxH)"))?;
    let w: u32 = ws.parse().map_err(|_| format!("bad width in '{s}'"))?;
    let h: u32 = hs.parse().map_err(|_| format!("bad height in '{s}'"))?;
    Ok((w, h))
}

fn print_usage() {
    println!(
        r#"Dullahan Artisan (`artisan`) — headless LLM-native 3D PBR & procedural audio compiler

USAGE:
  artisan pal <file.pal> [-o <prefix>] [--sheet <png>]
  artisan mesh <file.mdl> [-p <pal>] [-o <out.glb|.gltf>] [--ext-pal <prefix>]
               [--ov old:new[,old2:new2]] [--morph name:w[,...]] [--render <png>] [--turn <png>] [--focus <id>]
               [--anim <name>] [--paths <png>] [--track n1,n2] [--anim-sheet <png>] [--frames N]
               [--size WxH] [--yaw D] [--pitch D] [--fov D] [--fill F]
               [--bg #hex] [--ortho] [--wire] [--colliders] [--bones] [--hud 0|1]
  artisan sfx <file.sfx> [-k <kit>] [-o <out.wav>] [--var N]
              [--plot <png>] [--size WxH] [--seed S]
  artisan svg <file.svg> [-o <out.png>] [--preview <qa.png>]
              [--size WxH] [--scale S] [--bg #hex|checker]
              [--9slice T,R,B,L] [--grid] [--hud 0|1] [--bleed 0|1]
  artisan terrain <file.ter> [-p <pal>] [-o <prefix>] [--preview <diag.png>]
                  [--mesh <out.glb|.gltf>] [--render <3d.png>] [--turn <turn.png>]
                  [--grid N] [--tris N] [--skirt 0|1] [--smooth 0|1] [--col <node>] [--seed S]
  artisan terrain --from-heightmap <h.png> [--weightmap <w.png>] [-p <pal>] [--world W,H,D]
                  [--mesh <out.glb|.gltf>] [--render <3d.png>] [--preview <diag.png>] [--grid N] [--tris N]
  artisan validate <file.glb|.gltf|.wav|.svg> [--loop]
  artisan spec [pal|mesh|sfx|svg|terrain]"#
    );
}

fn cmd_spec(args: &[String]) -> Result<(), String> {
    let topic = args.first().map(|s| s.as_str()).unwrap_or("all");
    if matches!(topic, "-h" | "--help") {
        print_usage();
        return Ok(());
    }
    if topic == "all" || topic == "pal" {
        println!(
            r#"=== PAL (.pal) ===
Header: grid <C>x<R>@<tilePx> [bg=#RRGGBB]   (or shorthand: grid 8@16 / 8x16)
Swatch: <name> <#RGB|#RRGGBB|#RRGGBBAA> [rough=.88] [metal=0] [emissive=-|#hex|1] [ao=1] [at=c,r]
Output: <prefix>_basecolor.png (RGBA sRGB), <prefix>_orm.png (R=AO G=Rough B=Metal), <prefix>_emissive.png (RGB)
Note:   #RRGGBBAA with alpha<FF auto-routes triangles into an optional 2nd BLEND draw call.
"#
        );
    }
    if topic == "all" || topic == "mesh" {
        println!(
            r#"=== MESH (.mdl) — Right-handed Y-up (+X right, +Y up, +Z front, Y=0 ground), CCW outward ===
Directives: pal <rel.pal> | inc <rel.mdl> (or use <rel.mdl>) | $var = <expr> (+ - * / ^ sin cos tan sind cosd atan2 atan2d sqrt abs min max pi)
Blocks:     def <name> ... end | grp [@id] [mat] [x,y,z] [flags...] ... end
ChildNodes: node <name> [x,y,z] [r=rx,ry,rz] [s=sx,sy,sz] [m=...] ... end  (nestable glTF child mesh w/ local pivot!)
Skeleton:   skel ... end  (alias: rig; exports glTF 2.0 `skins` w/ auto smooth skinning + optional rigid `bone=<name>`)
            bone <name> <parent|-> <head_x,y,z> <tail_x,y,z> [r=rad] [falloff=3] [m=x]
Morphs:     morph <name> [w=0] ... end  (alias: shape; glTF 2.0 morph targets + mesh.extras.targetNames)
            scale   [@tag] <sx,sy,sz> [at=x,y,z] [r=rad] [m=x]
            move    [@tag] <dx,dy,dz> [at=x,y,z] [r=rad] [m=x]  (alias: shift)
            pull    [@tag] <x,y,z> r=<rad> d=<dx,dy,dz> [m=x]   (soft spherical pull)
            rot     [@tag] <rx,ry,rz> [at=x,y,z] [r=rad] [m=x]
            inflate [@tag] [x,y,z] [r=rad] amt=<dist> [m=x]     (alias: push, bulge)
Actions:    anim <name> [dur=1] [fps=24] [loop=0|1] ... end   (alias: action; glTF 2.0 animations + extras.events)
            key <sec|Nf> <node|bone> [p=x,y,z] [dp=dx,dy,dz] [r=rx,ry,rz] [s=u|sx,sy,sz] [i=step|lin|ease|easein|easeout|cubic]
            ik  <sec|Nf> <effector_bone> p=x,y,z [pole=px,py,pz] [chain=2] [tip=0|1] [i=...]  (2-bone analytical & FABRIK IK!)
            mw  <sec|Nf> <morph_name> <weight> [i=step|lin|ease|easein|easeout|cubic]         (animates glTF morph weights!)
            ev  <sec|Nf> <event_name> [data...]               (e.g. ev 0.00 sfx:footstep_l)
Colliders:  col <node_name> [x,y,z] [flags...] ... end   (exports separate child glTF node; mat optional inside)
Booleans:   csg [@id] [mat] [x,y,z] [flags...] ... end   (exact BSP mesh ops: add/sub/int <prim|use|grp>)
Metaballs:  sdf [@id] [mat] [x,y,z] [res=28] [sm=1] ... end   (Surface-Nets organic sculpting)
            SDF prims: sph <x,y,z> <r> | ell <x,y,z> <rx,ry,rz> | cap <x0,y0,z0> <x1,y1,z1> <r0[,r1]>
                       box <x,y,z> <sx,sy,sz> [rnd=r] | cyl <x,y,z> <r,h> [rnd=r] | tor <x,y,z> <R,r>
            SDF ops:   [add|sub|int|paint] <prim> ... [k=0] [r=rx,ry,rz] [m=x|y|z|xz]
Instance:   use [@id] <name> [x,y,z] [flags...] [ov=old:new,...]
Materials:  <swatch_name> | cell:c,r | uv:u,v
Primitives (1 line each):
  box [@id] <mat> <x,y,z> <sx,sy,sz>          [t=tx,tz] [o=ox,oz] [bev=b] [fc=...]
  cyl [@id] <mat> <x,y,z> <r,h|rb,rt,h>       [seg=12] [cap=1] [bev=b] [fc=...]   (rt=0 -> cone)
  cap [@id] <mat> <x,y,z> <r,h|rb,rt,h>       [seg=10] [rings=4]
  wdg [@id] <mat> <x,y,z> <sx,sy,sz>          [fc=...]   (vertical back -Z, slope toward +Z)
  pyr [@id] <mat> <x,y,z> <sx,sy,sz>          [sides=4] [o=ox,oz] [fc=...]
  ico [@id] <mat> <x,y,z> <r>                 [sub=1] [jit=0] [seed=1] [mats=m1,m2,...]
  tor [@id] <mat> <x,y,z> <R,r>               [seg=16] [tseg=8]
  pri [@id] <mat> <x,y,z> <depth> <x:y,...>   [bev=b] [fc=...]   (2D polygon extrude along Z)
  lat [@id] <mat> <x,y,z> <r:y,...>           [seg=12] [cap=1] [fc=...]
  tub [@id] <mat> <x,y,z> <r|r0,r1> <x:y:z,.> [seg=8] [cap=1]
  lof [@id] <mat> <x,y,z> <x:y:z/rx:ry[/tw][/mat],...> [seg=10] [ph=0] [cap=1]
  ter [@id] <mat> <x,y,z> <sx,sz>             [sub=8,8] [jit=.08] [mnd=0] [pin=1] [thk=0] [seed=1] [mats=...] [fc=...]
  tri [@id] <mat> <x0:y0:z0> <x1:y1:z1> <x2:y2:z2> [ds=0|1] [off=.001] [fc=...]
  rib [@id] <mat> <x,y,z> <w0,w1,h[,curve]>   [seg=4] [ds=1] [off=.001] [fc=...]   (curved tapered blade/ribbon)
Universal flags:
  r=rx,ry,rz  s=u|sx,sy,sz  a=c|b|t|o (default c for centered, b for rib, o for pri/lat/tub/lof/ter)
  m=x|y|z|xz  sm=0|1|w (crease-aware smooth)  wn=1|2..12 (face-weighted normals for crisp 1-seg bevels!)
  bend=deg:along[:toward]  twist=deg:axis  rep=N:dx,dy,dz[:rx,ry,rz]  rad=N:R[:axis][:startDeg]
  fc=t:m,b:m,s:m,f:m,k:m,l:m,r:m,sl:m
CLI flags:  --ov old:new[,old2:new2] (repeatable atomic palette swatch swap)  --ext-pal <prefix>
"#
        );
    }
    if topic == "all" || topic == "sfx" {
        println!(
            r#"=== SFX (.sfx / .kit) — Left-to-right pipe DSP & sequencer ===
Header: sfx <name> [dur=1 | bpm=120 bars=2 bpb=4] [sr=44100] [ch=2] [loop=0|1] [xfade=.02] [curve=lin|eqp] [norm=-1] [seed=1]
Kit/Vars: use <rel.kit> | $var = <expr> | def <voice> ... end | grp [flags] [| fx...] ... end | bus | <fx...>
Generators (+ common flags: g= pan= t= tb= dur= env=a,d,s,r|t:v,... penv=v0>v1:dt[:exp|lin|log|sm]|t0:v0,t1:v1,... cenv=... lfo=tgt:hz:dep[:wav][:ph] cl=0|1 flt=m:fc:q):
  osc <sin|tri|saw|sqr|pls> <hz|note> [pwm=.5] [ph=0..1]
  ssaw <hz|note> [w=saw] [v=5] [det=12] [spr=.8]
  fm <hz|note> [mr=2] [mi=2.5] [md=sec] [ph=0..1]
  mod <hz|note> p=<r:amp[:dec],...> [abs=0|1]
  plk <hz|note> [damp=.992] [br=.65]
  noi <white|pink|brown|crackle> [dens=40] [st=1]
Composition:
  ref <voice> [note=C4|freq=440|semi=0] [t=|tb=] [dur=] [g=] [pan=] [| fx...]
  pat <voice> "<X.x.o.g.>" [step=.25] [swing=0] [rep=1] [tb=0] [g=] [pan=] [| fx...]
  seq <voice> "<C3:1 [E3,G3]:1 -:0.5 D3:.5@.7>" [gate=.9] [rep=1] [tr=0] [tb=0] [g=] [pan=] [| fx...]
  sct <voice> n=12 [win=0..1] [gap=.01] [pr=-3..3] [gr=.5..1] [pan=-.5..0.5] [seed=1] [| fx...]
Pipe FX (| ...):
  flt <lp|hp|bp|notch|bell> <fc> [q=.707] [db=0] [mix=1]
  drv <amt> [asym=0] [mix=1] | crush <bits> [rate=11025] [mix=1]
  cho [rate=.5] [depthMs=3] [mix=.4] [dly=12] | dly <sec|b=beats> [mix=.3] [fb=.35] [damp=4000] [pp=0|1]
  rev [room=.75] [mix=.3] [damp=.4] [pre=0] | cmp [thr=-12] [rat=3] [atk=.01] [rel=.12] [mk=0] | wid <0..2>"#
        );
    }
    if topic == "all" || topic == "svg" {
        println!(
            r##"=== SVG (.svg) — Zero-dependency Game-UI SVG validator & PNG rasterizer ===
Root:      <svg viewBox="x y w h" [width="w"] [height="h"]>
Shapes:    <rect rx ry> <circle> <ellipse> <line> <polyline> <polygon> <path d="M L H V C S Q T A Z">
Structure: <g> <defs> <use href="#id" x y> <clipPath id>
Paint:     fill, stroke, fill-opacity, stroke-opacity, opacity, fill-rule (nonzero|evenodd), style="..."
           #RGB/#RGBA/#RRGGBB/#RRGGBBAA, rgb(), rgba(), named colors, currentColor, url(#grad)
Stroke:    stroke-width, stroke-linecap (butt|round|square), stroke-linejoin (miter|round|bevel),
           stroke-miterlimit, stroke-dasharray, stroke-dashoffset
Gradients: <linearGradient x1 y1 x2 y2> <radialGradient cx cy r fx fy>
           gradientUnits (objectBoundingBox|userSpaceOnUse), gradientTransform, spreadMethod, href, <stop>
Filters:   <filter id> with <feGaussianBlur stdDeviation> (+ <feMerge> SourceGraphic) or <feDropShadow>
Transform: translate, scale, rotate(deg [cx cy]), skewX, skewY, matrix
Notes:     Render fonts/text in-engine for localization (<text> emits a warning and is skipped).
           -o emits transparent RGBA PNG with GPU alpha-bleed (--bleed 1); --preview adds checkerboard,
           viewBox frame, HUD telemetry, optional --grid and --9slice T,R,B,L guides."##
        );
    }
    if topic == "all" || topic == "terrain" {
        println!(
            r#"=== TERRAIN (.ter) — Eroded 16-bit heightmaps, biome weightmaps & low-poly decimated GLB ===
Header:    terrain <name> [size=256] [world=100,24,100] [seed=42] [sea=0]  |  pal <rel.pal>  |  $var = expr
Stamps:    (normalized u,v in [-0.5..0.5], 0,0 = center)
  base <sketch.png> [scale=1] [bias=0] [blur=0]
  island [r=.44] [falloff=2] [h=.28]
  peak <u,v> <r> <h> [sharp=1.4] [op=add|max|set]
  ridge <u0,v0> <u1,v1> <w> <h> [sharp=1.35] [op=max|add]  (or polyline u0:v0,u1:v1,...)
  plateau <u,v> <r> <h> [flat=.72] [blend=.95]   (buildable flat pad / mesa)
  valley <u0,v0> <u1,v1> <w> <depth> [flat=.28]  (carves river / pass / road)
  crater <u,v> <r> <depth> [rim=.18]  |  slope <dir_deg> <lo> <hi>  |  smooth [iters=2]  |  norm
Noise:     noise <fbm|ridge|billow|warp> [oct=5] [freq=3] [amp=.18] [lac=2.05] [gain=.5] [mask=all|high|low|slope]
Erosion:   erode [drops=45000] [inertia=.12] [cap=4] [dep=.25] [ero=.3] [evap=.02] [rad=2] [steps=48] [thermal=8] [talus=.012]
Splats:    splat <r|g|b|a> <mat_name> [h=0..1] [slope=0..90] [flow=0..1] [w=1] [sharp=10]
Outputs:   -o <prefix> -> _height16.png (16-bit lossless), _splat.png (RGBA), _normal.png (RGB)
           --preview -> 2x2 QA sheet (hillshade, elevation heatmap, flow/erosion, biome splatmap)
           --mesh out.glb [--grid 64] [--tris 2500] [--skirt 1] [--col terrain-colonly]"#
        );
    }
    Ok(())
}
