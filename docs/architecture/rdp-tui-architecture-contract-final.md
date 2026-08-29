# rdp-tui: Architecture Contract (Final)

**Status:** Original implementation contract; normatively amended by `04-amendments.yaml`
**Reference implementation:** Rust + Ratatui + Crossterm, concurrency mechanism intentionally unspecified
**Protocol engine:** FreeRDP
**Primary platform:** Linux, with Arch Linux as the reference environment

This document is the original architecture contract. `04-amendments.yaml` is
the highest-priority normative source and overrides conflicting clauses here or
in the original machine-readable files. `REVIEW.md` records provenance and the
review outcome.

---

## 1. Product Definition

`rdp-tui` is a terminal-native RDP connection and session manager for Linux.

```text
impala   → iwd
bluetui  → BlueZ
lazygit  → Git
k9s      → Kubernetes
rdp-tui  → FreeRDP
```

FreeRDP owns the RDP protocol, RD Gateway protocol, authentication negotiation, graphics transport, device redirection, and session transport. `rdp-tui` owns profiles, credentials, configuration, route resolution, environment detection, diagnostics, process lifecycle, history, FreeRDP invocation, error interpretation, the TUI, and the CLI.

## 2. Non-Goals

Implementing RDP; embedding FreeRDP without demonstrated need; supporting VNC/SPICE/X2Go/etc.; becoming a generic remote-session application; exposing every FreeRDP option as a first-class UI field (rare options go through an advanced override mechanism).

## 3. Source Layout

```text
src/
├── main.rs
│
├── model/
│   ├── profile.rs        # Profile
│   ├── endpoint.rs        # Endpoint, Host
│   ├── route.rs            # Route, PlannedRoute
│   ├── plan.rs              # ConnectionPlan — pure data, zero I/O
│   ├── prepared.rs          # PreparedConnection, RouteHandle, TunnelHandle
│   ├── credential.rs        # CredentialBackend, CredentialPreference, CredentialRef
│   ├── session.rs           # AttemptState, SessionState, SessionResult — pure types
│   └── failure.rs           # ConnectionFailure
│
├── planner.rs                 # Profile -> ConnectionPlan (pure, no I/O)
├── profile_store.rs
├── credentials.rs
├── preflight.rs                # calls prepare() — validate/preflight/deep-test/connect tiers
│
├── runtime/                     # generic process lifecycle — shared by freerdp/ and ssh/
│   ├── process.rs                # spawn helpers, process-group setup, PDEATHSIG scoping
│   └── registry.rs                # orphan registry: (pid, starttime, uid) identity
│
├── freerdp/
│   ├── discover.rs                # executable/version/capability detection
│   ├── capabilities.rs
│   ├── command.rs                  # PreparedConnection -> argv (pure, testable)
│   └── process.rs                   # spawns FreeRDP via runtime::process
│
├── ssh/
│   └── tunnel.rs                    # spawns tunnel via runtime::process, owns TunnelHandle
│
├── secret/
│   ├── service.rs                    # Secret Service backend
│   └── file.rs                       # encrypted-file fallback backend
│
├── config/
│   ├── file.rs                        # atomic write + flock
│   └── migrate.rs
│
├── tui/
│   ├── app.rs
│   ├── terminal.rs                     # TerminalGuard, catch_unwind boundary
│   ├── layout.rs
│   ├── keymap.rs
│   ├── theme.rs
│   └── views/
│
└── cli/
    └── commands.rs
```

Two module notes worth stating explicitly, since they resolve ambiguity a smaller team would otherwise rediscover by trial and error:

- **`ConnectionPlan` vs. `PreparedConnection`.** `planner.rs` builds a `ConnectionPlan` from a `Profile` and is pure — no network calls, no process spawns, safe to run for `rdp-tui inspect`. Turning a plan into something connectable is a separate, explicit `prepare()` step, defined in `model/prepared.rs` and implemented in `preflight.rs`/`ssh/tunnel.rs`, that is the only code path allowed to spawn a process. Only `preflight`, `test`, and `connect` call `prepare()`; `inspect`, `validate`, and `list` never do.
- **`runtime/` vs. protocol modules.** `runtime/process.rs` and `runtime/registry.rs` hold everything that isn't specific to FreeRDP or SSH: process-group/session creation at spawn, and the orphan-tracking registry. Both `freerdp/process.rs` and `ssh/tunnel.rs` call into `runtime::process` to actually spawn their children and register/deregister with `runtime::registry` around that spawn.

