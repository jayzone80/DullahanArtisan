use std::path::Path;

use dullahan_artisan::common::png::inspect_and_verify_png;
use dullahan_artisan::mesh::gltf::{export_glb, export_gltf_json};
use dullahan_artisan::mesh::palette::Palette;
use dullahan_artisan::mesh::parser::load_and_compile_model;
use dullahan_artisan::mesh::render::{render_to_png, render_turntable_png, RenderOptions};
use dullahan_artisan::mesh::validator::{validate_glb_bytes, validate_gltf_json_str};
use dullahan_artisan::sfx::diagnostics::{analyze_wav, render_diagnostic_png};
use dullahan_artisan::sfx::engine::render_sound;
use dullahan_artisan::sfx::parser::load_sound_file;
use dullahan_artisan::sfx::wav::{decode_pcm16_bytes, encode_pcm16_bytes};

#[test]
fn palette_and_all_example_models_build_validate_render() {
    let pal = Palette::load_file(Path::new("examples/palette.pal")).unwrap();
    let sheet = pal.render_swatch_sheet_png();
    assert!(inspect_and_verify_png(&sheet).is_ok());

    for mdl in [
        "examples/models/offroad_4x4.mdl",
        "examples/models/modular_dungeon_kit.mdl",
        "examples/models/props_shotgun_medkit.mdl",
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
