//! Shared state-changing workflows used by both CLI and TUI frontends.

use crate::credentials::{forget, store_encrypted_password};
use crate::freerdp::certificate::{self, CertificateMismatch};
use crate::model::{CertificatePolicy, CredentialRef, Profile, Route};
use crate::profile_store::ProfileStore;
use secrecy::SecretString;
use std::path::Path;

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct ImportCounts {
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
    pub failed: usize,
}

/// Parse and merge an import in one profile-store transaction.
///
/// # Errors
///
/// Returns an error when parsing, validation, or the store transaction fails.
pub fn import_profiles(store: &ProfileStore, path: &Path) -> Result<ImportCounts, String> {
    let mut batch = crate::config::import::import_path_report(path)?;
    let failed = batch.failures.len();
    // Import files are portable profile descriptions, not authority to bind
    // opaque references to secrets already present on this machine.
    for profile in &mut batch.profiles {
        strip_credentials(profile);
    }
    let (merged, replaced) = store
        .merge_replacing(batch.profiles)
        .map_err(|error| error.to_string())?;
    let current = store.list().map_err(|error| error.to_string())?;
    let retained: Vec<CredentialRef> = current
        .iter()
        .flat_map(credential_references)
        .flatten()
        .collect();
    for reference in replaced.iter().flat_map(credential_references).flatten() {
        if !retained.contains(&reference) {
            forget(store.config_root(), reference);
        }
    }
    Ok(ImportCounts {
        added: merged.added,
        updated: merged.updated,
        skipped: merged.skipped,
        failed,
    })
}

/// Which independently stored password a frontend is editing.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CredentialSlot {
    Main,
    Gateway,
}

/// Save a profile and then remove credential objects that the previous version
/// owned but the saved version no longer references.
///
/// # Errors
///
/// Returns an error if the existing profile cannot be read or the update fails.
pub fn save_profile(
    store: &ProfileStore,
    config_root: &Path,
    profile: Profile,
) -> Result<(), String> {
    let retained = credential_references(&profile);
    let previous = store
        .upsert_replacing(profile)
        .map_err(|error| error.to_string())?;
    if let Some(previous) = previous {
        for reference in credential_references(&previous).into_iter().flatten() {
            if !retained.contains(&Some(reference)) {
                forget(config_root, reference);
            }
        }
    }
    Ok(())
}

/// Give a cloned profile independent credential ownership.
pub fn strip_credentials(profile: &mut Profile) {
    profile.credential = None;
    if let Route::RdGateway { credential, .. } = &mut profile.route {
        *credential = None;
    }
}

/// Replace one stored password without leaving a newly-created orphan when the
/// profile transaction fails.
///
/// # Errors
///
/// Returns an error for an invalid slot, credential failure, or profile update failure.
pub fn replace_password(
    store: &ProfileStore,
    config_root: &Path,
    mut profile: Profile,
    slot: CredentialSlot,
    password: &str,
) -> Result<Profile, String> {
    let reference = store_encrypted_password(config_root, &SecretString::from(password.to_owned()))
        .map_err(|error| error.to_string())?;
    match replace_reference(&mut profile, slot, Some(reference)) {
        Ok(_) => {}
        Err(error) => {
            forget(config_root, reference);
            return Err(error);
        }
    }
    if let Err(error) = save_profile(store, config_root, profile.clone()) {
        forget(config_root, reference);
        return Err(error);
    }
    Ok(profile)
}

/// Clear one password reference transactionally, then remove the now-unreferenced secret.
///
/// # Errors
///
/// Returns an error for an invalid slot or profile update failure.
pub fn clear_password(
    store: &ProfileStore,
    config_root: &Path,
    mut profile: Profile,
    slot: CredentialSlot,
) -> Result<(Profile, bool), String> {
    if replace_reference(&mut profile, slot, None)?.is_none() {
        return Ok((profile, false));
    }
    save_profile(store, config_root, profile.clone())?;
    Ok((profile, true))
}

