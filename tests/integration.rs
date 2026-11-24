use std::collections::HashMap;
use std::path::Path;

use dullahan_artisan::common::png::inspect_and_verify_png;
use dullahan_artisan::mesh::gltf::{export_glb, export_gltf_json};
use dullahan_artisan::mesh::palette::Palette;
use dullahan_artisan::mesh::parser::{
    compile_model, compile_model_with_dir, load_and_compile_model,
    load_and_compile_model_with_overrides,
};
use dullahan_artisan::mesh::render::{render_to_png, render_turntable_png, RenderOptions};
use dullahan_artisan::mesh::validator::{validate_glb_bytes, validate_gltf_json_str};
use dullahan_artisan::sfx::diagnostics::{analyze_wav, render_diagnostic_png};
use dullahan_artisan::sfx::engine::render_sound;
use dullahan_artisan::sfx::parser::{load_sound_file, parse_sound_doc};
use dullahan_artisan::sfx::wav::{decode_pcm16_bytes, encode_pcm16_bytes};
use dullahan_artisan::svg::parser::{load_svg_file, parse_svg};
use dullahan_artisan::svg::raster::{render_svg_png, SvgRenderOptions};
use dullahan_artisan::terrain::mesher::{build_terrain_mesh, TerrainMeshOptions};
use dullahan_artisan::terrain::parser::load_terrain_file;
use dullahan_artisan::terrain::preview::render_terrain_preview_png;
use dullahan_artisan::terrain::sim::{build_from_heightmap_png, simulate_terrain};

#[test]
fn palette_and_all_example_models_build_validate_render() {
    let pal = Palette::load_file(Path::new("examples/palette.pal")).unwrap();
    let sheet = pal.render_swatch_sheet_png();
    assert!(inspect_and_verify_png(&sheet).is_ok());

    for mdl in [
        "examples/models/offroad_4x4.mdl",
        "examples/models/modular_dungeon_kit.mdl",
        "examples/models/props_shotgun_medkit.mdl",
        "examples/models/fox_on_stones.mdl",
        "examples/models/workbench_tools.mdl",
    ] {
        let (parsed, p, _) = load_and_compile_model(Path::new(mdl), None).unwrap();
        assert!(!parsed.mesh.tris.is_empty(), "{mdl} produced 0 tris");

        let (glb, stats_glb) = export_glb(&parsed.mesh, &p, &parsed.name, None).unwrap();
        let rep_glb = validate_glb_bytes(&glb).unwrap();
        assert!(rep_glb.is_ok(), "{mdl} GLB errors: {:?}", rep_glb.errors);
        assert_eq!(rep_glb.images_checked, 3);

        let (gltf, stats_gltf) = export_gltf_json(&parsed.mesh, &p, &parsed.name, None).unwrap();
        let rep_gltf = validate_gltf_json_str(&gltf).unwrap();
        assert!(rep_gltf.is_ok(), "{mdl} glTF errors: {:?}", rep_gltf.errors);
        assert_eq!(stats_glb.draw_calls, stats_gltf.draw_calls);

        // External shared atlas mode
        let (gltf_ext, _) = export_gltf_json(&parsed.mesh, &p, &parsed.name, Some("../palette")).unwrap();
        let rep_ext = validate_gltf_json_str(&gltf_ext).unwrap();
        assert!(rep_ext.is_ok(), "{mdl} ext glTF errors: {:?}", rep_ext.errors);

        // Render quick hero + turntable PNGs and verify PNG integrity
        let mut opts = RenderOptions::default();
        opts.width = 240;
        opts.height = 180;
        let hero = render_to_png(&parsed.mesh, &p, &opts);
        assert!(inspect_and_verify_png(&hero).is_ok());
        let turn = render_turntable_png(&parsed.mesh, &p, &opts);
        assert!(inspect_and_verify_png(&turn).is_ok());
    }
}

