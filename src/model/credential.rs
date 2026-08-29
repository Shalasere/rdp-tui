use super::CredentialKey;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialBackend {
    SecretService,
    EncryptedFile,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "mode", content = "backend")]
pub enum CredentialPreference {
    #[default]
    Automatic,
    Explicit(CredentialBackend),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub struct CredentialRef {
    pub backend: CredentialBackend,
    pub key: CredentialKey,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
pub struct ResolvedCredentials {
    pub main: Option<CredentialRef>,
    pub gateway: Option<CredentialRef>,
}