/// Remove a profile first, then best-effort-delete every credential it owned.
///
/// # Errors
///
/// Returns an error if the profile-store transaction fails.
pub fn delete_profile(
    store: &ProfileStore,
    config_root: &Path,
    profile: &Profile,
) -> Result<bool, String> {
    let removed = store
        .remove(profile.id)
        .map_err(|error| error.to_string())?;
    if removed {
        if let Some(reference) = profile.credential {
            forget(config_root, reference);
        }
        if let Route::RdGateway {
            credential: Some(reference),
            ..
        } = profile.route
        {
            forget(config_root, reference);
        }
    }
    Ok(removed)
}

/// Confirm exactly the certificate fingerprint captured from a failed attempt.
/// The old pin is archived only if it still matches the recorded fingerprint.
///
/// # Errors
///
/// Returns an error if confirmation is stale or inexact, or persistence fails.
pub fn trust_certificate_mismatch(
    store: &ProfileStore,
    config_root: &Path,
    state_root: &Path,
    mut profile: Profile,
    supplied_fingerprint: &str,
) -> Result<CertificateMismatch, String> {
    let normalized = normalize_fingerprint(supplied_fingerprint)?;
    let mismatch = certificate::read_mismatch(state_root, profile.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            "no changed certificate is awaiting confirmation for this profile".to_string()
        })?;
    if mismatch.endpoint != profile.endpoint {
        return Err("the pending certificate belongs to an older profile endpoint".into());
    }
    if normalized != mismatch.presented_sha256 {
        return Err(
            "the supplied fingerprint does not match the certificate that was presented".into(),
        );
    }
    let expected_pin = mismatch.pinned_sha256.as_deref().ok_or_else(|| {
        "the previous pinned certificate is unavailable; reconnect before trusting".to_string()
    })?;

    let pin = certificate::pin_path(
        &crate::paths::freerdp_config_root(config_root),
        &mismatch.endpoint.host.to_string(),
        mismatch.endpoint.port,
    );
    let archived = certificate::archive(
        &pin,
        &state_root.join("certificate-backups"),
        Some(expected_pin),
    )
    .map_err(|error| error.to_string())?;

    profile.security.certificate_policy = CertificatePolicy::Tofu;
    if let Err(error) = store.upsert(profile) {
        if let Err(restore_error) = std::fs::rename(&archived, &pin) {
            return Err(format!(
                "could not save certificate policy ({error}) and could not restore the pin ({restore_error})"
            ));
        }
        return Err(error.to_string());
    }
    certificate::remove_mismatch(state_root, mismatch.profile_id)
        .map_err(|error| error.to_string())?;
    Ok(mismatch)
}

fn credential_references(profile: &Profile) -> [Option<CredentialRef>; 2] {
    let gateway = match &profile.route {
        Route::RdGateway { credential, .. } => *credential,
        _ => None,
    };
    [profile.credential, gateway]
}

fn replace_reference(
    profile: &mut Profile,
    slot: CredentialSlot,
    reference: Option<CredentialRef>,
) -> Result<Option<CredentialRef>, String> {
    match slot {
        CredentialSlot::Main => Ok(std::mem::replace(&mut profile.credential, reference)),
        CredentialSlot::Gateway => match &mut profile.route {
            Route::RdGateway { credential, .. } => Ok(std::mem::replace(credential, reference)),
            _ => Err("this profile does not use an RD Gateway".into()),
        },
    }
}

fn normalize_fingerprint(value: &str) -> Result<String, String> {
    let normalized = value.replace(':', "").to_ascii_uppercase();
    if normalized.len() == 64 && normalized.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(normalized)
    } else {
        Err("a full 64-hex-character SHA-256 fingerprint is required".into())
    }
}

#[cfg(test)]
mod tests {
    use super::{CredentialSlot, save_profile, strip_credentials, trust_certificate_mismatch};
    use crate::config::ConfigStore;
    use crate::credentials::CredentialStore as _;
    use crate::freerdp::certificate::{self, CertificateMismatch};
    use crate::model::{
        CertificatePolicy, DeviceConfig, DisplayConfig, IdentityConfig, Profile, ProfileId, Route,
        SecurityConfig,
    };
    use crate::profile_store::ProfileStore;
    use crate::secret::file::EncryptedFileStore;
    use secrecy::SecretString;
    use tempfile::TempDir;