#[test]
fn all_example_sfx_synthesize_validate_and_plot() {
    for (path, is_loop) in [
        ("examples/audio/footstep_gravel.sfx", false),
        ("examples/audio/boss_impact_sfx.sfx", false),
        ("examples/audio/woodland_ambience.sfx", true),
        ("examples/audio/arcade_anthem_loop.sfx", true),
    ] {
        let doc = load_sound_file(Path::new(path), None).unwrap();
        assert_eq!(doc.loop_mode, is_loop);
        let buf = render_sound(&doc, 0, None).unwrap();
        let metrics = analyze_wav(&buf, is_loop);
        assert!(metrics.is_ok(), "{path} errors: {:?}", metrics.errors);
        assert!(metrics.dc_offset < 0.02, "{path} DC offset too high: {}", metrics.dc_offset);
        assert_eq!(metrics.clipped_samples, 0, "{path} clipped samples");
        if is_loop {
            assert!(
                metrics.loop_seam_delta < 0.05,
                "{path} seam delta too high: {}",
                metrics.loop_seam_delta
            );
        }

        // Roundtrip PCM16 WAV encoding/decoding
        let wav_bytes = encode_pcm16_bytes(&buf);
        let decoded = decode_pcm16_bytes(&wav_bytes).unwrap();
        assert_eq!(decoded.len(), buf.len());

        // Diagnostic PNG verification
        let png = render_diagnostic_png(&buf, &metrics, &doc.name, is_loop, 640, 420);
        assert!(inspect_and_verify_png(&png).is_ok());
    }

    // Verify variation mode produces distinct takes
    let doc = load_sound_file(Path::new("examples/audio/boss_impact_sfx.sfx"), None).unwrap();
    let v1 = render_sound(&doc, 1, None).unwrap();
    let v2 = render_sound(&doc, 2, None).unwrap();
    let diff: f64 = v1.left.iter().zip(v2.left.iter()).map(|(a, b)| (a - b).abs()).sum();
    assert!(diff > 1.0, "expected distinct audio variations");
}

#[test]
fn example_svg_validates_and_renders() {
    let doc = load_svg_file(Path::new("examples/ui/dullahan_hud.svg")).unwrap();
    assert!(doc.is_ok(), "SVG errors: {:?}", doc.errors);
    assert!(doc.warnings.is_empty(), "unexpected SVG warnings: {:?}", doc.warnings);
    assert!(doc.stats.shapes > 100);
    assert!(doc.stats.gradients >= 10);

    let opts = SvgRenderOptions::for_doc(&doc);
    let png = render_svg_png(&doc, &opts);
    let info = inspect_and_verify_png(&png).unwrap();
    assert_eq!((info.width, info.height), (640, 360));

    let mut prev = opts.clone();
    prev.checker = true;
    prev.hud = true;
    prev.grid = true;
    prev.nine_slice = Some([16.0, 16.0, 16.0, 16.0]);
    let prev_png = render_svg_png(&doc, &prev);
    assert!(inspect_and_verify_png(&prev_png).is_ok());

    // Validator catches broken gradient references and disallowed <script>
    let bad = parse_svg(
        r##"<svg viewBox="0 0 32 32"><rect width="32" height="32" fill="url(#missing)"/><script/></svg>"##,
        "bad",
    )
    .unwrap();
    assert!(!bad.is_ok());
    assert!(bad.errors.len() >= 2);
}

#[test]
fn collider_blocks_export_separate_gltf_nodes() {
    let pal = Palette::load_file(Path::new("examples/palette.pal")).unwrap();
    let src = r#"
box @body wood_oak 0,.5,0 1,1,1 bev=.05
col hull-convcolonly
  box 0,.5,0 1.02,1.02,1.02
end
col UBX_crate_01
  box 0,.5,0 1.0,1.0,1.0
end
"#;
    let parsed = compile_model(src, "crate", &pal).unwrap();
    assert_eq!(parsed.mesh.colliders.len(), 2);
    assert_eq!(parsed.mesh.colliders[0].name, "hull-convcolonly");
    assert_eq!(parsed.mesh.colliders[1].name, "UBX_crate_01");

    let (gltf, stats) = export_gltf_json(&parsed.mesh, &pal, "crate", None).unwrap();
    assert_eq!(stats.draw_calls, 1);
    assert!(gltf.contains("\"hull-convcolonly\""));
    assert!(gltf.contains("\"UBX_crate_01\""));
    assert!(gltf.contains("\"children\":[1,2]"));
    let rep = validate_gltf_json_str(&gltf).unwrap();
    assert!(rep.is_ok(), "collider glTF errors: {:?}", rep.errors);

    let (glb, _) = export_glb(&parsed.mesh, &pal, "crate", None).unwrap();
    let rep_glb = validate_glb_bytes(&glb).unwrap();
    assert!(rep_glb.is_ok(), "collider GLB errors: {:?}", rep_glb.errors);

    let mut opts = RenderOptions::default();
    opts.width = 240;
    opts.height = 180;
    opts.show_colliders = true;
    let png = render_to_png(&parsed.mesh, &pal, &opts);
    assert!(inspect_and_verify_png(&png).is_ok());
}

