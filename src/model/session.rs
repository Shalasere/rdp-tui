use super::{ConnectionFailure, Renderer};
use semver::Version;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    Resolving,
    AcquiringRoute,
    Preflighting,
    Launching,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "result")]
pub enum SessionState {
    Running,
    Ended(SessionResult),
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionResult {
    #[serde(with = "duration_millis")]
    pub duration: Duration,
    pub exit_code: Option<i32>,
    pub failure: Option<ConnectionFailure>,
    pub renderer: Renderer,
    pub freerdp_version: Version,
}

mod duration_millis {
    use super::{Deserialize, Deserializer, Duration, Serialize, Serializer};

    pub fn serialize<S>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let millis = u64::try_from(value.as_millis()).map_err(serde::ser::Error::custom)?;
        millis.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Duration::from_millis(u64::deserialize(deserializer)?))
    }
}
