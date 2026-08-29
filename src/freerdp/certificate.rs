//! `FreeRDP` certificate pin discovery, fingerprinting, and archival.
//!
//! `FreeRDP` records accepted server certificates as PEM files under
//! `$XDG_CONFIG_HOME/freerdp/server/<host>_<port>.pem`. rdp-tui reads those pins
//! to show, trust, and archive them per the certificate contract
//! (`docs/architecture/04-amendments.yaml`). Fingerprints are the uppercase,
//! colon-free SHA-256 of the certificate DER so a pin and a presented value
//! compare directly.

use base64::Engine as _;
use regex::Regex;
use sha2::{Digest as _, Sha256};
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
const END: &str = "-----END CERTIFICATE-----";

/// Resolve the `FreeRDP` pin PEM path for `host:port` under a freerdp config dir.
#[must_use]
pub fn pin_path(freerdp_config: &Path, host: &str, port: u16) -> PathBuf {
    freerdp_config
        .join("server")
        .join(format!("{host}_{port}.pem"))
}

/// The uppercase, colon-free SHA-256 fingerprint of a PEM certificate's DER, or
/// `None` when the pin is absent or is not a certificate.
///
/// # Errors
///
/// Returns an I/O error when an existing pin cannot be read.
pub fn fingerprint(pin: &Path) -> io::Result<Option<String>> {
    let pem = match std::fs::read_to_string(pin) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(pem_to_der(&pem).map(|der| hex_upper(&Sha256::digest(&der))))
}

/// Parse the presented SHA-256 fingerprint from a `FreeRDP` changed-certificate
/// error, normalized to uppercase and colon-free, or `None` when the output is
/// not a certificate-change report.
#[must_use]
pub fn changed_fingerprint(output: &str) -> Option<String> {
    let lowered = output.to_lowercase();
    if !lowered.contains("new host identification")
        && !lowered.contains("certificate does not match")
    {
        return None;
    }
    let pattern =
        Regex::new(r"(?i)fingerprint[^\n]*?\bis\s+([0-9a-f]{2}(?::[0-9a-f]{2}){31})").ok()?;
    pattern
        .captures(output)
        .map(|captures| captures[1].replace(':', "").to_ascii_uppercase())
}

/// Archive a stale pin to the owner-only backup directory, verifying it still
/// matches `expected` first so a concurrently-changed pin is never moved.
/// Returns the backup path.
///
/// # Errors
///
/// Returns an I/O error when the pin changed under the confirmation, or when the
/// backup directory cannot be created or the pin moved.
pub fn archive(pin: &Path, backups: &Path, expected: Option<&str>) -> io::Result<PathBuf> {
    if let Some(expected) = expected
        && fingerprint(pin)?.as_deref() != Some(expected)
    {
        return Err(io::Error::other(
            "certificate pin changed while confirmation was open",
        ));
    }
    std::fs::create_dir_all(backups)?;
    std::fs::set_permissions(backups, std::fs::Permissions::from_mode(0o700))?;
    let name = pin
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("certificate.pem");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let destination = backups.join(format!("{name}.{stamp}.bak"));
    std::fs::rename(pin, &destination)?;
    std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600))?;
    Ok(destination)
}

fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    let start = pem.find(BEGIN)? + BEGIN.len();
    let end = pem[start..].find(END)? + start;
    let body: String = pem[start..end].split_whitespace().collect();
    base64::engine::general_purpose::STANDARD.decode(body).ok()
}

fn hex_upper(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02X}");
        text
    })
}

