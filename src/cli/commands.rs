//! Read-only and connection command implementations for the Rust frontend.

use crate::ProfileStore;
use crate::config::ConfigStore;
use crate::config::migrate::import_python_profiles;
use crate::credentials::{forget_encrypted, store_encrypted_password};
use crate::freerdp::discover::discover;
use crate::model::{CertificatePolicy, ConnectionPlan, Profile, ProfileId, Renderer};
use crate::planner::plan;
use crate::session::{connect_profile, test_profile};
use secrecy::SecretString;
use std::fmt::Write as _;
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
        [command, id] if command == "connect" => connect(&store, id),
        [command, sub, id] if command == "credential" && sub == "set" => {
            credential_set(&store, config_root, id)
        }
        [command, sub, id] if command == "credential" && sub == "clear" => {
            credential_clear(&store, config_root, id)
        }
        [command, id, policy] if command == "certificate" => certificate_policy(&store, id, policy),
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

fn credential_set(store: &ProfileStore, config_root: &Path, value: &str) -> Result<String, String> {
    let profile = load_profile(store, value)?;
    let name = profile.name.clone();
    let mut password = String::new();
    std::io::stdin()
        .read_line(&mut password)
        .map_err(|error| error.to_string())?;
    let password = password.trim_end_matches(['\n', '\r']);
    if password.is_empty() {
        return Err("no password was provided on stdin".into());
    }
    set_profile_credential(store, config_root, profile, password)?;
    Ok(format!("credential: stored a password for {name}\n"))
}

fn credential_clear(
    store: &ProfileStore,
    config_root: &Path,
    value: &str,
) -> Result<String, String> {
    let mut profile = load_profile(store, value)?;
    let Some(reference) = profile.credential.take() else {
        return Ok(format!("credential: {} had none to clear\n", profile.name));
    };
    let name = profile.name.clone();
    store.upsert(profile).map_err(|error| error.to_string())?;
    forget_encrypted(config_root, reference);
    Ok(format!("credential: cleared the password for {name}\n"))
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
    mut profile: Profile,
    password: &str,
) -> Result<(), String> {
    let reference = store_encrypted_password(config_root, &SecretString::from(password.to_owned()))
        .map_err(|error| error.to_string())?;
    let previous = profile.credential.replace(reference);
    store.upsert(profile).map_err(|error| error.to_string())?;
    if let Some(previous) = previous {
        forget_encrypted(config_root, previous);
    }
    Ok(())
}

fn certificate_policy(store: &ProfileStore, value: &str, policy: &str) -> Result<String, String> {
    let policy = parse_certificate_policy(policy)?;
    let mut profile = load_profile(store, value)?;
    let name = profile.name.clone();
    profile.security.certificate_policy = policy;
    store.upsert(profile).map_err(|error| error.to_string())?;
    Ok(format!("certificate: {name} now uses {policy:?}\n"))
}

fn parse_certificate_policy(value: &str) -> Result<CertificatePolicy, String> {
    match value {
        "tofu" => Ok(CertificatePolicy::Tofu),
        "system" => Ok(CertificatePolicy::System),
        "ignore" => Ok(CertificatePolicy::Ignore),
        "deny" => Ok(CertificatePolicy::Deny),
        other => Err(format!(
            "unknown certificate policy '{other}' (use tofu | system | ignore | deny)"
        )),
    }
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
    "usage: rdp-tui [list | show <id> | inspect <id> | validate | test <id> | connect <id> | credential set|clear <id> | certificate <id> <tofu|system|ignore|deny> | config-paths | info | doctor | migrate python [profiles.json]]"
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
}
