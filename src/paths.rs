//! Central XDG path resolution and private-directory creation.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

/// `$XDG_CONFIG_HOME/rdp-tui`, with the standard home-directory fallback.
#[must_use]
pub fn config_root() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("rdp-tui")
}

/// `$XDG_STATE_HOME/rdp-tui`, with the standard home-directory fallback.
#[must_use]
pub fn state_root() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"))
        .join("rdp-tui")
}

/// Per-user runtime session directory. The `/tmp` fallback includes the UID so
/// users never share a predictable application directory.
#[must_use]
pub fn runtime_sessions_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(
        || {
            std::env::temp_dir()
                .join(format!("rdp-tui-{}", rustix::process::getuid().as_raw()))
                .join("sessions")
        },
        |base| PathBuf::from(base).join("rdp-tui").join("sessions"),
    )
}

/// `FreeRDP`'s config root adjacent to rdp-tui's config directory.
#[must_use]
pub fn freerdp_config_root(config_root: &Path) -> PathBuf {
    config_root
        .parent()
        .map_or_else(|| PathBuf::from("freerdp"), |base| base.join("freerdp"))
}

/// Certificate backups stored in application state.
#[must_use]
pub fn certificate_backups_dir() -> PathBuf {
    state_root().join("certificate-backups")
}

/// Create `path` and ensure it is accessible only to its owner.
///
/// # Errors
///
/// Returns an I/O error when the directory cannot be created or restricted.
pub fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}
