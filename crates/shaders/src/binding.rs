//! Binding layer: turns the engine's module-scope declarations into bound WGSL and records the
//! bind-group table that the wgpu pipeline must match.
//!
//! The engine writes `uniform name: T;` (PlayCanvas-style, filled in by the engine at draw time)
//! and `var name: texture_2d<...>;` for textures. WGSL needs `@group/@binding` for both, so this
//! pass assigns them in order: group 0 for uniforms, group 1 for textures and samplers.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingKind {
    Uniform {
        ty: String,
    },
    /// `unfilterable` marks the engine's `texture_2d<uff>` sentinel: the WGSL type is `texture_2d<f32>`,
    /// and the host bind-group layout must declare the texture as unfilterable float.
    Texture {
        ty: String,
        unfilterable: bool,
    },
    /// Sampler (`sampler` or `sampler_comparison`), bound next to the textures in group 1.
    Sampler {
        ty: String,
    },
    /// Storage buffer; `access` is the WGSL access mode (`read` or `read_write`).
    Storage {
        ty: String,
        access: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub name: String,
    pub group: u32,
    pub binding: u32,
    pub kind: BindingKind,
}

/// Rewrites `uniform name: T;` and `var name: texture_*;` lines into bound declarations.
/// Returns the rewritten source and the bindings it assigned, in declaration order.
pub fn bind_engine_declarations(src: &str) -> (String, Vec<Binding>) {
    let mut out = String::with_capacity(src.len() + 256);
    let mut bindings = Vec::new();
    let mut next_uniform = 0u32;
    let mut next_texture = 0u32;
    let mut next_storage = 0u32;
    for line in src.lines() {
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        if let Some(rest) = trimmed.strip_prefix("uniform ") {
            if let Some((name, ty)) = split_decl(rest) {
                let b = Binding {
                    name: name.to_string(),
                    group: 0,
                    binding: next_uniform,
                    kind: BindingKind::Uniform { ty: ty.to_string() },
                };
                out += &format!(
                    "{indent}@group(0) @binding({}) var<uniform> {name}: {ty};\n",
                    next_uniform
                );
                next_uniform += 1;
                bindings.push(b);
                continue;
            }
        }
        if let Some(rest) = trimmed.strip_prefix("var<storage") {
            // `var<storage, read> name: T;` (access mode after the comma, binding assigned here).
            if let Some(end) = rest.find('>') {
                let access = rest[..end].trim_start_matches(',').trim().to_string();
                if let Some((name, ty)) = split_decl(rest[end + 1..].trim_start()) {
                    let b = Binding {
                        name: name.to_string(),
                        group: 2,
                        binding: next_storage,
                        kind: BindingKind::Storage {
                            ty: ty.to_string(),
                            access: access.clone(),
                        },
                    };
                    out += &format!(
                        "{indent}@group(2) @binding({}) var<storage, {access}> {name}: {ty};\n",
                        next_storage
                    );
                    next_storage += 1;
                    bindings.push(b);
                    continue;
                }
            }
        }
        if let Some(rest) = trimmed.strip_prefix("var ") {
            if let Some((name, ty)) = split_decl(rest) {
                if ty.starts_with("sampler") {
                    let b = Binding {
                        name: name.to_string(),
                        group: 1,
                        binding: next_texture,
                        kind: BindingKind::Sampler { ty: ty.to_string() },
                    };
                    out += &format!(
                        "{indent}@group(1) @binding({}) var {name}: {ty};\n",
                        next_texture
                    );
                    next_texture += 1;
                    bindings.push(b);
                    continue;
                }
                if ty.starts_with("texture_") {
                    let unfilterable = ty.contains("uff");
                    let wgsl_ty = if unfilterable { "texture_2d<f32>" } else { ty };
                    let b = Binding {
                        name: name.to_string(),
                        group: 1,
                        binding: next_texture,
                        kind: BindingKind::Texture {
                            ty: wgsl_ty.to_string(),
                            unfilterable,
                        },
                    };
                    out += &format!(
                        "{indent}@group(1) @binding({}) var {name}: {wgsl_ty};\n",
                        next_texture
                    );
                    next_texture += 1;
                    bindings.push(b);
                    continue;
                }
            }
        }
        out += line;
        out.push('\n');
    }
    (out, bindings)
}

/// Splits `name: type;` (with optional trailing comment) into `(name, type)`.
fn split_decl(rest: &str) -> Option<(&str, &str)> {
    let code = rest.split("//").next()?.trim();
    let code = code.strip_suffix(';')?.trim();
    let (name, ty) = code.split_once(':')?;
    let name = name.trim();
    let ty = ty.trim();
    if name.is_empty() || ty.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some((name, ty))
}

