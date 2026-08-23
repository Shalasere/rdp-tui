"""Curses UI for managing and launching FreeRDP profiles."""

from __future__ import annotations

import curses
import json
import shlex
import logging
import os
import shutil
import subprocess
import tempfile
import time
from dataclasses import replace
from pathlib import Path
from uuid import uuid4

from .logging_utils import LOG_PATH, STATE_DIR, configure_logging, load_last_session, save_last_session
from .profile_io import export_rdp, import_profiles, merge_profiles
from .profiles import (COLOR_DEPTHS, NETWORK_TYPES, RENDERERS, SCALE_FACTORS, Profile, command_for, local_display_settings,
                       freerdp_client, load_profiles, save_profiles, validate_profile)
from .secrets import SecretStoreError, delete_password, password_for, resolved_backend, save_password

EDITABLE = ("name", "host", "user", "domain", "fullscreen", "clipboard", "audio", "ignore_certificate", "extra_options")
ADVANCED_FIELDS = ("resolution", "dynamic_resolution", "multimon", "span_monitors", "smart_sizing", "scale",
                   "shared_folder", "microphone", "auto_reconnect", "network_type", "color_depth", "certificate_policy",
                   "renderer", "admin_session")
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
        "renderer": "RDP renderer", "admin_session": "Use console session",
    }
    selected, error = 0, ""
    cyclic = {
        "scale": tuple(sorted(SCALE_FACTORS)), "color_depth": tuple(sorted(COLOR_DEPTHS)),
        "network_type": tuple(sorted(NETWORK_TYPES)), "certificate_policy": ("default", "tofu", "ignore", "deny"),
        "renderer": tuple(RENDERERS),
    }
    while True:
        screen.erase()
        height, width = screen.getmaxyx()
        screen.addnstr(0, 0, "Advanced RDP settings", width - 1, curses.A_BOLD)
        screen.addnstr(1, 0, "Only change a setting when you need it; defaults preserve simple FreeRDP behavior.", width - 1)
        for index, field_name in enumerate(ADVANCED_FIELDS):
            current = getattr(value, field_name)
            rendered = ("On" if current is True else "Off" if current is False else
                        RENDERERS.get(current, str(current or "Default")) if field_name == "renderer" else str(current or "Default"))
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


