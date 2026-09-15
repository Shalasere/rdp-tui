//! Profile CRUD backed by the locked configuration store.

use crate::config::{ConfigStore, StoreError};
use crate::model::{Profile, ProfileId};

/// CRUD operations for profiles, always using a locked read-modify-write transaction.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProfileStore {
    config: ConfigStore,
}

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct MergeCounts {
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
}

impl ProfileStore {
    #[must_use]
    pub fn new(config: ConfigStore) -> Self {
        Self { config }
    }

    /// Configuration root backing this store.
    #[must_use]
    pub fn config_root(&self) -> &std::path::Path {
        self.config.root()
    }

    /// Return all persisted profiles in their saved order.
    ///
    /// # Errors
    ///
    /// Returns an error when the profile document cannot be read or validated.
    pub fn list(&self) -> Result<Vec<Profile>, StoreError> {
        Ok(self.config.load_profiles()?.profiles)
    }

    /// Find a profile by its stable identifier.
    ///
    /// # Errors
    ///
    /// Returns an error when the profile document cannot be read or validated.
    pub fn get(&self, id: ProfileId) -> Result<Option<Profile>, StoreError> {
        Ok(self.list()?.into_iter().find(|profile| profile.id == id))
    }

    /// Insert or replace one profile by ID.
    ///
    /// # Errors
    ///
    /// Returns an error for lock contention, invalid profiles, or filesystem failure.
    pub fn upsert(&self, profile: Profile) -> Result<(), StoreError> {
        self.upsert_replacing(profile).map(|_| ())
    }

    /// Insert or replace one profile and return the value replaced inside the
    /// same locked transaction.
    ///
    /// # Errors
    ///
    /// Returns an error for lock contention, invalid profiles, or filesystem failure.
    pub fn upsert_replacing(&self, profile: Profile) -> Result<Option<Profile>, StoreError> {
        let mut previous = None;
        self.config.update_profiles(|document| {
            if let Some(existing) = document
                .profiles
                .iter_mut()
                .find(|saved| saved.id == profile.id)
            {
                previous = Some(std::mem::replace(existing, profile));
            } else {
                document.profiles.push(profile);
            }
            Ok(())
        })?;
        Ok(previous)
    }

    /// Remove a profile and return whether it existed.
    ///
    /// # Errors
    ///
    /// Returns an error for lock contention, invalid profiles, or filesystem failure.
    pub fn remove(&self, id: ProfileId) -> Result<bool, StoreError> {
        let mut removed = false;
        self.config.update_profiles(|document| {
            let before = document.profiles.len();
            document.profiles.retain(|profile| profile.id != id);
            removed = document.profiles.len() != before;
            Ok(())
        })?;
        Ok(removed)
    }

    /// Merge a complete import batch in one locked read-modify-write transaction.
    ///
    /// # Errors
    ///
    /// Returns an error for lock contention, validation, or durable-write failure.
    pub fn merge(&self, profiles: Vec<Profile>) -> Result<MergeCounts, StoreError> {
        self.merge_replacing(profiles).map(|(counts, _)| counts)
    }

    /// Merge a batch and return profiles replaced inside that same transaction.
    ///
    /// # Errors
    ///
    /// Returns an error for lock contention, validation, or durable-write failure.
    pub fn merge_replacing(
        &self,
        profiles: Vec<Profile>,
    ) -> Result<(MergeCounts, Vec<Profile>), StoreError> {
        let mut counts = MergeCounts::default();
        let mut replaced = Vec::new();
        self.config.update_profiles(|document| {
            for profile in profiles {
                if let Some(index) = document
                    .profiles
                    .iter()
                    .position(|current| current.id == profile.id)
                {
                    if document.profiles[index] == profile {
                        counts.skipped += 1;
                    } else {
                        replaced.push(std::mem::replace(&mut document.profiles[index], profile));
                        counts.updated += 1;
                    }
                    continue;
                }
                if document
                    .profiles
                    .iter()
                    .any(|current| same_except_id(current, &profile))
                {
                    counts.skipped += 1;
                } else {
                    document.profiles.push(profile);
                    counts.added += 1;
                }
            }
            Ok(())
        })?;
        Ok((counts, replaced))
    }
}

fn same_except_id(current: &Profile, incoming: &Profile) -> bool {
    let mut incoming = incoming.clone();
    incoming.id = current.id;
    current == &incoming
}
