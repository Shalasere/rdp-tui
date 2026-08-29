"""Profile persistence and FreeRDP command construction."""

from __future__ import annotations

import json
import os
import re
import shlex
import shutil
import socket
import subprocess
from dataclasses import asdict, dataclass, field
from pathlib import Path
from uuid import uuid4

CONFIG_PATH = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "rdp-tui" / "profiles.json"
CLIENT_CANDIDATES = ("xfreerdp3", "xfreerdp")
SDL_CLIENT_CANDIDATES = ("sdl-freerdp3",)
RENDERERS = {"x11": "Stable X11", "wayland_sdl": "Wayland SDL (experimental)"}
NETWORK_TYPES = {"auto", "modem", "broadband", "broadband-low", "broadband-high", "wan", "lan"}
CERTIFICATE_POLICIES = {"default", "tofu", "ignore", "deny"}
COLOR_DEPTHS = {0, 8, 15, 16, 24, 32}
SCALE_FACTORS = {0, 100, 140, 180}
# These values are deliberately owned by structured profile fields.  Keeping
# them out of the escape hatch prevents credentials, routes, certificate
# policy, or a remote shell from bypassing UI validation and log redaction.
MANAGED_FREERDP_OPTIONS = {
    "v",
    "server",
    "port",
    "server-name",
    "u",
    "username",
    "d",
    "domain",
    "p",
    "password",
    "pth",
    "g",
    "gateway",
    "gu",
    "gateway-username",
    "gd",
    "gateway-domain",
    "gp",
    "gateway-password",
    "gateway-usage-method",
    "auth-only",
    "from-stdin",
    "args-from",
    "shell",
    "shell-dir",
    "app",
    "app-cmd",
    "load-balance-info",
    "assistance",
    "endpointfedauth",
    "proxy",
    "cert",
    "smartcard-logon",
}


@dataclass
class Profile:
    name: str
    host: str
    id: str = field(default_factory=lambda: str(uuid4()))
    password_backend: str = "automatic"
    user: str = ""
    domain: str = ""
    fullscreen: bool = True
    clipboard: bool = True
    audio: bool = False
    ignore_certificate: bool = False
    extra_options: str = ""
    resolution: str = ""
    dynamic_resolution: bool = False
    multimon: bool = False
    span_monitors: bool = False
    smart_sizing: bool = False
    scale: int = 0
    shared_folder: str = ""
    microphone: bool = False
    auto_reconnect: bool = False
    network_type: str = "auto"
    color_depth: int = 0
    certificate_policy: str = "tofu"
    renderer: str = "wayland_sdl"
    admin_session: bool = False
    gateway_host: str = ""
    gateway_user: str = ""
    gateway_domain: str = ""
    ssh_tunnel_host: str = ""

    @classmethod
    def from_dict(cls, value: dict[str, object]) -> Profile:
        # Older profile files can contain null entries for fields added in newer
        # releases. Omit them so dataclass defaults migrate the profile safely.
        fields = {key: value[key] for key in cls.__dataclass_fields__ if value.get(key) is not None}
        if fields.get("password_backend") not in {"automatic", "encrypted_file", "keyring"}:
            fields.pop("password_backend", None)
        if not isinstance(fields.get("id"), str):
            fields.pop("id", None)
        return cls(**fields)  # type: ignore[arg-type]


def load_profiles(path: Path = CONFIG_PATH) -> list[Profile]:
    try:
        with path.open(encoding="utf-8") as file:
            raw = json.load(file)
    except FileNotFoundError:
        return []
    except (json.JSONDecodeError, OSError) as exc:
        raise ValueError(f"Could not read {path}: {exc}") from exc
    if not isinstance(raw, list):
        raise ValueError(f"{path} must contain a JSON list")
    return [Profile.from_dict(item) for item in raw if isinstance(item, dict)]


def save_profiles(profiles: list[Profile], path: Path = CONFIG_PATH) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".tmp")
    with temporary.open("w", encoding="utf-8") as file:
        json.dump([asdict(profile) for profile in profiles], file, indent=2)
        file.write("\n")
    os.chmod(temporary, 0o600)
    temporary.replace(path)


