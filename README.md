# rdp-tui

A small terminal UI for saved [FreeRDP](https://www.freerdp.com/) connections. It launches a FreeRDP client; it does not attempt to draw an RDP desktop inside the terminal.

## Features

- Keyboard-first profile selection and editing
- Profiles stored at `~/.config/rdp-tui/profiles.json` with owner-only permissions
- Fullscreen, clipboard, audio, certificate, domain, and extra FreeRDP option controls
- Passwords saved by default in an encrypted local file, separate from the profile JSON and supplied through FreeRDP's askpass hook (not the command line)
- Import Remmina `.remmina`, standard `.rdp`, or native JSON backup profiles; export a selected profile as a password-free `.rdp` file

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

Press `a` to add a connection. Use arrow keys (or `j`/`k`) to choose a profile, then `Enter` to connect. The profile editor shows every field at once: select a row, press `Enter` to edit it (or `Space` to toggle an option), then use `A` to accept or `Q` to discard it. The footer shows FreeRDP availability and the result of the last session; press `s` for the selected profile's detailed status screen. Press `q` to quit.

Press `i` to import a `.remmina`, `.rdp`, or rdp-tui JSON profile backup; imported passwords are deliberately excluded. Press `x` to export the selected profile as a standard `.rdp` file, also without its saved password.

Press `c` to clone the selected profile (then adjust it before saving), or `f` to filter profiles by name, host, user, or domain. Leave the filter blank to clear it.

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
the selected connection's effective client, renderer, display mode, password
storage state, and last outcome. A credential-free copy of that last outcome is
kept in `~/.local/state/rdp-tui/last-session.json` so it survives a restart;
inspect the raw output with `tail -n 100 ~/.local/state/rdp-tui/rdp-tui.log`.

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

The **RDP renderer** advanced setting defaults to **Stable X11**. **Wayland SDL
(experimental)** starts `sdl-freerdp3` windowed at the detected physical RDP
size, then asks Hyprland to fullscreen only the compositor surface. This avoids
both XWayland's logical fullscreen size and SDL's known fullscreen monitor probe
on wlroots compositors. It requires Wayland, Hyprland, and the Arch `freerdp`
package. The log records the SDL PID, mapped window address, and fullscreen result.
It also leaves keyboard and mouse input ungrabbed so compositor shortcuts and gestures
remain available. If SDL exits with an error before its window maps, rdp-tui retries
once with the stable X11 frontend and records that recovery in the status and log.
**Use console session** adds FreeRDP's `/admin` option to request
the existing server console session instead of a separate RDP desktop.

`extra_options` is split on whitespace and is intended for simple FreeRDP flags, e.g. `/multimon +auto-reconnect`.