/// Converts the engine's remaining PlayCanvas-only constructs into WGSL so the module parses.
///
/// Decisions (documented in README.md):
/// - `half*` types are WGSL aliases for `f32` vectors. Precision is not reduced; this avoids
///   requiring the `f16` feature for the first version.
/// - `attribute name: T;` lines become fields of `VertexInput` with `@location` in declaration order.
/// - `varying` lines become fields of `VertexOutput`, with `@builtin(position)` added by the
///   host-generated struct. The engine's `position` is written explicitly by `vertexMain`.
/// - `uniform.name` accesses become `name`, since bound uniforms are module-scope variables.
pub fn fix_engine_constructs(src: &str) -> String {
    let mut inputs: Vec<String> = Vec::new();
    let mut input_names: Vec<(String, String)> = Vec::new();
    let mut output_decls: Vec<(String, String, String)> = Vec::new();
    let mut body = String::with_capacity(src.len());
    for line in src.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("attribute ") {
            if let Some((name, ty)) = split_decl(rest) {
                inputs.push(format!("    @location({}) {name}: {ty},", inputs.len()));
                input_names.push((name.to_string(), ty.to_string()));
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("varying ") {
            let (interp, decl) = match rest.strip_prefix("@interpolate(") {
                Some(r) => {
                    let end = r.find(')').unwrap_or(0);
                    (
                        Some(format!("@interpolate({})", &r[..end])),
                        r[end + 1..].trim_start(),
                    )
                }
                None => (None, rest),
            };
            if let Some((name, ty)) = split_decl(decl) {
                let ty = ty
                    .replace("half4", "vec4f")
                    .replace("half3", "vec3f")
                    .replace("half2", "vec2f")
                    .replace("half", "f32");
                let attr = match interp {
                    Some(i) => format!("{i} "),
                    None => String::new(),
                };
                output_decls.push((name.to_string(), attr, ty));
            }
            continue;
        }
        body += &line
            .replace("uniform.", "")
            .replace("{GSPLAT_INSTANCE_SIZE}", "128");
        body.push('\n');
    }
    // The engine's pcInstanceIndex builtin is mapped to a private global that vertexMain sets from
    // the instance_index builtin (see webgpu-shader-processor-wgsl.js: VERTEX_BUILTINS).
    // Same sorted-name location rule as the fragment stage (see fix_fragment_constructs).
    output_decls.sort_by(|a, b| a.0.cmp(&b.0));
    let outputs: Vec<String> = output_decls
        .iter()
        .enumerate()
        .map(|(i, (name, attr, ty))| format!("    @location({i}) {attr}{name}: {ty},"))
        .collect();
    // Vertex attributes are read from module-scope privates too (e.g. initSource reads vertex_position).
    let mut entry_prelude = String::from("    pcInstanceIndex = input.instanceIndex;\n");
    let mut privates = String::from("var<private> pcInstanceIndex: u32;\n");
    for (name, ty) in &input_names {
        privates += &format!("var<private> {name}: {ty};\n");
        entry_prelude += &format!("    {name} = input.{name};\n");
    }
    let body = body.replacen(
        "fn vertexMain(input: VertexInput) -> VertexOutput {",
        &format!("fn vertexMain(input: VertexInput) -> VertexOutput {{\n{entry_prelude}"),
        1,
    );
    let aliases = format!("alias half = f32;\nalias half2 = vec2f;\nalias half3 = vec3f;\nalias half4 = vec4f;\nalias half3x3 = mat3x3f;\n\n{privates}\n");
    let io = format!(
        "struct VertexInput {{\n{}\n    @builtin(instance_index) instanceIndex: u32,\n}};\n\nstruct VertexOutput {{\n    @builtin(position) position: vec4f,\n{}\n}};\n\n",
        inputs.join("\n"),
        outputs.join("\n")
    );
    format!("{aliases}{io}{body}")
}

/// Fragment-stage counterpart of `fix_engine_constructs` for the gsplat fragment shader.
///
/// - `varying` lines become fields of `FragmentInput`, with `@builtin(position) position` added.
/// - `pcPosition` (the engine's fragment position builtin) is a private global set from
///   `input.position` at the start of `fragmentMain`.
/// - `FragmentOutput` has a single `@location(0) color: vec4f` target.
pub fn fix_fragment_constructs(src: &str) -> String {
    let mut varying_decls: Vec<(String, String, String)> = Vec::new();
    let mut varyings: Vec<(String, String)> = Vec::new();
    let mut body = String::with_capacity(src.len());
    for line in src.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("varying ") {
            let (interp, decl) = match rest.strip_prefix("@interpolate(") {
                Some(r) => {
                    let end = r.find(')').unwrap_or(0);
                    (
                        Some(format!("@interpolate({})", &r[..end])),
                        r[end + 1..].trim_start(),
                    )
                }
                None => (None, rest),
            };
            if let Some((name, ty)) = split_decl(decl) {
                let ty = ty
                    .replace("half4", "vec4f")
                    .replace("half3", "vec3f")
                    .replace("half2", "vec2f")
                    .replace("half", "f32");
                let attr = interp.map(|i| format!("{i} ")).unwrap_or_default();
                varying_decls.push((name.to_string(), attr, ty.clone()));
                varyings.push((name.to_string(), ty));
            }
            continue;
        }
        body += &line.replace("uniform.", "");
        body.push('\n');
    }
    // Locations are assigned by sorted name, the same rule the vertex stage uses, so that
    // VertexOutput and FragmentInput agree on every varying.
    varying_decls.sort_by(|a, b| a.0.cmp(&b.0));
    let inputs: Vec<String> = varying_decls
        .iter()
        .enumerate()
        .map(|(i, (name, attr, ty))| format!("    @location({i}) {attr}{name}: {ty},"))
        .collect();
    // Varyings are read as module-scope globals by the engine's chunks; copy them from `input`.
    let mut prelude = String::from("    pcPosition = input.position;\n");
    let mut privates = String::from("var<private> pcPosition: vec4f;\n");
    for (name, ty) in &varyings {
        privates += &format!("var<private> {name}: {ty};\n");
        prelude += &format!("    {name} = input.{name};\n");
    }
    let body = body.replacen(
        "fn fragmentMain(input: FragmentInput) -> FragmentOutput {",
        &format!("fn fragmentMain(input: FragmentInput) -> FragmentOutput {{\n{prelude}"),
        1,
    );
    let aliases = format!("alias half = f32;\nalias half2 = vec2f;\nalias half3 = vec3f;\nalias half4 = vec4f;\nalias half3x3 = mat3x3f;\n\n{privates}\n");
    let io = format!(
        "struct FragmentInput {{\n    @builtin(position) position: vec4f,\n{}\n}};\n\nstruct FragmentOutput {{\n    @location(0) color: vec4f,\n}};\n\n",
        inputs.join("\n")
    );
    format!("{aliases}{io}{body}")
}

