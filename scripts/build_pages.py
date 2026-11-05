#!/usr/bin/env python3
"""Assemble a static GitHub Pages site without publishing unselected local assets."""

import argparse
import json
import math
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = ROOT / ".artifacts"
MARKER = ".countersplat-pages-output"
MARKER_CONTENT = "CounterSplat generated Pages output v1\n"
MAX_SITE_BYTES = 1_000_000_000
ID_RE = re.compile(r"^[0-9a-f]{8}$")
PACKAGE_FILES = [
    "countersplat_web.js", "countersplat_web_bg.wasm", "engine_api.lua",
    "metadata-worker.js",
    "minimap-worker.js",
    "skybox-worker.js", "voxel-worker.js",
    "wasmoon/index.js", "wasmoon/glue.wasm",
]
SITE_FILES = ["index.html", "site.css", "site.js", "game/index.html", "game/launcher.js", "favicon.svg", "social-card.png"]
HOSTED_RE = re.compile(r"^https://[A-Za-z0-9.-]+/[A-Za-z0-9._~/-]*/$")
PREVIEW_FILES = ["main.js", "decoder.js", "lua_runtime.js"]


def json_file(path):
    return json.loads(path.read_text(encoding="utf-8"))


def within(path, parent):
    try:
        path.relative_to(parent)
        return True
    except ValueError:
        return False


def safe_relative(value):
    if not isinstance(value, str) or not value or "\\" in value or ":" in value:
        raise ValueError("Invalid asset path.")
    parts = value.split("/")
    if any(part in ("", ".", "..") for part in parts) or any(ord(c) < 32 for c in value):
        raise ValueError("Invalid asset path.")
    return PurePosixPath(value)


def safe_asset(path):
    """Require a regular, non-symlink file within the project's asset tree."""
    assets = (ROOT / "assets").resolve()
    resolved = path.resolve()
    if not within(resolved, assets) or not path.is_file():
        raise ValueError("Asset is missing or outside assets/: " + str(path))
    cursor = path
    while cursor != ROOT and cursor != cursor.parent:
        if cursor.is_symlink():
            raise ValueError("Asset symlinks are not included: " + str(path))
        cursor = cursor.parent
    return resolved


def tracked_assets():
    output = subprocess.check_output(["git", "ls-files", "-z", "--", "assets"], cwd=ROOT)
    return set(output.decode("utf-8").split("\0"))


def require_asset(path, tracked, local):
    resolved = safe_asset(path)
    name = resolved.relative_to(ROOT.resolve()).as_posix()
    if not local and name not in tracked:
        raise ValueError("Selected asset is not tracked: " + name)
    return resolved


def optional_asset(path, tracked, local):
    if not path.exists():
        return None
    resolved = safe_asset(path)
    name = resolved.relative_to(ROOT.resolve()).as_posix()
    return resolved if local or name in tracked else None


def validate_level(value):
    if not isinstance(value, dict):
        raise ValueError("Each selected level must be an object.")
    identifier = value.get("id")
    if not isinstance(identifier, str) or not ID_RE.fullmatch(identifier):
        raise ValueError("Level IDs must be eight lowercase hexadecimal characters.")
    if value.get("format") not in ("single_sog", "streamed_sog"):
        raise ValueError("Level " + identifier + " has an unsupported format.")
    for field in ("name", "creator"):
        item = value.get(field)
        if not isinstance(item, str) or not item.strip() or len(item) > 512 or any(ord(c) < 32 for c in item):
            raise ValueError("Level " + identifier + " needs a valid " + field + ".")
    for field in ("collision", "thumbnail", "rotate_z_180"):
        if field in value and not isinstance(value[field], bool):
            raise ValueError("Level " + identifier + " has an invalid " + field + " setting.")
    hosted = value.get("hosted")
    if hosted is not None and (not isinstance(hosted, str) or len(hosted) > 512 or not HOSTED_RE.fullmatch(hosted)):
        raise ValueError("Level " + identifier + " needs an https:// hosted base URL ending in /.")
    if "source_url" in value and (not isinstance(value["source_url"], str) or not value["source_url"].startswith("https://")):
        raise ValueError("Level " + identifier + " has an invalid source_url.")
    scale = value.get("player_units_per_meter")
    if scale is not None and (isinstance(scale, bool) or not isinstance(scale, (int, float)) or
                              not math.isfinite(scale) or not 0.5 <= scale <= 2.5):
        raise ValueError("Level " + identifier + " has an invalid player scale.")
    return value


