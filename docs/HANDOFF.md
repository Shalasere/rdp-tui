# rdp-tui — Handoff & Development Guide

A terminal-first RDP launcher (Ratatui TUI + a scriptable CLI) built on FreeRDP.
This document is the entry point for anyone — human or agent — picking the
project up. Last updated **2026-08-29** (commit `1f31566`).

---

## 1. Status

Feature-complete against the architecture contract (all phases, all 13
invariants, all 18 anti-patterns) and at Python feature parity **plus** several
additions (deep-test, X11 fallback, connection history, RDP graphics depth). The
build gate is green on every commit. Tagged `v0.2.0-alpha.1`.

The authoritative conformance record is
[`docs/status/parity-2026-08-29.md`](status/parity-2026-08-29.md).

---

## 2. Build, install, run

```bash
cargo build --release           # -> target/release/rdp-tui
install -m0755 target/release/rdp-tui ~/.local/bin/rdp-tui   # (already installed)

rdp-tui tui                     # launch the interactive TUI
rdp-tui list                    # CLI: list profiles (no args also lists)
rdp-tui <command> ...           # see the full command list below
```

FreeRDP must be installed (`sdl-freerdp3` for the Wayland/SDL renderer,
`xfreerdp3` for X11). On this device both are present (3.30.0).

### The gate (run before every commit — all must pass)

```bash
cargo fmt
make test          # python unittest + cargo test --locked --all-targets
make lint          # cargo clippy --locked --all-targets --all-features -- -D warnings  (pedantic)
make format-check  # cargo fmt --check
git diff --check   # no trailing whitespace / conflict markers
```

Crate lints: `unsafe_code = "deny"` — unsafe is allowed only in reviewed,
commented blocks (currently one PDEATHSIG block via `rustix`, and the inherited
pipe/askpass fd handling).

---

## 3. Where data lives (XDG)

| Purpose | Path |
|---|---|
| Config | `$XDG_CONFIG_HOME/rdp-tui/` — `config.toml`, `profiles.toml` |
| State | `$XDG_STATE_HOME/rdp-tui/` — `history.toml`, `deep_test/<profile_id>.json` |
| Runtime | `$XDG_RUNTIME_DIR/rdp-tui/sessions/` — `<session-id>.json` |
| FreeRDP cert pins | `$XDG_CONFIG_HOME/freerdp/server/<host>_<port>.pem` |

All persistent writes are `flock()`-protected and atomically replaced. Locks use
a bounded retry so a transient fork→exec fd-inheritance window can't spuriously
fail a write (see `src/config/file.rs::acquire_lock`).

---

## 4. The contract is the source of truth

Read `docs/architecture/` in this order: `README.md` → `REVIEW.md` →
`04-amendments.yaml` (normative, wins conflicts) → `00-manifest.yaml` (module
graph + **forbidden edges**) → `01-types.yaml` → `02-rules.yaml`
(INV-*/AP-*/DEC-*) → `03-process.yaml` (phase plan + test matrix).

- **`tui` and `cli` must not depend on each other.** Shared connection /
  credential / diagnostic / persistence logic lives in a common module —
  usually `model` (e.g. `model::fields` holds the field parse/format/cycle
  helpers both frontends use). "Done" is defined by this contract.
- `tests/architecture_contract.rs` validates the contract (module graph,
  forbidden edges, stable-id resolution, binding types, source hash) on every
  build. It asserts exact counts on the **YAML**, so adding a Rust type/field
  (like `HistoryEntry` or `GraphicsMode`) does not affect it.
- Reference INV-/AP-/DEC- IDs in commit messages when code satisfies one.

---

## 5. Feature surface & full command list

```
rdp-tui [ list | show <id> | inspect <id> | validate | test <id>
        | deep-test <id> [--yes] | connect <id>
        | add <name> <host> | set <id> <field> <value> | clone <id> | delete <id>
        | credential set|clear <id>
        | certificate policy|show|trust|backups|restore <id> ...
        | import <path> | export <id> <path> | history [<id>]
        | config-paths | info | doctor | migrate python [profiles.json] ]
```

`set <id> <field> <value>` fields: `name`, `host`, `username`, `domain`,
`route` (`direct` | `gateway:<host>` | `ssh:<jump>`), `renderer`
(`wayland_sdl`|`x11`), `fullscreen`, `resolution` (`WxH`|`none`), `scale`
(100|140|180|none), `color-depth` (8|15|16|24|32|none), `dynamic-resolution`,
`multimon`, `span-monitors`, `smart-sizing`, `certificate`
(`tofu`|`system`|`ignore`|`deny`), `clipboard`, `audio`, `microphone`,
`printers`, `graphics` (`auto`|`rfx`|`avc420`|`avc444`), `admin-session`,
`network` (`auto`|`modem`|`broadband-low`|`broadband-high`|`wan`|`lan`).

