//! Secret acquisition boundary between persisted credential references and launch-time material.

pub mod askpass;

use crate::model::{CredentialRef, ResolvedCredentials};
use secrecy::SecretString;
use std::fmt;

/// A backend that resolves durable credential references into launch-time secrets.
pub trait CredentialStore {
    /// Retrieve one secret without logging, serializing, or placing it in argv.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the backend cannot provide the requested secret.
    fn retrieve(&self, reference: CredentialRef) -> Result<SecretString, CredentialError>;
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