def selection():
    value = json_file(ROOT / "docs" / "pages-assets.json")
    if not isinstance(value, dict) or value.get("version") != 1 or not isinstance(value.get("levels"), list):
        raise ValueError("Invalid docs/pages-assets.json.")
    levels = [validate_level(item) for item in value["levels"]]
    if len(levels) > 256 or len({item["id"] for item in levels}) != len(levels):
        raise ValueError("Selected level IDs must be unique, with at most 256 levels.")
    return levels


def local_level(identifier):
    if not ID_RE.fullmatch(identifier):
        raise ValueError("Local preview level ID must be eight lowercase hexadecimal characters.")
    catalog = json_file(ROOT / "assets" / "levels" / "catalog.json")
    item = next((item for item in catalog.get("levels", []) if item.get("id") == identifier), None)
    if not item:
        raise ValueError("Level is not in the local catalog: " + identifier)
    level = {key: item[key] for key in ("id", "name", "creator", "format")}
    for key in ("rotate_z_180", "player_units_per_meter"):
        if key in item:
            level[key] = item[key]
    base = ROOT / "assets" / "levels" / identifier
    level["collision"] = (base / "scene.voxel.json").is_file() and (base / "scene.voxel.bin").is_file()
    level["thumbnail"] = (base / "preview.webp").is_file()
    return validate_level(level)


def level_files(level, tracked, local):
    base = ROOT / "assets" / "levels" / level["id"]
    # A hosted level streams its splats and collision from its publisher (`hosted`); only its small viewer settings
    # (the start camera) and provenance record are copied into the site, when tracked.
    if level.get("hosted"):
        return {path for name in ("viewer-settings.json", "source.json")
                if (path := optional_asset(base / name, tracked, local))}
    single = level["format"] == "single_sog"
    manifest_name = "meta.json" if single else "lod-meta.json"
    manifest = require_asset(base / manifest_name, tracked, local)
    result = {manifest}
    data = json_file(manifest)
    if single:
        metadata = [manifest]
    else:
        names = data.get("filenames")
        if not isinstance(names, list) or not names or len(names) > 10000:
            raise ValueError("Invalid LOD metadata for " + level["id"])
        metadata = [require_asset(base.joinpath(*safe_relative(name).parts), tracked, local) for name in names]
        result.update(metadata)
    for path in metadata:
        chunk = json_file(path)
        for key in ("means", "quats", "scales", "sh0", "shN"):
            part = chunk.get(key) or {}
            names = part.get("files", [])
            if not isinstance(names, list):
                raise ValueError("Invalid splat references in " + str(path))
            for name in names:
                result.add(require_asset(path.parent.joinpath(*safe_relative(name).parts), tracked, local))
    for name in ("viewer-settings.json", "source.json"):
        path = optional_asset(base / name, tracked, local)
        if path:
            result.add(path)
    if level.get("collision", False):
        for name in ("scene.voxel.json", "scene.voxel.bin"):
            result.add(require_asset(base / name, tracked, local))
    if level.get("thumbnail", False):
        result.add(require_asset(base / "preview.webp", tracked, local))
    return result


def scene_files(tracked, local):
    result = set()
    skipped = []
    scenes = ROOT / "assets" / "scenes"
    for name in ("gameplay.json", "menu.json", "level-selector.json"):
        result.add(require_asset(scenes / name, tracked, local))
    for name in ("gameplay.json", "menu.json"):
        scene = json_file(scenes / name)
        resources = scene.get("resources", {})
        for category in ("assets", "scripts"):
            for entry in resources.get(category, {}).values():
                uri = entry.get("uri", "")
                if not isinstance(uri, str):
                    raise ValueError("Invalid scene resource URI.")
                if uri.startswith("/") or re.match(r"^[A-Za-z][A-Za-z0-9+.-]*:", uri):
                    continue
                path = scenes.joinpath(*PurePosixPath(uri).parts)
                # Assets loaded on demand ("streamable") that are not available are left out: the game requests
                # them only when they are shown (the weapon footage, while the scene spawns with the weapon stowed).
                if entry.get("load") == "streamable" and optional_asset(path, tracked, local) is None:
                    skipped.append(path)
                    continue
                result.add(require_asset(path, tracked, local))
    if skipped:
        folders = sorted({p.parent.resolve().relative_to(ROOT.resolve()).as_posix() for p in skipped})
        print(f"Pages build: left out {len(skipped)} unavailable on-demand scene assets in {len(folders)} folder(s), e.g. {folders[0]}")
    return result


