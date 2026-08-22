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

Press `a` to add a connection. Use arrow keys (or `j`/`k`) to choose a profile, then `Enter` to connect.

## Development

```sh
python -m unittest discover -s tests
```

`extra_options` is split on whitespace and is intended for simple FreeRDP flags, e.g. `/multimon +auto-reconnect`.
