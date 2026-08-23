# rdp-tui

A small, dependency-free terminal UI for saved [FreeRDP](https://www.freerdp.com/) connections. It launches `xfreerdp`; it does not attempt to draw an RDP desktop inside the terminal.

## Features

- Keyboard-first profile selection and editing
- Profiles stored at `~/.config/rdp-tui/profiles.json` with owner-only permissions
- Fullscreen, clipboard, audio, certificate, domain, and extra FreeRDP option controls
- Passwords are **not** stored or supplied on the command line; FreeRDP requests them when needed

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

Press `a` to add a connection. Use arrow keys (or `j`/`k`) to choose a profile, then `Enter` to connect. The footer shows FreeRDP availability and the result of the last session; press `s` for a detailed status line. Press `q` to quit.

## Development

```sh
make setup
make test
make run
```

The `.venv` directory is local-only and ignored by Git. This project has no
runtime or development package dependencies beyond Python, so an otherwise
empty `requirements.txt` is intentionally not included; dependencies are
declared in `pyproject.toml` when needed.

`extra_options` is split on whitespace and is intended for simple FreeRDP flags, e.g. `/multimon +auto-reconnect`.