def inventory():
    root = ROOT / "assets" / "levels"
    found = []
    if root.is_dir():
        for folder in sorted(root.iterdir()):
            if not folder.is_dir() or not ID_RE.fullmatch(folder.name) or folder.is_symlink():
                continue
            files = [p for p in folder.rglob("*") if p.is_file() and not p.is_symlink()]
            total = sum(p.stat().st_size for p in files)
            source = folder / "source.json"
            try:
                name = json_file(source).get("title", folder.name) if source.is_file() else folder.name
            except (OSError, ValueError):
                name = folder.name
            found.append((folder.name, name, len(files), total))
    print("Available local levels (nothing is staged or published):")
    for identifier, name, count, total in found:
        print(f"  {identifier}  {name}  {count} files  {total / 1048576:.1f} MiB")
    if not found:
        print("  No local level directories found.")
    for name in ("assets/images/first-person/playback", "assets/images/environments", "assets/sound/first-person"):
        folder = ROOT / name
        files = [p for p in folder.rglob("*") if p.is_file() and not p.is_symlink()] if folder.is_dir() else []
        print(f"  {name}: {len(files)} files, {sum(p.stat().st_size for p in files) / 1048576:.1f} MiB")


def output_directory(value):
    root = ARTIFACTS.resolve()
    project = ROOT.resolve()
    if root == project or not within(root, project):
        raise ValueError("The .artifacts directory must stay inside the project.")
    candidate = Path(value)
    candidate = ROOT / candidate if not candidate.is_absolute() else candidate
    if candidate.is_symlink():
        raise ValueError("Pages output cannot be a symbolic link.")
    out = candidate.resolve()
    if out == root or not within(out, root):
        raise ValueError("Pages output must be a subdirectory of .artifacts/.")
    return out


def prepare_output(out):
    # Only remove a previously generated directory after checking its resolved
    # absolute location and our marker. Never remove an arbitrary user directory.
    if out.exists():
        marker = out / MARKER
        if out.is_symlink() or not marker.is_file() or marker.read_text(encoding="utf8") != MARKER_CONTENT:
            raise ValueError("Output already exists without the CounterSplat Pages marker: " + str(out))
        shutil.rmtree(out)
    out.mkdir(parents=True)
    (out / MARKER).write_text(MARKER_CONTENT, encoding="utf8")


