"""Profile persistence and FreeRDP command construction."""

from __future__ import annotations

import json
import os
import shlex
import shutil
from dataclasses import asdict, dataclass, field
from pathlib import Path
from uuid import uuid4

CONFIG_PATH = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "rdp-tui" / "profiles.json"
CLIENT_CANDIDATES = ("xfreerdp3", "xfreerdp")


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

    @classmethod
    def from_dict(cls, value: dict[str, object]) -> "Profile":
        fields = {key: value[key] for key in cls.__dataclass_fields__ if key in value}
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


def freerdp_client() -> str | None:
    """Return the installed FreeRDP X11 client, preferring FreeRDP 3."""
    return next((client for client in CLIENT_CANDIDATES if shutil.which(client)), None)


def command_for(profile: Profile, client: str = "xfreerdp3") -> list[str]:
    command = [client, f"/v:{profile.host}"]
    if profile.user:
        command.append(f"/u:{profile.user}")
    # Explicitly pass an empty domain for local accounts so FreeRDP does not
    # prompt for one during credential collection.
    command.append(f"/d:{profile.domain}")
    if profile.fullscreen:
        command.append("/f")
    if profile.clipboard:
        command.append("+clipboard")
    if profile.audio:
        command.append("/sound")
    if profile.ignore_certificate:
        command.append("/cert:ignore")
    if profile.extra_options:
        command.extend(shlex.split(profile.extra_options))
    return command


def validate_profile(profile: Profile) -> list[str]:
    """Return actionable errors without attempting a network connection."""
    errors: list[str] = []
    if not profile.name.strip():
        errors.append("Profile name is required")
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
        shlex.split(profile.extra_options)
    except ValueError as exc:
        errors.append(f"Extra options are invalid: {exc}")
    return errors
