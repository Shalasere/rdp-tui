"""Curses UI for managing and launching FreeRDP profiles."""

from __future__ import annotations

import curses
import logging
import os
import subprocess
import tempfile
import time
from dataclasses import replace
from pathlib import Path

from .logging_utils import LOG_PATH, STATE_DIR, configure_logging
from .profiles import (COLOR_DEPTHS, NETWORK_TYPES, SCALE_FACTORS, Profile, command_for, local_display_resolution,
                       freerdp_client, load_profiles, save_profiles, validate_profile)
from .secrets import SecretStoreError, delete_password, password_for, resolved_backend, save_password

EDITABLE = ("name", "host", "user", "domain", "fullscreen", "clipboard", "audio", "ignore_certificate", "extra_options")
ADVANCED_FIELDS = ("resolution", "dynamic_resolution", "multimon", "span_monitors", "smart_sizing", "scale",
                   "shared_folder", "microphone", "auto_reconnect", "network_type", "color_depth", "certificate_policy")
FORM_FIELDS = (*EDITABLE, "advanced", "password_backend", "password")
LOGGER = logging.getLogger("rdp_tui")


def askpass_helper() -> str:
    """Create a short-lived helper used by FreeRDP to obtain a saved secret."""
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    descriptor, path = tempfile.mkstemp(prefix="askpass-", suffix=".sh", dir=STATE_DIR, text=True)
    with os.fdopen(descriptor, "w", encoding="utf-8") as file:
        file.write("#!/usr/bin/env sh\nprintf '%s' \"$RDP_TUI_PASSWORD\"\n")
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


def edit_advanced(screen: curses.window, value: Profile) -> None:
    """Edit optional RDP settings without crowding the basic profile form."""
    labels = {
        "resolution": "Custom resolution", "dynamic_resolution": "Dynamic resolution", "multimon": "Multi-monitor",
        "span_monitors": "Span monitors", "smart_sizing": "Smart sizing", "scale": "Display scale",
        "shared_folder": "Share folder", "microphone": "Redirect microphone", "auto_reconnect": "Auto reconnect",
        "network_type": "Network profile", "color_depth": "Colour depth", "certificate_policy": "Certificate policy",
    }
    selected, error = 0, ""
    cyclic = {
        "scale": tuple(sorted(SCALE_FACTORS)), "color_depth": tuple(sorted(COLOR_DEPTHS)),
        "network_type": tuple(sorted(NETWORK_TYPES)), "certificate_policy": ("default", "tofu", "ignore", "deny"),
    }
    while True:
        screen.erase()
        height, width = screen.getmaxyx()
        screen.addnstr(0, 0, "Advanced RDP settings", width - 1, curses.A_BOLD)
        screen.addnstr(1, 0, "Only change a setting when you need it; defaults preserve simple FreeRDP behavior.", width - 1)
        for index, field_name in enumerate(ADVANCED_FIELDS):
            current = getattr(value, field_name)
            rendered = "On" if current is True else "Off" if current is False else str(current or "Default")
            screen.addnstr(index + 3, 0, f"{labels[field_name]:<22} {rendered}", width - 1,
                           curses.A_REVERSE if index == selected else 0)
        if error:
            screen.addnstr(height - 2, 0, error, width - 1, curses.A_BOLD)
        screen.addnstr(height - 1, 0, "[↑/↓] Choose  [Enter] Change  [A] Accept  [Q] Back", width - 1)
        screen.refresh()
        key = screen.getch()
        field_name = ADVANCED_FIELDS[selected]
        current = getattr(value, field_name)
        if key in (ord("q"), ord("Q"), 27):
            return
        if key in (curses.KEY_UP, ord("k"), ord("K")):
            selected = (selected - 1) % len(ADVANCED_FIELDS)
        elif key in (curses.KEY_DOWN, ord("j"), ord("J")):
            selected = (selected + 1) % len(ADVANCED_FIELDS)
        elif key in (ord("a"), ord("A")):
            problems = validate_profile(value)
            advanced_problems = [problem for problem in problems if "Profile name" not in problem and "Host " not in problem]
            if advanced_problems:
                error = " · ".join(advanced_problems)
            else:
                return
        elif key in (ord(" "), 10, 13, curses.KEY_ENTER, ord("e"), ord("E")):
            if isinstance(current, bool):
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
        "name": "Profile name", "host": "Host (or host:port)", "user": "User", "domain": "Domain",
        "fullscreen": "Fullscreen", "clipboard": "Share clipboard", "audio": "Redirect audio",
        "ignore_certificate": "Ignore certificate", "extra_options": "Extra FreeRDP options",
        "advanced": "Advanced RDP settings",
        "password_backend": "Password storage",
        "password": "Saved password",
    }
    try:
        saved_password = password_for(value.id, value.password_backend) is not None
    except SecretStoreError:
        saved_password = False
    selected, error, pending_password = 0, "", None
    screen.keypad(True)
    while True:
        screen.erase()
        height, width = screen.getmaxyx()
        screen.addnstr(0, 0, "Edit RDP profile", width - 1, curses.A_BOLD)
        screen.addnstr(1, 0, "Choose a field, then edit it. Nothing is saved until you accept.", width - 1)
        for index, field_name in enumerate(FORM_FIELDS):
            if field_name == "password":
                current = "Saved" if saved_password else "Not saved"
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
        current = getattr(value, field_name) if field_name not in {"advanced", "password", "password_backend"} else None
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
                error = ""
            elif field_name == "password":
                answer = password_prompt(screen)
                if answer is not None:
                    pending_password = answer
                    saved_password = bool(answer)
                    error = ""
            elif isinstance(current, bool):
                setattr(value, field_name, not current)
                error = ""
            else:
                answer = prompt(screen, labels[field_name], current)
                if answer is not None:
                    setattr(value, field_name, answer)
                    error = ""


