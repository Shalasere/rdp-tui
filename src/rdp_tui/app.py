"""Curses UI for managing and launching FreeRDP profiles."""

from __future__ import annotations

import curses
import hashlib
import json
import logging
import os
import re
import shlex
import shutil
import signal
import socket
import ssl
import subprocess
import tempfile
import time
from dataclasses import replace
from pathlib import Path
from uuid import uuid4

from .logging_utils import LOG_PATH, STATE_DIR, configure_logging, load_last_session, save_last_session
from .profile_io import export_rdp, import_profiles, merge_profiles
from .profiles import (
    COLOR_DEPTHS,
    NETWORK_TYPES,
    RENDERERS,
    SCALE_FACTORS,
    Profile,
    command_for,
    freerdp_client,
    load_profiles,
    local_display_settings,
    resolved_host,
    save_profiles,
    validate_profile,
)
from .secrets import SecretStoreError, delete_password, password_for, resolved_backend, save_password

EDITABLE = ("name", "host", "user", "domain", "fullscreen", "clipboard", "audio", "ignore_certificate", "extra_options")
ADVANCED_FIELDS = (
    "resolution",
    "dynamic_resolution",
    "multimon",
    "span_monitors",
    "smart_sizing",
    "scale",
    "shared_folder",
    "microphone",
    "auto_reconnect",
    "network_type",
    "color_depth",
    "certificate_policy",
    "renderer",
    "admin_session",
    "gateway_host",
    "gateway_user",
    "gateway_domain",
    "ssh_tunnel",
)
FORM_FIELDS = (*EDITABLE, "advanced", "password_backend", "password", "gateway_password")
LIST_NICKNAME_WIDTH = 22
LIST_HOST_WIDTH = 30
FREERDP_SERVER_DIR = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "freerdp" / "server"
CERTIFICATE_BACKUP_DIR = (
    Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "rdp-tui" / "certificate-backups"
)
LOGGER = logging.getLogger("rdp_tui")
ASKPASS_SCRIPT = """#!/usr/bin/env sh
case "$*" in
  *GatewayPassword:*|*Gateway\\ Password:*) printf '%s' "$RDP_TUI_GATEWAY_PASSWORD" ;;
  *) printf '%s' "$RDP_TUI_PASSWORD" ;;
esac
"""


def askpass_helper() -> str:
    """Create a short-lived helper used by FreeRDP to obtain a saved secret."""
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    descriptor, path = tempfile.mkstemp(prefix="askpass-", suffix=".sh", dir=STATE_DIR, text=True)
    with os.fdopen(descriptor, "w", encoding="utf-8") as file:
        file.write(ASKPASS_SCRIPT)
    os.chmod(path, 0o700)
    return path


def prompt(screen: curses.window, label: str, default: str = "") -> str | None:
    """Read a line while handling terminal Backspace and Delete consistently."""
    response = list(default)
    screen.keypad(True)
    curses.curs_set(1)
    while True:
        screen.erase()
        screen.addstr(0, 0, label)
        screen.addstr(2, 0, "> " + "".join(response))
        screen.move(2, 2 + len(response))
        screen.refresh()
        key = screen.get_wch()
        if key in ("\n", "\r", curses.KEY_ENTER):
            curses.curs_set(0)
            answer = "".join(response).strip()
            return answer or default
        if key == "\x1b":
            curses.curs_set(0)
            return None
        if key in (curses.KEY_BACKSPACE, curses.KEY_DC, "\x08", "\x7f"):
            if response:
                response.pop()
            continue
        if isinstance(key, str) and key.isprintable() and len(response) < 512:
            response.append(key)


def password_prompt(screen: curses.window) -> str | None:
    """Prompt for a secret without echoing it to the terminal."""
    response: list[str] = []
    screen.keypad(True)
    curses.curs_set(1)
    while True:
        screen.erase()
        screen.addstr(0, 0, "Saved password (Enter saves; an empty value removes it)")
        screen.addstr(2, 0, "> " + "*" * len(response))
        screen.move(2, 2 + len(response))
        screen.refresh()
        key = screen.get_wch()
        if key in ("\n", "\r", curses.KEY_ENTER):
            curses.curs_set(0)
            return "".join(response)
        if key == "\x1b":
            curses.curs_set(0)
            return None
        if key in (curses.KEY_BACKSPACE, curses.KEY_DC, "\x08", "\x7f"):
            if response:
                response.pop()
        elif isinstance(key, str) and key.isprintable() and len(response) < 512:
            response.append(key)


def edit_ssh_tunnel(screen: curses.window, profile: Profile) -> None:
    """Explain and configure an SSH-config-backed RDP tunnel without secrets."""
    while True:
        screen.erase()
        height, width = screen.getmaxyx()
        lines = (
            "SSH tunnel for this RDP profile",
            "",
            "A tunnel forwards this RDP connection through an SSH jump host.",
            "rdp-tui will use your existing ~/.ssh/config, keys, and agent.",
            "It will not store an SSH password. Leave the host blank to disable it.",
            "",
            f"SSH config host: {profile.ssh_tunnel_host or 'Not configured'}",
            "",
            "[Enter/E] Set SSH config host  [A/Q/Esc] Return",
        )
        for index, line in enumerate(lines):
            screen.addnstr(index, 0, line, width - 1, curses.A_BOLD if index == 0 else 0)
        screen.refresh()
        key = screen.getch()
        if key in (ord("a"), ord("A"), ord("q"), ord("Q"), 27):
            return
        if key in (ord("e"), ord("E"), 10, 13, curses.KEY_ENTER):
            answer = prompt(screen, "SSH config host (for example: work-jump)", profile.ssh_tunnel_host)
            if answer is not None:
                profile.ssh_tunnel_host = answer


