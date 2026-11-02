#!/usr/bin/env python3
"""Test Pages selection, resource confinement, and stale-output removal."""

import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import build_pages as pages


class PagesBuildTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.artifacts = self.root / ".artifacts"
        self.artifacts.mkdir()
        self.root_patch = patch.object(pages, "ROOT", self.root)
        self.artifacts_patch = patch.object(pages, "ARTIFACTS", self.artifacts)
        self.root_patch.start()
        self.artifacts_patch.start()
        for name in pages.SITE_FILES:
            self.write("docs/" + name, "site")
        for name in pages.PREVIEW_FILES:
            self.write("preview/" + name, "preview")
        self.write("preview/index.html", '<html><div class="header-actions"></div>local preview</html>')
        for name in pages.PACKAGE_FILES:
            self.write(".artifacts/web/pkg/" + name, "package")
        self.write(".artifacts/web/pkg/assets/old-private-file.json", "not selected")
        self.write_json("assets/scenes/gameplay.json", {
            "name": "Test scene",
            "resources": {
                "assets": {"sky": {"uri": "../images/sky.hdr"}},
                "scripts": {"behavior": {"uri": "../scripts/behavior.lua"}},
            },
        })
        self.write_json("assets/scenes/menu.json", {"resources": {}})
        self.write_json("assets/scenes/level-selector.json", {"resources": {}})
        self.write("assets/images/sky.hdr", "sky")
        self.write("assets/scripts/behavior.lua", "return 1")
        self.write_json("assets/levels/1234abcd/meta.json", {
            "means": {"files": ["means_l.webp", "means_u.webp"]},
            "quats": {"files": ["quats.webp"]},
        })
        for name in ("means_l.webp", "means_u.webp", "quats.webp"):
            self.write("assets/levels/1234abcd/" + name, name)
        self.write("assets/levels/1234abcd/unreferenced.webp", "not selected")
        self.write("assets/levels/1234abcd/.env", "do not publish")
        self.write("assets/levels/87654321/secret.webp", "different level")
        self.level = {"id": "1234abcd", "name": "Test", "creator": "Artist", "format": "single_sog"}

    def tearDown(self):
        self.root_patch.stop()
        self.artifacts_patch.stop()
        self.temp.cleanup()

    def write(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf8")
        return path

    def write_json(self, name, value):
        return self.write(name, json.dumps(value))

    def test_empty_selection_publishes_only_site_and_unavailable_launcher(self):
        out = self.artifacts / "pages"
        pages.build([], out)
        self.assertTrue((out / "index.html").is_file())
        self.assertFalse((out / "game/play").exists())
        self.assertEqual(json.loads((out / "game/availability.json").read_text())["available"], False)
        self.assertFalse(any(p.name == ".env" for p in out.rglob("*")))

    def test_selected_level_copies_only_scene_and_referenced_level_data(self):
        out = self.artifacts / "pages"
        pages.build([self.level], out, local=True)
        game = out / "game/play"
        self.assertIn('href="../../"', (game / "index.html").read_text(encoding="utf8"))
        self.assertTrue((game / "assets/images/sky.hdr").is_file())
        self.assertTrue((game / "assets/levels/1234abcd/means_l.webp").is_file())
        self.assertFalse((game / "assets/levels/1234abcd/unreferenced.webp").exists())
        self.assertFalse((game / "assets/levels/1234abcd/.env").exists())
        self.assertFalse((game / "assets/levels/87654321").exists())
        self.assertFalse((game / "pkg/assets").exists(), "stale package assets must never be copied")
        inventory = json.loads((game / "assets/levels/1234abcd/download-inventory.json").read_text())
        listed = {item["path"] for item in inventory["files"]}
        self.assertIn("means_l.webp", listed)
        self.assertNotIn("unreferenced.webp", listed)
        self.assertNotIn("scene.voxel.bin", listed)
        self.assertEqual(json.loads((out / "game/availability.json").read_text())["available"], True)
        catalog = json.loads((game / "assets/levels/catalog.json").read_text())
        self.assertEqual(catalog["levels"][0]["manifest"], "assets/levels/1234abcd/meta.json")
        # A subsequent landing-only build must remove the previous game's assets.
        pages.build([], out)
        self.assertFalse((out / "game/play").exists())
        self.assertEqual(json.loads((out / "game/availability.json").read_text())["available"], False)

    def test_streamed_level_includes_each_lod_and_selected_collision(self):
        self.write_json("assets/levels/1234abcd/lod-meta.json", {"filenames": ["0/meta.json", "1/meta.json"]})
        for chunk in ("0", "1"):
            self.write_json("assets/levels/1234abcd/" + chunk + "/meta.json",
                            {"means": {"files": ["means_l.webp"]}})
            self.write("assets/levels/1234abcd/" + chunk + "/means_l.webp", chunk)
        self.write("assets/levels/1234abcd/scene.voxel.json", "{}")
        self.write("assets/levels/1234abcd/scene.voxel.bin", "voxel")
        self.write("assets/levels/1234abcd/preview.webp", "preview")
        selected = dict(self.level, format="streamed_sog", collision=True, thumbnail=True)
        out = self.artifacts / "pages"
        pages.build([selected], out, local=True)
        data = out / "game/play/assets/levels/1234abcd"
        for name in ("lod-meta.json", "0/meta.json", "0/means_l.webp", "1/meta.json",
                     "1/means_l.webp", "scene.voxel.json", "scene.voxel.bin", "preview.webp"):
            self.assertTrue((data / name).is_file(), name)
        inventory = json.loads((data / "download-inventory.json").read_text())
        self.assertIn("scene.voxel.bin", {item["path"] for item in inventory["files"]})
        catalog = json.loads((out / "game/play/assets/levels/catalog.json").read_text())
        self.assertEqual(catalog["levels"][0]["manifest"], "assets/levels/1234abcd/lod-meta.json")
        self.assertEqual(catalog["levels"][0]["thumbnail"], "assets/levels/1234abcd/preview.webp")

    def test_publish_build_with_tracked_assets_uses_only_selected_inputs(self):
        tracked = {p.relative_to(self.root).as_posix()
                   for p in (self.root / "assets").rglob("*") if p.is_file()}
        out = self.artifacts / "pages"
        with patch.object(pages, "tracked_assets", return_value=tracked):
            pages.build([self.level], out)
        game = out / "game/play"
        self.assertTrue((game / "assets/levels/1234abcd/means_l.webp").is_file())
        self.assertFalse((game / "assets/levels/1234abcd/.env").exists())
        self.assertFalse((game / "assets/levels/1234abcd/unreferenced.webp").exists())
        self.assertFalse((game / "pkg/assets").exists())

    def test_publish_build_requires_explicitly_tracked_assets(self):
        out = self.artifacts / "pages"
        with patch.object(pages, "tracked_assets", return_value=set()):
            with self.assertRaisesRegex(ValueError, "not tracked"):
                pages.build([self.level], out)
        self.assertFalse(out.exists(), "validation must happen before output is created")

    def test_unsafe_paths_and_unmanaged_output_are_rejected(self):
        for name in ("../secret", "/absolute", "a/../b", "a\\\\b", "a//b"):
            with self.assertRaises(ValueError):
                pages.safe_relative(name)
        out = self.artifacts / "user-data"
        out.mkdir()
        (out / "keep.txt").write_text("keep", encoding="utf8")
        with self.assertRaisesRegex(ValueError, "without the CounterSplat Pages marker"):
            pages.build([], out)
        self.assertEqual((out / "keep.txt").read_text(), "keep")
        with self.assertRaises(ValueError):
            pages.output_directory(self.root / "outside")


if __name__ == "__main__":
    unittest.main()
