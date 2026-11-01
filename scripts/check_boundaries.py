#!/usr/bin/env python3
"""Check workspace dependency direction and reject literal includes outside each owning crate."""
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ALLOWED = {
    "countersplat-core": set(),
    "countersplat-audio": set(),
    "countersplat-audio-ecs": {"countersplat-audio", "countersplat-ecs"},
    "countersplat-audio-runtime": {"countersplat-audio", "countersplat-runtime"},
    "countersplat-scene": set(),
    "countersplat-scene-ecs": {"countersplat-scene", "countersplat-ecs"},
    "countersplat-ui": set(),
    "countersplat-ui-ecs": {"countersplat-ui", "countersplat-ecs"},
    "countersplat-ui-wgpu": {"countersplat-ui"},
    "countersplat-ecs": set(),
    "countersplat-scripting-lua": {"countersplat-ecs"},
    "countersplat-runtime": set(),
    "countersplat-assets": set(),
    "countersplat-shaders": set(),
    "countersplat-physics": {"countersplat-assets"},
    "countersplat-render": {"countersplat-assets", "countersplat-shaders"},
    "countersplat-threaded-demo": {"countersplat-audio", "countersplat-audio-ecs", "countersplat-core", "countersplat-ecs", "countersplat-scripting-lua", "countersplat-runtime", "countersplat-physics", "countersplat-ui", "countersplat-ui-ecs", "countersplat-scene", "countersplat-scene-ecs"},
    "countersplat-web": {"countersplat-core", "countersplat-assets", "countersplat-render", "countersplat-ecs", "countersplat-ui", "countersplat-ui-ecs", "countersplat-ui-wgpu", "countersplat-scene", "countersplat-scene-ecs"},
    "countersplat-viewer": {"countersplat-core", "countersplat-assets", "countersplat-render", "countersplat-runtime", "countersplat-ecs", "countersplat-scene", "countersplat-scene-ecs"},
}
INCLUDE = re.compile(r'include_(?:str|bytes)!\s*\(\s*"([^"\n]+)"')
PATH_ATTR = re.compile(r'#\s*\[\s*path\s*=\s*"([^"\n]+)"\s*\]')


def main():
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    errors = []
    seen = set()
    for member in workspace["workspace"]["members"]:
        crate = (ROOT / member).resolve()
        manifest = tomllib.loads((crate / "Cargo.toml").read_text(encoding="utf-8"))
        name = manifest["package"]["name"]
        seen.add(name)
        if name not in ALLOWED:
            errors.append(f"{name}: package is not in the boundary policy")
            continue
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            for key, declaration in manifest.get(section, {}).items():
                dependency = declaration.get("package", key) if isinstance(declaration, dict) else key
                if dependency.startswith("countersplat-") and dependency not in ALLOWED[name]:
                    errors.append(f"{name}: disallowed {section} dependency {dependency}")
        for source in crate.rglob("*.rs"):
            for reference in INCLUDE.findall(source.read_text(encoding="utf-8")) + PATH_ATTR.findall(source.read_text(encoding="utf-8")):
                # Escaped format strings are not literal source paths.
                if "{" in reference or "}" in reference:
                    continue
                target = (source.parent / reference).resolve()
                if not target.is_relative_to(crate):
                    errors.append(f"{source.relative_to(ROOT)}: include leaves its owning crate: {reference}")
    for missing in ALLOWED.keys() - seen:
        errors.append(f"{missing}: package is missing from the workspace")
    if errors:
        print("Boundary check failed:", *errors, sep="\n  ", file=sys.stderr)
        return 1
    print(f"Boundary check passed for {len(seen)} workspace packages.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