def freerdp_client(renderer: str = "x11") -> str | None:
    """Return the selected FreeRDP frontend when it is installed."""
    candidates = SDL_CLIENT_CANDIDATES if renderer == "wayland_sdl" else CLIENT_CANDIDATES
    return next((client for client in candidates if shutil.which(client)), None)


def extra_options_for(value: str) -> list[str]:
    """Parse only FreeRDP switches that cannot override managed settings."""
    options = shlex.split(value)
    for option in options:
        normalized = option.lower()
        if normalized in {"+auth-only", "-auth-only"}:
            raise ValueError("managed FreeRDP options are not allowed")
        if not normalized.startswith("/"):
            continue
        name = re.split(r"[:=]", normalized[1:], maxsplit=1)[0]
        if name in MANAGED_FREERDP_OPTIONS:
            raise ValueError("managed FreeRDP options are not allowed")
    return options


def command_for(profile: Profile, client: str = "xfreerdp3", detected_resolution: str = "") -> list[str]:
    command = [client, f"/v:{resolved_host(profile.host)}"]
    if profile.user:
        command.append(f"/u:{profile.user}")
    # Explicitly pass an empty domain for local accounts so FreeRDP does not
    # prompt for one during credential collection.
    command.append(f"/d:{profile.domain}")
    if not profile.domain:
        # A local account should not wait for a Kerberos realm that is absent.
        command.append("/auth-pkg-list:none,ntlm")
    # SDL is intentionally started windowed.  Hyprland is asked to fullscreen
    # the mapped native Wayland window afterwards without telling the client it
    # is fullscreen, so FreeRDP retains the physical /size.
    if profile.fullscreen and profile.renderer != "wayland_sdl":
        command.append("/f")
    if profile.clipboard:
        command.append("+clipboard")
    if profile.audio:
        command.append("/sound")
    if profile.ignore_certificate or profile.certificate_policy == "ignore":
        command.append("/cert:ignore")
    elif profile.certificate_policy != "default":
        command.append(f"/cert:{profile.certificate_policy}")
    resolution = profile.resolution or detected_resolution
    if resolution:
        command.append(f"/size:{resolution}")
    if profile.admin_session:
        command.append("/admin")
    if profile.gateway_host:
        gateway = [f"g:{profile.gateway_host}"]
        if profile.gateway_user:
            gateway.append(f"u:{profile.gateway_user}")
        if profile.gateway_domain:
            gateway.append(f"d:{profile.gateway_domain}")
        command.append("/gateway:" + ",".join(gateway))
    if profile.dynamic_resolution:
        command.append("+dynamic-resolution")
    if profile.span_monitors:
        command.append("/span")
    elif profile.multimon:
        command.append("/multimon")
    if profile.smart_sizing:
        command.append("/smart-sizing")
    if profile.scale:
        command.append(f"/scale:{profile.scale}")
    if profile.renderer == "wayland_sdl":
        # Keep Hyprland shortcuts and touchpad gestures with the compositor.
        command.extend(("-grab-keyboard", "-grab-mouse"))
    if profile.shared_folder:
        command.append(f"/drive:rdp-tui,{Path(profile.shared_folder).expanduser()}")
    if profile.microphone:
        command.append("/microphone")
    if profile.auto_reconnect:
        command.append("+auto-reconnect")
    if profile.network_type != "auto":
        command.append(f"/network:{profile.network_type}")
    if profile.color_depth:
        command.append(f"/bpp:{profile.color_depth}")
    if profile.extra_options:
        command.extend(extra_options_for(profile.extra_options))
    return command


def local_display_resolution() -> str:
    """Return the focused Hyprland monitor's physical resolution."""
    return local_display_settings()[0]