def status_text(last_result: str = "") -> str:
    """Describe whether a usable FreeRDP client is currently available."""
    client = freerdp_client()
    if client is None:
        return "Status: FreeRDP unavailable — install freerdp (xfreerdp3 or xfreerdp not found)."
    if last_result:
        return f"Status: {last_result}"
    return f"Status: Ready — {client} detected."


def draw(screen: curses.window, profiles: list[Profile], selected: int, message: str, last_result: str) -> None:
    screen.erase()
    height, width = screen.getmaxyx()
    screen.addnstr(0, 0, "rdp-tui  •  FreeRDP profile launcher", width - 1, curses.A_BOLD)
    screen.addnstr(1, 0, "[Enter] Connect  [A] Add  [E] Edit  [D] Delete  [S] Status  [Q] Quit", width - 1)
    if not profiles:
        screen.addnstr(4, 0, "No profiles yet. Press a to add one.", width - 1)
    for index, profile in enumerate(profiles):
        marker = "> " if index == selected else "  "
        detail = f"{profile.name:<22} {profile.user + '@' if profile.user else ''}{profile.host}"
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
    selected, message, last_result = 0, "Passwords are never stored; FreeRDP will request them.", ""
    while True:
        selected = max(0, min(selected, len(profiles) - 1))
        draw(screen, profiles, selected, message, last_result)
        key = screen.getch()
        message = ""
        if key in (ord("q"), 27):
            return
        if key in (curses.KEY_UP, ord("k")) and selected:
            selected -= 1
        elif key in (curses.KEY_DOWN, ord("j")) and selected < len(profiles) - 1:
            selected += 1
        elif key == ord("a"):
            profile = edit_profile(screen)
            if profile:
                profiles.append(profile)
                save_profiles(profiles)
                LOGGER.info("Created profile name=%r host=%r", profile.name, profile.host)
                selected = len(profiles) - 1
        elif key == ord("e") and profiles:
            profile = edit_profile(screen, profiles[selected])
            if profile:
                profiles[selected] = profile
                save_profiles(profiles)
                LOGGER.info("Updated profile name=%r host=%r", profile.name, profile.host)
        elif key == ord("d") and profiles:
            name = profiles[selected].name
            if prompt(screen, f"Delete {name}? Type yes", "no").lower() == "yes":
                profiles.pop(selected)
                save_profiles(profiles)
                LOGGER.info("Deleted profile name=%r", name)
        elif key == ord("s"):
            client = freerdp_client()
            if client:
                message = f"Status: {client} ready · {len(profiles)} profile(s) · log: {LOG_PATH}"
            else:
                message = "Status: FreeRDP unavailable. Install the freerdp package and try again."
        elif key in (10, 13, curses.KEY_ENTER) and profiles:
            client = freerdp_client()
            if client is None:
                message = "FreeRDP is not installed or not on PATH (tried xfreerdp3, xfreerdp)."
                LOGGER.error("Launch blocked: no FreeRDP client")
                continue
            problems = validate_profile(profiles[selected])
            if problems:
                message = "Launch blocked: " + " · ".join(problems)
                LOGGER.error("Launch blocked for profile=%r: %s", profiles[selected].name, "; ".join(problems))
                continue
            detected_resolution = ""
            if not profiles[selected].resolution and not profiles[selected].multimon and not profiles[selected].span_monitors:
                detected_resolution = local_display_resolution()
            command = command_for(profiles[selected], client, detected_resolution)
            try:
                password = password_for(profiles[selected].id, profiles[selected].password_backend)
            except SecretStoreError as exc:
                message = f"Password store unavailable: {exc}"
                LOGGER.exception("Password store failed for profile=%r", profiles[selected].name)
                continue
            askpass_path = None
            environment = None
            if password is not None:
                askpass_path = askpass_helper()
                environment = os.environ | {"FREERDP_ASKPASS": askpass_path, "RDP_TUI_PASSWORD": password}
            curses.def_prog_mode()
            curses.endwin()
            try:
                requested_resolution = profiles[selected].resolution or detected_resolution or "FreeRDP default"
                LOGGER.info("Launching profile name=%r host=%r client=%s saved_password=%s requested_resolution=%s",
                            profiles[selected].name, profiles[selected].host, client, password is not None,
                            requested_resolution)
                started = time.monotonic()
                with LOG_PATH.open("a", encoding="utf-8") as output:
                    result = subprocess.run(command, stdin=subprocess.DEVNULL if password is not None else None,
                                            stdout=output, stderr=subprocess.STDOUT, env=environment,
                                            check=False)
                elapsed = time.monotonic() - started
                last_result = f"{client} exited with code {result.returncode} after {elapsed:.1f}s."
                if result.returncode:
                    LOGGER.error("FreeRDP exited code=%d after %.1fs for profile=%r", result.returncode, elapsed,
                                 profiles[selected].name)
                else:
                    LOGGER.info("FreeRDP completed after %.1fs for profile=%r", elapsed, profiles[selected].name)
            except OSError as exc:
                last_result = f"Could not start {client}: {exc}"
                LOGGER.exception("FreeRDP process could not start")
            finally:
                if askpass_path:
                    Path(askpass_path).unlink(missing_ok=True)
                curses.reset_prog_mode()
                curses.curs_set(0)
                screen.keypad(True)
                screen.erase()
                screen.refresh()


def main() -> None:
    global LOGGER
    LOGGER = configure_logging()
    try:
        curses.wrapper(run)
    except ValueError as exc:
        raise SystemExit(f"rdp-tui: {exc}") from exc


if __name__ == "__main__":
    main()
