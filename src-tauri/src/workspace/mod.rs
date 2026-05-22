pub(crate) mod archive;
pub(crate) mod branching;
pub mod files;
pub mod helpers;
pub(crate) mod lifecycle;
pub mod port_allocation;
pub mod pr_sync;
// The Unix PTY runner (scripts.rs) depends on libc::openpty, TIOCSWINSZ,
// setsid, pollfd, etc. — none of which exist on Windows. The Windows port
// Windows uses ConPTY via `portable-pty` in `scripts_windows.rs`; Unix
// `scripts.rs` is unchanged. Phase 3 note (historical):
// Phase 3 of the windows-port branch in this fork. Until then,
// `scripts_windows.rs` ships a stub that satisfies the public API surface
// so the rest of the desktop builds and the UI loads.
#[cfg(unix)]
pub mod scripts;
#[cfg(windows)]
#[path = "scripts_windows.rs"]
pub mod scripts;
pub mod sidebar_order;
pub mod state;
pub mod status;
pub mod workspaces;