def local_display_settings() -> tuple[str, int]:
    """Return focused Hyprland physical size and desktop scale percentage."""
    if not shutil.which("hyprctl"):
        return "", 0
    try:
        result = subprocess.run(["hyprctl", "monitors", "-j"], text=True, capture_output=True, check=False, timeout=2)
        monitors = json.loads(result.stdout)
    except (OSError, subprocess.TimeoutExpired, json.JSONDecodeError):
        return "", 0
    if not isinstance(monitors, list):
        return "", 0
    focused = next((monitor for monitor in monitors if isinstance(monitor, dict) and monitor.get("focused")), None)
    if not isinstance(focused, dict):
        return "", 0
    width, height, scale = focused.get("width"), focused.get("height"), focused.get("scale")
    if isinstance(width, int) and isinstance(height, int) and width > 0 and height > 0:
        percentage = round(scale * 100) if isinstance(scale, (int, float)) and scale > 0 else 0
        return f"{width}x{height}", percentage
    return "", 0


def resolved_host(host: str) -> str:
    """Resolve local mDNS names to IPv4 to avoid an IPv6 mDNS timeout."""
    name, separator, port = host.rpartition(":")
    hostname = name if separator and host.count(":") == 1 else host
    suffix = f":{port}" if separator and host.count(":") == 1 else ""
    if hostname.lower().endswith(".local"):
        try:
            return socket.gethostbyname(hostname) + suffix
        except OSError:
            pass
    return host


def validate_profile(profile: Profile) -> list[str]:
    """Return actionable errors without attempting a network connection."""
    errors: list[str] = []
    if not profile.name.strip():
        errors.append("Nickname is required")
    host = profile.host.strip()
    if not host:
        errors.append("Host is required")
    elif any(character.isspace() for character in host):
        errors.append("Host cannot contain whitespace")
    else:
        host_part, separator, port = host.rpartition(":")
        # A single colon identifies host:port; multiple colons are IPv6.
        if separator and host_part and host.count(":") == 1:
            if not port.isdecimal() or not 1 <= int(port) <= 65535:
                errors.append("Port must be between 1 and 65535")
    try:
        extra_options_for(profile.extra_options)
    except ValueError as exc:
        errors.append(f"Extra options are invalid: {exc}")
    if profile.resolution:
        match = re.fullmatch(r"(\d{2,5})x(\d{2,5})", profile.resolution)
        if not match or not all(200 <= int(part) <= 16384 for part in match.groups()):
            errors.append("Resolution must be WIDTHxHEIGHT (200–16384 each)")
    if profile.dynamic_resolution and profile.multimon:
        errors.append("Dynamic resolution cannot be used with multi-monitor")
    if profile.span_monitors and profile.dynamic_resolution:
        errors.append("Dynamic resolution cannot be used with span monitors")
    if profile.smart_sizing and profile.dynamic_resolution:
        errors.append("Dynamic resolution cannot be used with smart sizing")
    if profile.shared_folder:
        folder = Path(profile.shared_folder).expanduser()
        if not folder.is_absolute() or not folder.is_dir():
            errors.append("Shared folder must be an existing absolute directory")
        elif "," in profile.shared_folder:
            errors.append("Shared folder cannot contain a comma")
    if profile.network_type not in NETWORK_TYPES:
        errors.append("Network type is invalid")
    if profile.certificate_policy not in CERTIFICATE_POLICIES:
        errors.append("Certificate policy is invalid")
    if profile.color_depth not in COLOR_DEPTHS:
        errors.append("Colour depth must be 8, 15, 16, 24, or 32")
    if profile.scale not in SCALE_FACTORS:
        errors.append("Scale must be 100, 140, or 180")
    if profile.renderer not in RENDERERS:
        errors.append("RDP renderer is invalid")
    if profile.gateway_host:
        if any(character.isspace() for character in profile.gateway_host) or "," in profile.gateway_host:
            errors.append("Gateway host cannot contain whitespace or a comma")
        if any("," in value for value in (profile.gateway_user, profile.gateway_domain)):
            errors.append("Gateway user and domain cannot contain commas")
    if profile.ssh_tunnel_host and any(character.isspace() for character in profile.ssh_tunnel_host):
        errors.append("SSH tunnel host cannot contain whitespace")
    return errors
