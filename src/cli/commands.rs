//! Read-only command implementations for the Rust frontend.

use crate::ProfileStore;
use crate::config::ConfigStore;
use crate::config::migrate::import_python_profiles;
use crate::freerdp::discover::discover;
use crate::model::{ProfileId, Renderer};
use crate::planner::plan;
use std::fmt::Write as _;
use std::path::PathBuf;

/// Run a read-only CLI command and return text suitable for stdout.
///
/// # Errors
///
/// Returns a user-facing error when arguments are invalid or configuration
/// cannot be loaded and validated.
pub fn run(arguments: &[String], config_root: &PathBuf) -> Result<String, String> {
    let store = ProfileStore::new(ConfigStore::new(config_root));
    match arguments {
        [] => list(&store),
        [command] if command == "list" => list(&store),
        [command, id] if command == "show" => show(&store, id),
        [command, id] if command == "inspect" => inspect(&store, id),
        [command] if command == "validate" => validate(&store),
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

fn inspect(store: &ProfileStore, value: &str) -> Result<String, String> {
    let id = value
        .parse::<ProfileId>()
        .map_err(|error| error.to_string())?;
    let profile = store
        .get(id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("profile {id} was not found"))?;
    let discovered = discover(profile.display.renderer)?;
    let connection = plan(&profile, &discovered.capabilities, discovered.client)
        .map_err(|error| format!("cannot inspect profile: {error:?}"))?;
    Ok(format!(
        "profile: {}\ntarget: {}\nroute: {:?}\nclient: {} {}\n",
        profile.name,
        connection.target,
        connection.route,
        connection.client.executable.display(),
        connection.client.version
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
    let id = value
        .parse::<ProfileId>()
        .map_err(|error| error.to_string())?;
    let profile = store
        .get(id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("profile {id} was not found"))?;
    toml::to_string_pretty(&profile).map_err(|error| error.to_string())
}

fn validate(store: &ProfileStore) -> Result<String, String> {
    let profiles = store.list().map_err(|error| error.to_string())?;
    Ok(format!("valid: {} profile(s)\n", profiles.len()))
}

const fn usage() -> &'static str {
    "usage: rdp-tui [list | show <profile-id> | inspect <profile-id> | validate | config-paths | info | doctor | migrate python [profiles.json]]"
}
