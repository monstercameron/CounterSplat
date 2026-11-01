//! Composes complete gsplat shader modules for the SOG V2 format: include resolution with the
//! runtime-generated chunks, then the binding layer, then the PlayCanvas-construct fixups.
use crate::binding::{
    bind_engine_declarations, fix_engine_constructs, fix_fragment_constructs, Binding,
};
use crate::declarations::{sog_streams, wgsl_input_declarations};
use crate::preprocess::{preprocess_with, Defines, Overrides};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
}

/// Defines for the SOG V2 format with spherical harmonics, using numeric fog modes as the engine does.
pub fn sog_v2_sh_defines(stage: Stage) -> Defines {
    let mut d = Defines::new();
    d.insert("SH_BANDS".into(), "3".into());
    d.insert("SOG_V2".into(), String::new());
    // The engine's non-dithered materials set DITHER_NONE (gsplat-instance.js configureMaterial); without it
    // the fragment stage takes the stochastic opacity-dither path, which speckles and darkens the splats.
    d.insert("DITHER_NONE".into(), String::new());
    if stage == Stage::Vertex {
        d.insert("VERTEXSHADER".into(), String::new());
    }
    d.insert("NONE".into(), "0".into());
    d.insert("LINEAR".into(), "1".into());
    d.insert("EXP".into(), "2".into());
    d.insert("EXP2".into(), "3".into());
    d.insert("FOG".into(), "0".into());
    d
}

