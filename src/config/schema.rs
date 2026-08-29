use super::StoreError;
use crate::model::{CertificatePolicy, HistoryEntry, Profile, ProfileId, Renderer};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigDocument {
    pub version: u32,
    #[serde(default)]
    pub defaults: ConfigDefaults,
}

impl Default for ConfigDocument {
    fn default() -> Self {
        Self {
            version: CONFIG_SCHEMA_VERSION,
            defaults: ConfigDefaults::default(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ConfigDefaults {
    pub renderer: Option<Renderer>,
    pub certificate_policy: Option<CertificatePolicy>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilesDocument {
    pub version: u32,
    pub profiles: Vec<Profile>,
}

impl Default for ProfilesDocument {
    fn default() -> Self {
        Self {
            version: CONFIG_SCHEMA_VERSION,
            profiles: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryDocument {
    pub version: u32,
    #[serde(default)]
    pub entries: Vec<HistoryEntry>,
}

impl Default for HistoryDocument {
    fn default() -> Self {
        Self {
            version: CONFIG_SCHEMA_VERSION,
            entries: Vec::new(),
        }
    }
}

/// Parse and validate `history.toml` without filesystem access.
///
/// # Errors
///
/// Returns [`StoreError::Schema`] for malformed TOML, unknown fields, or an
/// unsupported schema version.
pub fn parse_history_document(text: &str) -> Result<HistoryDocument, StoreError> {
    let document = toml::from_str::<HistoryDocument>(text)
        .map_err(|error| toml_error("history.toml", &error))?;
    validate_version("history.toml", document.version)?;
    Ok(document)
}

/// Parse and semantically validate `config.toml` without filesystem access.
///
/// # Errors
///
/// Returns [`StoreError::Schema`] for malformed TOML, unknown fields, or an
/// unsupported schema version.
pub fn parse_config_document(text: &str) -> Result<ConfigDocument, StoreError> {
    let document = toml::from_str::<ConfigDocument>(text)
        .map_err(|error| toml_error("config.toml", &error))?;
    validate_version("config.toml", document.version)?;
    Ok(document)
}

/// Parse and semantically validate `profiles.toml` without filesystem access.
///
/// # Errors
///
/// Returns [`StoreError::Schema`] for malformed TOML, unknown/secret fields,
/// an unsupported schema version, duplicate IDs, or invalid profile semantics.
pub fn parse_profiles_document(text: &str) -> Result<ProfilesDocument, StoreError> {
    let document = toml::from_str::<ProfilesDocument>(text)
        .map_err(|error| toml_error("profiles.toml", &error))?;
    validate_version("profiles.toml", document.version)?;
    validate_profiles(&document)?;
    Ok(document)
}

fn validate_version(file: &str, version: u32) -> Result<(), StoreError> {
    if version == CONFIG_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(StoreError::Schema {
            file: file.into(),
            path: "version".into(),
            expected: CONFIG_SCHEMA_VERSION.to_string(),
            found: version.to_string(),
        })
    }
}

fn validate_profiles(document: &ProfilesDocument) -> Result<(), StoreError> {
    let mut ids = BTreeSet::<ProfileId>::new();
    for (index, profile) in document.profiles.iter().enumerate() {
        if !ids.insert(profile.id) {
            return Err(StoreError::Schema {
                file: "profiles.toml".into(),
                path: format!("profiles[{index}].id"),
                expected: "a unique profile ID".into(),
                found: profile.id.to_string(),
            });
        }
        if let Some(issue) = profile.validate().into_iter().next() {
            return Err(StoreError::Schema {
                file: "profiles.toml".into(),
                path: format!("profiles[{index}].{}", issue.path),
                expected: issue.message,
                found: "invalid value".into(),
            });
        }
    }
    Ok(())
}

fn toml_error(file: &str, error: &toml::de::Error) -> StoreError {
    StoreError::Schema {
        file: file.into(),
        path: error.span().map_or_else(
            || "document".into(),
            |span| format!("bytes {}..{}", span.start, span.end),
        ),
        expected: "valid schema-versioned TOML".into(),
        found: error.message().into(),
    }
}
