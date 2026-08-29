//! Sealed-memfd `FreeRDP` ASKPASS bridge.

use super::CredentialLease;
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
use secrecy::{ExposeSecret as _, SecretString};
use std::ffi::OsString;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::fd::{AsRawFd as _, OwnedFd};
use std::path::PathBuf;

const MAIN_FD: &str = "RDP_TUI_ASKPASS_MAIN_FD";
const GATEWAY_FD: &str = "RDP_TUI_ASKPASS_GATEWAY_FD";

/// Launch-time secret file descriptors and the helper `FreeRDP` invokes.
pub struct AskpassLease {
    main_fd: Option<OwnedFd>,
    gateway_fd: Option<OwnedFd>,
    helper: PathBuf,
}

impl std::fmt::Debug for AskpassLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AskpassLease")
            .field("main_fd", &self.main_fd.as_ref().map(|_| "<redacted>"))
            .field(
                "gateway_fd",
                &self.gateway_fd.as_ref().map(|_| "<redacted>"),
            )
            .field("helper", &self.helper)
            .finish()
    }
}

impl AskpassLease {
    /// Create sealed, inheritable memfds for the supplied secret lease.
    ///
    /// `helper` must be the current `rdp-tui` executable invoked in askpass
    /// helper mode. The memfds deliberately omit `CLOEXEC`, because `FreeRDP`
    /// must pass them through to that helper.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when creating, writing, or sealing a memfd fails.
    pub fn prepare(lease: &CredentialLease, helper: PathBuf) -> std::io::Result<Self> {
        Ok(Self {
            main_fd: lease.main.as_ref().map(create_secret_fd).transpose()?,
            gateway_fd: lease.gateway.as_ref().map(create_secret_fd).transpose()?,
            helper,
        })
    }

    /// Return the non-secret child environment required for `FreeRDP` ASKPASS.
    #[must_use]
    pub fn environment(&self) -> Vec<(OsString, OsString)> {
        if self.main_fd.is_none() && self.gateway_fd.is_none() {
            return Vec::new();
        }
        let mut environment = vec![("FREERDP_ASKPASS".into(), self.helper.clone().into())];
        if let Some(fd) = &self.main_fd {
            environment.push((MAIN_FD.into(), fd.as_raw_fd().to_string().into()));
        }
        if let Some(fd) = &self.gateway_fd {
            environment.push((GATEWAY_FD.into(), fd.as_raw_fd().to_string().into()));
        }
        environment
    }
}

/// Write the requested secret to stdout for a `FreeRDP` askpass invocation.
///
/// # Errors
///
/// Returns an I/O error if the selected inherited descriptor is invalid or
/// cannot be read, or if stdout cannot receive the secret.
pub fn run_helper(prompt: &str) -> std::io::Result<()> {
    let variable = if prompt.contains("GatewayPassword:") || prompt.contains("Gateway Password:") {
        GATEWAY_FD
    } else {
        MAIN_FD
    };
    let raw_fd = std::env::var(variable).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "askpass secret descriptor is unavailable",
        )
    })?;
    let secret = read_secret_fd(&raw_fd)?;
    std::io::stdout().write_all(&secret)
}

fn create_secret_fd(secret: &SecretString) -> std::io::Result<OwnedFd> {
    let fd = memfd_create("rdp-tui-askpass", MemfdFlags::ALLOW_SEALING)?;
    let mut file = File::from(fd);
    file.write_all(secret.expose_secret().as_bytes())?;
    file.sync_all()?;
    fcntl_add_seals(
        &file,
        SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE,
    )?;
    Ok(file.into())
}

fn read_secret_fd(value: &str) -> std::io::Result<Vec<u8>> {
    let fd = value.parse::<i32>().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid askpass descriptor",
        )
    })?;
    if fd < 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid askpass descriptor",
        ));
    }
    let mut file = File::open(format!("/proc/self/fd/{fd}"))?;
    let mut secret = Vec::new();
    file.read_to_end(&mut secret)?;
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::{AskpassLease, MAIN_FD, create_secret_fd, read_secret_fd};
    use crate::credentials::CredentialLease;
    use rustix::fs::{SealFlags, fcntl_get_seals};
    use secrecy::SecretString;
    use std::os::fd::AsRawFd as _;
    use std::path::PathBuf;

    #[test]
    fn secret_memfd_is_sealed_and_readable_only_by_descriptor() {
        let fd = create_secret_fd(&SecretString::from("not-in-environment")).unwrap();
        assert!(
            fcntl_get_seals(&fd)
                .unwrap()
                .contains(SealFlags::WRITE | SealFlags::SEAL)
        );
        assert_eq!(
            read_secret_fd(&fd.as_raw_fd().to_string()).unwrap(),
            b"not-in-environment"
        );
    }

    #[test]
    fn lease_environment_contains_only_helper_and_fd_metadata() {
        let lease = CredentialLease {
            main: Some(SecretString::from("top secret")),
            gateway: None,
        };
        let askpass = AskpassLease::prepare(&lease, PathBuf::from("/usr/bin/rdp-tui")).unwrap();
        let environment = askpass.environment();
        assert!(environment.iter().any(|(key, _)| key == "FREERDP_ASKPASS"));
        assert!(environment.iter().any(|(key, _)| key == MAIN_FD));
        assert!(
            environment.iter().all(|(key, value)| key != "top secret"
                && !value.to_string_lossy().contains("top secret"))
        );
    }
}