## 4. Dependency Rules

`model/` depends on nothing but the standard library and serialization traits — no Ratatui, Crossterm, FreeRDP command syntax, subprocess handles, D-Bus, XDG paths, or `runtime` types. `tui/` and `cli/` both depend on `model`, `planner`, `credentials`, `preflight`, `freerdp`, `ssh` directly — there is no intermediate "application service" layer, since one would do nothing but forward calls. `freerdp/` and `ssh/` depend on `model` and `runtime`. Forbidden: `model → tui`, `model → freerdp` argv implementation, `model → runtime`, or core logic duplicated between TUI and CLI. Do not add repository interfaces, DI containers, or generic trait hierarchies without a demonstrated concrete need.

## 5. Domain Types

```rust
struct Profile {
    id: ProfileId,
    name: String,
    endpoint: Endpoint,
    identity: IdentityConfig,
    route: Route,
    display: DisplayConfig,
    devices: DeviceConfig,
    security: SecurityConfig,
    credential: Option<CredentialRef>,
}

struct Endpoint { host: Host, port: u16 }
// Parsed exactly once, centrally. Supports hostname, hostname:port, IPv4, IPv4:port,
// [IPv6], [IPv6]:port. No subsystem independently reinterprets the endpoint string.

enum Route {
    Direct,
    RdGateway { gateway: Endpoint, credential: Option<CredentialRef> },
    SshTunnel { jump_host: String },   // stays compatible with ~/.ssh/config syntax
}
```

No FreeRDP-specific argv tokens belong in `Profile`'s semantic fields; advanced raw overrides exist separately.

## 6. ConnectionPlan — Pure, Inspectable Data

```rust
struct ConnectionPlan {
    target: Endpoint,
    route: PlannedRoute,
    identity: IdentityConfig,
    display: DisplayConfig,
    devices: DeviceConfig,
    security: SecurityConfig,
    credentials: ResolvedCredentials,   // which backend/key, not the secret itself
    client: FreeRdpClient,
}

enum PlannedRoute {
    Direct,
    RdGateway { gateway: Endpoint },
    SshTunnel { jump_host: String, target: Endpoint },
}
```

Built by `planner.rs` from a `Profile` plus detected `FreeRdpCapabilities`, and 100% pure regardless of route kind — safe to construct and print for `rdp-tui inspect` without ever touching the network or spawning a process, including for an SSH-tunnel route. Don't create duplicate "resolved" types for configuration that doesn't need resolution — `DisplayConfig` may be reused directly if no environmental transformation occurred; only environmental decisions (which client binary, which renderer) need a resolved representation, and those are plain data, not resources.

## 7. PreparedConnection — Acquired Execution State

```rust
struct PreparedConnection {
    plan: ConnectionPlan,
    effective_endpoint: Endpoint,   // 127.0.0.1:<local-port> for a tunnel, else plan.target
    route_handle: Option<RouteHandle>,
}

enum RouteHandle {
    SshTunnel(TunnelHandle),
    // Direct and RdGateway need no handle — nothing stateful to hold
}

struct TunnelHandle {
    child: Child,
    local_endpoint: Endpoint,
    established_at: Instant,
}
```

`prepare(plan: &ConnectionPlan) -> Result<PreparedConnection, ConnectionFailure>` is the only function allowed to spawn a tunnel process or otherwise acquire a stateful resource. `preflight`, `test`, and `connect` call it; `inspect`, `validate`, and `list` never do. The same `PreparedConnection` — for a tunnel route, the exact same live `TunnelHandle` — serves both the route-verification probe and the eventual FreeRDP launch; nothing tears the tunnel down and reacquires it. The FreeRDP adapter consumes a `PreparedConnection`, always launching against `effective_endpoint` rather than `plan.target` directly.

## 8. Process Lifecycle, Orphan Safety, and Session Detachment

Every spawned child (SSH tunnel, FreeRDP) is isolated into its own process group at spawn (`setsid()`/`setpgid()` via `runtime::process`).

