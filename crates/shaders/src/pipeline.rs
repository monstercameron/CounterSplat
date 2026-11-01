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
