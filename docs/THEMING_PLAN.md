# TUI theming implementation plan

Prepared 2026-10-31; implementation completed in the working tree the same day.
Runtime theme files, semantic rendering, typed status severity, recoverable
configuration warnings, documentation, and packaging are implemented. Automated
tests, lint, formatting, and release build pass. Kitty previews used isolated
synthetic profiles. Following user approval, the tested build was installed at
`~/.local/bin/rdp-tui` and Brogrammer activated in `~/.config/rdp-tui/theme.toml`.
The previous executable was backed up under the application's state directory.
The sections below retain the implementation design and acceptance criteria.

## Outcome and scope

Add consistent, configurable color to the Rust/Ratatui interface. Support the
terminal palette, a Brogrammer theme file, and the existing monochrome appearance.
Colored themes must be runtime-loaded data, not Rust palette constants.
Preserve keyboard behavior, connection semantics, credential handling, and
terminal restoration. Existing users retain monochrome until they opt in.

Use Ratatui's existing Style, Color, and widget styling APIs. No additional UI
framework, new runtime dependency, automatic terminal color probing, theme
marketplace, or layout redesign is needed for this first implementation.

## Verified starting points

- Ratatui 0.30.2 is already installed as a project dependency.
- `src/tui/app/view.rs` owns rendering. Its only explicit styles are reversed
  selections in the profile list and edit form.
- `src/tui/app.rs` owns state and events. Status messages are plain strings;
  several error paths already have explicit error presentation functions.
- Existing tests use TestBackend, but the rendered-text helper discards cell
  styles. Extend this test infrastructure rather than introducing snapshots
  or a new testing dependency.
- `config.toml` is versioned and rejects unknown fields. Do not add TUI theme
  keys to it: older binaries would reject that configuration.
- Respect the module graph and invariants in `docs/architecture/`, particularly
  the separation of config from TUI types, CLI/TUI separation, secret handling,
  durable writes, and terminal restoration.
- Two unrelated untracked status documents were present during planning; leave
  them untouched.

## Design decisions

### Configuration and compatibility

Read an optional `theme.toml` under the same resolved configuration root as
`config.toml`. Honor the existing root override and XDG resolution; do not
hard-code the home directory. Load once before entering raw terminal mode.
The CLI and detached session supervisor do not need to load this file.

The active user file is normally `~/.config/rdp-tui/theme.toml`, or
`$XDG_CONFIG_HOME/rdp-tui/theme.toml`. It completely controls the palette and
semantic styles. Example:

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

Ship `themes/brogrammer.toml`, `themes/terminal.toml`, and
`themes/monochrome.toml` as editable examples in the repository. Packages can
install copies under `/usr/share/rdp-tui/themes/`; document the source checkout
path for standalone binaries. Users select a theme by copying it to the active
`theme.toml`, then edit any palette entry or role and restart the TUI. Package
updates must never overwrite the active user file. Adding an arbitrary new
palette must require no program rebuild or supported-preset enum change.

Resolution is the minimal monochrome fallback, followed by the active file's
per-role properties. Omitted properties inherit from that fallback; an
explicit modifier list replaces that role's fallback modifiers. An empty list
therefore removes modifiers such as reversed. Use `default` to explicitly reset
a color to the terminal default. Distinguish missing properties from resets.

Accept documented ANSI color names, `default`, strict `#RRGGBB` values, and
`$name` references to the file's palette. Palette entries are literal colors,
not other aliases, so reference resolution cannot cycle. Initially accept
bold, dim, italic, underline, and reversed modifiers; omit hidden and blinking.
Reject unknown roles, fields, modifiers, undefined palette references, malformed
colors, and unsupported file versions with useful errors. Do not accept
executable configuration, theme file includes, or inheritance in this phase.

The only compiled fallback is terminal-default monochrome for usability when no
file exists or a file is broken. There are no hard-coded colored presets. No
implicit reads of Kitty, desktop themes, root's config, or system-wide theme
files: ANSI colors inherit the terminal's runtime palette naturally, while RGB
colors are exactly those chosen in the user file. System-wide configuration
layering and live reload can be added later if there is a concrete need; they
are not prerequisites for fully user-controlled themes.

Missing file: monochrome. Invalid/unreadable existing file: use monochrome and
show a persistent, dismissible configuration warning in the existing message
UI, including the file path and useful validation location. Never silently
ignore invalid settings, echo the entire file, overwrite it, or prevent access
to connection profiles solely because a theme is broken.

There is no application-written theme configuration in this phase. Documentation
and example files are sufficient. Any later save action must use the project's
existing locked, atomic persistence pattern.

### Roles and ownership

Start with roles that have an actual rendering use:

| Role | Responsibility |
| --- | --- |
| text | Normal body and profile text |
| text_muted | Secondary descriptions and metadata |
| title | Panel and dialog titles |
| border | Ordinary panel borders |
| border_focused | Keyboard focus on a panel or form |
| selection | Current profile or form row |
| field_focused | Text currently being edited |
| info | Informational and in-progress messages |
| success | Completed successful actions/checks |
| warning | Certificate and consequential-action notices |
| error | Failed operations and invalid input |

Add `selection_inactive` only when there is an actual unfocused selection to
render; do not publish unused knobs. Keep selection, keyboard focus, remote
session state, and severity independent.

Put serializable theme definitions and pure validation in
`src/config/theme.rs`, using ordinary data types without importing Ratatui or
the TUI. Put conversion to resolved semantic Styles and the minimal monochrome
fallback in `src/tui/theme.rs`; colored palettes live only in TOML files. Resolve
once and pass the Theme through App to view code.
Avoid new crate-root modules or dependencies between CLI and TUI.

### Palette behavior