#[test]
fn example_terrain_erodes_decimates_and_roundtrips_heightmap() {
    let pal = Palette::load_file(Path::new("examples/palette.pal")).unwrap();
    let doc = load_terrain_file(Path::new("examples/terrain/highland_cove.ter")).unwrap();
    let td = simulate_terrain(&doc, None).unwrap();
    assert_eq!((td.width, td.height), (256, 256));
    assert!(td.drops_simulated >= 50_000);

    // 16-bit lossless heightmap + RGBA splatmap + RGB normal map + 2x2 QA preview
    let hm16 = td.encode_height16_png();
    let hm_info = inspect_and_verify_png(&hm16).unwrap();
    assert_eq!((hm_info.width, hm_info.height, hm_info.bit_depth, hm_info.color_type), (256, 256, 16, 0));

    let splat = td.encode_splat_rgba_png();
    assert!(inspect_and_verify_png(&splat).is_ok());
    let norm = td.encode_normal_rgb_png();
    assert!(inspect_and_verify_png(&norm).is_ok());
    let prev = render_terrain_preview_png(&td, Some(&pal));
    assert!(inspect_and_verify_png(&prev).is_ok());

    // QEM decimated low-poly GLB mesh with diorama skirt + collider node
    let mut mopts = TerrainMeshOptions::default();
    mopts.grid = 48;
    mopts.target_tris = Some(1600);
    mopts.collider_name = Some("highland_cove-colonly".into());
    let mesh = build_terrain_mesh(&td, &pal, &mopts);
    assert!(mesh.tris.len() < 2400);
    assert_eq!(mesh.colliders.len(), 1);
    let (glb, _) = export_glb(&mesh, &pal, &td.name, None).unwrap();
    let rep = validate_glb_bytes(&glb).unwrap();
    assert!(rep.is_ok(), "terrain GLB errors: {:?}", rep.errors);

    // Round-trip from external 16-bit heightmap + RGBA weightmap PNGs -> GLB
    let td2 = build_from_heightmap_png(
        &hm16,
        Some(&splat),
        td.world,
        "from_hm",
        td.channel_mats.clone(),
    )
    .unwrap();
    let mesh2 = build_terrain_mesh(&td2, &pal, &TerrainMeshOptions { grid: 32, target_tris: Some(900), skirt: true, smooth: false, collider_name: None });
    let (glb2, _) = export_glb(&mesh2, &pal, "from_hm", None).unwrap();
    let rep2 = validate_glb_bytes(&glb2).unwrap();
    assert!(rep2.is_ok(), "from-heightmap GLB errors: {:?}", rep2.errors);
}

#[test]
fn beveled_tapered_cylinder_smooth_normals_never_invert() {
    let pal = Palette::load_file(Path::new("examples/palette.pal")).unwrap();
    let src = r#"
cyl iron_dark 0.5,0.5,0 0.165,0.155,0.045 seg=14 sm=1 bev=0.01 r=90,0,0 m=x
cyl iron_dark 0.5,0.5,0 0.30,0.15,0.045   seg=14 sm=1 bev=0.01 r=90,0,0 m=x
cyl steel_mid 0.0,0.5,0 0.40,0.08,0.04    seg=16 sm=1 bev=0.01 r=0,0,90 m=xz
"#;
    let parsed = compile_model(src, "bevel_cyl_repro", &pal).unwrap();
    let (glb, _) = export_glb(&parsed.mesh, &pal, "bevel_cyl_repro", None).unwrap();
    let rep = validate_glb_bytes(&glb).unwrap();
    assert!(rep.is_ok(), "beveled tapered smooth cyl errors: {:?}", rep.errors);
}

