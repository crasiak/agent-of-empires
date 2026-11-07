#!/usr/bin/env python3
"""Build inventory tests; these do not grant extension execution authority."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("export-pi-integration.py")


class IntegrationInventory(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location("export_pi_integration", SCRIPT)
        self.exporter = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.exporter)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "source"
        self.assets = self.source / "assets/session"
        self.assets.mkdir(parents=True)
        (self.source / "src/session").mkdir(parents=True)
        (self.assets / "aoe-session-id.js").write_bytes(b"export default () => {};\n")
        (self.assets / "aoe-afk-runtime.mjs").write_bytes(b"import './module.mjs';\n")
        (self.assets / "module.mjs").write_bytes(b"export const value = 1;\n")
        (self.source / "src/session/afk.rs").write_text(
            'include_bytes!("../../assets/session/aoe-afk-runtime.mjs");\n'
            'include_bytes!("../../assets/session/module.mjs");\n'
        )
        self.binary = self.root / "aoe"
        self.binary.write_bytes(b"native binary fixture")
        self.output = self.root / "output"

    def export(self, system="aarch64-darwin"):
        return self.exporter.export(self.source, self.binary, self.output, system, "a" * 40)

    def test_missing_embedded_asset_fails_without_publishing_inventory(self):
        (self.assets / "module.mjs").unlink()
        with self.assertRaises((ValueError, FileNotFoundError)):
            self.export()
        self.assertFalse((self.output / "manifest.json").exists())

    def test_unknown_embed_expression_fails_instead_of_omitting_code(self):
        (self.source / "src/session/afk.rs").write_text('include_bytes!(concat!("asset", ".mjs"));')
        with self.assertRaises(ValueError):
            self.export()
        self.assertFalse(self.output.exists())

    def test_symlinked_asset_and_existing_destination_are_preserved(self):
        asset = self.assets / "module.mjs"
        asset.unlink()
        asset.symlink_to(self.binary)
        with self.assertRaises(ValueError):
            self.export()
        self.assertTrue(asset.is_symlink())
        asset.unlink()
        asset.write_bytes(b"export {};\n")
        self.output.mkdir()
        sentinel = self.output / "owner-data"
        sentinel.write_bytes(b"keep")
        with self.assertRaises(FileExistsError):
            self.export()
        self.assertEqual(sentinel.read_bytes(), b"keep")

    def test_native_binary_identity_and_all_embedded_files_are_exported(self):
        self.export()
        manifest = json.loads((self.output / "manifest.json").read_text())
        self.assertEqual(manifest["binary"]["sha256"], hashlib.sha256(self.binary.read_bytes()).hexdigest())
        self.assertEqual(manifest["system"], "aarch64-darwin")
        self.assertEqual(manifest["source_revision"], "a" * 40)
        identity = manifest["bundles"]["identity"]
        self.assertEqual(identity["entrypoint"], "pi-aoe-session-id.js")
        self.assertEqual(set(manifest["bundles"]["afk"]["files"]), {"aoe-afk-runtime.mjs", "module.mjs"})
        for bundle in manifest["bundles"].values():
            for name, digest in bundle["files"].items():
                self.assertEqual(hashlib.sha256((self.output / name).read_bytes()).hexdigest(), digest)
        self.assertNotIn("grants", manifest)

    def test_platform_outputs_share_asset_identity_not_binary_identity(self):
        self.export()
        first = json.loads((self.output / "manifest.json").read_text())
        self.output = self.root / "linux-output"
        self.binary.write_bytes(b"different native Linux binary fixture")
        self.export("x86_64-linux")
        second = json.loads((self.output / "manifest.json").read_text())
        self.assertEqual(first["bundles"], second["bundles"])
        self.assertNotEqual(first["binary"], second["binary"])


if __name__ == "__main__":
    unittest.main()
