//! AES-GCM encrypted-file credential fallback for systems without Secret Service.

use crate::credentials::{CredentialError, CredentialStore};
use crate::model::{CredentialBackend, CredentialKey, CredentialRef};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use fs2::FileExt;
use secrecy::{ExposeSecret, SecretString};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

const NONCE_LENGTH: usize = 12;

/// Owner-only AES-GCM credential storage rooted in an application config directory.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct EncryptedFileStore {
    root: PathBuf,
}

impl EncryptedFileStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    fn key_path(&self) -> PathBuf {
        self.root.join(".credential-key")
    }
    fn data_path(&self) -> PathBuf {
        self.root.join("credentials.json")
    }
    /// Encrypt and persist a secret under a fresh opaque key.
    /// # Errors
    /// Returns an error for key generation, locking, encryption, or I/O failure.
    pub fn store(&self, secret: &SecretString) -> Result<CredentialRef, CredentialError> {
        let mut key = [0; 32];
        getrandom::fill(&mut key).map_err(unavailable)?;
        let reference = CredentialRef {
            backend: CredentialBackend::EncryptedFile,
            key: CredentialKey::from_bytes(key),
        };
        self.put(reference, secret)?;
        Ok(reference)
    }
    /// Persist a secret for a supplied key, used by explicit migration only.
    /// # Errors
    /// Returns an error for key generation, locking, encryption, or I/O failure.
    pub fn put(
        &self,
        reference: CredentialRef,
        secret: &SecretString,
    ) -> Result<(), CredentialError> {
        if reference.backend != CredentialBackend::EncryptedFile {
            return Err(CredentialError::Unavailable(
                "credential backend does not match encrypted file".into(),
            ));
        }
        let _lock = self.lock()?;
        let key = self.load_or_create_key()?;
        let mut data = self.load_data()?;
        data.insert(reference.key.to_string(), encrypt(&key, secret)?);
        self.save_data(&data)
    }
    /// Delete one encrypted credential.
    /// # Errors
    /// Returns an error for locking or I/O failure.
    pub fn delete(&self, reference: CredentialRef) -> Result<(), CredentialError> {
        let _lock = self.lock()?;
        let mut data = self.load_data()?;
        data.remove(&reference.key.to_string());
        self.save_data(&data)
    }
    fn lock(&self) -> Result<File, CredentialError> {
        fs::create_dir_all(&self.root).map_err(unavailable)?;
        restrict_directory(&self.root)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join(".credentials.lock"))
            .map_err(unavailable)?;
        restrict_file(&file)?;
        file.try_lock_exclusive()
            .map_err(|error| CredentialError::Unavailable(error.to_string()))?;
        Ok(file)
    }
    fn load_or_create_key(&self) -> Result<[u8; 32], CredentialError> {
        restrict_existing_file(&self.key_path())?;
        match fs::read(self.key_path()) {
            Ok(bytes) if bytes.len() == 32 => bytes
                .try_into()
                .map_err(|_| CredentialError::Unavailable("invalid encryption key".into())),
            Ok(_) => Err(CredentialError::Unavailable(
                "invalid encryption key".into(),
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut key = [0; 32];
                getrandom::fill(&mut key).map_err(unavailable)?;
                atomic_write(&self.key_path(), &key)?;
                Ok(key)
            }
            Err(error) => Err(unavailable(error)),
        }
    }
    fn load_data(&self) -> Result<BTreeMap<String, String>, CredentialError> {
        restrict_existing_file(&self.data_path())?;
        match fs::read_to_string(self.data_path()) {
            Ok(text) => serde_json::from_str(&text).map_err(|_| {
                CredentialError::Unavailable("encrypted credential store is corrupt".into())
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(unavailable(error)),
        }
    }
    fn save_data(&self, data: &BTreeMap<String, String>) -> Result<(), CredentialError> {
        let text = serde_json::to_string_pretty(data)
            .map_err(|error| CredentialError::Unavailable(error.to_string()))?;
        atomic_write(&self.data_path(), format!("{text}\n").as_bytes())
    }
}
impl CredentialStore for EncryptedFileStore {
    fn retrieve(&self, reference: CredentialRef) -> Result<SecretString, CredentialError> {
        if reference.backend != CredentialBackend::EncryptedFile {
            return Err(CredentialError::Unavailable(
                "credential backend does not match encrypted file".into(),
            ));
        }
        let _lock = self.lock()?;
        let key = self.load_or_create_key()?;
        let value = self
            .load_data()?
            .get(&reference.key.to_string())
            .cloned()
            .ok_or(CredentialError::Missing)?;
        decrypt(&key, &value)
    }
}
fn encrypt(key: &[u8; 32], secret: &SecretString) -> Result<String, CredentialError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| CredentialError::Unavailable("invalid encryption key".into()))?;
    let mut nonce = [0; NONCE_LENGTH];
    getrandom::fill(&mut nonce).map_err(unavailable)?;
    let payload = cipher
        .encrypt(Nonce::from_slice(&nonce), secret.expose_secret().as_bytes())
        .map_err(|_| CredentialError::Unavailable("credential encryption failed".into()))?;
    let mut encoded = nonce.to_vec();
    encoded.extend(payload);
    Ok(base64::engine::general_purpose::STANDARD.encode(encoded))
}
fn decrypt(key: &[u8; 32], value: &str) -> Result<SecretString, CredentialError> {
    let data = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| CredentialError::Unavailable("encrypted credential is corrupt".into()))?;
    let (nonce, payload) = data
        .split_at_checked(NONCE_LENGTH)
        .ok_or_else(|| CredentialError::Unavailable("encrypted credential is corrupt".into()))?;
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| CredentialError::Unavailable("invalid encryption key".into()))?;
    let plain = cipher
        .decrypt(Nonce::from_slice(nonce), payload)
        .map_err(|_| {
            CredentialError::Unavailable("encrypted credential cannot be decrypted".into())
        })?;
    String::from_utf8(plain)
        .map(SecretString::from)
        .map_err(|_| CredentialError::Unavailable("encrypted credential is not UTF-8".into()))
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), CredentialError> {
    let parent = path
        .parent()
        .ok_or_else(|| CredentialError::Unavailable("credential path has no parent".into()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(unavailable)?;
    restrict_file(temp.as_file())?;
    temp.write_all(bytes).map_err(unavailable)?;
    temp.as_file().sync_all().map_err(unavailable)?;
    temp.persist(path)
        .map_err(|error| unavailable(error.error))?;
    File::open(parent)
        .map_err(unavailable)?
        .sync_all()
        .map_err(unavailable)
}

fn restrict_directory(path: &Path) -> Result<(), CredentialError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(unavailable)
}

fn restrict_existing_file(path: &Path) -> Result<(), CredentialError> {
    match fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(unavailable(error)),
    }
}

fn restrict_file(file: &File) -> Result<(), CredentialError> {
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(unavailable)
}

#[allow(clippy::needless_pass_by_value)] // Accepts owned I/O and crypto errors at call sites.
fn unavailable(error: impl ToString) -> CredentialError {
    CredentialError::Unavailable(error.to_string())
}
