//! Phase 1 Windows stub for the Unix PTY runner in `scripts.rs`.
//!
//! The macOS / Linux implementation wraps `libc::openpty`, `TIOCSWINSZ`,
//! `setsid`, `pollfd`, `killpg`, etc. — none of which exist on Windows.
//! A proper Windows port will use the ConPTY API (Win10 1809+) via the
//! `portable-pty` crate; that's Phase 3 of the `windows-port` branch in
//! this fork. Until then, every entry point here either:
//!
//! - returns a `not-implemented` error so Run / Setup / Terminal panes
//!   surface a clear message in the UI instead of crashing the app, or
//! - is a benign no-op (the manager methods used by the graceful-quit
//!   path so quit doesn't blow up when no scripts are running).
//!
//! The public-API surface mirrors `scripts.rs` exactly so callers in
//! `commands/{script,terminal,forge,system}_commands.rs` and `lib.rs`
//! don't need any cfg-switching.

use anyhow::{bail, Result};
use serde::Serialize;
use tauri::ipc::Channel;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ScriptEvent {
    Started { pid: u32, command: String },
    Stdout { data: String },
    Stderr { data: String },
    Exited { code: Option<i32> },
    Error { message: String },
}

/// Same triple key shape as the Unix module so callers stay portable.
#[allow(dead_code)]
type ProcessKey = (String, String, Option<String>);

#[derive(Clone, Default)]
pub struct ScriptContext {
    pub root_path: String,
    pub workspace_path: Option<String>,
    pub workspace_name: Option<String>,
    pub default_branch: Option<String>,
    /// First port in the workspace's deterministic port block.
    pub port_base: Option<u16>,
    /// Size of the port block starting at `port_base`.
    pub port_count: Option<u16>,
}

#[derive(Clone, Default)]
pub struct ScriptProcessManager;

impl ScriptProcessManager {
    pub fn new() -> Self {
        Self
    }

    /// Phase 1 stub: no scripts can be running on Windows yet, so nothing
    /// to kill. Returning 0 keeps the non-concurrent run mode caller's
    /// arithmetic correct.
    pub fn kill_others_in_repo(
        &self,
        _repo_id: &str,
        _script_type: &str,
        _keep_workspace_id: Option<&str>,
    ) -> usize {
        0
    }

    /// Phase 1 stub: nothing registered, nothing to kill. Called by the
    /// graceful-quit path so it MUST NOT bail.
    pub fn kill_all(&self) -> usize {
        0
    }

    /// Phase 1 stub: no live handle to signal.
    pub fn kill(&self, _key: &ProcessKey) -> bool {
        false
    }

    /// Phase 1 stub: the user typed into a pane that is not yet wired up
    /// on Windows. Returning `Ok(false)` mirrors the Unix "unknown key"
    /// branch and is treated as a silent no-op by callers.
    pub fn write_stdin(&self, _key: &ProcessKey, _data: &[u8]) -> Result<bool> {
        Ok(false)
    }

    /// Phase 1 stub: no live PTY to resize. Same `Ok(false)` semantics as
    /// `write_stdin`.
    pub fn resize(&self, _key: &ProcessKey, _cols: u16, _rows: u16) -> Result<bool> {
        Ok(false)
    }
}

const NOT_IMPLEMENTED: &str = "PTY-based scripts are not yet implemented on \
    Windows. Tracked as Phase 3 of the windows-port branch (ConPTY via \
    portable-pty). Run / Setup / Terminal panes will start working once \
    that lands.";

#[allow(clippy::too_many_arguments)]
pub fn run_script(
    _manager: &ScriptProcessManager,
    _repo_id: &str,
    _script_type: &str,
    _workspace_id: Option<&str>,
    _script: &str,
    _working_dir: &str,
    _context: &ScriptContext,
    _channel: Channel<ScriptEvent>,
) -> Result<Option<i32>> {
    bail!("{NOT_IMPLEMENTED}")
}

#[allow(clippy::too_many_arguments)]
pub fn run_terminal_session(
    _manager: &ScriptProcessManager,
    _repo_id: &str,
    _script_type: &str,
    _workspace_id: Option<&str>,
    _working_dir: &str,
    _context: &ScriptContext,
    _channel: Channel<ScriptEvent>,
    _boot_input: Option<&str>,
) -> Result<Option<i32>> {
    bail!("{NOT_IMPLEMENTED}")
}
