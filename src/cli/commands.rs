//! Read-only and connection command implementations for the Rust frontend.

use crate::ProfileStore;
use crate::config::ConfigStore;
use crate::config::migrate::import_python_profiles;
use crate::credentials::SystemCredentialStore;
use crate::freerdp::certificate;
use crate::freerdp::discover::discover;
use crate::model::fields;
use crate::model::{
    CertificatePolicy, ConnectionPlan, DeviceConfig, DisplayConfig, Endpoint, GraphicsMode,
    IdentityConfig, NetworkProfile, Profile, ProfileId, Renderer, Route, SecurityConfig,
};
use crate::operations::{self, CredentialSlot};
use crate::planner::plan;
use crate::session::{connect_profile, test_profile};
use secrecy::zeroize::Zeroizing;
use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Run a CLI command and return text suitable for stdout.
///
/// # Errors
///
/// Returns a user-facing error when arguments are invalid or a command fails.
pub fn run(arguments: &[String], config_root: &PathBuf) -> Result<String, String> {
    let store = ProfileStore::new(ConfigStore::new(config_root));
    match arguments {
        [] => list(&store),
        [command] if command == "list" => list(&store),
        [command, id] if command == "show" => show(&store, id),
        [command, id] if command == "inspect" => inspect(&store, id),
        [command] if command == "validate" => validate(&store),
        [command, id] if command == "test" => test(&store, id),
        [command, id] if command == "deep-test" => deep_test(&store, config_root, id, false),
        [command, id, confirm] if command == "deep-test" => {
            deep_test(&store, config_root, id, affirmative(confirm))
        }
        [command] if command == "history" => history(&store, None),
        [command, id] if command == "history" => history(&store, Some(id)),
        [command, id] if command == "connect" => connect(&store, id),
        [command, sub, id] if command == "credential" && sub == "set" => {
            credential_set(&store, config_root, id, CredentialSlot::Main)
        }
        [command, sub, id] if command == "credential" && sub == "clear" => {
            credential_clear(&store, config_root, id, CredentialSlot::Main)
        }
        [command, sub, id, slot] if command == "credential" && sub == "set" => {
            credential_set(&store, config_root, id, parse_credential_slot(slot)?)
        }
        [command, sub, id, slot] if command == "credential" && sub == "clear" => {
            credential_clear(&store, config_root, id, parse_credential_slot(slot)?)
        }
        [command, sub, id] if command == "certificate" && sub == "show" => {
            certificate_show(&store, config_root, id)
        }
        [command, sub, id] if command == "certificate" && sub == "backups" => {
            certificate_backups(&store, id)
        }
        [command, sub, id, policy] if command == "certificate" && sub == "policy" => {
            certificate_policy(&store, id, policy)
        }
        [command, sub, id, backup] if command == "certificate" && sub == "restore" => {
            certificate_restore(&store, config_root, id, backup)
        }
        [command, sub, id, flag, fingerprint]
            if command == "certificate" && sub == "trust" && flag == "--fingerprint" =>
        {
            certificate_trust(&store, config_root, id, fingerprint)
        }
        [command, name, host] if command == "add" => add_command(&store, name, host),
        [command, id] if command == "delete" => delete_command(&store, config_root, id),
        [command, id] if command == "clone" => clone_command(&store, id),
        [command, id, field, new_value] if command == "set" => {
            set_command(&store, config_root, id, field, new_value)
        }
        [command, path] if command == "import" => import_command(&store, path),
        [command, id, path] if command == "export" => export_command(&store, id, path),
        [command] if command == "config-paths" => Ok(format!(
            "config.toml: {}\nprofiles.toml: {}\n",
            config_root.join("config.toml").display(),
            config_root.join("profiles.toml").display()
        )),
        [command] if command == "info" => Ok("rdp-tui Rust frontend: inspection mode\n".into()),
        [command] if command == "doctor" => Ok(doctor()),
        [command, kind, source] if command == "migrate" && kind == "python" => {
            migrate_python(&store, &config_root.join(source))
        }
        [command, kind] if command == "migrate" && kind == "python" => {
            migrate_python(&store, &config_root.join("profiles.json"))
        }
        _ => Err(usage().into()),
    }
}