- Monochrome preserves terminal defaults and reversed selection. Severity is
  still conveyed by words and, where useful, bold or underline.
- Terminal inherits foreground/background, uses ANSI accents for severity and
  focus, and retains reversed selection. It cannot guarantee contrast with
  arbitrary user-remapped ANSI palettes; document this limitation.
- The Brogrammer TOML example uses explicit RGB values matching the user's palette: background
  `#131313`, text `#d6dae4`, green `#2cc55d`, yellow `#ecb90f`, red `#f71118`,
  and blue `#2a84d2`. Use dark text on the light selection background. Adjust
  any accent used for small text if contrast testing shows it is insufficient;
  matching a palette must not make messages unreadable.

Do not globally repaint the terminal or change Kitty settings. Explicit preset
backgrounds apply within the application frame only. Terminal and monochrome
modes remain available where truecolor is unsuitable. Light backgrounds are
supported by inherited modes and explicit overrides; add a named light preset
later if needed.

### Rendering and severity

Apply base styles, component/content styles, and then interaction styles in a
documented order. For explicit-color selections, resolve a complete foreground
and background pair and clear inherited reversed/dim modifiers. For terminal
and monochrome selections, reverse the base text/background rather than an
arbitrary status-colored span. Verify the actual rendered cells.

Use modest color: neutral profile content, a clear selection, a focused-panel
border, and semantic status/dialog accents. Preserve visible markers such as
`>`, error/warning labels, and certificate instructions in every preset.
Colored status must not imply that starting a connection proves it succeeded.

Introduce a small typed status severity alongside the existing status message,
with helpers that update both together. Classify messages at their production
sites, including background results. Never infer severity from substrings such
as "error" or "success". Keep raw diagnostic bodies neutral and accent their
heading/border instead of attempting to colorize arbitrary FreeRDP output.
Preserve error queues and the rule against interrupting password entry.

## Ordered implementation slices

1. **Theme definitions and resolution.** Add configuration parsing, validation,
   role definitions, palette references, and override resolution. Supply three
   editable TOML themes; do not compile their palettes into Rust. Validate file
   loading and merging before changing rendering.
2. **Startup and renderer integration.** Load from the resolved root before raw
   mode; handle warnings; store a resolved Theme in App. Style browsing,
   editing, focused fields, prompts, footer, details, and error/certificate
   dialogs. Confirm monochrome still renders the same content and controls.
3. **Typed status and UX polish.** Classify current status production sites,
   including asynchronous test/session results and configuration warnings.
   Ensure cancellation remains neutral and warnings/errors retain wording.
   Avoid unrelated controller restructuring.
4. **Validation and documentation.** Complete the regression matrix, manually
   inspect the Brogrammer preset in Kitty and the dashboard pane, and document
   configuration, limitations, examples, reset, and recovery. Build a separate
   release binary for preview; replacing the installed executable is a later,
   explicit rollout step.

Each slice should leave a buildable application and a reviewable diff. Do not
spend effort reproducing the existing connection/credential test suite inside
theme tests; run that suite as the regression gate.

## Validation and completion criteria

### Focused automated checks

- Old/missing application configuration still works; absent theme is monochrome.
- A new custom TOML palette works without rebuilding the program. Shipped theme
  files are parsed by tests and remain editable runtime data.
- Partial overrides, explicit terminal resets, modifier replacement, and invalid
  values behave as documented. Malformed themes warn and leave the TUI usable.
- Theme resolution does not write configuration or load unrelated user files.
- Use TestBackend to inspect cell foreground/background/modifiers for selection,
  focused fields, focused borders, and each severity. Test composition, not
  merely equality to the theme struct.
- Render representative browsing, editing, error, certificate-warning, and
  password-entry states across presets. Password test data is synthetic and
  remains masked; tests never read real profiles, credentials, or cert pins.
- Cover 80x24, dashboard-like 100x25, narrow 40x12, and tiny 20x6 sizes, plus a
  resize during a dialog. Retain existing scrolling, Unicode, and error-queue
  tests. Theme integration must not truncate instructions or introduce panics.
- Shipped RGB theme text/background combinations target at least 4.5:1 contrast for
  ordinary text, including selected rows and status text. Limit this automated
  guarantee to known RGB pairs; arbitrary terminal colors cannot be proven
  readable without knowing their actual values. Still inspect them manually.

### Repository gate

Run focused tests during development. After the final implementation changes:

```sh
make test
make lint
make format-check
git diff --check
cargo build --release --locked
```

This includes the existing architecture checks and Rust/Python suites. Report
pre-existing failures separately instead of repairing unrelated code silently.

### Manual acceptance

Preview synthetic profiles in the real Kitty/Brogrammer environment at full
size and in a dashboard-sized pane. Also check inherited styling on a light
terminal background. Inspect focused rows, editing, all severities, certificate
warnings, dialog resizing, and quitting/restoration. No live business call or
network authentication is required to review visual behavior. Keep credentials
and real network details out of screenshots and committed fixtures.

Done means all gates pass, theme errors are recoverable, important states remain
understandable in monochrome, all supported presets are visibly usable, and the
README explains how to select Brogrammer and return to the previous appearance.

## Reference designs

- Ratatui Style and incremental style composition:
  <https://docs.rs/ratatui/latest/ratatui/style/struct.Style.html>
- Helix semantic scopes, palettes, and inheritance:
  <https://docs.helix-editor.com/themes.html>
- Lazygit focused borders, selection states, and terminal background handling:
  <https://github.com/jesseduffield/lazygit/blob/master/docs/Config.md>
- Yazi component styles and partial configuration:
  <https://yazi-rs.github.io/docs/configuration/theme/>

These inform this design; their naming schemes are not a shared standard.
