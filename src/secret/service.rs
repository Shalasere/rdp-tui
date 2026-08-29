//! Secret Service backend implemented through the system `secret-tool` client.

use crate::credentials::{CredentialError, CredentialStore};
use crate::model::{CredentialBackend, CredentialKey, CredentialRef};
use secrecy::{ExposeSecret, SecretString};
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Secret Service backend using a fixed application namespace and opaque key.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SecretServiceStore {
    executable: PathBuf,
}

impl Default for SecretServiceStore {
    fn default() -> Self {
        Self::new("secret-tool")
    }
}

impl SecretServiceStore {
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    /// Store a secret and return the concrete reference suitable for persistence.
    ///
    /// # Errors
    ///
    /// Returns an error when `secret-tool` cannot write to Secret Service.
    pub fn store(&self, secret: &SecretString) -> Result<CredentialRef, CredentialError> {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes)
            .map_err(|error| CredentialError::Unavailable(error.to_string()))?;
        let key = CredentialKey::from_bytes(bytes);
        let output = Command::new(&self.executable)
            .args([
                "store",
                "--label=rdp-tui credential",
                "application",
                "rdp-tui",
                "credential",
                &key.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| command_error(&error))?;
        let mut child = output;
        child
            .stdin
            .take()
            .ok_or_else(|| CredentialError::Unavailable("secret-tool stdin unavailable".into()))?
            .write_all(secret.expose_secret().as_bytes())
            .map_err(|error| CredentialError::Unavailable(error.to_string()))?;
        let status = child
            .wait()
            .map_err(|error| CredentialError::Unavailable(error.to_string()))?;
        if !status.success() {
            return Err(CredentialError::Unavailable(
                "Secret Service rejected credential".into(),
            ));
        }
        Ok(CredentialRef {
            backend: CredentialBackend::SecretService,
            key,
        })
    }

    /// Delete a Secret Service entry. A missing entry is treated as deleted.
    ///
    /// # Errors
    ///
    /// Returns an error when `secret-tool` cannot contact Secret Service.
    pub fn delete(&self, reference: CredentialRef) -> Result<(), CredentialError> {
        let status = Command::new(&self.executable)
            .args([
                "clear",
                "application",
                "rdp-tui",
                "credential",
                &reference.key.to_string(),
            ])
            .status()
            .map_err(|error| command_error(&error))?;
        if status.success() {
            Ok(())
        } else {
            Err(CredentialError::Unavailable(
                "Secret Service rejected deletion".into(),
            ))
        }
    }
}

impl CredentialStore for SecretServiceStore {
    fn retrieve(&self, reference: CredentialRef) -> Result<SecretString, CredentialError> {
        if reference.backend != CredentialBackend::SecretService {
            return Err(CredentialError::Unavailable(
                "credential backend does not match Secret Service".into(),
            ));
        }
        let output = Command::new(&self.executable)
            .args([
                "lookup",
                "application",
                "rdp-tui",
                "credential",
                &reference.key.to_string(),
            ])
            .output()
            .map_err(|error| command_error(&error))?;
        if !output.status.success() {
            return Err(CredentialError::Missing);
        }
        let value = String::from_utf8(output.stdout).map_err(|_| {
            CredentialError::Unavailable("Secret Service returned non-UTF-8 data".into())
        })?;
        Ok(SecretString::from(value.trim_end_matches(['\r', '\n'])))
    }
}

fn command_error(error: &std::io::Error) -> CredentialError {
    CredentialError::Unavailable(error.to_string())
}
