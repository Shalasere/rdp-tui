//! Locked, durable persistence for schema-versioned configuration documents.

use super::{
    ConfigDocument, HistoryDocument, ProfilesDocument, StoreError, parse_config_document,
    parse_history_document, parse_profiles_document,
};
use crate::model::HistoryEntry;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Filesystem location and persistence operations for rdp-tui configuration.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ConfigStore {
    root: PathBuf,
}

impl ConfigStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    #[must_use]
    pub fn profiles_path(&self) -> PathBuf {
        self.root.join("profiles.toml")
    }

    /// Load `config.toml`; a missing file represents default configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the existing document cannot be read or validated.
    pub fn load_config(&self) -> Result<ConfigDocument, StoreError> {
        load_document(
            &self.config_path(),
            ConfigDocument::default(),
            parse_config_document,
        )
    }

    /// Load `profiles.toml`; a missing file represents an empty profile list.
    ///
    /// # Errors
    ///
    /// Returns an error when the existing document cannot be read or validated.
    pub fn load_profiles(&self) -> Result<ProfilesDocument, StoreError> {
        load_document(
            &self.profiles_path(),
            ProfilesDocument::default(),
            parse_profiles_document,
        )
    }

    /// Validate and durably replace `config.toml` while holding its flock lock.
    ///
    /// # Errors
    ///
    /// Returns an error for lock contention, invalid input, or filesystem failure.
    pub fn save_config(&self, document: &ConfigDocument) -> Result<(), StoreError> {
        save_document(&self.config_path(), document, parse_config_document)
    }

    /// Validate and durably replace `profiles.toml` while holding its flock lock.
    ///
    /// # Errors
    ///
    /// Returns an error for lock contention, invalid input, or filesystem failure.
    pub fn save_profiles(&self, document: &ProfilesDocument) -> Result<(), StoreError> {
        save_document(&self.profiles_path(), document, parse_profiles_document)
    }

    /// Lock, load, mutate, validate, and save profiles as one transaction.
    ///
    /// # Errors
    ///
    /// Returns an error for lock contention, invalid persisted or updated data,
    /// callback failure, or filesystem failure.
    pub fn update_profiles(
        &self,
        update: impl FnOnce(&mut ProfilesDocument) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let path = self.profiles_path();
        let _lock = acquire_lock(&path)?;
        let mut document =
            load_document(&path, ProfilesDocument::default(), parse_profiles_document)?;
        update(&mut document)?;
        write_document(&path, &document, parse_profiles_document)
    }

    /// Location of the connection-history document.
    #[must_use]
    pub fn history_path(&self) -> PathBuf {
        self.root.join("history.toml")
    }

    /// Load the connection history; a missing file is an empty history.
    ///
    /// # Errors
    ///
    /// Returns an error when the document cannot be read or validated.
    pub fn load_history(&self) -> Result<HistoryDocument, StoreError> {
        load_document(
            &self.history_path(),
            HistoryDocument::default(),
            parse_history_document,
        )
    }

    /// Append one entry, keeping only the most recent cap, as a single locked
    /// read-modify-write transaction (INV-5).
    ///
    /// # Errors
    ///
    /// Returns an error for lock contention or filesystem failure.
    pub fn record_history(&self, entry: HistoryEntry, cap: usize) -> Result<(), StoreError> {
        let path = self.history_path();
        let _lock = acquire_lock(&path)?;
        let mut document =
            load_document(&path, HistoryDocument::default(), parse_history_document)?;
        document.entries.push(entry);
        let len = document.entries.len();
        if cap > 0 && len > cap {
            document.entries.drain(0..len - cap);
        }
        write_document(&path, &document, parse_history_document)
    }
}

fn load_document<T>(
    path: &Path,
    default: T,
    parse: fn(&str) -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(default),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => Err(StoreError::Corrupt),
        Err(error) => Err(StoreError::Io(error)),
    }
}

fn save_document<T: serde::Serialize>(
    path: &Path,
    document: &T,
    parse: fn(&str) -> Result<T, StoreError>,
) -> Result<(), StoreError> {
    let _lock = acquire_lock(path)?;
    write_document(path, document, parse)
}

fn write_document<T: serde::Serialize>(
    path: &Path,
    document: &T,
    parse: fn(&str) -> Result<T, StoreError>,
) -> Result<(), StoreError> {
    let text = toml::to_string_pretty(document).map_err(|error| StoreError::Schema {
        file: path.display().to_string(),
        path: "document".into(),
        expected: "serializable schema-versioned TOML".into(),
        found: error.to_string(),
    })?;
    parse(&text)?;
    atomic_write(path, &(text + "\n"))
}

fn acquire_lock(path: &Path) -> Result<File, StoreError> {
    const LOCK_ATTEMPTS: u32 = 50;
    const LOCK_BACKOFF: std::time::Duration = std::time::Duration::from_millis(10);
    fs::create_dir_all(path.parent().ok_or(StoreError::Corrupt)?)?;
    let lock_path = path.with_file_name(format!(
        ".{}.lock",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    // Retry briefly before giving up. A lock can be held only transiently -- most
    // notably an fd inherited into the fork->exec window of a spawned child (e.g.
    // the detached supervisor), whose O_CLOEXEC fd stays open until exec -- and a
    // short, bounded wait rides that out instead of failing a legitimate write. A
    // genuinely contended lock still errors after the budget (INV-5,
    // DEC-lock-primitive; the contract allows a writer to block or error).
    for attempt in 1..=LOCK_ATTEMPTS {
        match lock.try_lock_exclusive() {
            Ok(()) => return Ok(lock),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if attempt == LOCK_ATTEMPTS {
                    return Err(StoreError::Locked);
                }
                std::thread::sleep(LOCK_BACKOFF);
            }
            Err(error) => return Err(StoreError::Io(error)),
        }
    }
    Err(StoreError::Locked)
}

fn atomic_write(path: &Path, content: &str) -> Result<(), StoreError> {
    let parent = path.parent().ok_or(StoreError::Corrupt)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(content.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| StoreError::Io(error.error))?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ConfigStore;
    use crate::model::{HistoryEntry, ProfileId, Renderer};
    use tempfile::TempDir;

    fn entry(finished_at: u64) -> HistoryEntry {
        HistoryEntry {
            profile_id: ProfileId::generate(),
            finished_at,
            duration_ms: 1000,
            exit_code: Some(0),
            failure: None,
            renderer: Renderer::X11,
        }
    }

    #[test]
    fn history_round_trips_and_keeps_only_the_most_recent() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path());
        for finished_at in 0..5 {
            store.record_history(entry(finished_at), 3).unwrap();
        }
        let loaded = store.load_history().unwrap();
        assert_eq!(loaded.entries.len(), 3);
        assert_eq!(loaded.entries[0].finished_at, 2);
        assert_eq!(loaded.entries[2].finished_at, 4);
    }
}