def edit_advanced(screen: curses.window, value: Profile) -> None:
    """Edit optional RDP settings without crowding the basic profile form."""
    labels = {
        "resolution": "Custom resolution",
        "dynamic_resolution": "Dynamic resolution",
        "multimon": "Multi-monitor",
        "span_monitors": "Span monitors",
        "smart_sizing": "Smart sizing",
        "scale": "Display scale",
        "shared_folder": "Share folder",
        "microphone": "Redirect microphone",
        "auto_reconnect": "Auto reconnect",
        "network_type": "Network profile",
        "color_depth": "Colour depth",
        "certificate_policy": "Certificate policy",
        "renderer": "RDP renderer",
        "admin_session": "Use console session",
        "gateway_host": "Gateway host",
        "gateway_user": "Gateway user",
        "gateway_domain": "Gateway domain",
        "ssh_tunnel": "SSH tunnel",
    }
    selected, error = 0, ""
    cyclic = {
        "scale": tuple(sorted(SCALE_FACTORS)),
        "color_depth": tuple(sorted(COLOR_DEPTHS)),
        "network_type": tuple(sorted(NETWORK_TYPES)),
        "certificate_policy": ("tofu", "default", "ignore", "deny"),
        "renderer": tuple(RENDERERS),
    }
    while True:
        screen.erase()
        height, width = screen.getmaxyx()
        screen.addnstr(0, 0, "Advanced RDP settings", width - 1, curses.A_BOLD)
        screen.addnstr(
            1, 0, "Only change a setting when you need it; defaults preserve simple FreeRDP behavior.", width - 1
        )
        for index, field_name in enumerate(ADVANCED_FIELDS):
            current = value.ssh_tunnel_host if field_name == "ssh_tunnel" else getattr(value, field_name)
            rendered = (
                "On"
                if current is True
                else "Off"
                if current is False
                else RENDERERS.get(current, str(current or "Default"))
                if field_name == "renderer"
                else str(current or "Disabled")
            )
            screen.addnstr(
                index + 3,
                0,
                f"{labels[field_name]:<22} {rendered}",
                width - 1,
                curses.A_REVERSE if index == selected else 0,
            )
        if error:
            screen.addnstr(height - 2, 0, error, width - 1, curses.A_BOLD)
        screen.addnstr(height - 1, 0, "[↑/↓] Choose  [Enter] Change  [A] Accept  [Q] Back", width - 1)
        screen.refresh()
        key = screen.getch()
        field_name = ADVANCED_FIELDS[selected]
        current = value.ssh_tunnel_host if field_name == "ssh_tunnel" else getattr(value, field_name)
        if key in (ord("q"), ord("Q"), 27):
            return
        if key in (curses.KEY_UP, ord("k"), ord("K")):
            selected = (selected - 1) % len(ADVANCED_FIELDS)
        elif key in (curses.KEY_DOWN, ord("j"), ord("J")):
            selected = (selected + 1) % len(ADVANCED_FIELDS)
        elif key in (ord("a"), ord("A")):
            problems = validate_profile(value)
            advanced_problems = [
                problem for problem in problems if "Nickname" not in problem and "Host " not in problem
            ]
            if advanced_problems:
                error = " · ".join(advanced_problems)
            else:
                return
        elif key in (ord(" "), 10, 13, curses.KEY_ENTER, ord("e"), ord("E")):
            if field_name == "ssh_tunnel":
                edit_ssh_tunnel(screen, value)
            elif isinstance(current, bool):
                setattr(value, field_name, not current)
            elif field_name in cyclic:
                choices = cyclic[field_name]
                setattr(value, field_name, choices[(choices.index(current) + 1) % len(choices)])
            else:
                answer = prompt(screen, labels[field_name], str(current))
                if answer is not None:
                    setattr(value, field_name, answer)
            error = ""