#[test]
fn mdl_kits_cli_overrides_child_nodes_and_foliage_primitives() {
    let pal = Palette::load_file(Path::new("examples/palette.pal")).unwrap();
    let tmp = std::env::temp_dir().join("artisan_test_kit_dir");
    let _ = std::fs::create_dir_all(&tmp);
    let kit_path = tmp.join("shared_parts.mdl");
    std::fs::write(
        &kit_path,
        r#"
$lug_r = 0.08
def lift_lug
  tor steel_mid 0,0,0 $lug_r,0.02 seg=10 tseg=6
end
"#,
    )
    .unwrap();

    // Model using shared kit via `use shared_parts.mdl`, animated child `node`, `tri ds=1`, and `rib`
    let src = r#"
use shared_parts.mdl
box @base olive_body 0,.25,0 1,.5,1 bev=.03
use lift_lug 0,.55,0
tri @leaf grass_light 0:0:0 .2:0:0 .1:.4:.05 ds=1
rib @blade grass_mid .3,0,0 .08,0,.45,.15 seg=4 rad=4:.04:y

node LeverPivot 0,.5,.2 r=0,0,20
  cyl iron_dark 0,0,0 .03,.03,.5 a=b seg=8
  ico red_paint 0,.5,0 .06 sub=1
end
"#;

    // Atomic CLI override swap: olive_body <-> red_paint simultaneously!
    let mut ov = HashMap::new();
    ov.insert("olive_body".into(), "red_paint".into());
    ov.insert("red_paint".into(), "olive_body".into());

    let parsed = compile_model_with_dir(src, "switch_stand", &pal, Some(&tmp), &ov).unwrap();
    assert_eq!(parsed.mesh.nodes.len(), 1);
    assert_eq!(parsed.mesh.nodes[0].name, "LeverPivot");
    assert!(parsed.mesh.part_bounds.contains_key("LeverPivot"));

    // Verify atomic swap took effect on both root (`olive_body` -> `red_paint`) and child node (`red_paint` -> `olive_body`)
    let red_uv = pal.resolve_mat("red_paint").unwrap().uv;
    let olive_uv = pal.resolve_mat("olive_body").unwrap().uv;
    assert!(parsed.mesh.tris.iter().any(|t| (t.uv.x - red_uv.x).abs() < 1e-4 && (t.uv.y - red_uv.y).abs() < 1e-4));
    assert!(parsed.mesh.nodes[0].tris.iter().any(|t| (t.uv.x - olive_uv.x).abs() < 1e-4 && (t.uv.y - olive_uv.y).abs() < 1e-4));

    // Export glTF & GLB and verify child node name, translation, rotation quaternion, and validation
    let (gltf, stats) = export_gltf_json(&parsed.mesh, &pal, "switch_stand", None).unwrap();
    assert!(stats.draw_calls >= 2);
    assert!(gltf.contains("\"LeverPivot\""));
    assert!(gltf.contains("\"translation\""));
    assert!(gltf.contains("\"rotation\""));
    let rep_gltf = validate_gltf_json_str(&gltf).unwrap();
    assert!(rep_gltf.is_ok(), "child node glTF errors: {:?}", rep_gltf.errors);

    let (glb, _) = export_glb(&parsed.mesh, &pal, "switch_stand", None).unwrap();
    let rep_glb = validate_glb_bytes(&glb).unwrap();
    assert!(rep_glb.is_ok(), "child node GLB errors: {:?}", rep_glb.errors);

    // Render hero PNG including child node
    let mut ropts = RenderOptions::default();
    ropts.width = 240;
    ropts.height = 180;
    let png = render_to_png(&parsed.mesh, &pal, &ropts);
    assert!(inspect_and_verify_png(&png).is_ok());

    // Also test load_and_compile_model_with_overrides with bad override swatch fails loudly
    let mdl_path = tmp.join("switch_stand.mdl");
    let pal_abs = std::fs::canonicalize("examples/palette.pal").unwrap();
    std::fs::write(&mdl_path, format!("pal {}\n{src}", pal_abs.display())).unwrap();
    let mut bad_ov = HashMap::new();
    bad_ov.insert("olive_body".into(), "nonexistent_swatch".into());
    assert!(load_and_compile_model_with_overrides(&mdl_path, None, &bad_ov).is_err());

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn sfx_piecewise_sweeps_mono_loop_norm_phase_and_slope_seam() {
    // 1. Piecewise penv + cenv with note names and explicit curve
    let bird_src = r#"
sfx bird_chirp dur=.22 sr=22050 ch=1 norm=-1 seed=5
fm C6 dur=.18 mr=1.5 mi=1.2 ph=.25 penv=0:C6,0.05:G6,0.12:D6,0.18:A6:sm cenv=0:1200,0.09:4200,0.18:1800:lin env=.005,.04,.6,.06 g=.8 | flt bp 2400 q=1.2
"#;
    let bird_doc = parse_sound_doc(bird_src, "bird_chirp", Path::new("."), None).unwrap();
    let bird_buf = render_sound(&bird_doc, 0, None).unwrap();
    let bird_m = analyze_wav(&bird_buf, false);
    assert!(bird_m.is_ok(), "bird_chirp errors: {:?}", bird_m.errors);
    assert!((bird_m.peak_db - (-1.0)).abs() < 0.15, "expected ~-1.0 dB, got {}", bird_m.peak_db);

    // 2. Mono (ch=1) loop with stereo noise + reverb: on-disk WAV peak must match norm=-2.5 dB!
    let hiss_src = r#"
sfx air_hiss dur=.6 sr=22050 ch=1 loop=1 xfade=.05 curve=eqp norm=-2.5 seed=9
noi pink dur=.6 g=.7 | flt bp 2200 q=1.2 | rev room=.4 mix=.35
"#;
    let hiss_doc = parse_sound_doc(hiss_src, "air_hiss", Path::new("."), None).unwrap();
    let hiss_buf = render_sound(&hiss_doc, 0, None).unwrap();
    let wav_bytes = encode_pcm16_bytes(&hiss_buf);
    let disk_buf = decode_pcm16_bytes(&wav_bytes).unwrap();
    let mem_m = analyze_wav(&hiss_buf, true);
    let disk_m = analyze_wav(&disk_buf, true);
    assert!(
        (mem_m.peak_db - (-2.5)).abs() < 0.15,
        "in-memory mono loop peak {} != -2.5 dB",
        mem_m.peak_db
    );
    assert!(
        (disk_m.peak_db - (-2.5)).abs() < 0.15,
        "on-disk mono loop peak {} != -2.5 dB (was mem {})",
        disk_m.peak_db,
        mem_m.peak_db
    );

    // 3. High-frequency 3 kHz cycle-locked sine loop must NOT trigger false-positive seam warning
    let screech_src = r#"
sfx flange_screech dur=.5 sr=22050 ch=1 loop=1 xfade=.01 curve=lin norm=-2 seed=3
osc sin 3000 dur=.5 cl=1 g=.8
"#;
    let screech_doc = parse_sound_doc(screech_src, "flange_screech", Path::new("."), None).unwrap();
    let screech_buf = render_sound(&screech_doc, 0, None).unwrap();
    let screech_m = analyze_wav(&screech_buf, true);
    assert!(
        screech_m.loop_seam_delta < 0.02,
        "3 kHz cycle-locked sine had false-positive seam delta {}",
        screech_m.loop_seam_delta
    );
    assert!(
        screech_m.warnings.is_empty(),
        "unexpected seam warning on 3 kHz sine: {:?}",
        screech_m.warnings
    );

    // 4. Verify ph=.25 produces a cosine-like starting peak (|sample[0]| > 0.8 vs ~0 at ph=0)
    let ph_src = r#"
sfx phase_test dur=.05 sr=22050 ch=1 norm=-1
osc sin 440 dur=.05 ph=.25 g=1
"#;
    let ph_doc = parse_sound_doc(ph_src, "phase_test", Path::new("."), None).unwrap();
    let ph_buf = render_sound(&ph_doc, 0, None).unwrap();
    assert!(ph_buf.left[0].abs() > 0.7, "expected ph=.25 peak near start, got {}", ph_buf.left[0]);
}

#[test]
fn weighted_normals_keep_flat_faces_flat_and_bevels_smooth() {
    let pal = Palette::load_file(Path::new("examples/palette.pal")).unwrap();
    let src = r#"
box steel_mid 0,.5,0 1,1,1 bev=.06 wn=1
cyl brass_gold 1.5,.5,0 .35,.32,.8 seg=16 bev=.03 wn=1
"#;
    let parsed = compile_model(src, "wn_test", &pal).unwrap();
    // On the main axis-aligned faces of the beveled box (first 12 tris), vertex normals should stay >0.999 flat!
    for (ti, t) in parsed.mesh.tris.iter().take(12).enumerate() {
        for vn in [t.na, t.nb, t.nc] {
            let d = vn.dot(t.face_normal);
            assert!(
                d > 0.999,
                "box main face tri {ti} pillowed under wn=1: dot={d:.5} vn={vn:?} fn={:?}",
                t.face_normal
            );
        }
    }
    // On the bevel edge quads (tris 12..36), vertex normals should smoothly span between orthogonal faces!
    let mut curved_bevels = 0;
    for t in parsed.mesh.tris.iter().skip(12).take(24) {
        if t.na.dot(t.nb) < 0.85 || t.nb.dot(t.nc) < 0.85 || t.nc.dot(t.na) < 0.85 {
            curved_bevels += 1;
        }
    }
    assert!(curved_bevels >= 12, "expected beveled edges to smoothly interpolate normals, got {curved_bevels}");

    let (glb, _) = export_glb(&parsed.mesh, &pal, "wn_test", None).unwrap();
    let rep = validate_glb_bytes(&glb).unwrap();
    assert!(rep.is_ok(), "wn=1 GLB errors: {:?}", rep.errors);
}
