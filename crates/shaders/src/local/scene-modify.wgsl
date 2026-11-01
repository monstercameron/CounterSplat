// Per-slot opacity is written by the sorter for crossfades between streamed LODs.
var<storage, read> splatFade: array<f32>;
// LOD labels and an optional debug mode are owned by the streaming renderer.
var<storage, read> splatLod: array<u32>;
var<storage, read> splatDebug: array<u32>;

fn modifySplatCenter(center: ptr<function, vec3f>) {}

fn modifySplatRotationScale(originalCenter: vec3f, modifiedCenter: vec3f, rotation: ptr<function, vec4f>, scale: ptr<function, vec3f>) {}

fn lodColour(lod: u32) -> vec3f {
    switch lod {
        case 0u: { return vec3f(0.1, 1.0, 0.2); }
        case 1u: { return vec3f(1.0, 0.95, 0.1); }
        case 2u: { return vec3f(1.0, 0.5, 0.05); }
        default: { return vec3f(1.0, 0.1, 0.1); }
    }
}

fn modifySplatColor(center: vec3f, color: ptr<function, vec4f>) {
    *color = vec4f((*color).rgb, (*color).a * splatFade[splat.index]);
    #ifdef PREPASS_PASS
        return;
    #endif
    let lod = splatLod[splat.index] & 15u;
    if (splatDebug[0] == 1u && lod > 0u) {
        *color = vec4f(lodColour(lod - 1u), (*color).a);
        return;
    }
    if (splatDebug[0] == 2u && lod > 0u) {
        let distance = length((uniform.matrix_view * uniform.matrix_model * vec4f(center, 1.0)).xyz);
        *color = vec4f(clamp(distance / 128.0, 0.0, 1.0), 0.0, 0.0, (*color).a);
    }
}
