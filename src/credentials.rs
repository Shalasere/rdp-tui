//! Secret acquisition boundary between persisted credential references and launch-time material.

pub mod askpass;

use crate::model::{CredentialBackend, CredentialRef, ResolvedCredentials};
use crate::secret::file::EncryptedFileStore;
use crate::secret::service::SecretServiceStore;
use secrecy::SecretString;
use std::fmt;
use std::path::{Path, PathBuf};

/// A backend that resolves durable credential references into launch-time secrets.
pub trait CredentialStore {
    /// Retrieve one secret without logging, serializing, or placing it in argv.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the backend cannot provide the requested secret.
    fn retrieve(&self, reference: CredentialRef) -> Result<SecretString, CredentialError>;
}

/// A credential store that dispatches each reference to its pinned backend.
///
/// The backend is read from the concrete `CredentialRef`, never re-resolved
/// from the environment (INV-4).
pub struct SystemCredentialStore {
    secret_service: SecretServiceStore,
    encrypted_file: EncryptedFileStore,
}

impl SystemCredentialStore {
    /// Build a system store rooted at the rdp-tui configuration directory.
    #[must_use]
    pub fn new(config_root: impl Into<PathBuf>) -> Self {
        Self {
            secret_service: SecretServiceStore::default(),
            encrypted_file: EncryptedFileStore::new(config_root),
        }
    }
}

impl CredentialStore for SystemCredentialStore {
    fn retrieve(&self, reference: CredentialRef) -> Result<SecretString, CredentialError> {
        match reference.backend {
            CredentialBackend::SecretService => self.secret_service.retrieve(reference),
            CredentialBackend::EncryptedFile => self.encrypted_file.retrieve(reference),
        }
    }
}

/// Store a password in the encrypted-file backend and return its pinned,
/// concrete reference. Shared by the CLI and TUI so both save passwords the
/// same way (INV-8).
///
/// # Errors
///
/// Returns a backend error when the secret cannot be written.
pub fn store_encrypted_password(
    config_root: &Path,
    password: &SecretString,
) -> Result<CredentialRef, CredentialError> {
    EncryptedFileStore::new(config_root).store(password)
}

/// Best-effort removal of a previously stored encrypted-file secret.
pub fn forget_encrypted(config_root: &Path, reference: CredentialRef) {
    let _ = EncryptedFileStore::new(config_root).delete(reference);
}

/// Non-serializable secrets held only for the lifetime of a connection attempt.
pub struct CredentialLease {
    pub main: Option<SecretString>,
    pub gateway: Option<SecretString>,
}

impl fmt::Debug for CredentialLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialLease")
            .field("main", &self.main.as_ref().map(|_| "<redacted>"))
            .field("gateway", &self.gateway.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CredentialError {
    Missing,
    Unavailable(String),
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str("credential is missing"),
            Self::Unavailable(message) => {
                write!(formatter, "credential backend unavailable: {message}")
            }
        }
    }
}

impl std::error::Error for CredentialError {}

/// Resolve the references selected by planning into a short-lived secret lease.
///
/// # Errors
///
/// Returns the first backend failure and drops any already-acquired secret.
pub fn acquire(
    store: &impl CredentialStore,
    references: ResolvedCredentials,
) -> Result<CredentialLease, CredentialError> {
    let main = references
        .main
        .map(|reference| store.retrieve(reference))
        .transpose()?;
    let gateway = references
        .gateway
        .map(|reference| store.retrieve(reference))
        .transpose()?;
    Ok(CredentialLease { main, gateway })
}