fn load_profile(store: &ProfileStore, value: &str) -> Result<Profile, String> {
    let id = value
        .parse::<ProfileId>()
        .map_err(|error| error.to_string())?;
    store
        .get(id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("profile {id} was not found"))
}

fn inspect(store: &ProfileStore, value: &str) -> Result<String, String> {
    let profile = load_profile(store, value)?;
    let discovered = discover(profile.display.renderer)?;
    let connection: ConnectionPlan = plan(&profile, &discovered.capabilities, discovered.client)
        .map_err(|error| format!("cannot plan connection: {error:?}"))?;
    Ok(format!(
        "profile: {}\ntarget: {}\nroute: {:?}\nclient: {} {}\n",
        profile.name,
        connection.target,
        connection.route,
        connection.client.executable.display(),
        connection.client.version
    ))
}

fn test(store: &ProfileStore, value: &str) -> Result<String, String> {
    let profile = load_profile(store, value)?;
    test_profile(&profile, TEST_TIMEOUT).map_err(|error| error.to_string())?;
    Ok(format!("test: {} is reachable\n", profile.name))
}

fn connect(store: &ProfileStore, value: &str) -> Result<String, String> {
    let profile = load_profile(store, value)?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let session = connect_profile(&profile, &executable).map_err(|error| error.to_string())?;
    Ok(format!(
        "connect: launched detached session {session} for {}\n",
        profile.name
    ))
}

fn credential_set(
    store: &ProfileStore,
    config_root: &Path,
    value: &str,
    slot: CredentialSlot,
) -> Result<String, String> {
    let profile = load_profile(store, value)?;
    let name = profile.name.clone();
    let mut password = Zeroizing::new(String::new());
    std::io::stdin()
        .read_line(&mut password)
        .map_err(|error| error.to_string())?;
    let password = password.trim_end_matches(['\n', '\r']);
    if password.is_empty() {
        return Err("no password was provided on stdin".into());
    }
    operations::replace_password(store, config_root, profile, slot, password)?;
    Ok(format!(
        "credential: stored the {} password for {name}\n",
        slot_name(slot)
    ))
}

fn credential_clear(
    store: &ProfileStore,
    config_root: &Path,
    value: &str,
    slot: CredentialSlot,
) -> Result<String, String> {
    let profile = load_profile(store, value)?;
    let name = profile.name.clone();
    let (_, cleared) = operations::clear_password(store, config_root, profile, slot)?;
    Ok(if cleared {
        format!(
            "credential: cleared the {} password for {name}\n",
            slot_name(slot)
        )
    } else {
        format!(
            "credential: {name} had no {} password to clear\n",
            slot_name(slot)
        )
    })
}

/// Store `password` in the encrypted-file backend and pin the resulting concrete
/// `CredentialRef` on the profile (INV-4). Any previously pinned secret is
/// removed on a best-effort basis.
///
/// # Errors
///
/// Returns an error when the secret cannot be stored or the profile saved.
pub fn set_profile_credential(
    store: &ProfileStore,
    config_root: &Path,
    profile: Profile,
    password: &str,
) -> Result<(), String> {
    operations::replace_password(store, config_root, profile, CredentialSlot::Main, password)
        .map(|_| ())
}

fn parse_credential_slot(value: &str) -> Result<CredentialSlot, String> {
    match value {
        "main" => Ok(CredentialSlot::Main),
        "gateway" => Ok(CredentialSlot::Gateway),
        _ => Err("credential slot must be 'main' or 'gateway'".into()),
    }
}

