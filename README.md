# rdp-tui

A small, dependency-free terminal UI for saved [FreeRDP](https://www.freerdp.com/) connections. It launches `xfreerdp`; it does not attempt to draw an RDP desktop inside the terminal.

## Features

- Keyboard-first profile selection and editing
- Profiles stored at `~/.config/rdp-tui/profiles.json` with owner-only permissions
- Fullscreen, clipboard, audio, certificate, domain, and extra FreeRDP option controls
- Passwords saved by default in an encrypted local file, separate from the profile JSON and sent to FreeRDP through stdin (not the command line)

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

`extra_options` is split on whitespace and is intended for simple FreeRDP flags, e.g. `/multimon +auto-reconnect`.