def edit_profile(screen: curses.window, profile: Profile | None = None) -> Profile | None:
    """Edit one profile in a selectable form instead of a prompt sequence."""
    value = replace(profile) if profile else Profile(name="", host="")
    labels = {
        "name": "Nickname",
        "host": "Host (or host:port)",
        "user": "User",
        "domain": "Domain",
        "fullscreen": "Fullscreen",
        "clipboard": "Share clipboard",
        "audio": "Redirect audio",
        "ignore_certificate": "Ignore certificate",
        "extra_options": "Extra FreeRDP options",
        "advanced": "Advanced RDP settings",
        "password_backend": "Password storage",
        "password": "Saved password",
        "gateway_password": "Saved gateway password",
    }
    try:
        saved_password = password_for(value.id, value.password_backend) is not None
        saved_gateway_password = password_for(f"{value.id}:gateway", value.password_backend) is not None
    except SecretStoreError:
        saved_password = False
        saved_gateway_password = False
    selected, error, pending_password, pending_gateway_password = 0, "", None, None
    screen.keypad(True)
    while True:
        screen.erase()
        height, width = screen.getmaxyx()
        screen.addnstr(0, 0, "Edit RDP profile", width - 1, curses.A_BOLD)
        screen.addnstr(1, 0, "Choose a field, then edit it. Nothing is saved until you accept.", width - 1)
        for index, field_name in enumerate(FORM_FIELDS):
            if field_name == "password":
                current = "Saved" if saved_password else "Not saved"
            elif field_name == "gateway_password":
                current = "Saved" if saved_gateway_password else "Not saved"
            elif field_name == "advanced":
                current = "Enter to configure"
            elif field_name == "password_backend":
                labels_by_backend = {
                    "automatic": f"Automatic → {resolved_backend('automatic').replace('_', ' ')}",
                    "encrypted_file": "Encrypted file",
                    "keyring": "Keyring (Secret Service)",
                }
                current = labels_by_backend[value.password_backend]
            else:
                current = getattr(value, field_name)
            rendered = "On" if current is True else "Off" if current is False else str(current or "—")
            row = f"{labels[field_name]:<22} {rendered}"
            screen.addnstr(index + 3, 0, row, width - 1, curses.A_REVERSE if index == selected else 0)
        if error:
            screen.addnstr(height - 2, 0, error, width - 1, curses.A_BOLD)
        screen.addnstr(height - 1, 0, "[↑/↓] Choose  [Enter/E] Edit  [Space] Toggle  [A] Accept  [Q] Quit", width - 1)
        screen.refresh()

        key = screen.getch()
        field_name = FORM_FIELDS[selected]
        current = (
            getattr(value, field_name)
            if field_name not in {"advanced", "password", "gateway_password", "password_backend"}
            else None
        )
        if key in (ord("q"), ord("Q"), 27):
            return None
        if key in (curses.KEY_UP, ord("k"), ord("K")):
            selected = (selected - 1) % len(FORM_FIELDS)
        elif key in (curses.KEY_DOWN, ord("j"), ord("J")):
            selected = (selected + 1) % len(FORM_FIELDS)
        elif key in (ord("a"), ord("A")):
            problems = validate_profile(value)
            if not problems:
                if pending_password is not None:
                    try:
                        if pending_password:
                            save_password(value.id, pending_password, value.password_backend)
                        else:
                            delete_password(value.id, value.password_backend)
                    except SecretStoreError as exc:
                        error = f"Could not update saved password: {exc}"
                        continue
                if pending_gateway_password is not None:
                    try:
                        if pending_gateway_password:
                            save_password(f"{value.id}:gateway", pending_gateway_password, value.password_backend)
                        else:
                            delete_password(f"{value.id}:gateway", value.password_backend)
                    except SecretStoreError as exc:
                        error = f"Could not update gateway password: {exc}"
                        continue
                return value
            error = " · ".join(problems)
        elif key in (ord(" "), 10, 13, curses.KEY_ENTER, ord("e"), ord("E")):
            if field_name == "advanced":
                edit_advanced(screen, value)
            elif field_name == "password_backend":
                choices = ("automatic", "encrypted_file", "keyring")
                value.password_backend = choices[(choices.index(value.password_backend) + 1) % len(choices)]
                try:
                    saved_password = password_for(value.id, value.password_backend) is not None
                except SecretStoreError:
                    saved_password = False
                    saved_gateway_password = False
                error = ""
            elif field_name == "password":
                answer = password_prompt(screen)
                if answer is not None:
                    pending_password = answer
                    saved_password = bool(answer)
                    error = ""
            elif field_name == "gateway_password":
                if not value.gateway_host:
                    error = "Set Gateway host in Advanced RDP settings first"
                    continue
                answer = password_prompt(screen)
                if answer is not None:
                    pending_gateway_password = answer
                    saved_gateway_password = bool(answer)
                    error = ""
            elif isinstance(current, bool):
                setattr(value, field_name, not current)
                error = ""
            else:
                answer = prompt(screen, labels[field_name], current)
                if answer is not None:
                    setattr(value, field_name, answer)
                    error = ""


