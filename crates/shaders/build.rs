//! Embeds every extracted WGSL chunk and the include-key map into the crate.
use std::{
    fs,
    path::{Path, PathBuf},
};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            walk(&p, out)
        } else if p.extension().is_some_and(|e| e == "wgsl") {
            out.push(p)
        }
    }
}

fn main() {
    let root = PathBuf::from("shaders");
    println!("cargo:rerun-if-changed=shaders");
    println!("cargo:rerun-if-changed=chunk_map.json");
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    let mut src = String::from("pub static CHUNKS: &[(&str, &str)] = &[\n");
    for f in &files {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let abs = fs::canonicalize(f).unwrap();
        src += &format!(
            "    ({:?}, include_str!({:?})),\n",
            rel,
            abs.to_string_lossy()
        );
    }
    src += "];\n\npub static KEYS: &[(&str, &str)] = &[\n";
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&fs::read_to_string("chunk_map.json").unwrap()).unwrap();
    for (k, v) in &map {
        src += &format!("    ({:?}, {:?}),\n", k, v.as_str().unwrap());
    }
    src += "];\n";
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("chunks.rs");
    fs::write(out, src).unwrap();
}
