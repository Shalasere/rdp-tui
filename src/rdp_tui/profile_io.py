"""Portable, credential-free import and export formats for RDP profiles."""

from __future__ import annotations

import configparser
import json
from pathlib import Path
from uuid import uuid4

from .profiles import COLOR_DEPTHS, NETWORK_TYPES, Profile


def _user_and_domain(username: str, domain: str = "") -> tuple[str, str]:
    """Split the common DOMAIN\\user spelling without requiring it."""
    if "\\" in username and not domain:
        parsed_domain, parsed_user = username.split("\\", 1)
        return parsed_user, parsed_domain
    return username, domain


def _as_bool(value: str, default: bool = False) -> bool:
    return value.strip().lower() in {"1", "true", "yes", "on"} if value else default


def _positive_resolution(width: str, height: str) -> str:
    """Return a valid custom resolution, ignoring Remmina's 0x0 sentinel."""
    if width.isdecimal() and height.isdecimal() and int(width) > 0 and int(height) > 0:
        return f"{width}x{height}"
    return ""


def _remmina_fullscreen(section: configparser.SectionProxy) -> bool:
    """Translate Remmina's persisted connection-window mode."""
    if section.get("fullscreen", ""):
        return _as_bool(section.get("fullscreen", ""))
    viewmode = section.get("viewmode", "")
    return viewmode in {"3", "4"} if viewmode else True


def _remmina_network(value: str) -> str:
    """Translate Remmina network labels to FreeRDP's supported labels."""
    aliases = {"none": "auto", "autodetect": "auto", "broadband": "broadband-high"}
    network = aliases.get(value.strip().lower(), value.strip().lower())
    return network if network in NETWORK_TYPES else "auto"


def _remmina_color_depth(value: str) -> int:
    """Keep explicit colour depths while treating Remmina's 99 as automatic."""
    depth = int(value) if value.isdecimal() else 0
    return depth if depth in COLOR_DEPTHS else 0


def import_remmina(path: Path) -> list[Profile]:
    """Import connection settings from a Remmina .remmina file, never secrets."""
    config = configparser.ConfigParser(interpolation=None)
    config.read(path, encoding="utf-8")
    section = config["remmina"] if config.has_section("remmina") else config.defaults()
    if section.get("protocol", "RDP").upper() != "RDP":
        raise ValueError("Only Remmina RDP profiles can be imported")
    host = section.get("server", "").strip()
    if not host:
        raise ValueError("Remmina profile has no server")
    user, domain = _user_and_domain(section.get("username", ""), section.get("domain", ""))
    width, height = section.get("resolution_width", ""), section.get("resolution_height", "")
    resolution = _positive_resolution(width, height)
    scale_mode = section.get("scale", "0").strip()
    sound = section.get("sound", "").strip().lower()
    multimon = _as_bool(section.get("multimon", "")) or _as_bool(section.get("force_multimon", ""))
    span_monitors = _as_bool(section.get("span", ""))
    return [
        Profile(
            name=section.get("name", path.stem),
            host=host,
            user=user,
            domain=domain,
            fullscreen=_remmina_fullscreen(section),
            clipboard=not _as_bool(section.get("disableclipboard", "")),
            audio=sound.startswith("local"),
            ignore_certificate=_as_bool(section.get("cert_ignore", "")),
            resolution=resolution,
            dynamic_resolution=scale_mode == "2" and not multimon and not span_monitors,
            multimon=multimon,
            span_monitors=span_monitors,
            smart_sizing=scale_mode == "1",
            microphone=section.get("microphone", "").strip().lower() not in {"", "0", "off"},
            auto_reconnect=not _as_bool(section.get("disableautoreconnect", "")),
            network_type=_remmina_network(section.get("network", "")),
            color_depth=_remmina_color_depth(section.get("colordepth", "")),
            admin_session=_as_bool(section.get("console", "")),
            gateway_host=section.get("gateway_server", "").strip(),
            gateway_user=section.get("gateway_username", "").strip(),
            gateway_domain=section.get("gateway_domain", "").strip(),
        )
    ]


