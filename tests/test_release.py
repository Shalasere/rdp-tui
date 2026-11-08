import hashlib
import struct
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

if sys.version_info >= (3, 11):
    from scripts import release
else:
    release = None


@unittest.skipIf(release is None, "release tooling requires Python 3.11+")
class ReleaseTests(unittest.TestCase):
    def elf(self, machine=62, program_type=1):
        data = bytearray(120)
        data[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", data, 18, machine)
        struct.pack_into("<Q", data, 32, 64)
        struct.pack_into("<HH", data, 54, 56, 1)
        struct.pack_into("<I", data, 64, program_type)
        return data

    def test_release_tag_must_match_manifest(self):
        current = release.version()
        self.assertEqual(release.version(f"v{current}"), current)
        with self.assertRaises(ValueError):
            release.version("v999.0.0")

    def test_packaging_rejects_wrong_architecture_and_dynamic_loader(self):
        release.verify_binary(self.elf(), "x86_64-unknown-linux-musl")
        release.verify_binary(self.elf(machine=183), "aarch64-unknown-linux-musl")
        for binary in [self.elf(machine=183), self.elf(program_type=3), b"not an executable"]:
            with self.assertRaises(ValueError):
                release.verify_binary(binary, "x86_64-unknown-linux-musl")

    def test_publishing_requires_both_archives_and_hashes_their_actual_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            current = release.version()
            archive = directory / f"rdp-tui-{current}-x86_64-unknown-linux-musl.tar.gz"
            archive.write_bytes(b"archive contents")
            with self.assertRaises(ValueError):
                release.checksums(directory, require_all=True)
            other = directory / f"rdp-tui-{current}-aarch64-unknown-linux-musl.tar.gz"
            other.write_bytes(b"other contents")
            release.checksums(directory, require_all=True)
            text = (directory / "SHA256SUMS").read_text()
            self.assertIn(f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}", text)
            self.assertIn(f"{hashlib.sha256(other.read_bytes()).hexdigest()}  {other.name}", text)
            (directory / "rdp-tui-unexpected.tar.gz").write_bytes(b"wrong release")
            with self.assertRaises(ValueError):
                release.checksums(directory, require_all=True)

    def test_release_archive_contains_editable_themes_and_their_documentation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "target/x86_64-unknown-linux-musl/release/rdp-tui"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(self.elf())
            for name in ["LICENSE", "README.md", "docs/RELEASING.md", "docs/THEMES.md"]:
                destination = root / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_text(name)
            themes = root / "themes"
            themes.mkdir()
            for source in (release.ROOT / "themes").glob("*.toml"):
                (themes / source.name).write_bytes(source.read_bytes())
            self.assertEqual(len(list(themes.glob("*.toml"))), 3)
            output = root / "dist"
            with (
                patch.object(release, "ROOT", root),
                patch.object(release, "version", return_value="1.0.0"),
                patch.object(release, "git_output", side_effect=["commit", "1000"]),
                patch.object(
                    release.subprocess,
                    "check_output",
                    side_effect=["rdp-tui Rust frontend", "rustc test"],
                ),
            ):
                release.bundle("x86_64-unknown-linux-musl", output)
            with tarfile.open(next(output.glob("*.tar.gz"))) as archive:
                prefix = "rdp-tui-1.0.0-x86_64-unknown-linux-musl"
                for source in themes.glob("*.toml"):
                    member = archive.extractfile(f"{prefix}/themes/{source.name}")
                    self.assertEqual(member.read(), source.read_bytes())
                self.assertIsNotNone(archive.getmember(f"{prefix}/docs/THEMES.md"))