def fullscreen_wayland_sdl_window(pid: int, timeout: float = 3.0) -> bool:
    """Fullscreen a mapped SDL window without changing its client fullscreen state."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            result = subprocess.run(
                ["hyprctl", "clients", "-j"], text=True, capture_output=True, check=False, timeout=1
            )
            clients = json.loads(result.stdout)
        except (OSError, subprocess.TimeoutExpired, json.JSONDecodeError):
            LOGGER.exception("Could not inspect Hyprland clients for SDL process pid=%d", pid)
            return False
        if isinstance(clients, list):
            window = next((item for item in clients if isinstance(item, dict) and item.get("pid") == pid), None)
            address = window.get("address") if isinstance(window, dict) else None
            if isinstance(address, str) and address:
                # Hyprland 0.55 uses Lua dispatch. internal=2 fullscreen the
                # compositor surface; client=0 keeps SDL windowed so RDP does
                # not renegotiate to a logical Wayland size.
                expression = (
                    'hl.dsp.window.fullscreen_state({ internal = 2, client = 0, action = "set", '
                    f'window = "address:{address}" }})'
                )
                dispatch = subprocess.run(
                    ["hyprctl", "dispatch", expression], text=True, capture_output=True, check=False, timeout=1
                )
                if dispatch.returncode == 0:
                    LOGGER.info("Hyprland fullscreened SDL RDP window pid=%d address=%s", pid, address)
                    return True
                LOGGER.error("Hyprland fullscreen request failed for pid=%d: %s", pid, dispatch.stderr.strip())
                return False
        time.sleep(0.1)
    LOGGER.warning("SDL RDP window pid=%d did not map within %.1fs; leaving it windowed", pid, timeout)
    return False


def should_fallback_to_x11(renderer: str, returncode: int, fullscreened: bool) -> bool:
    """Retry stable X11 only when the experimental SDL window never became usable."""
    return renderer == "wayland_sdl" and returncode != 0 and not fullscreened


def rdp_endpoint(host: str, default_port: int = 3389) -> tuple[str, int]:
    """Split a supported RDP host[:port] value, retaining IPv6 literals."""
    if host.startswith("[") and "]:" in host:
        name, port = host[1:].split("]:", 1)
        return name, int(port)
    name, separator, port = host.rpartition(":")
    if separator and name and host.count(":") == 1 and port.isdecimal():
        return name, int(port)
    return host, default_port


def tcp_rdp_reachable(host: str, timeout: float = 1.5, default_port: int = 3389) -> tuple[bool, str]:
    """Check that a resolved target accepts a TCP connection on its RDP port."""
    name, port = rdp_endpoint(resolved_host(host), default_port)
    try:
        addresses = socket.getaddrinfo(name, port, type=socket.SOCK_STREAM)
    except OSError as exc:
        return False, f"cannot resolve {name}: {exc}"
    failure = "connection refused or timed out"
    for family, kind, protocol, _, address in addresses:
        try:
            with socket.socket(family, kind, protocol) as connection:
                connection.settimeout(timeout)
                if connection.connect_ex(address) == 0:
                    return True, f"{address[0]}:{port} reachable"
                failure = f"{address[0]}:{port} did not accept a connection"
        except OSError as exc:
            failure = str(exc)
    return False, failure


def preflight_profile(profile: Profile) -> list[str]:
    """Return launch-blocking issues before FreeRDP changes terminal state."""
    issues = validate_profile(profile)
    client = freerdp_client(profile.renderer)
    if client is None:
        issues.append(f"FreeRDP renderer {profile.renderer!r} is not installed")
    if profile.renderer == "wayland_sdl" and (not os.environ.get("WAYLAND_DISPLAY") or not shutil.which("hyprctl")):
        issues.append("Wayland SDL requires an active Wayland/Hyprland session")
    if not issues:
        reachable, detail = tcp_rdp_reachable(profile.host)
        if not reachable:
            issues.append(f"RDP network check failed: {detail}")
        if profile.gateway_host:
            reachable, detail = tcp_rdp_reachable(profile.gateway_host, default_port=443)
            if not reachable:
                issues.append(f"Gateway network check failed: {detail}")
    return issues


def status_text(last_result: str = "") -> str:
    """Describe whether a usable FreeRDP client is currently available."""
    client = freerdp_client()
    if client is None:
        return "Status: FreeRDP unavailable — install freerdp (xfreerdp3 or xfreerdp not found)."
    if last_result:
        return f"Status: {last_result}"
    return f"Status: Ready — {client} detected."


def profile_status_lines(profile: Profile | None, last_session: dict[str, object] | None = None) -> list[str]:
    """Build a concise, credential-free status report for the selected profile."""
    if profile is None:
        return ["No profile selected."]
    client = freerdp_client(profile.renderer)
    resolution, desktop_scale = local_display_settings()
    requested_resolution = profile.resolution or resolution or "FreeRDP default"
    password_state = "not saved"
    try:
        password_state = (
            f"saved ({resolved_backend(profile.password_backend).replace('_', ' ')})"
            if password_for(profile.id, profile.password_backend) is not None
            else "not saved"
        )
    except SecretStoreError:
        password_state = "storage unavailable"
    gateway_state = "not configured"
    if profile.gateway_host:
        try:
            gateway_state = (
                "saved" if password_for(f"{profile.id}:gateway", profile.password_backend) is not None else "not saved"
            )
        except SecretStoreError:
            gateway_state = "storage unavailable"
    renderer = RENDERERS.get(profile.renderer, profile.renderer)
    certificate = profile.certificate_policy
    if profile.ignore_certificate:
        certificate = "ignore (legacy setting)"
    elif profile.certificate_policy == "default" and profile.renderer == "wayland_sdl":
        certificate += " — interactive certificate prompts may be hidden by SDL"
    lines = [
        f"Nickname: {profile.name}",
        f"Target: {profile.user + '@' if profile.user else ''}{profile.host}",
        f"Client: {client or 'not installed'}  •  Renderer: {renderer}",
        f"Certificate: {certificate}",
        f"RDP size: {requested_resolution}" + (f"  •  Local scale: {desktop_scale}%" if desktop_scale else ""),
        f"Session: {'console (/admin)' if profile.admin_session else 'new RDP desktop'}  •  Smart sizing: {'on' if profile.smart_sizing else 'off'}",
        f"Password: {password_state}",
        f"Gateway: {profile.gateway_host or 'none'}  •  Password: {gateway_state}",
    ]
    if last_session and last_session.get("profile_id") == profile.id:
        outcome = "completed" if last_session.get("exit_code") == 0 else "failed"
        lines.append(
            f"Last session: {outcome} (exit {last_session.get('exit_code', '?')}, "
            f"{last_session.get('elapsed_seconds', '?')}s) at {last_session.get('finished_at', 'unknown time')}"
        )
    else:
        lines.append("Last session: no recorded session for this profile")
    lines.append(f"Log: {LOG_PATH}")
    return lines


def show_status(screen: curses.window, profile: Profile | None, last_session: dict[str, object]) -> None:
    """Present connection details without leaving the TUI or exposing secrets."""
    lines = profile_status_lines(profile, last_session)
    if profile is not None:
        issues = preflight_profile(profile)
        lines.insert(-1, "Preflight: " + ("ready" if not issues else " · ".join(issues)))
    while True:
        screen.erase()
        height, width = screen.getmaxyx()
        screen.addnstr(0, 0, "Connection status", width - 1, curses.A_BOLD)
        for index, line in enumerate(lines):
            if index + 2 >= height - 1:
                break
            screen.addnstr(index + 2, 0, line, width - 1)
        screen.addnstr(height - 1, 0, "[Enter/Esc/Q] Return", width - 1)
        screen.refresh()
        key = screen.getch()
        if key in (10, 13, curses.KEY_ENTER, 27, ord("q"), ord("Q")):
            return


def certificate_change_fingerprint(output: str) -> str | None:
    """Return a presented SHA-256 fingerprint from a FreeRDP certificate-change error."""
    lowered = output.casefold()
    changed = "new host identification" in lowered or "certificate does not match" in lowered
    if not changed:
        return None
    match = re.search(r"fingerprint[^\n]*?\bis\s+([0-9a-f]{2}(?::[0-9a-f]{2}){31})", output, re.IGNORECASE)
    return match.group(1).upper() if match else None


def freerdp_certificate_path(host: str, server_dir: Path | None = None) -> Path | None:
    """Resolve the FreeRDP certificate pin used by a conventional host or IPv4 target."""
    endpoint, port = rdp_endpoint(resolved_host(host))
    if not re.fullmatch(r"[A-Za-z0-9._-]+", endpoint):
        return None
    return (server_dir or FREERDP_SERVER_DIR) / f"{endpoint}_{port}.pem"


def certificate_fingerprint(path: Path) -> str | None:
    """Read a PEM certificate's SHA-256 fingerprint without invoking another process."""
    try:
        der = ssl.PEM_cert_to_DER_cert(path.read_text(encoding="ascii"))
    except (OSError, UnicodeError, ValueError):
        return None
    digest = hashlib.sha256(der).hexdigest().upper()
    return ":".join(digest[index : index + 2] for index in range(0, len(digest), 2))