TUI keys (press `?` in the app for this list): `Enter` connect · `a/e` add/edit ·
`c` clone · `d` delete · `f`/`/` find · `i/x` import/export · `s` status · `h`
history · `D` deep-test · `p` set/clear password · `t` test · `q` quit.

Anything not modelled can be passed to FreeRDP verbatim via a profile's
`security.advanced.freerdp_args`.

---

## 6. Module map (`src/`)

| Module | Responsibility |
|---|---|
| `model/` | Pure data types (Profile, ConnectionPlan, PreparedConnection, Endpoint, Route, HistoryEntry, …) + `fields` (shared parse/format/cycle). Depends only on stdlib+serde. |
| `config/` | Locked/atomic persistence: `ConfigStore` (config.toml, profiles.toml, history.toml), schema validation, import (Remmina/.rdp), python migration, contract validation. |
| `profile_store.rs` | Profile CRUD as locked read-modify-write transactions. |
| `secret/` | Credential backends: encrypted-file (owner-only key) + Secret Service. |
| `credentials.rs` | Backend dispatch, askpass lease (secrets via sealed memfd, never argv/env). |
| `freerdp/` | Client discovery + capabilities, argv builder (`command.rs` — the only place FreeRDP argv is emitted), launch, certificate TOFU, auth-only deep-test. |
| `ssh/` | SSH tunnel lifecycle (bounded ephemeral-port retry, orphan-safe). |
| `runtime/` | Owned-child spawning with detachment policy (PDEATHSIG only for one-shot), process registry / orphan scan (compound identity). |
| `preflight.rs`, `planner.rs` | Topology-aware preflight; Profile → ConnectionPlan (zero I/O). |
| `session/` | Detached connect supervisor (`launcher` hands the plan over a pipe → `supervisor` owns the session, writes the record + history), session records, scan. |
| `cli/` | CLI command implementations. |
| `tui/` | Ratatui frontend (`app.rs`), terminal guard (catch_unwind restore). |

---

## 7. Open items (pick up here)

1. **Manual TUI pass** — the one thing headless tests can't cover: launch
   `rdp-tui tui`, add/edit a profile through the form, connect. Everything else
   is test-verified.
2. **Mechanism-only test-matrix rows** — the behaviours exist and their core
   latches are unit-tested, but two adversarial scenarios aren't scripted
   because they need a harness: terminal raw-mode restore end-to-end (needs a
   PTY) and SIGKILL tunnel-survival (needs a real process kill).
3. **Test hygiene** — `app_with` in `src/tui/app.rs` tests uses a fixed
   `/tmp/rdp-tui-config`; benign (those tests don't write) but should be a
   `TempDir`.
4. **GFX fine-tuning** — thin-client / cache-size / progressive knobs are only
   reachable via `freerdp_args`; promote to first-class fields if a bad-enough
   link needs them.

---

## 8. Working on this repo remotely

The repo lives at `~/src/rdp-tui` on the Arch device (`shalasere`). You can
develop locally on the box (edit → gate → commit → push) — that's simplest.

If driving it from elsewhere (e.g. a Windows/WSL host over SSH, as this project
was built): reach the box with `ssh cultellus` (WSL ssh → OPNsense `10.0.0.5`
Tailscale jump → `100.70.25.85`). Ship files as **base64** (PowerShell eats
shell `$vars`, and `ssh -n` nulls stdin so heredocs vanish); decode remotely
with `base64 -d`. Deploy **one file per ssh call** — a single command argument is
capped at ~128 KB (`MAX_ARG_STRLEN`).

GitHub remote: `Shalasere/rdp-tui`, branch `main`.

---

## 9. How to continue development

- Follow the contract; when in doubt, `04-amendments.yaml` wins.
- Keep shared logic out of `tui`/`cli` — put it in `model` (or another common
  module) so both frontends use one implementation (INV-8).
- Secrets never touch argv, env, logs, or on-disk config (INV-3).
- Every change: gate green, commit (cite INV-/AP-/DEC- where relevant), push.
- Add a test at the layer you changed; extend `docs/status/parity-*.md` if the
  feature surface moves.
