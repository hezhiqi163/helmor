//! Hide console windows when spawning child processes from the GUI app.
//!
//! Without `CREATE_NO_WINDOW`, every `git` / `gh` / `claude` / `taskkill` /
//! `powershell` invocation briefly flashes a `cmd` or console window on Windows.

use std::process::Command;

/// `CreateProcess` flag: do not allocate a visible console for the child.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Apply [`CREATE_NO_WINDOW`] to a [`Command`] (no-op on non-Windows targets).
#[cfg(windows)]
pub fn hide_console_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

#[cfg(not(windows))]
pub fn hide_console_window(cmd: &mut Command) -> &mut Command {
    cmd
}
