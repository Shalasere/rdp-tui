#!/usr/bin/env python3
"""Package verified Linux binaries and produce release checksums."""

import argparse
import gzip
import hashlib
import io
import json
import os
import re
import struct
import subprocess
import tarfile
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parent.parent
TARGETS = {"x86_64-unknown-linux-musl": 62, "aarch64-unknown-linux-musl": 183}


def version(tag=None):
    value = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?", value):
        raise ValueError(f"Unsupported release version: {value}")
    if tag and tag != f"v{value}":
        raise ValueError(f"Tag {tag!r} does not match Cargo version v{value}")
    return value


def git_output(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def verify_binary(data, target):
    if data[:6] != b"\x7fELF\x02\x01" or len(data) < 64:
        raise ValueError("Expected a 64-bit little-endian Linux ELF executable")
    if struct.unpack_from("<H", data, 18)[0] != TARGETS[target]:
        raise ValueError(f"Binary architecture does not match {target}")
    offset = struct.unpack_from("<Q", data, 32)[0]
    entry_size, count = struct.unpack_from("<HH", data, 54)
    if entry_size < 56 or count == 0 or offset + entry_size * count > len(data):
        raise ValueError("Invalid ELF program headers")
    if any(struct.unpack_from("<I", data, offset + i * entry_size)[0] == 3 for i in range(count)):
        raise ValueError("Release binary requires a dynamic loader; expected a static musl build")


def bundle(target, output, tag=None):
    release_version = version(tag)
    binary = ROOT / "target" / target / "release" / "rdp-tui"
    data = binary.read_bytes()
    verify_binary(data, target)
    executable = subprocess.check_output([str(binary), "info"], text=True, cwd=ROOT)
    if "rdp-tui Rust frontend" not in executable:
        raise ValueError("Release executable failed its CLI smoke check")
    base = f"rdp-tui-{release_version}-{target}"
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"{base}.tar.gz"
    commit = git_output("rev-parse", "HEAD")
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", git_output("show", "-s", "--format=%ct", "HEAD")))
    build_info = {
        "version": release_version,
        "target": target,
        "commit": commit,
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "binary_sha256": hashlib.sha256(data).hexdigest(),
    }
    instructions = (
        f"rdp-tui {release_version} ({target})\n\n"
        "1. Download SHA256SUMS from the same GitHub release. Verify this archive:\n"
        f"   sha256sum --ignore-missing -c SHA256SUMS\n"
        f"2. Extract: tar -xzf {base}.tar.gz\n"
        "3. Install FreeRDP 3 separately (Arch: sudo pacman -S freerdp).\n"
        f"4. Install: install -Dm755 {base}/rdp-tui ~/.local/bin/rdp-tui\n"
        "5. Ensure ~/.local/bin is on PATH, then run: rdp-tui tui\n\n"
        "Optional: openssh for SSH tunnels; secret-tool (libsecret on Arch) for\n"
        "Secret Service password storage; hyprctl for Hyprland monitor detection.\n"
        "The archive contains the Rust frontend. FreeRDP and its graphical\n"
        "dependencies are not bundled. BUILD-INFO.json identifies the source commit.\n"
        "Hostname lookup uses /etc/hosts and DNS, without glibc NSS plugins.\n"
        "For names requiring an NSS plugin (e.g. nss-mdns), use an IP address\n"
        "or a native source build such as the Arch package recipe.\n"
    )
    files = {
        "rdp-tui": (data, 0o755),
        "LICENSE": ((ROOT / "LICENSE").read_bytes(), 0o644),
        "README.md": ((ROOT / "README.md").read_bytes(), 0o644),
        "docs/RELEASING.md": ((ROOT / "docs/RELEASING.md").read_bytes(), 0o644),
        "INSTALL.txt": (instructions.encode(), 0o644),
        "BUILD-INFO.json": ((json.dumps(build_info, indent=2) + "\n").encode(), 0o644),
    }
    with archive.open("wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=epoch) as compressed:
        with tarfile.open(fileobj=compressed, mode="w") as tar:
            for name, (content, mode) in sorted(files.items()):
                info = tarfile.TarInfo(f"{base}/{name}")
                info.size, info.mode, info.mtime = len(content), mode, epoch
                tar.addfile(info, io.BytesIO(content))
    print(archive)


def checksums(directory, require_all=False):
    release_version = version()
    archives = sorted(directory.glob("rdp-tui-*.tar.gz"))
    expected = {f"rdp-tui-{release_version}-{target}.tar.gz" for target in TARGETS}
    names = {archive.name for archive in archives}
    if not names or not names <= expected or (require_all and names != expected):
        raise ValueError(f"Unexpected or missing release archives: {sorted(names)}")
    lines = [f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n" for archive in archives]
    path = directory / "SHA256SUMS"
    path.write_text("".join(lines))
    print(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    metadata = commands.add_parser("version")
    metadata.add_argument("--tag")
    package = commands.add_parser("bundle")
    package.add_argument("--target", required=True, choices=TARGETS)
    package.add_argument("--output", type=Path, default=ROOT / "dist")
    package.add_argument("--tag")
    hashes = commands.add_parser("checksums")
    hashes.add_argument("--directory", type=Path, default=ROOT / "dist")
    hashes.add_argument("--require-all", action="store_true")
    args = parser.parse_args()
    if args.command == "version":
        print(version(args.tag))
    elif args.command == "bundle":
        bundle(args.target, args.output, args.tag)
    else:
        checksums(args.directory, args.require_all)


if __name__ == "__main__":
    main()
