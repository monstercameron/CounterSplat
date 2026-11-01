
var means_l: texture_2d<f32>;
fn loadMeans_l() -> vec4f { return textureLoad(means_l, splat.uv, 0); }
fn loadMeans_lWithIndex(index: u32) -> vec4f { return textureLoad(means_l, vec2i(i32(index % uniform.splatTextureSize), i32(index / uniform.splatTextureSize)), 0); }


var means_u: texture_2d<f32>;
fn loadMeans_u() -> vec4f { return textureLoad(means_u, splat.uv, 0); }
fn loadMeans_uWithIndex(index: u32) -> vec4f { return textureLoad(means_u, vec2i(i32(index % uniform.splatTextureSize), i32(index / uniform.splatTextureSize)), 0); }


var quats: texture_2d<f32>;
fn loadQuats() -> vec4f { return textureLoad(quats, splat.uv, 0); }
fn loadQuatsWithIndex(index: u32) -> vec4f { return textureLoad(quats, vec2i(i32(index % uniform.splatTextureSize), i32(index / uniform.splatTextureSize)), 0); }


var scales: texture_2d<f32>;
fn loadScales() -> vec4f { return textureLoad(scales, splat.uv, 0); }
fn loadScalesWithIndex(index: u32) -> vec4f { return textureLoad(scales, vec2i(i32(index % uniform.splatTextureSize), i32(index / uniform.splatTextureSize)), 0); }


var sh0: texture_2d<f32>;
fn loadSh0() -> vec4f { return textureLoad(sh0, splat.uv, 0); }
fn loadSh0WithIndex(index: u32) -> vec4f { return textureLoad(sh0, vec2i(i32(index % uniform.splatTextureSize), i32(index / uniform.splatTextureSize)), 0); }


var sh_labels: texture_2d<f32>;
fn loadSh_labels() -> vec4f { return textureLoad(sh_labels, splat.uv, 0); }
fn loadSh_labelsWithIndex(index: u32) -> vec4f { return textureLoad(sh_labels, vec2i(i32(index % uniform.splatTextureSize), i32(index / uniform.splatTextureSize)), 0); }


var sh_centroids: texture_2d<f32>;
fn loadSh_centroids() -> vec4f { return textureLoad(sh_centroids, splat.uv, 0); }
fn loadSh_centroidsWithIndex(index: u32) -> vec4f { return textureLoad(sh_centroids, vec2i(i32(index % uniform.splatTextureSize), i32(index / uniform.splatTextureSize)), 0); }


var sogCodebook: texture_2d<uff>;
fn loadSogCodebook() -> vec4f { return textureLoad(sogCodebook, splat.uv, 0); }
fn loadSogCodebookWithIndex(index: u32) -> vec4f { return textureLoad(sogCodebook, vec2i(i32(index % uniform.splatTextureSize), i32(index / uniform.splatTextureSize)), 0); }
