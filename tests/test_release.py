import hashlib
import struct
import sys
import tempfile
import unittest
from pathlib import Path

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
