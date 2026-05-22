pub(crate) mod archive;
pub(crate) mod branching;
pub mod files;
pub mod helpers;
pub(crate) mod lifecycle;
pub mod port_allocation;
pub mod pr_sync;
// Unix PTY (`scripts.rs`) uses libc openpty / poll; Windows uses ConPTY via
// `portable-pty` in `scripts_windows.rs`. macOS/Linux `scripts.rs` is unchanged.
#[cfg(unix)]
pub mod scripts;
#[cfg(windows)]
#[path = "scripts_windows.rs"]
pub mod scripts;
pub mod sidebar_order;
pub mod state;
pub mod status;
pub mod workspaces;
