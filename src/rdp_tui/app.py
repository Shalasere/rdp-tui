"""Curses UI for managing and launching FreeRDP profiles."""

from __future__ import annotations

import curses
import shutil
import subprocess
from dataclasses import fields

from .profiles import Profile, command_for, load_profiles, save_profiles

EDITABLE = ("name", "host", "user", "domain", "fullscreen", "clipboard", "audio", "ignore_certificate", "extra_options")


def prompt(screen: curses.window, label: str, default: str = "") -> str | None:
    curses.echo()
    screen.clear()
    screen.addstr(0, 0, label)
    screen.addstr(2, 0, "> " + default)
    screen.move(2, 2 + len(default))
    try:
        response = screen.getstr(2, 2, 512).decode().strip()
    except KeyboardInterrupt:
        response = ""
    finally:
        curses.noecho()
    return response or default


def edit_profile(screen: curses.window, profile: Profile | None = None) -> Profile | None:
    value = profile or Profile(name="", host="")
    labels = {
        "name": "Profile name", "host": "Host (or host:port)", "user": "User", "domain": "Domain",
        "fullscreen": "Fullscreen", "clipboard": "Share clipboard", "audio": "Redirect audio",
        "ignore_certificate": "Ignore certificate", "extra_options": "Extra FreeRDP options",
    }
    for field_name in EDITABLE:
        current = getattr(value, field_name)
        if isinstance(current, bool):
            answer = prompt(screen, f"{labels[field_name]} [y/n]", "y" if current else "n")
            if answer is None:
                return None
            setattr(value, field_name, answer.lower() in {"y", "yes", "true", "1"})
        else:
            answer = prompt(screen, labels[field_name], current)
            if answer is None:
                return None
            setattr(value, field_name, answer)
    if not value.name or not value.host:
        return None
    return value


def draw(screen: curses.window, profiles: list[Profile], selected: int, message: str) -> None:
    screen.erase()
    height, width = screen.getmaxyx()
    screen.addnstr(0, 0, "rdp-tui  •  FreeRDP profile launcher", width - 1, curses.A_BOLD)
    screen.addnstr(1, 0, "Enter connect  a add  e edit  d delete  q quit", width - 1)
    if not profiles:
        screen.addnstr(4, 0, "No profiles yet. Press a to add one.", width - 1)
    for index, profile in enumerate(profiles):
        marker = "> " if index == selected else "  "
        detail = f"{profile.name:<22} {profile.user + '@' if profile.user else ''}{profile.host}"
        screen.addnstr(index + 4, 0, marker + detail, width - 1, curses.A_REVERSE if index == selected else 0)
    if message:
        screen.addnstr(height - 1, 0, message, width - 1)
    screen.refresh()


def run(screen: curses.window) -> None:
    curses.curs_set(0)
    profiles = load_profiles()
    selected, message = 0, "Passwords are never stored; FreeRDP will request them."
    while True:
        selected = max(0, min(selected, len(profiles) - 1))
        draw(screen, profiles, selected, message)
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
        elif key in (10, 13, curses.KEY_ENTER) and profiles:
            if not shutil.which("xfreerdp"):
                message = "xfreerdp is not installed or not on PATH."
                continue
            command = command_for(profiles[selected])
            curses.endwin()
            try:
                subprocess.run(command, check=False)
            finally:
                screen.refresh()


def main() -> None:
    try:
        curses.wrapper(run)
    except ValueError as exc:
        raise SystemExit(f"rdp-tui: {exc}") from exc


if __name__ == "__main__":
    main()
