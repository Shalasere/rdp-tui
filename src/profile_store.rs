//! Profile CRUD backed by the locked configuration store.

use crate::config::{ConfigStore, StoreError};
use crate::model::{Profile, ProfileId};

/// CRUD operations for profiles, always using a locked read-modify-write transaction.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProfileStore {
    config: ConfigStore,
}

impl ProfileStore {
    #[must_use]
    pub fn new(config: ConfigStore) -> Self {
        Self { config }
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
        self.config.update_profiles(|document| {
            if let Some(existing) = document
                .profiles
                .iter_mut()
                .find(|saved| saved.id == profile.id)
            {
                *existing = profile;
            } else {
                document.profiles.push(profile);
            }
            Ok(())
        })
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
}