def fullscreen_wayland_sdl_window(pid: int, timeout: float = 3.0) -> bool:
    """Fullscreen a mapped SDL window without changing its client fullscreen state."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            result = subprocess.run(["hyprctl", "clients", "-j"], text=True, capture_output=True,
                                    check=False, timeout=1)
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
                    "hl.dsp.window.fullscreen_state({ internal = 2, client = 0, action = \"set\", "
                    f"window = \"address:{address}\" }})"
                )
                dispatch = subprocess.run(["hyprctl", "dispatch", expression], text=True, capture_output=True,
                                          check=False, timeout=1)
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
        password_state = f"saved ({resolved_backend(profile.password_backend).replace('_', ' ')})" if \
            password_for(profile.id, profile.password_backend) is not None else "not saved"
    except SecretStoreError:
        password_state = "storage unavailable"
    renderer = RENDERERS.get(profile.renderer, profile.renderer)
    lines = [
        f"Profile: {profile.name}",
        f"Target: {profile.user + '@' if profile.user else ''}{profile.host}",
        f"Client: {client or 'not installed'}  •  Renderer: {renderer}",
        f"RDP size: {requested_resolution}" + (f"  •  Local scale: {desktop_scale}%" if desktop_scale else ""),
        f"Session: {'console (/admin)' if profile.admin_session else 'new RDP desktop'}  •  Smart sizing: {'on' if profile.smart_sizing else 'off'}",
        f"Password: {password_state}",
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
    while True:
        screen.erase()
        height, width = screen.getmaxyx()
        screen.addnstr(0, 0, "Connection status", width - 1, curses.A_BOLD)
        for index, line in enumerate(profile_status_lines(profile, last_session)):
            if index + 2 >= height - 1:
                break
            screen.addnstr(index + 2, 0, line, width - 1)
        screen.addnstr(height - 1, 0, "[Enter/Esc/Q] Return", width - 1)
        screen.refresh()
        key = screen.getch()
        if key in (10, 13, curses.KEY_ENTER, 27, ord("q"), ord("Q")):
            return


def filtered_profiles(profiles: list[Profile], query: str) -> list[Profile]:
    """Return profiles matching a case-insensitive name, host, or user query."""
    terms = query.casefold().split()
    if not terms:
        return profiles
    return [profile for profile in profiles if all(term in " ".join((profile.name, profile.host, profile.user,
                                                                        profile.domain)).casefold() for term in terms)]


def profile_position(profiles: list[Profile], selected: Profile) -> int:
    """Find a selected object by identity, even if profile values are duplicated."""
    return next(index for index, profile in enumerate(profiles) if profile is selected)


def draw(screen: curses.window, profiles: list[Profile], selected: int, message: str, last_result: str,
         query: str = "") -> None:
    screen.erase()
    height, width = screen.getmaxyx()
    screen.addnstr(0, 0, "rdp-tui  •  FreeRDP profile launcher", width - 1, curses.A_BOLD)
    screen.addnstr(1, 0, "[Enter] Connect  [A] Add  [E] Edit  [C] Clone  [D] Delete  [F] Find  [I] Import  [X] Export  [S] Status  [Q] Quit", width - 1)
    if not profiles:
        screen.addnstr(4, 0, "No matching profiles. Press a to add one or f to clear the filter.", width - 1)
    elif query:
        screen.addnstr(3, 0, f"Filter: {query}", width - 1)
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
    selected, message, last_result, query = 0, "Passwords are saved securely when you set one in the profile editor.", "", ""
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
            answer = prompt(screen, "Filter profiles by name, host, user, or domain (blank clears)", "")
            if answer is not None:
                query = answer
                selected = 0
        elif key == ord("i"):
            answer = prompt(screen, "Import .remmina, .rdp, or rdp-tui JSON backup")
            if answer:
                try:
                    imported = import_profiles(Path(answer).expanduser())
                    if not imported:
                        raise ValueError("The file contains no profiles")
                    profiles = merge_profiles(profiles, imported)
                    save_profiles(profiles)
                    query = ""
                    selected = len(profiles) - len(imported)
                    message = f"Imported {len(imported)} profile(s); passwords are not imported."
                    LOGGER.info("Imported %d profile(s) from %s", len(imported), answer)
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
            if client is None:
                message = f"FreeRDP renderer {profile.renderer!r} is not installed."
                LOGGER.error("Launch blocked: no FreeRDP client")
                continue
            if profile.renderer == "wayland_sdl" and (not os.environ.get("WAYLAND_DISPLAY") or not shutil.which("hyprctl")):
                message = "Wayland SDL renderer requires an active Wayland/Hyprland session."
                LOGGER.error("Launch blocked: Wayland SDL requirements unavailable")
                continue
            problems = validate_profile(profile)
            if problems:
                message = "Launch blocked: " + " · ".join(problems)
                LOGGER.error("Launch blocked for profile=%r: %s", profile.name, "; ".join(problems))
                continue
            detected_resolution, detected_desktop_scale = "", 0
            if not profile.resolution and not profile.multimon and not profile.span_monitors:
                detected_resolution, detected_desktop_scale = local_display_settings()
            command = command_for(profile, client, detected_resolution)
            try:
                password = password_for(profile.id, profile.password_backend)
            except SecretStoreError as exc:
                message = f"Password store unavailable: {exc}"
                LOGGER.exception("Password store failed for profile=%r", profile.name)
                continue
            askpass_path = None
            environment = None
            if password is not None:
                askpass_path = askpass_helper()
                environment = os.environ | {"FREERDP_ASKPASS": askpass_path, "RDP_TUI_PASSWORD": password}
            curses.def_prog_mode()
            curses.endwin()
            try:
                requested_resolution = profile.resolution or detected_resolution or "FreeRDP default"
                requested_scale = profile.scale or detected_desktop_scale or 100
                LOGGER.info("Launching profile name=%r host=%r client=%s renderer=%s saved_password=%s requested_resolution=%s desktop_scale=%s",
                            profile.name, profile.host, client, profile.renderer, password is not None,
                            requested_resolution, requested_scale)
                LOGGER.info("FreeRDP command: %s", shlex.join(command))
                started = time.monotonic()
                effective_client, effective_renderer = client, profile.renderer
                fallback_used = False
                with LOG_PATH.open("a", encoding="utf-8") as output:
                    if profile.renderer == "wayland_sdl":
                        process = subprocess.Popen(command, stdin=subprocess.DEVNULL if password is not None else None,
                                                   stdout=output, stderr=subprocess.STDOUT, env=environment)
                        LOGGER.info("Started SDL RDP process pid=%d; waiting for mapped Wayland window", process.pid)
                        fullscreened = profile.fullscreen and fullscreen_wayland_sdl_window(process.pid)
                        returncode = process.wait()
                        if profile.fullscreen and should_fallback_to_x11(profile.renderer, returncode, fullscreened):
                            fallback_client = freerdp_client("x11")
                            if fallback_client:
                                fallback_profile = replace(profile, renderer="x11")
                                fallback_command = command_for(fallback_profile, fallback_client, detected_resolution)
                                LOGGER.warning("SDL failed before mapping (exit=%d); retrying stable X11 client=%s: %s",
                                               returncode, fallback_client, shlex.join(fallback_command))
                                result = subprocess.run(fallback_command,
                                                        stdin=subprocess.DEVNULL if password is not None else None,
                                                        stdout=output, stderr=subprocess.STDOUT, env=environment,
                                                        check=False)
                                returncode = result.returncode
                                effective_client, effective_renderer, fallback_used = fallback_client, "x11", True
                            else:
                                LOGGER.error("SDL failed before mapping and no stable X11 FreeRDP client is installed")
                    else:
                        result = subprocess.run(command, stdin=subprocess.DEVNULL if password is not None else None,
                                                stdout=output, stderr=subprocess.STDOUT, env=environment,
                                                check=False)
                        returncode = result.returncode
                elapsed = time.monotonic() - started
                last_result = f"{effective_client} exited with code {returncode} after {elapsed:.1f}s."
                if fallback_used:
                    last_result = "SDL failed before mapping; " + last_result
                save_last_session({
                    "profile_id": profile.id,
                    "profile_name": profile.name,
                    "client": effective_client,
                    "renderer": effective_renderer,
                    "requested_resolution": requested_resolution,
                    "exit_code": returncode,
                    "elapsed_seconds": round(elapsed, 1),
                    "finished_at": time.strftime("%Y-%m-%d %H:%M:%S %Z"),
                })
                if returncode:
                    LOGGER.error("FreeRDP exited code=%d after %.1fs for profile=%r renderer=%s", returncode, elapsed,
                                 profile.name, effective_renderer)
                else:
                    LOGGER.info("FreeRDP completed after %.1fs for profile=%r", elapsed, profile.name)
            except OSError as exc:
                last_result = f"Could not start {client}: {exc}"
                save_last_session({
                    "profile_id": profile.id,
                    "profile_name": profile.name,
                    "client": client,
                    "renderer": profile.renderer,
                    "requested_resolution": requested_resolution,
                    "exit_code": "not started",
                    "elapsed_seconds": 0,
                    "finished_at": time.strftime("%Y-%m-%d %H:%M:%S %Z"),
                })
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
