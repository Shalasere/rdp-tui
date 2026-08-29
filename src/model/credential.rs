use super::CredentialKey;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialBackend {
    SecretService,
    EncryptedFile,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(
    rename_all = "snake_case",
    tag = "mode",
    content = "backend",
    deny_unknown_fields
)]
pub enum CredentialPreference {
    #[default]
    Automatic,
    Explicit(CredentialBackend),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialRef {
    pub backend: CredentialBackend,
    pub key: CredentialKey,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ResolvedCredentials {
    pub main: Option<CredentialRef>,
    pub gateway: Option<CredentialRef>,
}
