#!/usr/bin/env python3
"""Serve the CounterSplat WebGPU canvas and its local level catalog on loopback.

Build the browser package first. Serves preview files, declared scene resources, and local levels.
"""
import argparse
import json
import mimetypes
import os
import re
import unicodedata
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from functools import lru_cache
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
PREVIEW = (ROOT / "preview").resolve()
LEVELS = (ROOT / "assets" / "levels").resolve()
SCENES = (ROOT / "assets" / "scenes").resolve()


def safe_child(root: Path, relative: str):
    """Resolve a URL path without serving files outside its explicitly selected root."""
    candidate = (root / relative).resolve()
    return candidate if candidate.is_relative_to(root) and candidate.is_file() else None


def package_resource(root: Path, relative: str):
    """Serve generated browser bindings only from the ignored artifact directory."""
    return safe_child(root / ".artifacts" / "web" / "pkg", relative)


def level_resource(root: Path, relative: str):
    """Serve the catalog or files within an eight-character scene ID directory."""
    if relative == "catalog.json":
        return safe_child(root, relative)
    parts = relative.split("/", 1)
    if len(parts) != 2 or not re.fullmatch(r"[0-9a-f]{8}", parts[0]):
        return None
    level_root = (root / parts[0]).resolve()
    if not level_root.is_relative_to(root.resolve()):
        return None
    return safe_child(level_root, parts[1])


def scene_resource(root: Path, filename: str):
    """Serve only simple JSON filenames from the scenes directory."""
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,80}\.json", filename):
        return None
    return safe_child(root, filename)


def scene_catalog(root: Path):
    """Discover playable scenes without exposing unrelated files or subdirectories."""
    scenes = (root / "assets" / "scenes").resolve()
    entries = []
    for path in sorted(scenes.glob("*.json"), key=lambda p: (p.name != "gameplay.json", p.name))[:256]:
        if scene_resource(scenes, path.name) is None:
            continue
        try:
            if path.stat().st_size > 4 * 1024 * 1024:
                continue
            scene = json.loads(path.read_text(encoding="utf-8"))
            resources = scene.get("resources", {})
            assets = resources.get("assets", {})
            preview = resources.get("preview", {})
            level_id = preview.get("level_asset", "level")
            player = any(e.get("id") == "player" and "PlayerCamera" in e.get("components", {})
                         for e in scene.get("entities", []) if isinstance(e, dict))
            name = scene.get("name")
            if (not player or not isinstance(name, str) or not name.strip() or len(name.encode("utf-8")) > 512
                    or any(unicodedata.category(c) == "Cc" for c in name)
                    or assets.get(level_id, {}).get("kind") != "sog_level"
                    or assets.get("pause_menu", {}).get("kind") != "scene"):
                continue
            entries.append({"id": path.name, "name": name})
        except (OSError, ValueError, AttributeError, TypeError):
            continue
    return {"version": 1, "scenes": entries}


def _lexical_path(path: Path):
    """Normalize a declaration without following symlinks or requiring a file."""
    return os.path.normcase(os.path.abspath(path))


def _scene_signature(scenes: Path):
    """Invalidate the index when a scene is added, replaced, edited, or removed."""
    result = []
    for candidate in sorted(scenes.glob("*.json")):
        try:
            info = candidate.stat()
            result.append((candidate.name, str(candidate.resolve()), info.st_mtime_ns,
                           info.st_ctime_ns, info.st_size))
        except OSError:
            continue
    return tuple(result)


