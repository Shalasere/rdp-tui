//! Opt-in, owner-only diagnostic logging.
//!
//! Set `RDP_TUI_LOG` to a file path to enable logging. Callers must never send
//! credentials, command lines, or environment-derived secrets here.

use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const LOG_PATH_ENV: &str = "RDP_TUI_LOG";
static LOG_PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

fn configured_path() -> &'static Mutex<Option<PathBuf>> {
    LOG_PATH.get_or_init(|| Mutex::new(None))
}

fn lock_path() -> MutexGuard<'static, Option<PathBuf>> {
    configured_path()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Enable diagnostics for this process and any supervisor it launches.
///
/// The path must be absolute so that logs never depend on a changing working
/// directory. Its parent is created with the user's normal permissions.
///
/// # Errors
///
/// Returns an error for a relative or parentless path, or when its parent
/// directory cannot be created.
pub fn enable(path: PathBuf) -> std::io::Result<()> {
    if !path.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "diagnostic log path must be absolute",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "diagnostic log path has no parent directory",
        )
    })?;
    std::fs::create_dir_all(parent)?;
    *lock_path() = Some(path);
    Ok(())
}

/// Disable diagnostics for this process and any supervisor it launches.
pub fn disable() {
    *lock_path() = None;
}

/// Return the enabled diagnostic path, if any.
#[must_use]
pub fn path() -> Option<PathBuf> {
    lock_path().clone()
}

/// Load the explicit CLI opt-in environment variable into process state.
pub fn configure_from_env() {
    if let Some(path) = std::env::var_os(LOG_PATH_ENV) {
        let _ = enable(PathBuf::from(path));
    }
}

/// Pass the active opt-in setting to a detached supervisor.
pub fn configure_command(command: &mut Command) {
    if let Some(path) = path() {
        command.env(LOG_PATH_ENV, path);
    }
}

/// Append one diagnostic event when the user has explicitly configured a log.
///
/// Logging failures deliberately do not affect RDP connectivity. The log file
/// is set to owner read/write on each use, including when it already existed.
pub fn log(event: fmt::Arguments<'_>) {
    let Some(path) = path() else {
        return;
    };
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let _ = writeln!(file, "{timestamp} {event}");
}

#[cfg(test)]
mod tests {
    use super::{disable, enable, log};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn opt_in_logging_writes_an_owner_only_file() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("diagnostics.log");
        enable(path.clone()).unwrap();
        log(format_args!("preflight tcp failed: synthetic diagnostic"));

        let contents = std::fs::read_to_string(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        disable();

        assert!(contents.contains("synthetic diagnostic"));
        assert_eq!(mode, 0o600);
    }
}
