//! `FreeRDP` auth-only deep-test: verify credentials without opening a session.
//!
//! Uses `FreeRDP`'s documented `/auth-only` mode (validated for `FreeRDP` 3.x
//! against a real host). Auth-only ignores `FREERDP_ASKPASS` and `/from-stdin`,
//! so the password is delivered through an inherited memfd read via
//! `/args-from:fd` — never in argv or an ordinary environment value (INV-3). The
//! exit status is unreliable (a UPN username can trigger a Kerberos abort), so
//! the outcome is read from the logged connection result, not the status.

use crate::model::{Endpoint, IdentityConfig};
use rustix::fs::{MemfdFlags, memfd_create};
use secrecy::{ExposeSecret as _, SecretString};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{Seek as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::path::Path;
use std::process::Command;

/// The result of a `FreeRDP` auth-only attempt.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthOutcome {
    Authenticated,
    LogonFailure,
    Unreachable,
}

/// Run `FreeRDP` auth-only against `target` and classify the logged result. The
/// certificate is ignored — a deep-test probes credentials, not trust.
///
/// # Errors
///
/// Returns an I/O error when the argument descriptor cannot be created or the
/// client cannot be run.
pub fn authenticate(
    executable: &Path,
    target: &Endpoint,
    identity: &IdentityConfig,
    password: Option<&SecretString>,
) -> std::io::Result<AuthOutcome> {
    let mut arguments = format!("/v:{target}\n");
    if !identity.username.is_empty() {
        let _ = writeln!(arguments, "/u:{}", identity.username);
    }
    let _ = writeln!(arguments, "/d:{}", identity.domain);
    if let Some(password) = password {
        let _ = writeln!(arguments, "/p:{}", password.expose_secret());
    }
    arguments.push_str("/auth-only\n/cert:ignore\n");

    // /args-from must be the sole argument, so the password (in /p:) lives only
    // in this inherited memfd, never in the process argument list.
    let descriptor = memfd_create("rdp-tui-authargs", MemfdFlags::empty())?;
    let mut file = File::from(descriptor);
    file.write_all(arguments.as_bytes())?;
    file.rewind()?;
    let raw = file.as_raw_fd();

    let output = Command::new(executable)
        .arg(format!("/args-from:fd:{raw}"))
        .output()?;
    drop(file); // hold the memfd open across the spawn, then release it
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(classify(&text))
}

fn classify(output: &str) -> AuthOutcome {
    if output.contains("LOGON_FAILURE") {
        // The host reached the auth stage and rejected the credentials.
        AuthOutcome::LogonFailure
    } else if output.contains("CONNECT_CANCELLED") {
        // Auth-only completes NLA and then tears the RDP connect down; without a
        // logon failure, that cancellation means the credentials were accepted.
        AuthOutcome::Authenticated
    } else if output.contains("ERRCONNECT") {
        // Failed before the auth stage — DNS, TCP, or pre-connect.
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
            classify("ERRCONNECT_CONNECT_FAILED could not reach host"),
            AuthOutcome::Unreachable
        );
    }

    #[test]
    fn classifies_a_cancelled_connect_after_auth_as_authenticated() {
        // Auth-only tears the RDP connect down once NLA succeeds.
        assert_eq!(
            classify(
                "kerberos noise\nAuthentication only, exit status 1\nERRCONNECT_CONNECT_CANCELLED [0x0002000B]"
            ),
            AuthOutcome::Authenticated
        );
    }

    #[test]
    fn classifies_a_clean_authentication() {
        assert_eq!(
            classify("Authentication only. Don't connect to X."),
            AuthOutcome::Authenticated
        );
    }
}
