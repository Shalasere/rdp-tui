"""Persistent diagnostic logging for launcher and FreeRDP events."""

from __future__ import annotations

import logging
import os
from pathlib import Path

STATE_DIR = Path(os.environ.get("XDG_STATE_HOME", Path.home() / ".local" / "state")) / "rdp-tui"
LOG_PATH = STATE_DIR / "rdp-tui.log"


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