def archive_freerdp_certificate(path: Path, backup_dir: Path | None = None) -> Path:
    """Move a stale FreeRDP pin to an owner-only, recoverable backup directory."""
    destination_dir = backup_dir or CERTIFICATE_BACKUP_DIR
    destination_dir.mkdir(parents=True, exist_ok=True)
    os.chmod(destination_dir, 0o700)
    timestamp = time.strftime("%Y%m%d-%H%M%S")
    destination = destination_dir / f"{path.name}.{timestamp}.bak"
    counter = 1
    while destination.exists():
        destination = destination_dir / f"{path.name}.{timestamp}.{counter}.bak"
        counter += 1
    path.replace(destination)
    os.chmod(destination, 0o600)
    return destination


def _fingerprint_lines(label: str, fingerprint: str) -> tuple[str, str]:
    compact = fingerprint.replace(":", "")
    return f"{label}: {compact[:32]}", f"{' ' * (len(label) + 2)}{compact[32:]}"


def confirm_certificate_replacement(
    screen: curses.window, profile: Profile, pinned_fingerprint: str, presented_fingerprint: str
) -> bool:
    """Ask before replacing a changed RDP certificate pin."""
    pinned_lines = _fingerprint_lines("Pinned SHA-256", pinned_fingerprint or "unavailable")
    presented_lines = _fingerprint_lines("Presented SHA-256", presented_fingerprint)
    lines = (
        "RDP certificate changed",
        "",
        f"Nickname: {profile.name}",
        f"Host: {profile.host}",
        *pinned_lines,
        *presented_lines,
        "",
        "Only trust this replacement if the remote PC was reinstalled, reset, or had its RDP certificate renewed.",
        "The old pin will be archived and this profile will use TOFU.",
        "",
        "[T] Trust replacement  [Q/Esc] Cancel",
    )
    while True:
        screen.erase()
        height, width = screen.getmaxyx()
        for index, line in enumerate(lines):
            if index >= height - 1:
                break
            screen.addnstr(index, 0, line, width - 1, curses.A_BOLD if index == 0 else 0)
        screen.refresh()
        key = screen.getch()
        if key in (ord("t"), ord("T")):
            return True
        if key in (ord("q"), ord("Q"), 27):
            return False


