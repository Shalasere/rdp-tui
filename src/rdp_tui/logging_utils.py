"""Persistent diagnostic logging for launcher and FreeRDP events."""

from __future__ import annotations

import json
import logging
import os
from pathlib import Path

STATE_DIR = Path(os.environ.get("XDG_STATE_HOME", Path.home() / ".local" / "state")) / "rdp-tui"
LOG_PATH = STATE_DIR / "rdp-tui.log"
STATUS_PATH = STATE_DIR / "last-session.json"


def load_last_session(path: Path = STATUS_PATH) -> dict[str, object]:
    """Read the non-secret summary of the most recently completed session."""
    try:
        with path.open(encoding="utf-8") as file:
            value = json.load(file)
    except (FileNotFoundError, OSError, json.JSONDecodeError):
        return {}
    return value if isinstance(value, dict) else {}


def save_last_session(value: dict[str, object], path: Path = STATUS_PATH) -> None:
    """Atomically retain a non-secret session result for the status screen."""
    path.parent.mkdir(parents=True, exist_ok=True)
    os.chmod(path.parent, 0o700)
    temporary = path.with_suffix(".tmp")
    with temporary.open("w", encoding="utf-8") as file:
        json.dump(value, file, indent=2, sort_keys=True)
        file.write("\n")
    os.chmod(temporary, 0o600)
    temporary.replace(path)


def configure_logging() -> logging.Logger:
    """Create an owner-only application log without duplicating handlers."""
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    os.chmod(STATE_DIR, 0o700)
    logger = logging.getLogger("rdp_tui")
    logger.setLevel(logging.INFO)
    if not logger.handlers:
        handler = logging.FileHandler(LOG_PATH, encoding="utf-8")
        os.chmod(LOG_PATH, 0o600)
        handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(message)s"))
        logger.addHandler(handler)
    return logger
