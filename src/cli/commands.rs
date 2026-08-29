//! Read-only command implementations for the Rust frontend.

use crate::ProfileStore;
use crate::config::ConfigStore;
use crate::model::ProfileId;
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
        [command] if command == "validate" => validate(&store),
        [command] if command == "config-paths" => Ok(format!(
            "config.toml: {}\nprofiles.toml: {}\n",
            config_root.join("config.toml").display(),
            config_root.join("profiles.toml").display()
        )),
        [command] if command == "info" => Ok("rdp-tui Rust frontend: inspection mode\n".into()),
        _ => Err(usage().into()),
    }
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
    "usage: rdp-tui [list | show <profile-id> | validate | config-paths | info]"
}
