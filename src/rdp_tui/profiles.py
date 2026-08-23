"""Profile persistence and FreeRDP command construction."""

from __future__ import annotations

import json
import os
import shutil
from dataclasses import asdict, dataclass
from pathlib import Path

CONFIG_PATH = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "rdp-tui" / "profiles.json"
CLIENT_CANDIDATES = ("xfreerdp3", "xfreerdp")


@dataclass
class Profile:
    name: str
    host: str
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
    if profile.domain:
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
        # Deliberately split on whitespace: options needing spaces should be quoted
        # in the profile and are not supported in this deliberately simple field.
        command.extend(profile.extra_options.split())
    return command