#[cfg(test)]
mod tests {
    use super::{archive, changed_fingerprint, fingerprint, pin_path};
    use base64::Engine as _;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    const CERT_B64: &str = "LS0tLS1CRUdJTiBDRVJUSUZJQ0FURS0tLS0tCk1JSUREekNDQWZlZ0F3SUJBZ0lVTFFMeVFnTDVpNllTY1BCeGhyV08vNVY3TlNRd0RRWUpLb1pJaHZjTkFRRUwKQlFBd0Z6RVZNQk1HQTFVRUF3d01jbVJ3TFhSMWFTMTBaWE4wTUI0WERUSTJNRGd5T1RJd01ESXhNbG9YRFRJMgpNRGd6TURJd01ESXhNbG93RnpFVk1CTUdBMVVFQXd3TWNtUndMWFIxYVMxMFpYTjBNSUlCSWpBTkJna3Foa2lHCjl3MEJBUUVGQUFPQ0FROEFNSUlCQ2dLQ0FRRUEzWGwvaXFvcmtJRXRLVUhWSGN4SU9pRlBLN1dQZ3VOd1l6TzkKV2tWTkFPU21qazBKZ0FodzBGZ1RZZmY2QlRZbER6aitueWNIWkZKQTlpQW10b2IvcDl4anozalJ1OTNBQllmZQpxOXlqckgrYWZNaVQxMWZacHNBQTluNmdPUGJTbTdWMmZvbzJwc0dZZHpzd0JVUE9hbUJqMEg5OVZKSk92MTZUCmNOMEREdlZwSTkrd0hORmFJdVFpSmgrYmxXS1YrWFV2SGY3M3RCaUwrYnZvamtOWS9wSytWd1VRWTU3UWNnT1MKeHdMTzI4c0RETk11aURiSlV4amhwWGxqVVVIdEpCc3BHMHFoOG55bVJYclREMmNzaHFydmhKUG9TdFp4dTE2bwpCZUZMU0hkZmY1ZU1mbmJLSXUxUmdVYmZGRUpPMmtTVWE5NDJGMnZOY2ptSEd4S1lUd0lEQVFBQm8xTXdVVEFkCkJnTlZIUTRFRmdRVXVhcjJ5WVFUS0x5VzlvUzhoOWk0TFdNbzVSb3dId1lEVlIwakJCZ3dGb0FVdWFyMnlZUVQKS0x5VzlvUzhoOWk0TFdNbzVSb3dEd1lEVlIwVEFRSC9CQVV3QXdFQi96QU5CZ2txaGtpRzl3MEJBUXNGQUFPQwpBUUVBZUlPNm9BWVlNMzlVOFhlMzk5QXlsNitOQkxrZTFZT3N2NjE5dWxPcllsRFJlR3pxZDR1Rm5wWHZEdXA5CmFGcXV6Tll6enRiMWNFWG5xdHFKTW9XeEVsTXk3YkJCTVFHRWNqWEdoRWhVLzJkVGNQa3RJbTA0eGtFUTVDT2EKZ1UrN2RQWmlsOEZYK01CbStTYWRSRFFxa2cvUE9ZRFZSb29LWGFiakpNdkpBVWxjejhBeG4vRWNEOUlGRk5qeQp1V1hLZnRpRlROWnBQbFlib3ZUR085ejB6akIxc3hDUlpSUXVtWWV6R2RRbkp3MGRVYy83dG5NQnI4SmxNWTl6CkVITFdtVnNSU0E3aXBlcnBCaEpiL2UwL3ByNUVmcStKWHdPempSRjRPYTdYcTREdjZwY1B1TFdyaVk1S1J0Q3AKRnRobzhTY2JVengweGFLeWdvYWVGSDEvM1E9PQotLS0tLUVORCBDRVJUSUZJQ0FURS0tLS0tCg==";
    const CERT_FINGERPRINT: &str =
        "097F45AD7EADDD47DFEE3F2B1AD1EAECA7C314B7F238E130A5C058C6AE892350";

    fn write_pin(dir: &Path) -> PathBuf {
        let pem = base64::engine::general_purpose::STANDARD
            .decode(CERT_B64)
            .unwrap();
        let path = dir.join("10.0.0.111_3389.pem");
        std::fs::write(&path, pem).unwrap();
        path
    }

    #[test]
    fn pin_path_uses_the_freerdp_server_layout() {
        let path = pin_path(Path::new("/cfg/freerdp"), "10.0.0.111", 3389);
        assert!(path.ends_with("server/10.0.0.111_3389.pem"));
    }

    #[test]
    fn fingerprints_a_pem_and_reports_a_missing_pin_as_none() {
        let dir = TempDir::new().unwrap();
        let pin = write_pin(dir.path());
        assert_eq!(
            fingerprint(&pin).unwrap().as_deref(),
            Some(CERT_FINGERPRINT)
        );
        assert_eq!(fingerprint(&dir.path().join("absent.pem")).unwrap(), None);
    }

    #[test]
    fn parses_a_changed_certificate_fingerprint_and_ignores_normal_output() {
        let presented = "09:7f:45:ad:7e:ad:dd:47:df:ee:3f:2b:1a:d1:ea:ec:a7:c3:14:b7:f2:38:e1:30:a5:c0:58:c6:ae:89:23:50";
        let output = format!(
            "The certificate does not match the previously stored one.\nThe fingerprint is {presented}\n"
        );
        assert_eq!(
            changed_fingerprint(&output).as_deref(),
            Some(CERT_FINGERPRINT)
        );
        assert_eq!(changed_fingerprint("connected, no change here"), None);
    }

    #[test]
    fn archives_a_verified_pin_owner_only_and_removes_the_original() {
        let dir = TempDir::new().unwrap();
        let pin = write_pin(dir.path());
        let backups = dir.path().join("certificate-backups");
        let expected = fingerprint(&pin).unwrap().unwrap();

        let destination = archive(&pin, &backups, Some(&expected)).unwrap();
        assert!(!pin.exists());
        assert!(destination.exists());
        let mode = std::fs::metadata(&destination)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn refuses_to_archive_a_pin_that_no_longer_matches() {
        let dir = TempDir::new().unwrap();
        let pin = write_pin(dir.path());
        let backups = dir.path().join("certificate-backups");
        assert!(archive(&pin, &backups, Some("00DEADBEEF")).is_err());
        assert!(pin.exists());
    }
}
