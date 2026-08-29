//! RAII terminal guard: the single owner of raw mode and the alternate screen.
//!
//! Restoration happens exactly once — via Drop or an explicit [`TerminalGuard::restore`] —
//! so the `catch_unwind` boundary in [`super::app::run`] can put the terminal
//! back after a panic without a global panic hook doing raw terminal I/O
//! (DEC-terminal-safety-mechanism, INV-12, AP-12).

use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use std::io;

/// Owns raw mode and the alternate screen for the lifetime of the TUI.
pub struct TerminalGuard {
    restored: bool,
}

impl TerminalGuard {
    /// Enter raw mode and the alternate screen.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the terminal cannot be reconfigured.
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(Self { restored: false })
    }

    /// Restore the terminal to its normal state. Safe to call more than once.
    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}
