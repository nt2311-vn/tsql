//! ERD-scoped mouse capture toggle.
//!
//! Enabling mouse capture globally would break terminal-native text
//! selection everywhere. So we toggle it on only when the user enters
//! the whole-schema canvas view and off again on exit.

use std::io;

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;

/// Turn on terminal mouse reporting. Safe to call multiple times.
pub fn enable() -> Result<()> {
    execute!(io::stdout(), EnableMouseCapture).map_err(Into::into)
}

/// Turn off terminal mouse reporting. Safe to call multiple times.
pub fn disable() -> Result<()> {
    execute!(io::stdout(), DisableMouseCapture).map_err(Into::into)
}
