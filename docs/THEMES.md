# Rust TUI themes

The Rust frontend reads one optional `theme.toml` in its resolved configuration
directory, normally `~/.config/rdp-tui/`. Setting `XDG_CONFIG_HOME` changes this
to `$XDG_CONFIG_HOME/rdp-tui/theme.toml`. The legacy Python UI is unaffected.

Theme files are loaded at startup. Restart the TUI to apply edits. The program
does not rewrite your theme, change your terminal palette, read Kitty's config,
or require rebuilding to accept a new palette. There is no compiled list of
colored themes, implicit system-wide theme selection, or executable theme code.

## Select a theme

Copy an example to the active file, then customize it:

```sh
mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}/rdp-tui"
cp -i themes/brogrammer.toml "${XDG_CONFIG_HOME:-$HOME/.config}/rdp-tui/theme.toml"
rdp-tui tui
```

The source checkout and release archives include a `themes/` directory. The Arch
package installs these examples under `/usr/share/rdp-tui/themes/`; substitute
that path when copying an installed example. Package updates do not replace
your active user file.

- `brogrammer.toml`: explicit RGB colors, a dark background, and a contrasting
  light selected row.
- `terminal.toml`: terminal-default text/background, terminal ANSI accents,
  and reversed selection. Its appearance follows the terminal's palette.
- `monochrome.toml`: inherited colors and reversed selection.

To return to the previous default, rename the active theme file or copy the
monochrome example over it. A missing file uses monochrome. A malformed or
unreadable file also falls back to monochrome and opens a dismissible warning;
fix the file and restart. CLI commands and detached RDP sessions do not load
the theme file.

## File format

```toml
version = 1

[palette]
background = "#131313"
foreground = "#d6dae4"
accent = "#2cc55d"

[styles.text]
fg = "$foreground"
bg = "$background"

[styles.border_focused]
fg = "$accent"
modifiers = ["bold"]

[styles.selection]
fg = "$background"
bg = "$foreground"
modifiers = []
```

Palette names are yours to choose. A `$name` value in a style refers to that
palette entry. Palette entries contain literal colors, not references to other
entries. You can also put a literal color directly in a style.

Styles accept `fg`, `bg`, and `modifiers`. Omitted properties retain the
monochrome fallback for that role. An explicit modifier list replaces fallback
modifiers: `modifiers = []` removes them, which matters when replacing reversed
selection with an explicit foreground/background pair.

`default` resets a color to the terminal default. RGB values use `#RRGGBB`.
ANSI names are `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`,
`gray`, `dark_gray`, `light_red`, `light_green`, `light_yellow`, `light_blue`,
`light_magenta`, `light_cyan`, and `white`. Supported modifiers are `bold`,
`dim`, `italic`, `underline`, and `reversed`. Unknown fields, roles, references, and unsupported
versions produce a warning rather than silently changing meaning.

## Style roles

| Role | Purpose |
| --- | --- |
| `text` | Base application text and background |
| `text_muted` | Secondary information and help text |
| `title` | Panel titles |
| `border` | Ordinary borders |
| `border_focused` | Borders indicating keyboard focus |
| `selection` | Selected profile or form row |
| `field_focused` | Field currently accepting text |
| `info` | Informational and in-progress status |
| `success` | Successful completed actions |
| `warning` | Important notices and certificate warnings |
| `error` | Failed actions and invalid input |

Focus, selection, and connection status are separate. Starting an RDP client
is not proof of successful authentication. Error text and warning instructions
remain visible without color; raw diagnostic bodies remain ordinary text.

## Contrast and terminal compatibility

Use a contrasting foreground/background pair for selection. Named ANSI colors
can be redefined by terminal themes: a color called dark gray may actually be
light. Explicit RGB colors avoid that ambiguity, but require a terminal that
supports truecolor. Use the terminal or monochrome example for other terminals.

The terminal backend also honors a nonempty `NO_COLOR` environment variable,
which suppresses color output even with an RGB theme. To test a colored theme
without changing your shell configuration, run `env -u NO_COLOR rdp-tui tui`.

Application styles are layered before interaction styles. Selection clears
inherited dim/reversed attributes where necessary so explicit selection colors
do not get unexpectedly inverted. Use the actual rendered result to assess
custom themes, especially editing, selected rows, warnings, and errors.

Automatic color probing, inheritance/includes, and live reload are not part of
this implementation. They are not required to edit or share arbitrary themes.