def log_output_since(offset: int, path: Path = LOG_PATH) -> str:
    """Read only the output appended during the current FreeRDP launch."""
    try:
        with path.open("rb") as file:
            file.seek(offset)
            return file.read().decode("utf-8", errors="replace")
    except OSError:
        return ""


def _stop_certificate_wait(process: subprocess.Popen) -> int:
    """Stop only the FreeRDP child that is waiting on a hidden certificate prompt."""
    process.send_signal(signal.SIGINT)
    try:
        return process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        process.terminate()
    try:
        return process.wait(timeout=1)
    except subprocess.TimeoutExpired:
        process.kill()
        return process.wait(timeout=1)


def wait_for_process_or_certificate(
    process: subprocess.Popen, log_offset: int, poll_interval: float = 0.1
) -> tuple[int, str | None]:
    """Wait for FreeRDP while interrupting a hidden changed-certificate prompt."""
    while True:
        fingerprint = certificate_change_fingerprint(log_output_since(log_offset))
        returncode = process.poll()
        if fingerprint:
            if returncode is None:
                returncode = _stop_certificate_wait(process)
            return returncode, fingerprint
        if returncode is not None:
            return returncode, None
        time.sleep(poll_interval)


def filtered_profiles(profiles: list[Profile], query: str) -> list[Profile]:
    """Return profiles matching a case-insensitive name, host, or user query."""
    terms = query.casefold().split()
    if not terms:
        return profiles
    return [
        profile
        for profile in profiles
        if all(
            term in " ".join((profile.name, profile.host, profile.user, profile.domain)).casefold() for term in terms
        )
    ]


def profile_position(profiles: list[Profile], selected: Profile) -> int:
    """Find a selected object by identity, even if profile values are duplicated."""
    return next(index for index, profile in enumerate(profiles) if profile is selected)


def profile_list_row(profile: Profile) -> str:
    """Format a stable nickname, host, and user row for the selector."""
    nickname = profile.name[:LIST_NICKNAME_WIDTH]
    host = profile.host[:LIST_HOST_WIDTH]
    user = f"{profile.domain}\\{profile.user}" if profile.domain and profile.user else profile.user
    return f"{nickname:<{LIST_NICKNAME_WIDTH}} {host:<{LIST_HOST_WIDTH}} {user or '—'}"


def draw(
    screen: curses.window, profiles: list[Profile], selected: int, message: str, last_result: str, query: str = ""
) -> None:
    screen.erase()
    height, width = screen.getmaxyx()
    screen.addnstr(0, 0, "rdp-tui  •  FreeRDP profile launcher", width - 1, curses.A_BOLD)
    screen.addnstr(
        1,
        0,
        "[Enter] Connect  [A] Add  [E] Edit  [C] Clone  [D] Delete  [F] Find  [I] Import  [X] Export  [S] Status  [Q] Quit",
        width - 1,
    )
    if query:
        screen.addnstr(2, 0, f"Filter: {query}", width - 1)
    header = f"{'Nickname':<{LIST_NICKNAME_WIDTH}} {'Hostname / address':<{LIST_HOST_WIDTH}} User"
    screen.addnstr(3, 2, header, max(1, width - 3), curses.A_BOLD | curses.A_UNDERLINE)
    if not profiles:
        screen.addnstr(4, 0, "No matching profiles. Press a to add one or f to clear the filter.", width - 1)
    for index, profile in enumerate(profiles):
        if index + 4 >= height - 1:
            break
        marker = "> " if index == selected else "  "
        detail = profile_list_row(profile)
        screen.addnstr(index + 4, 0, marker + detail, width - 1, curses.A_REVERSE if index == selected else 0)
    footer = message or status_text(last_result)
    screen.addnstr(height - 1, 0, footer, width - 1)
    screen.refresh()


