//! A record of one finished connect session, persisted to `history.toml`.
//!
//! History is data, not intent: it stores the stable `profile_id` and the
//! session outcome, never a secret (INV-3). The profile's current name is
//! resolved at display time, so a renamed or deleted profile stays readable.

use crate::model::{ConnectionFailure, ProfileId, Renderer, SessionResult};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// One finished connect session.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryEntry {
    pub profile_id: ProfileId,
    /// Finish time as Unix epoch seconds (UTC).
    pub finished_at: u64,
    /// How long the session lasted, in milliseconds.
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub failure: Option<ConnectionFailure>,
    pub renderer: Renderer,
}

impl HistoryEntry {
    /// Build an entry from a finished session's [`SessionResult`].
    #[must_use]
    pub fn from_result(profile_id: ProfileId, result: &SessionResult, finished_at: u64) -> Self {
        Self {
            profile_id,
            finished_at,
            duration_ms: u64::try_from(result.duration.as_millis()).unwrap_or(u64::MAX),
            exit_code: result.exit_code,
            failure: result.failure,
            renderer: result.renderer,
        }
    }

    /// True when the session ended cleanly (exit 0, no recorded failure).
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.failure.is_none() && self.exit_code == Some(0)
    }

    /// The session's duration.
    #[must_use]
    pub const fn duration(&self) -> Duration {
        Duration::from_millis(self.duration_ms)
    }

    /// A one-line human summary of this entry for the given profile name at the
    /// current time (epoch seconds).
    #[must_use]
    pub fn summarize(&self, name: &str, now: u64) -> String {
        let outcome = if self.succeeded() { "ok" } else { "failed" };
        let exit = self
            .exit_code
            .map_or_else(|| "?".to_owned(), |code| code.to_string());
        format!(
            "{name} - {outcome} (exit {exit}, ran {}) - {} ago",
            format_span(self.duration().as_secs()),
            format_span(now.saturating_sub(self.finished_at))
        )
    }
}

/// Render a duration in seconds as a compact string such as 45s or 3m.
fn format_span(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::HistoryEntry;
    use crate::model::{ProfileId, Renderer};

    #[test]
    fn summarize_reports_outcome_exit_and_age() {
        let entry = HistoryEntry {
            profile_id: ProfileId::generate(),
            finished_at: 1000,
            duration_ms: 65_000,
            exit_code: Some(0),
            failure: None,
            renderer: Renderer::X11,
        };
        let line = entry.summarize("Anima", 1120);
        assert!(line.contains("Anima"));
        assert!(line.contains("ok"));
        assert!(line.contains("2m ago"));
        assert!(line.contains("ran 1m"));
    }
}