const fn slot_name(slot: CredentialSlot) -> &'static str {
    match slot {
        CredentialSlot::Main => "main",
        CredentialSlot::Gateway => "gateway",
    }
}

fn certificate_policy(store: &ProfileStore, value: &str, policy: &str) -> Result<String, String> {
    let policy = CertificatePolicy::from_token(policy).ok_or_else(|| {
        format!("unknown certificate policy '{policy}' (use tofu | system | ignore | deny)")
    })?;
    let mut profile = load_profile(store, value)?;
    let name = profile.name.clone();
    profile.security.certificate_policy = policy;
    store.upsert(profile).map_err(|error| error.to_string())?;
    Ok(format!("certificate: {name} now uses {policy:?}\n"))
}

fn certificate_show(
    store: &ProfileStore,
    config_root: &Path,
    value: &str,
) -> Result<String, String> {
    let profile = load_profile(store, value)?;
    let (host, port) = endpoint_host_port(&profile);
    let pin = certificate::pin_path(&freerdp_config_dir(config_root), &host, port);
    let pinned = certificate::fingerprint(&pin).map_err(|error| error.to_string())?;
    let pending =
        certificate::read_mismatch(&state_dir(), profile.id).map_err(|error| error.to_string())?;
    Ok(format!(
        "profile: {}\nendpoint: {}\npolicy: {:?}\npinned: {}\npresented: {}\n",
        profile.name,
        profile.endpoint,
        profile.security.certificate_policy,
        pinned.as_deref().unwrap_or("none"),
        pending
            .as_ref()
            .map_or("none", |mismatch| mismatch.presented_sha256.as_str()),
    ))
}

fn certificate_backups(store: &ProfileStore, value: &str) -> Result<String, String> {
    let profile = load_profile(store, value)?;
    let (host, port) = endpoint_host_port(&profile);
    let prefix = format!("{host}_{port}.pem.");
    let mut names: Vec<String> = match std::fs::read_dir(certificate_backups_dir()) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with(&prefix))
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.to_string()),
    };
    names.sort();
    if names.is_empty() {
        Ok(format!("no certificate backups for {}\n", profile.name))
    } else {
        Ok(format!("{}\n", names.join("\n")))
    }
}

fn certificate_restore(
    store: &ProfileStore,
    config_root: &Path,
    value: &str,
    backup: &str,
) -> Result<String, String> {
    let profile = load_profile(store, value)?;
    let (host, port) = endpoint_host_port(&profile);
    let backup_path = Path::new(backup);
    if backup_path.file_name().and_then(std::ffi::OsStr::to_str) != Some(backup)
        || !backup.starts_with(&format!("{host}_{port}.pem."))
        || backup_path.extension().and_then(std::ffi::OsStr::to_str) != Some("bak")
    {
        return Err(format!(
            "backup {backup} does not belong to {}",
            profile.name
        ));
    }
    let source = certificate_backups_dir().join(backup);
    match std::fs::symlink_metadata(&source) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(format!("backup {backup} is not a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("backup {backup} was not found"));
        }
        Err(error) => return Err(error.to_string()),
    }
    if certificate::fingerprint(&source)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return Err(format!("backup {backup} is not a valid PEM certificate"));
    }
    let pin = certificate::pin_path(&freerdp_config_dir(config_root), &host, port);
    if let Some(parent) = pin.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let current = match certificate::fingerprint(&pin).map_err(|error| error.to_string())? {
        Some(fingerprint) => Some(
            certificate::archive(&pin, &certificate_backups_dir(), Some(&fingerprint))
                .map_err(|error| error.to_string())?,
        ),
        None => None,
    };
    if let Err(error) = std::fs::rename(&source, &pin) {
        if let Some(current) = current {
            let _ = std::fs::rename(current, &pin);
        }
        return Err(error.to_string());
    }
    std::fs::set_permissions(&pin, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "restored {backup} as the pinned certificate for {}\n",
        profile.name
    ))
}