    fn profile() -> Profile {
        Profile {
            id: ProfileId::generate(),
            name: "gateway profile".into(),
            endpoint: "server.example:3389".parse().unwrap(),
            identity: IdentityConfig::default(),
            route: Route::RdGateway {
                gateway: "gateway.example:443".parse().unwrap(),
                username: "gateway-user".into(),
                domain: "EDGE".into(),
                credential: None,
            },
            display: DisplayConfig::default(),
            devices: DeviceConfig::default(),
            security: SecurityConfig::default(),
            credential: None,
        }
    }

    #[test]
    fn changing_away_from_gateway_forgets_its_password() {
        let temporary = TempDir::new().unwrap();
        let store = ProfileStore::new(ConfigStore::new(temporary.path()));
        let saved = super::replace_password(
            &store,
            temporary.path(),
            profile(),
            CredentialSlot::Gateway,
            "secret",
        )
        .unwrap();
        let Route::RdGateway {
            credential: Some(reference),
            ..
        } = saved.route
        else {
            panic!("gateway credential was not saved");
        };

        let mut direct = saved;
        direct.route = Route::Direct;
        save_profile(&store, temporary.path(), direct).unwrap();
        assert!(
            EncryptedFileStore::new(temporary.path())
                .retrieve(reference)
                .is_err()
        );
    }

    #[test]
    fn clones_drop_both_independently_owned_credentials() {
        let mut profile = profile();
        let reference = crate::credentials::store_encrypted_password(
            TempDir::new().unwrap().path(),
            &SecretString::from("secret"),
        )
        .unwrap();
        profile.credential = Some(reference);
        if let Route::RdGateway { credential, .. } = &mut profile.route {
            *credential = Some(reference);
        }
        strip_credentials(&mut profile);
        assert!(profile.credential.is_none());
        assert!(matches!(
            profile.route,
            Route::RdGateway {
                credential: None,
                ..
            }
        ));
    }

    #[test]
    fn certificate_trust_requires_the_recorded_presented_fingerprint() {
        let temporary = TempDir::new().unwrap();
        let config_root = temporary.path().join("config/rdp-tui");
        let state_root = temporary.path().join("state/rdp-tui");
        let store = ProfileStore::new(ConfigStore::new(&config_root));
        let mut profile = profile();
        profile.security.certificate_policy = CertificatePolicy::Deny;
        store.upsert(profile.clone()).unwrap();

        let pin = certificate::pin_path(
            &crate::paths::freerdp_config_root(&config_root),
            &profile.endpoint.host.to_string(),
            profile.endpoint.port,
        );
        std::fs::create_dir_all(pin.parent().unwrap()).unwrap();
        std::fs::write(
            &pin,
            "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let pinned = certificate::fingerprint(&pin).unwrap().unwrap();
        let presented = "AB".repeat(32);
        certificate::write_mismatch(
            &state_root,
            &CertificateMismatch {
                profile_id: profile.id,
                endpoint: profile.endpoint.clone(),
                pinned_sha256: Some(pinned),
                presented_sha256: presented.clone(),
            },
        )
        .unwrap();

        assert!(
            trust_certificate_mismatch(
                &store,
                &config_root,
                &state_root,
                profile.clone(),
                &"CD".repeat(32),
            )
            .is_err()
        );
        assert!(pin.exists());
        trust_certificate_mismatch(
            &store,
            &config_root,
            &state_root,
            profile.clone(),
            &presented,
        )
        .unwrap();
        assert!(!pin.exists());
        assert!(
            certificate::read_mismatch(&state_root, profile.id)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .get(profile.id)
                .unwrap()
                .unwrap()
                .security
                .certificate_policy,
            CertificatePolicy::Tofu
        );
    }
}