/// Rewrites the `@group(g) @binding(b)` of every resource declaration to the value given for its
/// name in `table` (name -> (group, binding)). Used so that both pipeline stages agree.
pub fn rebind(src: &str, table: &std::collections::HashMap<String, (u32, u32)>) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("@group(") {
            let indent = &line[..line.len() - trimmed.len()];
            let name = trimmed
                .split_whitespace()
                .find(|t| t.ends_with(':'))
                .map(|t| t.trim_end_matches(':'));
            if let Some((g, b)) = name.and_then(|n| table.get(n)) {
                // Replace only the @group and @binding attributes; the declaration is unchanged.
                let rest = trimmed.split_once(" var").map(|(_, r)| r).unwrap_or("");
                out += &format!("{indent}@group({g}) @binding({b}) var{rest}\n");
                continue;
            }
        }
        out += line;
        out.push('\n');
    }
    out
}

/// Assigns one (group, binding) per resource name across several stages: the union of resources,
/// numbered per group in name order. Stages sharing a name get the same slot.
pub fn unify(
    stage_bindings: &[&[Binding]],
) -> (std::collections::HashMap<String, (u32, u32)>, Vec<Binding>) {
    use std::collections::{BTreeMap, HashMap};
    let mut by_name: BTreeMap<(u32, String), Binding> = BTreeMap::new();
    for stage in stage_bindings {
        for b in stage.iter() {
            by_name
                .entry((b.group, b.name.clone()))
                .or_insert_with(|| b.clone());
        }
    }
    let mut next: HashMap<u32, u32> = HashMap::new();
    let mut table = HashMap::new();
    let mut unified = Vec::new();
    for ((group, name), mut b) in by_name {
        let slot = next.entry(group).or_insert(0);
        b.binding = *slot;
        b.group = group;
        table.insert(name, (group, *slot));
        *slot += 1;
        unified.push(b);
    }
    (table, unified)
}
