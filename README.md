# rdp-tui

A small terminal UI for saved [FreeRDP](https://www.freerdp.com/) connections. It launches a FreeRDP client; it does not attempt to draw an RDP desktop inside the terminal.

## Features

- Keyboard-first profile selection and editing
- Profiles stored at `~/.config/rdp-tui/profiles.json` with owner-only permissions
- Fullscreen, clipboard, audio, certificate, domain, and extra FreeRDP option controls
- Passwords saved by default in an encrypted local file, separate from the profile JSON and supplied through FreeRDP's askpass hook (not the command line)

## Install and run

Install FreeRDP first (for example, `sudo pacman -S freerdp` on Arch or `sudo apt install freerdp2-x11` on Debian/Ubuntu), then:

```sh
git clone <your-repository-url>
cd rdp-tui
./rdp-tui
```

For an installed `rdp-tui` command instead, create a virtual environment and
install the project with `python -m venv .venv && .venv/bin/pip install -e .`.

On Arch, FreeRDP 3 provides `xfreerdp3`; older releases and some other
distributions use `xfreerdp`. rdp-tui detects either, preferring `xfreerdp3`.

Press `a` to add a connection. Use arrow keys (or `j`/`k`) to choose a profile, then `Enter` to connect. The profile editor shows every field at once: select a row, press `Enter` to edit it (or `Space` to toggle an option), then use `A` to accept or `Q` to discard it. The footer shows FreeRDP availability and the result of the last session; press `s` for a detailed status line. Press `q` to quit.

Leave **Domain** empty for a local account; rdp-tui passes that as an explicit empty domain so FreeRDP does not request one. The **Password storage** row defaults to **Automatic**, like Remmina: it uses a currently running Secret Service keyring when available and otherwise chooses the encrypted file. It does not start a keyring just to check. Select **Saved password** to store or replace a password. The fallback is encrypted in `~/.config/rdp-tui/secrets.json`, with its owner-only key held in `~/.config/rdp-tui/.password-key`; neither is committed or shared.

## Development

```sh
make setup
make test
make run
```

The `.venv` directory is local-only and ignored by Git. Runtime dependencies
are declared in `pyproject.toml`; an extra `requirements.txt` is intentionally
not maintained as a second source of truth.

## Diagnostics

rdp-tui validates required fields, ports, and quoted extra options before it
launches FreeRDP. Activity and FreeRDP output are retained in the owner-only
log at `~/.local/state/rdp-tui/rdp-tui.log`. Use `S` in the launcher to see
its location, or inspect the latest output with `tail -n 100 ~/.local/state/rdp-tui/rdp-tui.log`.

## Advanced RDP settings

The basic form keeps common connection settings concise. Select **Advanced RDP
settings** only when needed to configure custom or dynamic resolution,
multi-monitor/span mode, smart sizing, display scale, an existing local folder
share, microphone redirection, automatic reconnect, a network profile, colour
depth, or certificate policy. These map directly to documented FreeRDP options.
When no explicit resolution is set, rdp-tui detects the focused Hyprland monitor's
physical resolution at each launch. The stable X11 FreeRDP frontend is used even
under Wayland; experimental native Wayland frontends are deliberately not selected
automatically. The exact command is recorded in the log.

`extra_options` is split on whitespace and is intended for simple FreeRDP flags, e.g. `/multimon +auto-reconnect`.
