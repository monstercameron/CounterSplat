#!/usr/bin/env python3
"""Exercise the local canvas server's path confinement."""
import sys
import tempfile
import unittest
from pathlib import Path

# Importing the preview server should not create bytecode in the source tree.
sys.dont_write_bytecode = True
from preview import declared_scene_resource, level_resource, package_resource, safe_child, scene_catalog, scene_resource
from download_previews import preview_url


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


    def test_generated_package_is_served_from_artifacts_only(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            package = root / ".artifacts" / "web" / "pkg"
            package.mkdir(parents=True)
            binding = package / "countersplat_web.js"
            binding.write_text("export {}", encoding="utf-8")
            secret = root / ".artifacts" / "web" / "outside.txt"
            secret.write_text("outside", encoding="utf-8")
            legacy = root / "preview" / "pkg"
            legacy.mkdir(parents=True)
            (legacy / "legacy.js").write_text("old", encoding="utf-8")
            self.assertEqual(package_resource(root, "countersplat_web.js"), binding)
            self.assertIsNone(package_resource(root, "../outside.txt"))
            self.assertIsNone(package_resource(root, "legacy.js"))

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

    def test_playable_scene_discovery_and_scene_path_confinement(self):
        import json
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            scenes = root / "assets" / "scenes"
            scenes.mkdir(parents=True)
            playable = {"name":"My scene", "resources":{"assets":{
                "level":{"kind":"sog_level"}, "pause_menu":{"kind":"scene"}},
                "preview":{"level_asset":"level"}},
                "entities":[{"id":"player", "components":{"PlayerCamera":{}}}]}
            (scenes / "my-scene.json").write_text(json.dumps(playable), encoding="utf-8")
            (scenes / "menu.json").write_text(json.dumps({"name":"Menu"}), encoding="utf-8")
            (scenes / "bad.json").write_text("not json", encoding="utf-8")
            self.assertEqual(scene_catalog(root)["scenes"], [{"id":"my-scene.json", "name":"My scene"}])
            self.assertEqual(scene_resource(scenes, "my-scene.json"), scenes / "my-scene.json")
            self.assertIsNone(scene_resource(scenes, "../secret.json"))
            self.assertIsNone(scene_resource(scenes, "child/scene.json"))
            scripts = root / "assets" / "scripts"
            scripts.mkdir()
            script = scripts / "custom.lua"
            script.write_text("return 1", encoding="utf-8")
            playable["resources"]["scripts"] = {"custom":{"uri":"../scripts/custom.lua"}}
            (scenes / "my-scene.json").write_text(json.dumps(playable), encoding="utf-8")
            self.assertEqual(declared_scene_resource(root, "assets/scripts/custom.lua"), (True, script))

    def test_preview_metadata_must_match_the_requested_scene(self):
        page = '<meta property="og:image" content="https://s3-eu-west-1.amazonaws.com/images.playcanvas.com/splat/1234abcd/v1/xl.webp">'
        self.assertTrue(preview_url("1234abcd", page).endswith("/1234abcd/v1/xl.webp"))
        with self.assertRaises(ValueError):
            preview_url("ffffffff", page)
        with self.assertRaises(ValueError):
            preview_url("1234abcd", page.replace("s3-eu-west-1.amazonaws.com", "example.com"))

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
