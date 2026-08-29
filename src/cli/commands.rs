//! Read-only and connection command implementations for the Rust frontend.

use crate::ProfileStore;
use crate::config::ConfigStore;
use crate::config::migrate::import_python_profiles;
use crate::freerdp::discover::discover;
use crate::model::{ConnectionPlan, Profile, ProfileId, Renderer};
use crate::planner::plan;
use crate::session::{connect_profile, test_profile};
use std::fmt::Write as _;
use std::path::PathBuf;
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
        [command, id] if command == "connect" => connect(&store, id),
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

fn migrate_python(store: &ProfileStore, source: &std::path::Path) -> Result<String, String> {
    let text = std::fs::read_to_string(source).map_err(|error| error.to_string())?;
    let document = import_python_profiles(&text).map_err(|error| error.to_string())?;
    let count = document.profiles.len();
    for profile in document.profiles {
        store.upsert(profile).map_err(|error| error.to_string())?;
    }
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
    if let Some(dir) = crate::session::record::sessions_dir() {
        let sessions = crate::session::scan_sessions(&dir);
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
    Ok(format!("valid: {} profile(s)\n", profiles.len()))
}

const fn usage() -> &'static str {
    "usage: rdp-tui [list | show <id> | inspect <id> | validate | test <id> | connect <id> | config-paths | info | doctor | migrate python [profiles.json]]"
}