fn certificate_trust(
    store: &ProfileStore,
    config_root: &Path,
    value: &str,
    fingerprint: &str,
) -> Result<String, String> {
    let profile = load_profile(store, value)?;
    let name = profile.name.clone();
    let mismatch = operations::trust_certificate_mismatch(
        store,
        config_root,
        &state_dir(),
        profile,
        fingerprint,
    )?;
    Ok(format!(
        "trusting {} for {name}; reconnect to pin it (policy set to tofu)\n",
        mismatch.presented_sha256
    ))
}

fn freerdp_config_dir(config_root: &Path) -> PathBuf {
    crate::paths::freerdp_config_root(config_root)
}

fn certificate_backups_dir() -> PathBuf {
    crate::paths::certificate_backups_dir()
}

fn endpoint_host_port(profile: &Profile) -> (String, u16) {
    (profile.endpoint.host.to_string(), profile.endpoint.port)
}

fn import_command(store: &ProfileStore, path: &str) -> Result<String, String> {
    let counts = operations::import_profiles(store, Path::new(path))?;
    Ok(format!(
        "import: {} added, {} updated, {} skipped, {} failed\n",
        counts.added, counts.updated, counts.skipped, counts.failed
    ))
}

fn export_command(store: &ProfileStore, value: &str, path: &str) -> Result<String, String> {
    let profile = load_profile(store, value)?;
    let mut destination = PathBuf::from(path);
    if destination.extension().and_then(std::ffi::OsStr::to_str) != Some("rdp") {
        destination.set_extension("rdp");
    }
    if let Some(parent) = destination.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(&destination, crate::config::import::export_rdp(&profile))
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "exported {} to {} (password excluded)\n",
        profile.name,
        destination.display()
    ))
}

fn add_command(store: &ProfileStore, name: &str, host: &str) -> Result<String, String> {
    let endpoint = host
        .parse::<Endpoint>()
        .map_err(|error| format!("invalid host '{host}': {error}"))?;
    let profile = Profile {
        id: ProfileId::generate(),
        name: name.to_owned(),
        endpoint,
        identity: IdentityConfig::default(),
        route: Route::Direct,
        display: DisplayConfig::default(),
        devices: DeviceConfig::default(),
        security: SecurityConfig::default(),
        credential: None,
    };
    let id = profile.id;
    store.upsert(profile).map_err(|error| error.to_string())?;
    Ok(format!("added {name} as {id}\n"))
}

fn delete_command(store: &ProfileStore, config_root: &Path, value: &str) -> Result<String, String> {
    let profile = load_profile(store, value)?;
    let name = profile.name.clone();
    operations::delete_profile(store, config_root, &profile)?;
    Ok(format!("deleted {name}\n"))
}

fn clone_command(store: &ProfileStore, value: &str) -> Result<String, String> {
    let mut profile = load_profile(store, value)?;
    profile.id = ProfileId::generate();
    profile.name = format!("{} (copy)", profile.name);
    // A clone starts without the source's secret: the CredentialRef points at one
    // stored file, and sharing it would let deleting either profile forget both.
    operations::strip_credentials(&mut profile);
    let (id, name) = (profile.id, profile.name.clone());
    store.upsert(profile).map_err(|error| error.to_string())?;
    Ok(format!("cloned to {name} ({id})\n"))
}