def build(levels, out, local=False):
    sources = {}
    generated = {}

    def add(destination, source):
        destination = str(safe_relative(destination))
        if not source.is_file():
            raise ValueError("Build input is missing: " + str(source))
        if destination in sources and sources[destination] != source:
            raise ValueError("Conflicting build input: " + destination)
        sources[destination] = source

    for name in SITE_FILES:
        add(name, ROOT / "docs" / name)
    generated[".nojekyll"] = b""
    generated["game/availability.json"] = json.dumps({"available": bool(levels)}).encode() + b"\n"
    if levels:
        tracked = set() if local else tracked_assets()
        base = "game/play/"
        preview_html = (ROOT / "preview" / "index.html").read_text(encoding="utf8")
        header = '<div class="header-actions">'
        if header not in preview_html:
            raise ValueError("The browser preview header no longer supports the Pages return link.")
        back = '<a class="arrange" href="../../">Project</a>'
        preview_html = preview_html.replace(header, header + back, 1).replace("local preview", "browser demo")
        title_end = "</title>"
        if title_end not in preview_html:
            raise ValueError("The browser preview no longer has a title to add the page tags after.")
        tags = (
            '\n  <meta name="description" content="The CounterSplat browser demo: a first-person WebGPU game inside a '
            'Gaussian-splat scan.">\n  <link rel="icon" href="../../favicon.svg" type="image/svg+xml">\n'
            '  <meta name="theme-color" content="#10191a">\n'
            '  <meta property="og:title" content="Play CounterSplat">\n'
            '  <meta property="og:image" content="https://monstercameron.github.io/CounterSplat/social-card.png">'
        )
        preview_html = preview_html.replace(title_end, title_end + tags, 1)
        generated[base + "index.html"] = preview_html.encode("utf8")
        for name in PREVIEW_FILES:
            add(base + name, ROOT / "preview" / name)
        for name in PACKAGE_FILES:
            add(base + "pkg/" + name, ARTIFACTS / "web" / "pkg" / name)
        for path in scene_files(tracked, local):
            add(base + path.relative_to(ROOT.resolve()).as_posix(), path)
        for level in levels:
            chosen = level_files(level, tracked, local)
            level_root = (ROOT / "assets" / "levels" / level["id"]).resolve()
            for path in chosen:
                add(base + path.relative_to(ROOT.resolve()).as_posix(), path)
            if level.get("hosted"):
                continue
            # Report only files actually included in this site, rather than a
            # local download inventory that may list unselected collision data.
            inventory = {"scene_id": level["id"], "files": [
                {"path": path.relative_to(level_root).as_posix()} for path in sorted(chosen)
            ]}
            generated[base + "assets/levels/" + level["id"] + "/download-inventory.json"] = (
                json.dumps(inventory, ensure_ascii=False).encode("utf8") + b"\n"
            )
        catalog = {"version": 1, "levels": []}
        for level in levels:
            entry = {key: level[key] for key in ("id", "name", "creator", "format")}
            entry["manifest"] = "assets/levels/" + level["id"] + ("/meta.json" if level["format"] == "single_sog" else "/lod-meta.json")
            if level.get("thumbnail"):
                entry["thumbnail"] = "assets/levels/" + level["id"] + "/preview.webp"
            for key in ("rotate_z_180", "player_units_per_meter", "hosted"):
                if key in level:
                    entry[key] = level[key]
            catalog["levels"].append(entry)
        generated[base + "assets/levels/catalog.json"] = json.dumps(catalog, ensure_ascii=False, indent=2).encode("utf8") + b"\n"
        gameplay = json_file(ROOT / "assets/scenes/gameplay.json")
        scene_catalog = {"version": 1, "scenes": [{"id": "gameplay.json", "name": gameplay["name"]}]}
        generated[base + "assets/scenes/catalog.json"] = json.dumps(scene_catalog, ensure_ascii=False).encode("utf8") + b"\n"

    size = sum(path.stat().st_size for path in sources.values()) + sum(len(value) for value in generated.values())
    if size > MAX_SITE_BYTES:
        raise ValueError("Selected Pages output exceeds GitHub Pages' 1 GB site limit.")
    prepare_output(out)
    for name, source in sources.items():
        target = out.joinpath(*PurePosixPath(name).parts)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
    for name, data in generated.items():
        target = out.joinpath(*PurePosixPath(name).parts)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    print(f"Pages build: {out} ({size / 1048576:.1f} MiB, {len(levels)} selected level(s))")
    if not levels:
        print("No assets selected: the landing page and game availability page are ready.")


def main():
    # Level names may contain characters outside a Windows console's code page.
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="backslashreplace")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", default=".artifacts/pages", help="generated directory under .artifacts/")
    parser.add_argument("--preview-local", metavar="LEVEL_ID", help="locally test one ignored catalog level without selecting or committing assets")
    parser.add_argument("--inventory", action="store_true", help="show local asset sizes without staging or publishing")
    parser.add_argument("--check-selection", action="store_true", help="print true if a game build is selected")
    args = parser.parse_args()
    try:
        if args.inventory:
            inventory()
        elif args.check_selection:
            print("true" if selection() else "false")
        else:
            levels = [local_level(args.preview_local)] if args.preview_local else selection()
            build(levels, output_directory(args.output), local=bool(args.preview_local))
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        print("Pages build: " + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