@lru_cache(maxsize=16)
def _declared_index(root: Path, signature: tuple):
    # Index exact declared paths once, rather than resolving every frame on every
    # request. Recheck the requested target's real path when it is served so a
    # changed symlink can never escape the assets directory.
    scenes = (root / "assets" / "scenes").resolve()
    declared = set()
    for name, *_ in signature:
        try:
            scene_path = scene_resource(scenes, name)
            if scene_path is None:
                continue
            scene = json.loads(scene_path.read_text(encoding="utf-8"))
            resources = scene.get("resources", {})
            definitions = list(resources.get("assets", {}).values()) + list(resources.get("scripts", {}).values())
            for definition in definitions:
                uri = definition.get("uri", "")
                if not isinstance(uri, str):
                    continue
                parsed = urlsplit(uri)
                if not parsed.scheme and not parsed.netloc and not parsed.path.startswith("/"):
                    declared.add(_lexical_path(scene_path.parent / unquote(parsed.path)))
        except (OSError, ValueError, AttributeError, TypeError):
            continue
    return frozenset(declared)


def declared_scene_resource(root: Path, relative: str):
    """Resolve exactly declared resources, confined to their assets directory."""
    root = root.resolve()
    scenes = (root / "assets" / "scenes").resolve()
    lexical = _lexical_path(root / relative)
    if lexical not in _declared_index(root, _scene_signature(scenes)):
        return False, None
    try:
        requested = (root / relative).resolve()
        assets = (root / "assets").resolve()
        return True, requested if requested.is_relative_to(assets) and requested.is_file() else None
    except OSError:
        return True, None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--level", type=Path, help="optional legacy level folder containing lod-meta.json")
    parser.add_argument("--port", type=int, default=18763)
    args = parser.parse_args()
    level = args.level.resolve() if args.level else None
    if level is not None and not (level / "lod-meta.json").is_file():
        parser.error("--level must contain lod-meta.json")

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            route = unquote(urlsplit(self.path).path)
            if route in ("/", "/index.html"):
                path = safe_child(PREVIEW, "index.html")
            elif route in ("/main.js", "/decoder.js", "/lua_runtime.js", "/asset_loader.js"):
                path = safe_child(PREVIEW, route.removeprefix("/"))
            elif route == "/pkg/scene.json":
                path = safe_child(ROOT / "assets" / "scenes", "gameplay.json")
            elif route == "/assets/scenes/catalog.json":
                content = json.dumps(scene_catalog(ROOT), ensure_ascii=False).encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "application/json; charset=utf-8")
                self.send_header("Content-Length", str(len(content)))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(content)
                return
            elif route.startswith("/assets/scenes/"):
                path = scene_resource(SCENES, route.removeprefix("/assets/scenes/"))
            elif route.startswith("/assets/levels/"):
                path = level_resource(LEVELS, route.removeprefix("/assets/levels/"))
            elif route.startswith("/assets/"):
                _, path = declared_scene_resource(ROOT, route.removeprefix("/"))
            elif route.startswith("/levels/"):
                path = level_resource(LEVELS, route.removeprefix("/levels/"))
            elif route == "/pkg/engine_api.lua":
                path = safe_child(ROOT / "crates" / "scripting-lua" / "src", "engine_api.lua")
            elif route.startswith("/pkg/"):
                relative = route.removeprefix("/pkg/")
                declared, path = declared_scene_resource(ROOT, relative)
                if not declared:
                    path = package_resource(ROOT, relative)
            elif route.startswith("/level/") and level is not None:
                path = safe_child(level, route.removeprefix("/level/"))
            else:
                path = None
            if path is None:
                self.send_error(404)
                return
            try:
                content = path.read_bytes()
            except OSError:
                self.send_error(503, "File is unavailable")
                return
            content_type = {".wasm": "application/wasm", ".js": "text/javascript; charset=utf-8"}.get(
                path.suffix.lower(), mimetypes.guess_type(path.name)[0] or "application/octet-stream"
            )
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(content)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(content)

    with ThreadingHTTPServer(("127.0.0.1", args.port), Handler) as server:
        print(f"CounterSplat canvas: http://127.0.0.1:{args.port}/ (levels: {LEVELS})", flush=True)
        server.serve_forever()


if __name__ == "__main__":
    main()