/// Runtime chunks for SOG V2 with SH, as the engine's `GSplatSogResource` registers them.
pub fn sog_v2_sh_overrides() -> Overrides {
    let mut o = Overrides::new();
    // Positions already live in the shared CPU-decoded centre buffer (also used
    // by the sorter). The original quantized means textures
    // are never read by this renderer's getCenter, so do not bind them twice.
    let streams: Vec<_> = sog_streams(true, true)
        .into_iter()
        .filter(|stream| !matches!(stream.name.as_str(), "means_l" | "means_u"))
        .collect();
    o.insert(
        "gsplatContainerDeclarationsVS".into(),
        wgsl_input_declarations(&streams),
    );
    o.insert(
        "gsplatDeclarationsVS".into(),
        "#include \"gsplatContainerDeclVS\"".into(),
    );
    o.insert("gsplatReadVS".into(), "#include \"gsplatSogVS\"".into());
    const SOG: &str = include_str!("../shaders/gsplat/vert/formats/sog.wgsl");
    let from = SOG
        .find("fn getCenter() -> vec3f {")
        .expect("SOG center getter");
    let to = from + SOG[from..].find("\n}").expect("SOG center end") + 2;
    let mut sog = SOG.to_string();
    sog.replace_range(from..to,"var<storage, read> splatCenters: array<f32>;\nfn getCenter() -> vec3f { let i=splat.index*3u; return vec3f(splatCenters[i],splatCenters[i+1u],splatCenters[i+2u]); }");
    // The quantized-position decode uniforms are dead with this getter too.
    sog = sog
        .replace("uniform means_mins: vec3f;", "")
        .replace("uniform means_maxs: vec3f;", "");
    o.insert("gsplatSogVS".into(), sog);

    // Camera effects use two portable vertex uniforms: exposure/focus/shutter and
    // the previous submitted clip transform. Defaults are zero and leave old hosts unchanged.
    const STRUCTS: &str = include_str!("../shaders/gsplat/vert/gsplatStructs.wgsl");
    const CORNER: &str = include_str!("../shaders/gsplat/vert/gsplatCorner.wgsl");
    let structs = STRUCTS.replace("\r\n", "\n");
    const CORNER_UV: &str = "    uv: half2,            // corner uv";
    assert_eq!(
        structs.matches(CORNER_UV).count(),
        1,
        "splat corner layout changed"
    );
    o.insert(
        "gsplatStructsVS".into(),
        structs.replace(
            CORNER_UV,
            "    uv: half2,            // corner uv\n    lensFactor: f32,",
        ),
    );
    let corner = CORNER.replace("\r\n", "\n");
    const VIEWPORT: &str = "uniform viewport_size: vec4f;";
    assert_eq!(
        corner.matches(VIEWPORT).count(),
        1,
        "splat lens uniform hook changed"
    );
    let corner = corner.replace(VIEWPORT, "uniform lens_effects: vec4f;\nuniform previous_clip_from_model: mat4x4f;\nuniform viewport_size: vec4f;");
    const DIAGONAL: &str = "    let diagonal1 = cov[0][0] + 0.3;\n    let offDiagonal = cov[0][1];\n    let diagonal2 = cov[1][1] + 0.3;";
    assert_eq!(
        corner.matches(DIAGONAL).count(),
        1,
        "splat covariance hook changed"
    );
    let corner = corner.replace(DIAGONAL, r#"
    var diagonal1 = cov[0][0] + 0.3;
    var offDiagonal = cov[0][1];
    var diagonal2 = cov[1][1] + 0.3;
    corner.lensFactor = 1.0;
    if (uniform.lens_effects.z > 0.0 || uniform.lens_effects.w > 0.0) {
        let originalDet = max(diagonal1 * diagonal2 - offDiagonal * offDiagonal, 0.000001);
        if (uniform.lens_effects.z > 0.0) {
            let depth = max(-center.view.z, 0.001);
            let diameter = min(24.0, uniform.lens_effects.z * abs(1.0 - uniform.lens_effects.y / depth));
            let variance = diameter * diameter / 16.0;
            diagonal1 += variance;
            diagonal2 += variance;
        }
        if (uniform.lens_effects.w > 0.0) {
            let oldClip = uniform.previous_clip_from_model * vec4f(center.modelCenterModified, 1.0);
            if (oldClip.w > 0.001 && center.proj.w > 0.001) {
                var motion = (center.proj.xy / center.proj.w - oldClip.xy / oldClip.w) *
                    (0.5 * uniform.viewport_size.xy * uniform.lens_effects.w);
                motion *= min(1.0, 32.0 / max(length(motion), 0.001));
                diagonal1 += motion.x * motion.x / 12.0;
                offDiagonal += motion.x * motion.y / 12.0;
                diagonal2 += motion.y * motion.y / 12.0;
            }
        }
        let blurredDet = max(diagonal1 * diagonal2 - offDiagonal * offDiagonal, originalDet);
        corner.lensFactor = sqrt(clamp(originalDet / blurredDet, 0.0, 1.0));
    }
"#);
    o.insert("gsplatCornerVS".into(), corner);
    o.insert("gsplatSogSHVS".into(), sog_sh_24bit_labels());
    // Streaming fade and optional LOD diagnostics are scene features, not game logic.
    o.insert(
        "gsplatModifyVS".into(),
        include_str!("local/scene-modify.wgsl").into(),
    );
    const VS: &str = include_str!("../shaders/gsplat/vert/gsplat.wgsl");
    const DIR: &str = "let dir = normalize(viewDir * modelView3x3);";
    assert!(VS.contains(DIR), "gsplat SH direction hook changed");
    // Git checkouts may use CRLF on Windows; source hooks are defined with LF.
    let vs = VS.replace("\r\n", "\n");
    // Stream pools reserve centroid zero as exactly zero SH. Other scenes,
    // including ordinary SOG assets, may have a real colour at label zero. This per-scene
    // capability defaults to zero and is enabled only after the pool's zero
    // centroid has been initialized. Test all three bytes of the 24-bit label.
    const VARYING: &str = "varying gaussianUV: half2;";
    const SH_START: &str =
        "    #if SH_BANDS > 0\n        // calculate the model-space view direction.";
    const SH_RESULT: &str = "        clr = half4(clr.xyz + evalSH(&sh, dir) * half(scale), clr.a);";
    assert_eq!(
        vs.matches(VARYING).count(),
        1,
        "gsplat SH uniform hook changed"
    );
    assert_eq!(
        vs.matches(SH_START).count(),
        1,
        "gsplat SH start hook changed"
    );
    assert_eq!(
        vs.matches(SH_RESULT).count(),
        1,
        "gsplat SH result hook changed"
    );
    let vs = vs.replace(
        VARYING,
        r#"
#if SH_BANDS > 0
uniform sh_zero_reserved: u32;
#endif
varying gaussianUV: half2;
"#,
    );
    let vs = vs.replace(SH_START, r#"
    #if SH_BANDS > 0
        #ifndef SPLAT_FULL_SH_REFERENCE
            if (uniform.sh_zero_reserved == 0u || any(textureLoad(sh_labels, splat.uv, 0).xyz != vec3f(0.0))) {
        #endif
        // calculate the model-space view direction."#);
    let vs = vs.replace(
        SH_RESULT,
        r#"
        clr = half4(clr.xyz + evalSH(&sh, dir) * half(scale), clr.a);
        #ifndef SPLAT_FULL_SH_REFERENCE
            }
        #endif
"#,
    );
    // Opacity alone can reject a depth splat before covariance projection. Keep the
    // original post-AA/colour-hook rejection too. This specialization applies only
    // when AA is disabled, so no opacity amplification or changed operation order
    // can move a surviving splat across the cutoff.
    const CENTER: &str = "var modelCenter: vec3f = getCenter();";
    assert!(vs.contains(CENTER), "gsplat early opacity hook changed");
    let vs = vs.replace(
        CENTER,
        r#"
#if defined(PREPASS_PASS) && !GSPLAT_AA
    let prepassOpacity = textureLoad(sh0, splat.uv, 0).w;
    if (prepassOpacity * splatFade[splat.index] <= uniform.alphaClip) {
        output.position = discardVec;
        return output;
    }
#endif
    var modelCenter: vec3f = getCenter();
"#,
    );
    // Lens blur expands the covariance and preserves its integrated opacity.
    let vs = vs.replace(
        "    var corner: SplatCorner;",
        "    var corner: SplatCorner;\n    corner.lensFactor = 1.0;",
    );
    let vs = vs.replace(
        "    // evaluate spherical harmonics",
        "    clr.a = clr.a * half(corner.lensFactor);\n\n    // evaluate spherical harmonics",
    );
    // SOG color arrives in gamma space. Apply photographic exposure in linear
    // light, then return to the source representation for the existing output path.
    let vs = vs.replace("    // user hook operates in f32", r#"
    #ifndef PREPASS_PASS
        if (uniform.lens_effects.x != 0.0) {
            let linearColor = pow(max(vec3f(clr.xyz), vec3f(0.0)), vec3f(2.2)) * exp2(uniform.lens_effects.x);
            clr = half4(pow(linearColor, vec3f(1.0 / 2.2)), clr.a);
        }
    #endif
    // user hook operates in f32"#);
    const READ_COLOR: &str = "var clr: half4 = half4(getColor());";
    assert!(vs.contains(READ_COLOR), "gsplat color hook changed");
    o.insert("gsplatVS".into(), vs.replace(READ_COLOR, "#ifdef PREPASS_PASS\n var clr: half4 = half4(0.0, 0.0, 0.0, textureLoad(sh0, splat.uv, 0).w);\n#else\n var clr: half4 = half4(getColor());\n#endif"));

    o
}

/// The engine's SH reader with a 24-bit centroid label: byte 2 of `sh_labels` (blue) is the high byte. A level merged
/// from several chunks needs more than 65,536 centroids. Plain SOG files leave blue at zero once loaded (the loader
/// clears it), so they read the same as before.
fn sog_sh_24bit_labels() -> String {
    const SOURCE: &str = include_str!("../shaders/gsplat/vert/formats/sogSH.wgsl");
    const LABEL: &str = "let n = i32(labels.x * 255.0 + 0.5) + i32(labels.y * 255.0 + 0.5) * 256;";
    assert!(SOURCE.contains(LABEL), "sogSH.wgsl label line changed");
    SOURCE.replace(LABEL, "let n = i32(labels.x * 255.0 + 0.5) + i32(labels.y * 255.0 + 0.5) * 256 + i32(labels.z * 255.0 + 0.5) * 65536;")
}

/// Composes one stage with its own binding numbering (see `compose_pipeline` for the shared one).
pub fn compose_gsplat(stage: Stage) -> Result<(String, Vec<Binding>), crate::preprocess::Error> {
    compose_gsplat_with(stage, &[])
}

/// Composes one stage with extra defines on top of the SOG V2 set, e.g. `PREPASS_PASS` for the depth prepass.
/// `NAME=VALUE` sets a value and overrides the default, e.g. `SH_BANDS=0`.
/// `SPLAT_FULL_SH_REFERENCE` disables the reserved-zero SH shortcut for comparison.
pub fn compose_gsplat_with(
    stage: Stage,
    extra: &[&str],
) -> Result<(String, Vec<Binding>), crate::preprocess::Error> {
    let key = match stage {
        Stage::Vertex => "gsplatVS",
        Stage::Fragment => "gsplatPS",
    };
    let mut defines = sog_v2_sh_defines(stage);
    for define in extra {
        let (name, value) = define.split_once('=').unwrap_or((define, ""));
        defines.insert(name.to_string(), value.to_string());
    }
    let src = preprocess_with(key, &mut defines, &sog_v2_sh_overrides())?;
    let (bound, bindings) = bind_engine_declarations(&src);
    let fixed = match stage {
        Stage::Vertex => fix_engine_constructs(&bound),
        Stage::Fragment => fix_fragment_constructs(&bound),
    };
    Ok((fixed, bindings))
}

/// Composes both stages with one shared binding table, so every resource name has the same
/// (group, binding) in the vertex and fragment modules. Returns (vertex, fragment, union bindings).
pub fn compose_pipeline() -> Result<(String, String, Vec<Binding>), crate::preprocess::Error> {
    compose_pipeline_with(&[])
}

/// `compose_pipeline` with extra defines on both stages. `&["PREPASS_PASS"]` gives the depth prepass, which discards
/// fragments below the `alphaClip` uniform.
pub fn compose_pipeline_with(
    extra: &[&str],
) -> Result<(String, String, Vec<Binding>), crate::preprocess::Error> {
    let (vs_src, vs_b) = compose_gsplat_with(Stage::Vertex, extra)?;
    let (fs_src, fs_b) = compose_gsplat_with(Stage::Fragment, extra)?;
    let (table, unified) = crate::binding::unify(&[&vs_b, &fs_b]);
    Ok((
        crate::binding::rebind(&vs_src, &table),
        crate::binding::rebind(&fs_src, &table),
        unified,
    ))
}