fn set_command(
    store: &ProfileStore,
    config_root: &Path,
    value: &str,
    field: &str,
    new_value: &str,
) -> Result<String, String> {
    let mut profile = load_profile(store, value)?;
    match field {
        "name" => new_value.clone_into(&mut profile.name),
        "host" => {
            profile.endpoint = new_value
                .parse::<Endpoint>()
                .map_err(|error| format!("invalid host '{new_value}': {error}"))?;
        }
        "username" => new_value.clone_into(&mut profile.identity.username),
        "domain" => new_value.clone_into(&mut profile.identity.domain),
        "gateway-username" => match &mut profile.route {
            Route::RdGateway { username, .. } => new_value.clone_into(username),
            _ => return Err("set route to gateway:<host> before setting gateway-username".into()),
        },
        "gateway-domain" => match &mut profile.route {
            Route::RdGateway { domain, .. } => new_value.clone_into(domain),
            _ => return Err("set route to gateway:<host> before setting gateway-domain".into()),
        },
        "fullscreen" => profile.display.fullscreen = fields::parse_bool(new_value)?,
        "renderer" => {
            profile.display.renderer = Renderer::from_token(new_value)
                .ok_or_else(|| format!("unknown renderer '{new_value}' (use wayland_sdl | x11)"))?;
        }
        "resolution" => profile.display.resolution = fields::parse_resolution(new_value)?,
        "route" => profile.route = Route::from_token(new_value)?,
        "multimon" => profile.display.multimon = fields::parse_bool(new_value)?,
        "span-monitors" => profile.display.span_monitors = fields::parse_bool(new_value)?,
        "smart-sizing" => profile.display.smart_sizing = fields::parse_bool(new_value)?,
        "dynamic-resolution" => profile.display.dynamic_resolution = fields::parse_bool(new_value)?,
        "scale" => profile.display.scale_percent = fields::parse_scale(new_value)?,
        "color-depth" => profile.display.color_depth = fields::parse_color_depth(new_value)?,
        "clipboard" => profile.devices.clipboard = fields::parse_bool(new_value)?,
        "audio" => profile.devices.audio_playback = fields::parse_bool(new_value)?,
        "microphone" => profile.devices.microphone = fields::parse_bool(new_value)?,
        "printers" => profile.devices.printers = fields::parse_bool(new_value)?,
        "graphics" => {
            profile.display.graphics = GraphicsMode::from_token(new_value).ok_or_else(|| {
                format!("unknown graphics mode '{new_value}' (use auto | rfx | avc420 | avc444)")
            })?;
        }
        "admin-session" => profile.security.admin_session = fields::parse_bool(new_value)?,
        "network" => {
            profile.security.network_profile = NetworkProfile::from_token(new_value).ok_or_else(
                || {
                    format!(
                        "unknown network profile '{new_value}' (use auto | modem | broadband-low | broadband-high | wan | lan)"
                    )
                },
            )?;
        }
        other => {
            return Err(format!(
                "unknown field '{other}' (name | host | username | domain | fullscreen | \
                 renderer | resolution | route | gateway-username | gateway-domain | multimon | span-monitors | smart-sizing | \
                 dynamic-resolution | scale | color-depth | clipboard | audio | microphone | \
                 printers | graphics | admin-session | network)"
            ));
        }
    }
    let name = profile.name.clone();
    operations::save_profile(store, config_root, profile)?;
    Ok(format!("set {field} on {name}\n"))
}

fn migrate_python(store: &ProfileStore, source: &std::path::Path) -> Result<String, String> {
    let text = std::fs::read_to_string(source).map_err(|error| error.to_string())?;
    let document = import_python_profiles(&text).map_err(|error| error.to_string())?;
    let count = document.profiles.len();
    store
        .merge(document.profiles)
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "migrated: {count} profile(s); secrets were not migrated\n"
    ))
}

fn doctor() -> String {
    let mut output = String::new();
    for (label, renderer) in [
        ("wayland_sdl", Renderer::WaylandSdl),
        ("x11", Renderer::X11),
    ] {
        match discover(renderer) {
            Ok(found) => writeln!(
                output,
                "{label}: {} ({})",
                found.client.executable.display(),
                found.client.version
            )
            .expect("writing to a String cannot fail"),
            Err(error) => writeln!(output, "{label}: unavailable ({error})")
                .expect("writing to a String cannot fail"),
        }
    }
    let sessions = crate::session::scan_sessions(&crate::session::record::sessions_dir());
    if sessions.is_empty() {
        writeln!(output, "sessions: none active").expect("writing to a String cannot fail");
    } else {
        for status in sessions {
            writeln!(
                output,
                "session {}: {:?} (profile {})",
                status.record.session_id, status.health, status.record.profile_id
            )
            .expect("writing to a String cannot fail");
        }
    }
    output
}

