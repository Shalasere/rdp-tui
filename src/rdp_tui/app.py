"""Curses UI for managing and launching FreeRDP profiles."""

from __future__ import annotations

import curses
import subprocess
from dataclasses import replace

from .profiles import Profile, command_for, freerdp_client, load_profiles, save_profiles

EDITABLE = ("name", "host", "user", "domain", "fullscreen", "clipboard", "audio", "ignore_certificate", "extra_options")


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


def edit_profile(screen: curses.window, profile: Profile | None = None) -> Profile | None:
    """Edit one profile in a selectable form instead of a prompt sequence."""
    value = replace(profile) if profile else Profile(name="", host="")
    labels = {
        "name": "Profile name", "host": "Host (or host:port)", "user": "User", "domain": "Domain",
        "fullscreen": "Fullscreen", "clipboard": "Share clipboard", "audio": "Redirect audio",
        "ignore_certificate": "Ignore certificate", "extra_options": "Extra FreeRDP options",
    }
    selected, error = 0, ""
    screen.keypad(True)
    while True:
        screen.erase()
        height, width = screen.getmaxyx()
        screen.addnstr(0, 0, "Edit RDP profile", width - 1, curses.A_BOLD)
        screen.addnstr(1, 0, "Choose a field, then edit it. Nothing is saved until you accept.", width - 1)
        for index, field_name in enumerate(EDITABLE):
            current = getattr(value, field_name)
            rendered = "On" if current is True else "Off" if current is False else str(current or "—")
            row = f"{labels[field_name]:<22} {rendered}"
            screen.addnstr(index + 3, 0, row, width - 1, curses.A_REVERSE if index == selected else 0)
        if error:
            screen.addnstr(height - 2, 0, error, width - 1, curses.A_BOLD)
        screen.addnstr(height - 1, 0, "[↑/↓] Choose  [Enter/E] Edit  [Space] Toggle  [A] Accept  [Q] Quit", width - 1)
        screen.refresh()

        key = screen.getch()
        field_name = EDITABLE[selected]
        current = getattr(value, field_name)
        if key in (ord("q"), ord("Q"), 27):
            return None
        if key in (curses.KEY_UP, ord("k"), ord("K")):
            selected = (selected - 1) % len(EDITABLE)
        elif key in (curses.KEY_DOWN, ord("j"), ord("J")):
            selected = (selected + 1) % len(EDITABLE)
        elif key in (ord("a"), ord("A")):
            if value.name and value.host:
                return value
            error = "Profile name and host are required before accepting."
        elif key in (ord(" "), 10, 13, curses.KEY_ENTER, ord("e"), ord("E")):
            if isinstance(current, bool):
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
    profiles = load_profiles()
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
                selected = len(profiles) - 1
        elif key == ord("e") and profiles:
            profile = edit_profile(screen, profiles[selected])
            if profile:
                profiles[selected] = profile
                save_profiles(profiles)
        elif key == ord("d") and profiles:
            name = profiles[selected].name
            if prompt(screen, f"Delete {name}? Type yes", "no").lower() == "yes":
                profiles.pop(selected)
                save_profiles(profiles)
        elif key == ord("s"):
            client = freerdp_client()
            if client:
                message = f"Status: {client} ready · {len(profiles)} profile(s) · config: ~/.config/rdp-tui/profiles.json"
            else:
                message = "Status: FreeRDP unavailable. Install the freerdp package and try again."
        elif key in (10, 13, curses.KEY_ENTER) and profiles:
            client = freerdp_client()
            if client is None:
                message = "FreeRDP is not installed or not on PATH (tried xfreerdp3, xfreerdp)."
                continue
            command = command_for(profiles[selected], client)
            curses.endwin()
            try:
                result = subprocess.run(command, check=False)
                last_result = f"{client} exited with code {result.returncode}."
            finally:
                screen.refresh()


def main() -> None:
    try:
        curses.wrapper(run)
    except ValueError as exc:
        raise SystemExit(f"rdp-tui: {exc}") from exc


if __name__ == "__main__":
    main()
