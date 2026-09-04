#!/usr/bin/env python3

import base64
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
VALIDATOR = ROOT / "scripts" / "validate_updater_release.py"
REPOSITORY = "example/Codex-X-Pro"
RELEASE_TAG = "v0.3.1"
VERSION = "0.3.1"
PLATFORM_ASSETS = {
    "darwin-aarch64": "Codex-X-Pro.app.tar.gz",
    "darwin-aarch64-app": "Codex-X-Pro.app.tar.gz",
    "darwin-x86_64": "Codex-X-Pro-intel.app.tar.gz",
    "darwin-x86_64-app": "Codex-X-Pro-intel.app.tar.gz",
    "windows-x86_64": "Codex-X-Pro.msi",
    "windows-x86_64-msi": "Codex-X-Pro.msi",
    "linux-x86_64": "Codex-X-Pro.AppImage",
    "linux-x86_64-deb": "Codex-X-Pro.deb",
    "linux-x86_64-rpm": "Codex-X-Pro.rpm",
    "linux-x86_64-appimage": "Codex-X-Pro.AppImage",
}


def draft_url(asset_name: str) -> str:
    return (
        "https://github.com/example/Codex-X-Pro/releases/download/"
        f"untagged-test/{asset_name}"
    )


class ValidateUpdaterReleaseTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp_dir.cleanup)
        self.root = Path(self.temp_dir.name)
        self.manifest_path = self.root / "latest.json"
        self.assets_path = self.root / "assets.json"

        signature = base64.b64encode(b"s" * 64).decode("ascii")
        manifest = {
            "version": VERSION,
            "platforms": {
                platform: {
                    "signature": signature,
                    "url": draft_url(asset_name),
                }
                for platform, asset_name in PLATFORM_ASSETS.items()
            },
        }
        asset_names = sorted(set(PLATFORM_ASSETS.values()))
        assets = [
            {
                "name": asset_name,
                "url": f"https://api.github.com/assets/{index}",
                "browser_download_url": draft_url(asset_name),
            }
            for index, asset_name in enumerate(asset_names, start=1)
        ]
        assets.append(
            {
                "name": "latest.json",
                "url": "https://api.github.com/assets/latest",
                "browser_download_url": draft_url("latest.json"),
            }
        )
        self.manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        self.assets_path.write_text(json.dumps(assets), encoding="utf-8")

    def run_validator(self, *extra_args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(VALIDATOR),
                "--manifest",
                str(self.manifest_path),
                "--assets",
                str(self.assets_path),
                "--version",
                VERSION,
                "--repository",
                REPOSITORY,
                "--release-tag",
                RELEASE_TAG,
                *extra_args,
            ],
            check=False,
            capture_output=True,
            text=True,
        )

    def test_rewrites_draft_urls_to_stable_release_tag(self) -> None:
        result = self.run_validator("--rewrite-download-urls")
        self.assertEqual(result.returncode, 0, result.stderr)

        manifest = json.loads(self.manifest_path.read_text(encoding="utf-8"))
        urls = {entry["url"] for entry in manifest["platforms"].values()}
        self.assertTrue(urls)
        self.assertTrue(
            all("/releases/download/v0.3.1/" in url for url in urls),
            urls,
        )
        self.assertTrue(all("untagged-" not in url for url in urls), urls)

    def test_rejects_draft_urls_without_rewrite(self) -> None:
        result = self.run_validator()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("stable release tag download URL", result.stderr)

    def test_repairs_stale_urls_after_release_is_published(self) -> None:
        assets = json.loads(self.assets_path.read_text(encoding="utf-8"))
        for asset in assets:
            asset["browser_download_url"] = (
                f"https://github.com/{REPOSITORY}/releases/download/"
                f"{RELEASE_TAG}/{asset['name']}"
            )
        self.assets_path.write_text(json.dumps(assets), encoding="utf-8")

        result = self.run_validator("--rewrite-download-urls")
        self.assertEqual(result.returncode, 0, result.stderr)
        manifest = json.loads(self.manifest_path.read_text(encoding="utf-8"))
        self.assertTrue(
            all(
                "/releases/download/v0.3.1/" in entry["url"]
                for entry in manifest["platforms"].values()
            )
        )


if __name__ == "__main__":
    unittest.main()
