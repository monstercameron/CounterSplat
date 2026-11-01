#!/usr/bin/env python3
"""Serve the CounterSplat WebGPU canvas and its local level catalog on loopback.

Build the browser package first. Serves preview files, declared scene resources, and local levels.
"""
import argparse
import json
import mimetypes
import re
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
PREVIEW = (ROOT / "preview").resolve()
LEVELS = (ROOT / "assets" / "levels").resolve()


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


def declared_scene_resource(root: Path, relative: str):
    """Resolve resources declared by known scenes, confined to their assets directory."""
    requested = (root / relative).resolve()
    assets = (root / "assets").resolve()
    for name in ("gameplay.json", "menu.json", "level-selector.json"):
        try:
            scene_path = root / "assets" / "scenes" / name
            scene = json.loads(scene_path.read_text(encoding="utf-8"))
            resources = scene.get("resources", {})
            definitions = list(resources.get("assets", {}).values()) + list(resources.get("scripts", {}).values())
            for definition in definitions:
                uri = definition.get("uri", "")
                if not isinstance(uri, str):
                    continue
                parsed = urlsplit(uri)
                if not parsed.scheme and not parsed.netloc and not parsed.path.startswith("/"):
                    if (scene_path.parent / unquote(parsed.path)).resolve() == requested:
                        return True, requested if requested.is_relative_to(assets) and requested.is_file() else None
        except (OSError, ValueError, AttributeError, TypeError):
            continue
    return False, None


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
            elif route in ("/assets/scenes/gameplay.json", "/assets/scenes/level-selector.json", "/assets/scenes/menu.json"):
                path = safe_child(ROOT / "assets" / "scenes", route.rsplit("/", 1)[-1])
            elif route.startswith("/assets/levels/"):
                path = level_resource(LEVELS, route.removeprefix("/assets/levels/"))
            elif route.startswith("/assets/"):
                _, path = declared_scene_resource(ROOT, route.removeprefix("/"))
            elif route.startswith("/levels/"):
                path = level_resource(LEVELS, route.removeprefix("/levels/"))
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