fn list(store: &ProfileStore) -> Result<String, String> {
    let profiles = store.list().map_err(|error| error.to_string())?;
    let mut output = String::new();
    for profile in profiles {
        writeln!(
            output,
            "{}\t{}\t{}",
            profile.id, profile.name, profile.endpoint
        )
        .expect("writing to a String cannot fail");
    }
    Ok(output)
}

fn show(store: &ProfileStore, value: &str) -> Result<String, String> {
    let profile = load_profile(store, value)?;
    toml::to_string_pretty(&profile).map_err(|error| error.to_string())
}

fn validate(store: &ProfileStore) -> Result<String, String> {
    let profiles = store.list().map_err(|error| error.to_string())?;
    let mut output = String::new();
    let mut issues = 0_usize;
    for profile in &profiles {
        match validate_profile(profile) {
            Ok(()) => writeln!(output, "{}: ok", profile.name),
            Err(reason) => {
                issues += 1;
                writeln!(output, "{}: {reason}", profile.name)
            }
        }
        .expect("writing to a String cannot fail");
    }
    writeln!(
        output,
        "{} profile(s) checked, {issues} with issues",
        profiles.len()
    )
    .expect("writing to a String cannot fail");
    Ok(output)
}

fn validate_profile(profile: &Profile) -> Result<(), String> {
    let discovered =
        discover(profile.display.renderer).map_err(|_| "FreeRDP unavailable".to_string())?;
    plan(profile, &discovered.capabilities, discovered.client)
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}

fn history(store: &ProfileStore, filter: Option<&str>) -> Result<String, String> {
    let wanted = match filter {
        Some(value) => Some(
            value
                .parse::<ProfileId>()
                .map_err(|error| error.to_string())?,
        ),
        None => None,
    };
    let document = ConfigStore::new(state_dir())
        .load_history()
        .map_err(|error| error.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let mut output = String::new();
    let mut shown = 0_usize;
    for entry in document.entries.iter().rev() {
        if wanted.is_some_and(|id| entry.profile_id != id) {
            continue;
        }
        let name = store.get(entry.profile_id).ok().flatten().map_or_else(
            || format!("(removed {})", entry.profile_id),
            |profile| profile.name,
        );
        writeln!(output, "{}", entry.summarize(&name, now))
            .expect("writing to a String cannot fail");
        shown += 1;
        if shown >= 20 {
            break;
        }
    }
    if output.is_empty() {
        Ok("no connection history yet\n".to_string())
    } else {
        Ok(output)
    }
}

fn affirmative(value: &str) -> bool {
    matches!(value, "yes" | "--yes" | "-y" | "y")
}

fn deep_test(
    store: &ProfileStore,
    config_root: &Path,
    value: &str,
    acknowledged: bool,
) -> Result<String, String> {
    use crate::session::{DEEP_TEST_WARNING, DeepTest};
    let profile = load_profile(store, value)?;
    let credentials = SystemCredentialStore::new(config_root);
    let outcome =
        crate::session::deep_test_profile(&profile, &credentials, &state_dir(), acknowledged)
            .map_err(|error| error.to_string())?;
    Ok(match outcome {
        DeepTest::NeedsAcknowledgement => format!(
            "{DEEP_TEST_WARNING}\nRe-run to proceed: rdp-tui deep-test {} --yes\n",
            profile.id
        ),
        DeepTest::Authenticated => format!("deep-test: {} — credentials accepted\n", profile.name),
        DeepTest::AuthFailed => format!(
            "deep-test: {} — authentication failed (the host rejected the credentials)\n",
            profile.name
        ),
        DeepTest::Unreachable => {
            format!(
                "deep-test: {} — could not reach the host to authenticate\n",
                profile.name
            )
        }
        DeepTest::Indeterminate => format!(
            "deep-test: {} — result indeterminate; credentials were not confirmed\n",
            profile.name
        ),
        DeepTest::NotSupported => format!(
            "deep-test: {} — auth-only is not supported by this FreeRDP build\n",
            profile.name
        ),
        DeepTest::RateLimited => {
            format!(
                "deep-test: {} — skipped, deep-tested too recently\n",
                profile.name
            )
        }
    })
}

fn state_dir() -> PathBuf {
    crate::paths::state_root()
}

const fn usage() -> &'static str {
    "usage: rdp-tui [list | show <id> | inspect <id> | validate | test <id> | deep-test <id> [--yes] | connect <id> | add <name> <host> | set <id> <field> <value> | clone <id> | delete <id> | credential set|clear <id> [main|gateway] | certificate policy|show|trust|backups|restore <id> ... | import <path> | export <id> <path> | history [<id>] | config-paths | info | doctor | migrate python [profiles.json]]"
}