def import_rdp(path: Path) -> list[Profile]:
    """Import a standard Microsoft .rdp file, never its password fields."""
    raw = path.read_bytes()
    text = raw.decode("utf-16") if raw.startswith((b"\xff\xfe", b"\xfe\xff")) else raw.decode("utf-8-sig")
    values: dict[str, str] = {}
    for line in text.splitlines():
        parts = line.split(":", 2)
        if len(parts) == 3:
            values[parts[0].strip().lower()] = parts[2].strip()
    host = values.get("full address", "")
    if not host:
        raise ValueError("RDP file has no full address")
    user, domain = _user_and_domain(values.get("username", ""), values.get("domain", ""))
    width, height = values.get("desktopwidth", ""), values.get("desktopheight", "")
    resolution = f"{width}x{height}" if width.isdecimal() and height.isdecimal() else ""
    return [
        Profile(
            name=path.stem,
            host=host,
            user=user,
            domain=domain,
            fullscreen=values.get("screen mode id", "2") == "2",
            clipboard=values.get("redirectclipboard", "1") != "0",
            audio=values.get("audiomode", "2") == "0",
            resolution=resolution,
            multimon=values.get("use multimon", "0") == "1",
        )
    ]


def import_profiles(path: Path) -> list[Profile]:
    """Import native JSON backups, Remmina profiles, or Microsoft RDP files."""
    if path.is_dir():
        remmina_files = sorted(path.glob("*.remmina"))
        if not remmina_files:
            raise ValueError("The directory contains no .remmina profiles")
        profiles: list[Profile] = []
        for remmina_file in remmina_files:
            profiles.extend(import_remmina(remmina_file))
        return profiles
    suffix = path.suffix.lower()
    if suffix == ".remmina":
        return import_remmina(path)
    if suffix == ".rdp":
        return import_rdp(path)
    if suffix == ".json":
        try:
            raw = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise ValueError(f"Could not read JSON backup: {exc}") from exc
        if not isinstance(raw, list):
            raise ValueError("JSON backup must contain a profile list")
        return [Profile.from_dict(item) for item in raw if isinstance(item, dict)]
    raise ValueError("Choose a .remmina, .rdp, or rdp-tui .json file")


def merge_profiles(current: list[Profile], incoming: list[Profile]) -> list[Profile]:
    """Append new profiles, skipping unchanged imports and avoiding ID collisions."""
    used_ids = {profile.id for profile in current}
    existing = [{key: value for key, value in profile.__dict__.items() if key != "id"} for profile in current]
    added: list[Profile] = []
    for profile in incoming:
        comparable = {key: value for key, value in profile.__dict__.items() if key != "id"}
        if comparable in existing:
            continue
        if profile.id in used_ids:
            profile.id = str(uuid4())
        used_ids.add(profile.id)
        existing.append(comparable)
        added.append(profile)
    return [*current, *added]


def export_rdp(profile: Profile, path: Path) -> None:
    """Write a conventional .rdp file without exporting any saved password."""
    username = f"{profile.domain}\\{profile.user}" if profile.domain and profile.user else profile.user
    values = [
        "screen mode id:i:" + ("2" if profile.fullscreen else "1"),
        f"full address:s:{profile.host}",
        f"username:s:{username}",
        f"domain:s:{profile.domain}",
        "redirectclipboard:i:" + ("1" if profile.clipboard else "0"),
        "audiomode:i:" + ("0" if profile.audio else "2"),
        "use multimon:i:" + ("1" if profile.multimon else "0"),
    ]
    if profile.resolution:
        width, height = profile.resolution.split("x", 1)
        values.extend((f"desktopwidth:i:{width}", f"desktopheight:i:{height}"))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\r\n".join(values) + "\r\n", encoding="utf-8")
