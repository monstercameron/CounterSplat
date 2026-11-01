#!/usr/bin/env python3
"""Exercise the local canvas server's path confinement."""
import tempfile
import unittest
from pathlib import Path

from preview import declared_scene_resource, level_resource, safe_child


class PreviewPaths(unittest.TestCase):
    def test_only_files_under_the_selected_root_are_served(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary).resolve()
            level = base / "level"
            level.mkdir()
            (level / "lod-meta.json").write_text("{}", encoding="utf-8")
            secret = base / "outside.txt"
            secret.write_text("outside", encoding="utf-8")
            self.assertEqual(safe_child(level, "lod-meta.json"), level / "lod-meta.json")
            self.assertIsNone(safe_child(level, "../outside.txt"))
            self.assertIsNone(safe_child(level, str(secret)))
            self.assertIsNone(safe_child(level, "."))
            self.assertIsNone(safe_child(level, "missing.json"))


    def test_local_level_paths_are_confined_to_scene_directories(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            level = root / "1234abcd"
            level.mkdir()
            (level / "lod-meta.json").write_text("{}", encoding="utf-8")
            (root / "catalog.json").write_text("{}", encoding="utf-8")
            (root / "secret.txt").write_text("private", encoding="utf-8")
            self.assertEqual(level_resource(root, "catalog.json"), root / "catalog.json")
            self.assertEqual(level_resource(root, "1234abcd/lod-meta.json"), level / "lod-meta.json")
            self.assertIsNone(level_resource(root, "1234abcd/../secret.txt"))
            self.assertIsNone(level_resource(root, "../secret.txt"))
            self.assertIsNone(level_resource(root, "other/secret.txt"))

    def test_only_declared_scene_resources_are_served(self):
        import json
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            scenes = root / "assets" / "scenes"
            scenes.mkdir(parents=True)
            images = root / "assets" / "images"
            images.mkdir()
            sky = images / "sky.hdr"
            sky.write_bytes(b"hdr")
            secret = root / "secret.txt"
            secret.write_text("private", encoding="utf-8")
            scene = {"resources": {"assets": {"sky": {"uri": "../images/sky.hdr"}},
                                   "scripts": {"escape": {"uri": "../../secret.txt"}}}}
            (scenes / "gameplay.json").write_text(json.dumps(scene), encoding="utf-8")
            self.assertEqual(declared_scene_resource(root, "assets/images/sky.hdr"), (True, sky))
            self.assertEqual(declared_scene_resource(root, "secret.txt"), (True, None))
            self.assertEqual(declared_scene_resource(root, "assets/other.txt"), (False, None))
            sky.unlink()
            self.assertEqual(declared_scene_resource(root, "assets/images/sky.hdr"), (True, None))

    def test_menu_scripts_are_declared_by_the_menu_scene(self):
        import json
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            scenes = root / "assets" / "scenes"
            scripts = root / "assets" / "scripts"
            scenes.mkdir(parents=True)
            scripts.mkdir()
            script = scripts / "menu.lua"
            script.write_text("return 1", encoding="utf-8")
            (scenes / "menu.json").write_text(json.dumps({"resources": {"scripts": {
                "menu": {"uri": "../scripts/menu.lua"}
            }}}), encoding="utf-8")
            self.assertEqual(declared_scene_resource(root, "assets/scripts/menu.lua"), (True, script))
            self.assertEqual(declared_scene_resource(root, "assets/scripts/other.lua"), (False, None))


if __name__ == "__main__":
    unittest.main()
