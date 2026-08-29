//! `FreeRDP` auth-only deep-test: verify credentials without opening a session.
//!
//! Uses `FreeRDP`'s documented `/auth-only` mode (validated for `FreeRDP` 3.x
//! against a real host). The exit status is unreliable — a UPN username can
//! trigger a Kerberos abort — so the outcome is read from the logged connection
//! result, not the status code. Credentials arrive through the sealed askpass
//! bridge; none appear in argv (INV-2, INV-3).

use crate::credentials::askpass::AskpassLease;
use crate::model::{CertificatePolicy, Endpoint, IdentityConfig};
use std::path::Path;
use std::process::Command;

/// The result of a `FreeRDP` auth-only attempt.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthOutcome {
    Authenticated,
    LogonFailure,
    Unreachable,
}

/// Run `FreeRDP` auth-only against `target` and classify the logged result.
///
/// # Errors
///
/// Returns an I/O error when the client cannot be run or its output captured.
pub fn authenticate(
    executable: &Path,
    target: &Endpoint,
    identity: &IdentityConfig,
    certificate_policy: CertificatePolicy,
    askpass: &AskpassLease,
) -> std::io::Result<AuthOutcome> {
    let mut command = Command::new(executable);
    command.arg(format!("/v:{target}"));
    if !identity.username.is_empty() {
        command.arg(format!("/u:{}", identity.username));
    }
    command.arg(format!("/d:{}", identity.domain));
    command.arg("/auth-only");
    match certificate_policy {
        CertificatePolicy::Tofu => {
            command.arg("/cert:tofu");
        }
        CertificatePolicy::Ignore => {
            command.arg("/cert:ignore");
        }
        CertificatePolicy::Deny => {
            command.arg("/cert:deny");
        }
        CertificatePolicy::System => {}
    }
    command.envs(askpass.environment());
    let output = command.output()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(classify(&text))
}

fn classify(output: &str) -> AuthOutcome {
    if output.contains("LOGON_FAILURE") {
        AuthOutcome::LogonFailure
    } else if output.contains("ERRCONNECT") {
        AuthOutcome::Unreachable
    } else {
        AuthOutcome::Authenticated
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthOutcome, classify};

    #[test]
    fn classifies_a_logon_failure() {
        assert_eq!(
            classify("[ERROR] nla_recv_pdu ERRCONNECT_LOGON_FAILURE [0x00020014]"),
            AuthOutcome::LogonFailure
        );
    }

    #[test]
    fn classifies_an_unreachable_target() {
        assert_eq!(
            classify("ERRCONNECT_CONNECT_CANCELLED could not connect"),
            AuthOutcome::Unreachable
        );
    }

    #[test]
    fn classifies_a_successful_authentication() {
        assert_eq!(
            classify(
                "Authentication only. Don't connect to X.\nAuthentication only, exit status 0"
            ),
            AuthOutcome::Authenticated
        );
    }
}