#[cfg(test)]
mod tests {
    use super::{ProfileStore, set_profile_credential};
    use crate::config::ConfigStore;
    use crate::credentials::CredentialStore as _;
    use crate::model::{
        CertificatePolicy, CredentialBackend, DeviceConfig, DisplayConfig, Endpoint,
        IdentityConfig, Profile, ProfileId, Route, SecurityConfig,
    };
    use crate::secret::file::EncryptedFileStore;
    use secrecy::ExposeSecret as _;
    use tempfile::TempDir;

    fn sample_profile() -> Profile {
        Profile {
            id: ProfileId::generate(),
            name: "Sample".into(),
            endpoint: "10.0.0.5:3389".parse::<Endpoint>().unwrap(),
            identity: IdentityConfig::default(),
            route: Route::Direct,
            display: DisplayConfig::default(),
            devices: DeviceConfig::default(),
            security: SecurityConfig::default(),
            credential: None,
        }
    }

    #[test]
    fn setting_a_credential_pins_an_encrypted_file_reference_that_retrieves() {
        let dir = TempDir::new().unwrap();
        let config_root = dir.path().to_path_buf();
        let store = ProfileStore::new(ConfigStore::new(&config_root));
        let profile = sample_profile();
        let id = profile.id;
        store.upsert(profile.clone()).unwrap();

        set_profile_credential(&store, &config_root, profile, "hunter2").unwrap();

        let saved = store.get(id).unwrap().unwrap();
        let reference = saved
            .credential
            .expect("credential is pinned on the profile");
        assert_eq!(reference.backend, CredentialBackend::EncryptedFile);
        let secret = EncryptedFileStore::new(config_root.as_path())
            .retrieve(reference)
            .unwrap();
        assert_eq!(secret.expose_secret(), "hunter2");
    }

    #[test]
    fn setting_a_certificate_policy_updates_the_profile() {
        let dir = TempDir::new().unwrap();
        let store = ProfileStore::new(ConfigStore::new(dir.path()));
        let mut profile = sample_profile();
        profile.security.certificate_policy = CertificatePolicy::System;
        let id = profile.id;
        store.upsert(profile).unwrap();

        super::certificate_policy(&store, &id.to_string(), "tofu").unwrap();
        let saved = store.get(id).unwrap().unwrap();
        assert_eq!(saved.security.certificate_policy, CertificatePolicy::Tofu);

        assert!(super::certificate_policy(&store, &id.to_string(), "bogus").is_err());
    }