**Orphan registry.** Live children are tracked at `$XDG_RUNTIME_DIR/rdp-tui/sessions/<session-id>.json` by a compound identity, not a bare PID:

```rust
struct ProcessIdentity {
    pid: u32,
    start_time_ticks: u64,   // /proc/<pid>/stat field 22 — the kernel's own process-generation counter
    uid: u32,
    kind: ChildKind,         // Tunnel | FreeRdp
    parent_session_id: SessionId,
}
```

A bare-PID match is unsafe: PIDs are recycled, and on a long-uptime machine the gap between a crash and the next `rdp-tui doctor` run is easily long enough for the kernel to hand that PID to an unrelated process. `doctor --fix` only ever acts on an entry where PID, `starttime`, and `uid` all still match what was recorded at spawn. On startup, `rdp-tui` scans this registry as a background task (reported via `BackgroundEvent`, not blocking the first frame): a non-matching entry is stale and silently discarded; a matching entry whose recorded parent session is no longer alive is surfaced passively, never terminated automatically.

**Session detachment policy.** A session launched via `connect` is **detached by default**: if `rdp-tui` itself dies, the running FreeRDP process — the window the user is actually looking at — is left running. This matches ordinary expectation (closing the launcher isn't the same action as closing the remote desktop window). A surviving FreeRDP process found on the next orphan scan is reported as **"still running,"** not as an orphan needing cleanup — those are different UI treatments for the same detection mechanism. A per-profile `detach = false` override may be added later if a concrete need for manager-owned sessions appears; it isn't required to ship v1.

**`PR_SET_PDEATHSIG`, scoped to not contradict detachment.** For a tunnel spawned by a one-shot, non-detaching invocation (`rdp-tui test`/a bare `preflight` that completes and exits without handing off to `connect`), no session depends on the tunnel once that process exits, so setting `PR_SET_PDEATHSIG = SIGTERM` on it before `exec` is a safe cleanup guarantee. **This must not be applied to a tunnel backing a `connect`ed session** — the whole point of detachment is that the session (and the tunnel it depends on) is allowed to survive `rdp-tui`'s death; `PDEATHSIG` there would sever a live, user-visible session the instant the launcher exits. `runtime::process` takes an explicit `detached: bool` at spawn and only sets `PDEATHSIG` when `detached == false`. Note: the signal fires when the specific *thread* that called `prctl` exits, not the whole process — safe for a synchronous single-threaded spawn, but if tunnel-spawning ever moves onto an async worker thread, the call must happen on a thread guaranteed to live exactly as long as the process, or the child can receive a spurious death signal from an unrelated worker exiting.

**Ephemeral SSH local port allocation.** Chosen by binding `127.0.0.1:0`, reading the assigned port, and closing that socket before spawning `ssh` — leaving a brief, unavoidable TOCTOU window before `ssh` actually binds it. Acceptable because `ExitOnForwardFailure=yes` (§9) converts a lost race into a clean, detectable failure: on failure, retry with a freshly bound candidate, bounded to a small number of attempts before surfacing a clear diagnostic.

## 9. SSH Tunnels

OpenSSH is spawned directly rather than an SSH library embedded — this preserves `~/.ssh/config`, identity selection, agent integration, FIDO/PKCS#11 support, `ProxyJump`, host-key policy, `known_hosts`, and `ControlMaster` behavior for free.

```text
ssh -N -T \
    -o BatchMode=yes \
    -o StrictHostKeyChecking=accept-new \
    -o ExitOnForwardFailure=yes \
    -L 127.0.0.1:<local-port>:<target>:<target-port> \
    <jump-host>
```

`BatchMode=yes` disables every interactive prompt — SSH fails fast with a diagnosable exit instead of hanging indefinitely once the process has been detached from a controlling terminal via `setsid()`; authentication must go through the agent or an unencrypted/agent-loaded key, and a passphrase-gated key fails with a clear "add the key to your agent" diagnostic rather than hanging silently. `StrictHostKeyChecking=accept-new` accepts a genuinely new host's key non-interactively (normal TOFU behavior) but still hard-fails, never silently accepts, if a previously-known host's key has *changed* — the one case where quiet non-interactive behavior would be a real risk. `ExitOnForwardFailure=yes` confirms SSH could establish the local listener; it does **not** prove the final target is reachable through it. Tunnel diagnostics distinguish SSH connection / local forwarding listener / target-through-tunnel (✓ / ✗ / unknown) as three separate rows.

`Route::SshTunnel` is user intent; `PlannedRoute::SshTunnel` is the same intent inside a pure plan; the live tunnel only exists inside `PreparedConnection::route_handle` after `prepare()` runs — see §6–7.

## 10. FreeRDP Discovery and Adapter

```rust
struct FreeRdpCapabilities {
    version: Version,
    sdl: bool,
    x11: bool,
    askpass: bool,
    gateway: bool,
    multimon: bool,
    dynamic_resolution: bool,
    auth_only: AuthOnlySupport,   // populated by validating deep-test's mechanism, §12
}
```

Executables, version, renderer availability, and capabilities are detected at startup or first use and cached appropriately; unsupported features fail during plan resolution where practical.

Exactly one module (`freerdp/command.rs`) translates a `PreparedConnection` into executable + argv + environment — no TUI, CLI, profile, or preflight code constructs FreeRDP switches. Command construction is a pure, testable operation. No shell invocation unless required: `Command::new(executable).arg(...)`, never `sh -c "xfreerdp ..."`.

## 11. Credentials

```rust
enum CredentialPreference { Automatic, Explicit(CredentialBackend) }
enum CredentialBackend { SecretService, EncryptedFile }
struct CredentialRef { backend: CredentialBackend, key: CredentialKey }
```

`automatic` is a *preference*, never persisted state. Resolution happens exactly once, at first storage: if the user picked an explicit backend, use it; if `Automatic`, attempt Secret Service and fall back to the encrypted file if unusable — then persist the concrete result into `CredentialRef.backend` immediately. Every subsequent read/write/delete uses that stored concrete backend directly, with zero re-probing — the environment changing later (a keyring daemon starting or stopping) never silently moves a secret. Changing backends is only ever the explicit `migrate` operation:

```text
retrieve old → write new → verify new → persist new CredentialRef → COMMIT → delete old
```

Failure before commit: old remains authoritative. Failure deleting old after commit: new remains authoritative, old becomes a recoverable orphan `rdp-tui doctor` may clean up later. The system always prefers a duplicate secret over a lost one. Secrets never appear in `profiles.toml`, `config.toml`, logs, history, dry-run output, diagnostic dumps, or argv where avoidable — the preferred authentication path is FreeRDP's ASKPASS mechanism.

## 12. Validate, Preflight, Deep-Test, and Connect

Four distinct operations, each with a documented evidentiary meaning the UI must never overstate:

**Validate** — schema, endpoint syntax, required fields, logical incompatibilities, known unsupported capabilities. No network access.

**Preflight** — non-invasive, topology-aware checks.

- *Direct:* DNS, TCP target, FreeRDP availability, credential availability.
- *RD Gateway:* target syntax; gateway DNS (required); gateway TCP/443 (required); local target DNS is **informational only** — many legitimate gateway deployments route to internal hostnames the local client cannot resolve at all, so a failed local lookup reports `NotApplicable`, never `Fail`. Direct target TCP reachability is never required for a gateway route. End-to-end target reachability through the gateway remains genuinely `Unknown` at this tier.
- *SSH:* SSH process starts; forward listener established; target through the *retained* tunnel is probed via the same `PreparedConnection` used for connect — never a throwaway tunnel that gets torn down and reacquired.

**Deep-Test** — an actual FreeRDP authentication/connectivity attempt, using FreeRDP's documented auth-only connection mode. Two safety requirements apply unconditionally:

1. Only exposed for a FreeRDP version/security-mode combination whose auth-only behavior has been validated at capability-detection time (`FreeRdpCapabilities.auth_only`); an unvalidated combination reports `NotSupported` rather than being attempted.
2. **Never triggered automatically**, always an explicit, individually-confirmed user action, with a one-time warning shown before the first deep-test against a given profile. A `last_deep_test` timestamp is persisted per profile under `$XDG_STATE_HOME` (through the same locked config-write path as everything else) as a courtesy rate limit against accidental repeated attempts from any process — this is a UI guardrail, not a lockout-prevention guarantee, since `rdp-tui` has no visibility into the target domain's actual lockout policy.

**Connect** — launch the real session via `prepare() → PreparedConnection → FreeRDP adapter`, as a detached process per §8's policy.

Diagnostic states are `Pass`, `Fail`, `Unknown`, `NotApplicable`, `InProgress`, `NotSupported` — `Unknown`/`NotApplicable` must never collapse into `Fail`.

## 13. Persistent Configuration

TOML unless implementation evidence strongly favors otherwise.

```text
$XDG_CONFIG_HOME/rdp-tui/   config.toml, profiles.toml
$XDG_STATE_HOME/rdp-tui/    history.toml, per-profile last_deep_test timestamps
$XDG_CACHE_HOME/rdp-tui/    transient capability/probe state
$XDG_RUNTIME_DIR/rdp-tui/   live-session registry (§8)
```

Every mutable persistent file gets atomic replacement (unique temp file, `fsync`, `rename()` onto the target, `fsync` the parent directory) and an exclusive lock around the read-modify-write window, implemented as **`flock()` on an open file descriptor — never a lock-file-existence check.** These have opposite crash behavior: a kernel-held `flock()` releases automatically the moment its holder's file descriptors close, including on a crash or `SIGKILL`, so no stale-lock state can outlive the process; a file-existence check leaves a permanent lock behind after any crash, wedging every future writer until a human deletes it by hand. The lock file itself may exist permanently and unlocked — its mere presence means nothing; only a currently-held `flock()` on it defines ownership, and it must never be unlinked as part of routine cleanup. Lock contention produces a typed error (e.g. `StoreError::Locked`), never a silent overwrite. Corrupt persistent files produce an explicit error and are preserved for recovery, never silently replaced with defaults.

Configuration carries a schema version (`version = 1`); migrations are explicit, testable, non-destructive, and backed up where appropriate. The existing Python configuration gets a documented import path.

## 14. Session and Failure Model

```rust
enum AttemptState { Resolving, AcquiringRoute, Preflighting, Launching }
enum SessionState { Running, Ended(SessionResult) }

struct SessionResult {
    duration: Duration,
    exit_code: Option<i32>,
    failure: Option<ConnectionFailure>,
    renderer: Renderer,
    freerdp_version: Version,
}

enum ConnectionFailure {
    Configuration, Dns, Network, Timeout,
    Ssh, Tunnel,
    Gateway, Authentication, Certificate,
    UnsupportedCapability, FreeRdpMissing, FreeRdpVersion, Renderer,
    ProcessFailure, Unknown,
}
```

`AttemptState` covers phases `rdp-tui` performs itself and therefore genuinely knows; `SessionState` covers only what an external FreeRDP process can actually be observed to be doing — there is deliberately no `Authenticating`/`Connecting`/`Reconnecting` distinction, since FreeRDP doesn't reliably expose those as structured, version-independent signals. Add finer states later only behind explicit, version-gated capability detection. Classify failures when evidence permits; preserve raw diagnostic context alongside the friendly classification rather than replacing it.

## 15. Background Work

The event loop never blocks on network probes, SSH startup, FreeRDP process lifetime, slow credential D-Bus calls, or filesystem operations with meaningful blocking potential. UI-local operations (navigation, filtering) are direct function calls with no messaging overhead. Background results arrive through one narrow event type:

```rust
enum BackgroundEvent {
    PreflightFinished(ProfileId, PreflightResult),
    TunnelFailed(ProfileId, TunnelError),
    SessionExited(SessionId, SessionResult),
    OrphanScanComplete(Vec<DiscoveredProcess>),
}
```

Implementation is `std::thread` + `mpsc` initially; move to Tokio only if actual workflow complexity demonstrably warrants it — and if so, respect the `PR_SET_PDEATHSIG` thread-scoping note in §8 before moving tunnel-spawning onto a worker thread.

## 16. Terminal Failure Safety

```rust
let mut terminal = TerminalGuard::enter()?;   // raw mode + alternate screen

let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
    run_tui(&mut terminal)
}));

terminal.restore();   // always runs

if let Err(panic) = result {
    std::panic::resume_unwind(panic);
}
```

`TerminalGuard` owns setup/teardown; `restore()` runs unconditionally whether the TUI returned normally or unwound. A global `std::panic::set_hook` alone is insufficient and actively dangerous here: it fires for *any* thread's panic, including a background probe or tunnel-management worker, and could tear down the alternate screen while the render thread is still alive and drawing into it. If a defensive fallback hook exists at all, it must be gated on an atomic `TERMINAL_ACTIVE` flag the guard sets/clears, and must never itself be able to panic. `SIGINT` does not run terminal code directly from within the signal handler — Crossterm operations aren't async-signal-safe and risk deadlocking against a lock the interrupted code already held; instead the handler sets an atomic flag (or writes to a self-pipe) that the normal event loop polls each tick, letting shutdown and `TerminalGuard::restore()` run through the ordinary, non-signal-handler code path. None of this covers `SIGKILL` — that's handled by the orphan-detection mechanism in §8 on next launch, not by in-process cleanup, because it's fundamentally not preventable in-process in any language.

## 17. TUI and CLI Contracts

Terminal-first, keyboard-complete: `↑↓`/`jk` navigate, `Enter` primary action, `Space` select/toggle, `Tab` focus, `/` search, `?` help, `Esc` back/cancel, `q` quit/back, `Ctrl-C` safe termination. Mouse support may supplement; keybindings should eventually be configurable. Layouts: wide, standard, compact — the TUI must never panic because the terminal is too small, and per §16, even if something else panics, the terminal itself is still restored.

```text
rdp-tui list
rdp-tui show <profile>
rdp-tui inspect <profile>       # never calls prepare() — pure ConnectionPlan only
rdp-tui validate <profile>
rdp-tui test <profile>          # calls prepare(); non-detaching; PDEATHSIG applies
rdp-tui connect <profile>       # calls prepare(); detached session
rdp-tui import <file.rdp>
rdp-tui export <profile>
rdp-tui doctor
rdp-tui config paths
rdp-tui info
```

TUI and CLI invoke the same underlying implementation — there is exactly one connection/credential/diagnostic/persistence code path, not parallel ones. `rdp-tui inspect` shows the *planned* route and a template FreeRDP command; it never resolves `effective_endpoint` for a tunnel, since that value doesn't exist until `prepare()` runs, and `inspect` never calls `prepare()`. Secrets are always redacted in inspection output.

## 18. Logging

Levels: error, warn, info, debug, trace. Logs answer: what profile was resolved, what route was selected, what FreeRDP client/version was selected, what network checks occurred, what resource failed, what exit code FreeRDP returned. Never log secrets; generated commands pass through explicit redaction before logging.

## 19. Testing Contract

**Model** — endpoint parsing, IPv6, ports, route configuration, certificate policy, profile parsing, `ConnectionPlan` construction with an assertion that zero I/O occurs for any route kind including `SshTunnel`. No terminal or FreeRDP dependency.

**FreeRDP adapter** — given a `PreparedConnection`, assert generated argv for direct, RD Gateway, SSH effective endpoint, fullscreen, multimon, dynamic resolution, audio, clipboard, drive, microphone, certificate modes, advanced options, plus redacted logging output.

**Credentials** — Automatic+available → pinned SecretService; Automatic+unavailable → pinned EncryptedFile; a subsequent environment change never changes an already-pinned backend; migration succeeds / fails-before-commit / fails-after-commit; delete-profile cleanup.

**Persistence** — atomic replacement; corrupt-file detection; concurrent writer locking, including a test that kills the lock-holding process mid-write and confirms `flock()` releases automatically and the next writer isn't wedged; unique temp files; a failed write leaves the previous file valid.

**SSH tunnel** — SSH exits immediately; forward cannot be created; forward becomes available; target-through-tunnel reachable/unavailable; tunnel dies mid-session; cleanup on normal end and on failed launch; `rdp-tui` killed while a tunnel backs a `connect`ed (detached) session → tunnel *survives*, reported as "still running"; `rdp-tui` killed while a tunnel backs a `test`-only (non-detached) invocation → `PDEATHSIG` actually terminates it; ephemeral-port allocation retry under a forced steal-before-bind race.

**Terminal safety** — force a panic inside TUI render/input handling and assert raw mode and the alternate screen are correctly restored; force a panic on a background worker thread and assert the terminal is *not* touched while the main loop's `catch_unwind` boundary is otherwise still active; the same restoration assertion for a simulated `SIGINT` delivered through the deferred-flag path.

**Deep-test** — the persisted rate limit blocks a second invocation from a *separate process* within the window; the one-time warning appears before the first deep-test per profile; a version capability-detected as lacking validated auth-only behavior returns `NotSupported` rather than silently attempting a full connection.

**TUI** — Ratatui test backend where useful; representative geometry/content cases (80×24, normal desktop, very wide; 0 profiles, many profiles, long strings, modal/error state) rather than a mandated combinatorial matrix; add regression snapshots when actual rendering bugs are found.

## 20. Packaging and Hackability

```text
pacman -S rdp-tui
rdp-tui
```

or AUR equivalent before official inclusion. Required runtime dependency: FreeRDP. Optional: a Secret Service implementation, OpenSSH, compositor-specific helpers — missing optional integrations degrade explicitly rather than crashing.

Prefer ordinary structs and enums, plain functions, explicit error paths, small modules, minimal macros and generics — avoid architecture whose primary benefit is demonstrating Rust sophistication. `git clone && cargo run && cargo test` should just work. The source should stay readable enough that a Python-comfortable Linux developer can inspect and modify behavior without expert-level Rust knowledge.

## 21. Rewrite Strategy

Before committing to the full Rust port, spike-test the two genuinely uncertain external-tool facts this contract depends on — independent of implementation language, since these are empirical questions about OpenSSH and FreeRDP, not design choices:

- SSH's actual behavior under `BatchMode=yes`/`StrictHostKeyChecking=accept-new` against real jump-host configurations this project targets.
- FreeRDP's auth-only mode behavior across the specific `xfreerdp`/`sdl-freerdp3` versions in the packaging target.

Credential pinning and atomic-write-plus-lock, by contrast, are fully specified patterns with no remaining uncertainty and don't need a Python-side proving ground — implement them directly in Phase 1/2 with unit tests.

```text
Phase 1  model (incl. ConnectionPlan/PreparedConnection split), endpoint parsing, config parsing
Phase 2  persistence, credentials, FreeRDP discovery (incl. auth_only capability
         detection), FreeRDP argv, SSH tunnel lifecycle (process-group, orphan
         registry, detachment policy), topology-aware preflight
Phase 3  CLI: list, inspect, validate, test, connect
Phase 4  Ratatui frontend (incl. catch_unwind-based terminal safety)
Phase 5  import/migration from the Python implementation, behavior comparison,
         regression verification
```

The existing Python implementation is an executable behavioral reference for Phase 5 — parity means preserving intended user-visible behavior, not preserving its known implementation defects.

## 22. Core Invariants

1. **Intent vs. execution, in three distinct types** — `Profile` (user intent) → `ConnectionPlan` (resolved-but-unacquired data, zero I/O) → `PreparedConnection` (acquired execution state). Plan resolution never has side effects.
2. **FreeRDP syntax boundary** — only the FreeRDP adapter emits FreeRDP argv syntax, and only from a `PreparedConnection`.
3. **Secret safety** — secrets never appear in normal config, argv where avoidable, logs, history, dry-run output, or diagnostic reports.
4. **Concrete credential provenance** — after first storage, every credential has a concrete authoritative backend, never implicitly re-resolved.
5. **Persistent mutation safety** — every read-modify-write is `flock()`-protected and atomically replaced; the lock file's mere existence never implies ownership.
6. **Resource lifecycle correctness** — stateful routes retain acquired resources for the lifetime of the connection depending on them; the tunnel used for preflight is the tunnel used to connect.
7. **Honest observability** — the UI never claims knowledge the system doesn't expose. Unknown/NotApplicable is not Fail. TCP/443-reachable is not RD-Gateway-valid. Unresolvable local DNS through a gateway is not a broken profile. A live FreeRDP process isn't labeled "Authenticating" unless that's actually observable.
8. **Shared implementation** — CLI and TUI invoke the same connection, credential, diagnostic, and persistence logic.
9. **Semantic core isolation** — the model depends on none of Ratatui, Crossterm, FreeRDP CLI syntax, D-Bus, filesystem layout, or `runtime`'s process types.
10. **Human inspectability without resource acquisition** — configuration, resolved plan, client choice, capabilities, credential backend, redacted FreeRDP invocation, and diagnostics are all inspectable without acquiring any resource or starting any process, including a tunnel.
11. **Orphan safety with correct identity** — a child left behind by an abnormally-terminated `rdp-tui` is matched via `(pid, starttime, uid)`, never bare PID, before any cleanup action is offered.
12. **Terminal safety without cross-thread interference** — a panic anywhere restores the terminal exactly once, through the guarded main-loop boundary, without an unrelated background-thread panic doing the same thing prematurely.
13. **Detachment is a stated policy, and lifecycle mechanisms must match it** — a `connect`ed session survives `rdp-tui`'s death by design; `PDEATHSIG` is applied only where it cannot contradict that (one-shot, non-detaching invocations only).

## 23. Guiding Principle

> Prefer the design that makes failure semantics, state ownership, and underlying Linux mechanisms explicit.

Do not add abstraction merely to look architecturally complete. Do not remove structure merely to reduce file count. The goal is one source of truth, clear resource ownership, safe persistence, honest diagnostics, and simple code. `rdp-tui` should behave like a first-class Linux tool whose TUI happens to be sophisticated, rather than a GUI application translated into terminal widgets or a thin wrapper around a long FreeRDP command.

---

## Appendix — Design Rationale

A few decisions in this contract aren't obvious from the rule alone; here's the reasoning behind each, briefly:

- **Why `ConnectionPlan` can't own a live process handle.** Early drafts let the resolved route type carry a spawned tunnel process directly. That silently gave *plan resolution* — something `inspect` promises to do without side effects — the power to start a process for any SSH profile. Splitting into a pure `ConnectionPlan` and a `PreparedConnection` produced by an explicit `prepare()` call keeps the "resolve vs. acquire" boundary real instead of aspirational.
- **Why RD Gateway preflight can't require local target DNS.** A gateway's entire purpose is often reaching targets the local client can't resolve or route to at all (split-horizon/internal DNS). Requiring it to succeed fails legitimate, common deployments and violates the contract's own "don't claim knowledge the topology doesn't give you" principle.
- **Why credential backend selection is pinned instead of re-checked every time.** "Automatic" resolved at every read/write (checking whether a keyring daemon happens to be running *right now*) creates a real bug: a profile's secret can end up split across two backends as the desktop environment's keyring state changes between saves, with nothing ever cleaning up the resulting orphan. Resolving once, at first storage, and treating the result as fixed until an explicit migration closes that gap entirely.
- **Why the lock primitive matters, not just "have a lock."** A kernel-held `flock()` disappears automatically if its holder crashes; a "does this file exist" check leaves a permanent lock after any crash. Given the whole reason for locking is defending against processes dying unexpectedly, only the former actually satisfies the requirement.
- **Why deep-test needs both a feasibility gate and a persisted rate limit.** FreeRDP's auth-only behavior isn't guaranteed identical across versions and security modes, so it needs per-version validation before being trusted. Separately, repeated authentication attempts against a real target can trigger account lockout policy `rdp-tui` has no visibility into — so the rate limit is framed honestly as a courtesy against accidents, not a safety guarantee, and the one requirement that actually matters unconditionally is that it's never automatic.
- **Why sessions are detached by default, and why that determines where `PDEATHSIG` can be used.** Killing the FreeRDP window because the launcher's terminal crashed would violate ordinary user expectation — nobody expects their open remote desktop to vanish because of that. Once that's the policy, a tunnel backing a live session can't be unconditionally death-signaled without contradicting it; `PDEATHSIG` is scoped narrowly to the one case (a one-shot `test`/`preflight` invocation) where nothing depends on the tunnel surviving.
- **Why orphan detection matches on more than PID.** PIDs are recycled by the kernel; matching on PID alone risks a cleanup command someday terminating a completely unrelated process that happens to have inherited the number. Adding the process's start-time tick count from `/proc/<pid>/stat` — the same technique real process supervisors use — makes the match unambiguous at negligible cost.
- **Why terminal restoration is `catch_unwind`-based rather than a global panic hook.** A panic hook is process-global and fires for *any* thread, including background workers — a hook alone risks tearing down the terminal out from under a render thread that's still alive and drawing. A `catch_unwind` boundary scoped to the actual TUI run loop, with the hook kept only as a narrow, flag-gated fallback, keeps the two properly separated.