def run(screen: curses.window) -> None:
    screen.keypad(True)
    curses.curs_set(0)
    try:
        profiles = load_profiles()
    except ValueError as exc:
        LOGGER.exception("Could not load profiles")
        raise SystemExit(f"rdp-tui: {exc}") from exc
    LOGGER.info("Launcher started with %d profile(s)", len(profiles))
    selected, message, last_result, query = (
        0,
        "Passwords are saved securely when you set one in the profile editor.",
        "",
        "",
    )
    while True:
        visible = filtered_profiles(profiles, query)
        selected = max(0, min(selected, len(visible) - 1))
        draw(screen, visible, selected, message, last_result, query)
        key = screen.getch()
        message = ""
        if key in (ord("q"), 27):
            return
        if key in (curses.KEY_UP, ord("k")) and selected:
            selected -= 1
        elif key in (curses.KEY_DOWN, ord("j")) and selected < len(visible) - 1:
            selected += 1
        elif key == ord("a"):
            profile = edit_profile(screen)
            if profile:
                profiles.append(profile)
                save_profiles(profiles)
                LOGGER.info("Created profile name=%r host=%r", profile.name, profile.host)
                query = ""
                selected = len(profiles) - 1
        elif key == ord("e") and visible:
            profile = edit_profile(screen, visible[selected])
            if profile:
                profiles[profile_position(profiles, visible[selected])] = profile
                save_profiles(profiles)
                LOGGER.info("Updated profile name=%r host=%r", profile.name, profile.host)
        elif key == ord("c") and visible:
            source = visible[selected]
            duplicate = replace(source, id=str(uuid4()), name=f"{source.name} (copy)")
            profile = edit_profile(screen, duplicate)
            if profile:
                profiles.append(profile)
                save_profiles(profiles)
                query = ""
                selected = len(profiles) - 1
                LOGGER.info("Cloned profile source=%r clone=%r", source.name, profile.name)
        elif key == ord("d") and visible:
            name = visible[selected].name
            if prompt(screen, f"Delete {name}? Type yes", "no").lower() == "yes":
                profiles.pop(profile_position(profiles, visible[selected]))
                save_profiles(profiles)
                LOGGER.info("Deleted profile name=%r", name)
        elif key == ord("f"):
            answer = prompt(screen, "Filter profiles by nickname, host, user, or domain (blank clears)", "")
            if answer is not None:
                query = answer
                selected = 0
        elif key == ord("i"):
            default_import = Path.home() / ".local" / "share" / "remmina"
            answer = prompt(
                screen,
                "Import a Remmina directory, .remmina, .rdp, or rdp-tui JSON backup",
                str(default_import) if default_import.is_dir() else "",
            )
            if answer:
                try:
                    imported = import_profiles(Path(answer).expanduser())
                    if not imported:
                        raise ValueError("The file contains no profiles")
                    merged = merge_profiles(profiles, imported)
                    added = len(merged) - len(profiles)
                    profiles = merged
                    save_profiles(profiles)
                    query = ""
                    selected = max(0, len(profiles) - added)
                    message = (
                        f"Imported {added} profile(s); skipped {len(imported) - added} unchanged; "
                        "passwords are not imported."
                    )
                    LOGGER.info(
                        "Imported %d profile(s), skipped %d unchanged from %s",
                        added,
                        len(imported) - added,
                        answer,
                    )
                except (OSError, ValueError) as exc:
                    message = f"Import failed: {exc}"
                    LOGGER.warning("Import failed from %s: %s", answer, exc)
        elif key == ord("x") and visible:
            default_path = Path.home() / f"{visible[selected].name}.rdp"
            answer = prompt(screen, "Export selected profile to .rdp (password is excluded)", str(default_path))
            if answer:
                destination = Path(answer).expanduser()
                if destination.suffix.lower() != ".rdp":
                    destination = destination.with_suffix(".rdp")
                try:
                    export_rdp(visible[selected], destination)
                    message = f"Exported {visible[selected].name} to {destination} (no password)."
                    LOGGER.info("Exported profile name=%r to %s", visible[selected].name, destination)
                except OSError as exc:
                    message = f"Export failed: {exc}"
                    LOGGER.warning("Export failed to %s: %s", destination, exc)
        elif key == ord("s"):
            show_status(screen, visible[selected] if visible else None, load_last_session())
        elif key in (10, 13, curses.KEY_ENTER) and visible:
            profile = visible[selected]
            client = freerdp_client(profile.renderer)
            problems = preflight_profile(profile)
            if problems or client is None:
                message = "Launch blocked: " + " · ".join(problems or ["FreeRDP client unavailable"])
                LOGGER.error("Launch blocked for profile=%r: %s", profile.name, "; ".join(problems))
                continue
            detected_resolution, detected_desktop_scale = "", 0
            if not profile.resolution and not profile.multimon and not profile.span_monitors:
                detected_resolution, detected_desktop_scale = local_display_settings()
            command = command_for(profile, client, detected_resolution)
            try:
                password = password_for(profile.id, profile.password_backend)
                gateway_password = (
                    password_for(f"{profile.id}:gateway", profile.password_backend) if profile.gateway_host else None
                )
            except SecretStoreError as exc:
                message = f"Password store unavailable: {exc}"
                LOGGER.exception("Password store failed for profile=%r", profile.name)
                continue
            askpass_path = None
            environment = None
            certificate_change = None
            if password is not None or gateway_password is not None:
                askpass_path = askpass_helper()
                environment = os.environ | {
                    "FREERDP_ASKPASS": askpass_path,
                    "RDP_TUI_PASSWORD": password or "",
                    "RDP_TUI_GATEWAY_PASSWORD": gateway_password or "",
                }
            curses.def_prog_mode()
            curses.endwin()
            try:
                requested_resolution = profile.resolution or detected_resolution or "FreeRDP default"
                requested_scale = profile.scale or detected_desktop_scale or 100
                LOGGER.info(
                    "Launching profile name=%r host=%r client=%s renderer=%s saved_password=%s requested_resolution=%s desktop_scale=%s",
                    profile.name,
                    profile.host,
                    client,
                    profile.renderer,
                    password is not None,
                    requested_resolution,
                    requested_scale,
                )
                LOGGER.info("FreeRDP command: %s", shlex.join(command))
                started = time.monotonic()
                effective_client, effective_renderer = client, profile.renderer
                fallback_used = False
                session_log_start = LOG_PATH.stat().st_size if LOG_PATH.exists() else 0
                with LOG_PATH.open("a", encoding="utf-8") as output:
                    if profile.renderer == "wayland_sdl":
                        process = subprocess.Popen(
                            command,
                            stdin=subprocess.DEVNULL if password is not None else None,
                            stdout=output,
                            stderr=subprocess.STDOUT,
                            env=environment,
                        )
                        LOGGER.info("Started SDL RDP process pid=%d; waiting for mapped Wayland window", process.pid)
                        fullscreened = profile.fullscreen and fullscreen_wayland_sdl_window(process.pid)
                        returncode, certificate_change = wait_for_process_or_certificate(process, session_log_start)
                        if (
                            not certificate_change
                            and profile.fullscreen
                            and should_fallback_to_x11(profile.renderer, returncode, fullscreened)
                        ):
                            fallback_client = freerdp_client("x11")
                            if fallback_client:
                                fallback_profile = replace(profile, renderer="x11")
                                fallback_command = command_for(fallback_profile, fallback_client, detected_resolution)
                                LOGGER.warning(
                                    "SDL failed before mapping (exit=%d); retrying stable X11 client=%s: %s",
                                    returncode,
                                    fallback_client,
                                    shlex.join(fallback_command),
                                )
                                fallback_process = subprocess.Popen(
                                    fallback_command,
                                    stdin=subprocess.DEVNULL if password is not None else None,
                                    stdout=output,
                                    stderr=subprocess.STDOUT,
                                    env=environment,
                                )
                                returncode, certificate_change = wait_for_process_or_certificate(
                                    fallback_process, session_log_start
                                )
                                effective_client, effective_renderer, fallback_used = fallback_client, "x11", True
                            else:
                                LOGGER.error("SDL failed before mapping and no stable X11 FreeRDP client is installed")
                    else:
                        process = subprocess.Popen(
                            command,
                            stdin=subprocess.DEVNULL if password is not None else None,
                            stdout=output,
                            stderr=subprocess.STDOUT,
                            env=environment,
                        )
                        returncode, certificate_change = wait_for_process_or_certificate(process, session_log_start)
                    output.flush()
                    certificate_change = certificate_change or certificate_change_fingerprint(
                        log_output_since(session_log_start)
                    )
                elapsed = time.monotonic() - started
                last_result = f"{effective_client} exited with code {returncode} after {elapsed:.1f}s."
                if fallback_used:
                    last_result = "SDL failed before mapping; " + last_result
                save_last_session(
                    {
                        "profile_id": profile.id,
                        "profile_name": profile.name,
                        "client": effective_client,
                        "renderer": effective_renderer,
                        "requested_resolution": requested_resolution,
                        "exit_code": returncode,
                        "elapsed_seconds": round(elapsed, 1),
                        "finished_at": time.strftime("%Y-%m-%d %H:%M:%S %Z"),
                    }
                )
                if returncode:
                    LOGGER.error(
                        "FreeRDP exited code=%d after %.1fs for profile=%r renderer=%s",
                        returncode,
                        elapsed,
                        profile.name,
                        effective_renderer,
                    )
                else:
                    LOGGER.info("FreeRDP completed after %.1fs for profile=%r", elapsed, profile.name)
            except OSError as exc:
                last_result = f"Could not start {client}: {exc}"
                save_last_session(
                    {
                        "profile_id": profile.id,
                        "profile_name": profile.name,
                        "client": client,
                        "renderer": profile.renderer,
                        "requested_resolution": requested_resolution,
                        "exit_code": "not started",
                        "elapsed_seconds": 0,
                        "finished_at": time.strftime("%Y-%m-%d %H:%M:%S %Z"),
                    }
                )
                LOGGER.exception("FreeRDP process could not start")
            finally:
                if askpass_path:
                    Path(askpass_path).unlink(missing_ok=True)
                curses.reset_prog_mode()
                curses.curs_set(0)
                screen.keypad(True)
                screen.erase()
                screen.refresh()
            if certificate_change:
                certificate_path = freerdp_certificate_path(profile.host)
                pinned_fingerprint = certificate_fingerprint(certificate_path) if certificate_path else None
                if certificate_path and certificate_path.is_file():
                    if confirm_certificate_replacement(
                        screen, profile, pinned_fingerprint or "unavailable", certificate_change
                    ):
                        try:
                            archived = archive_freerdp_certificate(certificate_path)
                            profile.certificate_policy = "tofu"
                            save_profiles(profiles)
                            message = f"Archived old certificate to {archived}; connect again to pin the replacement."
                            LOGGER.warning(
                                "User trusted changed RDP certificate profile=%r host=%r old=%s new=%s backup=%s",
                                profile.name,
                                profile.host,
                                pinned_fingerprint or "unavailable",
                                certificate_change,
                                archived,
                            )
                        except OSError as exc:
                            message = f"Could not archive the old certificate: {exc}"
                            LOGGER.exception("Could not archive changed RDP certificate for host=%r", profile.host)
                    else:
                        message = "Certificate replacement canceled; no trust settings were changed."
                elif not certificate_path or not certificate_path.is_file():
                    message = "Certificate changed, but rdp-tui could not locate the existing FreeRDP certificate pin."


def main() -> None:
    global LOGGER
    LOGGER = configure_logging()
    try:
        curses.wrapper(run)
    except ValueError as exc:
        raise SystemExit(f"rdp-tui: {exc}") from exc


if __name__ == "__main__":
    main()