    #[test]
    fn certificate_show_reports_no_pin_when_absent() {
        let dir = TempDir::new().unwrap();
        let config_root = dir.path().join("rdp-tui");
        let store = ProfileStore::new(ConfigStore::new(config_root.as_path()));
        let profile = sample_profile();
        let id = profile.id;
        store.upsert(profile).unwrap();

        let output =
            super::certificate_show(&store, config_root.as_path(), &id.to_string()).unwrap();
        assert!(output.contains("pinned: none"));
        assert!(output.contains("10.0.0.5:3389"));
    }

    #[test]
    fn add_creates_a_direct_profile_and_set_edits_its_fields() {
        let dir = TempDir::new().unwrap();
        let store = ProfileStore::new(ConfigStore::new(dir.path()));

        super::add_command(&store, "Workbench", "10.0.0.9").unwrap();
        let created = store.list().unwrap();
        assert_eq!(created.len(), 1);
        let profile = &created[0];
        assert_eq!(profile.name, "Workbench");
        assert_eq!(profile.endpoint.to_string(), "10.0.0.9:3389");
        assert!(matches!(profile.route, Route::Direct));

        let id = profile.id.to_string();
        super::set_command(&store, dir.path(), &id, "username", "operator").unwrap();
        super::set_command(&store, dir.path(), &id, "host", "10.0.0.9:9833").unwrap();
        super::set_command(&store, dir.path(), &id, "resolution", "1920x1080").unwrap();
        super::set_command(&store, dir.path(), &id, "route", "ssh:jump.example").unwrap();
        super::set_command(&store, dir.path(), &id, "multimon", "yes").unwrap();
        super::set_command(&store, dir.path(), &id, "scale", "140").unwrap();
        super::set_command(&store, dir.path(), &id, "audio", "off").unwrap();
        // The store rejects an out-of-range scale rather than corrupting the profile.
        assert!(super::set_command(&store, dir.path(), &id, "scale", "150").is_err());

        let saved = store.get(profile.id).unwrap().unwrap();
        assert_eq!(saved.identity.username, "operator");
        assert_eq!(saved.endpoint.to_string(), "10.0.0.9:9833");
        assert_eq!(saved.display.resolution, Some((1920, 1080)));
        assert!(matches!(saved.route, Route::SshTunnel { .. }));
        assert!(saved.display.multimon);
        assert_eq!(saved.display.scale_percent, Some(140));
        assert!(!saved.devices.audio_playback);

        assert!(super::set_command(&store, dir.path(), &id, "route", "bogus").is_err());
        assert!(super::set_command(&store, dir.path(), &id, "nonesuch", "x").is_err());
    }

    #[test]
    fn clone_duplicates_a_profile_without_its_credential_and_delete_removes_it() {
        let dir = TempDir::new().unwrap();
        let config_root = dir.path().to_path_buf();
        let store = ProfileStore::new(ConfigStore::new(&config_root));
        let mut profile = sample_profile();
        profile.identity.username = "operator".into();
        let original = profile.id;
        store.upsert(profile.clone()).unwrap();
        set_profile_credential(&store, &config_root, profile, "hunter2").unwrap();

        super::clone_command(&store, &original.to_string()).unwrap();
        let all = store.list().unwrap();
        assert_eq!(all.len(), 2);
        let copy = all.iter().find(|current| current.id != original).unwrap();
        assert_eq!(copy.name, "Sample (copy)");
        assert_eq!(copy.identity.username, "operator");
        assert!(copy.credential.is_none(), "a clone starts without a secret");

        super::delete_command(&store, &config_root, &original.to_string()).unwrap();
        assert!(store.get(original).unwrap().is_none());
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn certificate_trust_requires_a_full_fingerprint() {
        let dir = TempDir::new().unwrap();
        let config_root = dir.path().join("rdp-tui");
        let store = ProfileStore::new(ConfigStore::new(config_root.as_path()));
        let profile = sample_profile();
        let id = profile.id;
        store.upsert(profile).unwrap();

        assert!(
            super::certificate_trust(&store, config_root.as_path(), &id.to_string(), "ab:cd")
                .is_err()
        );
    }
}
