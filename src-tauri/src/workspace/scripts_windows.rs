//! Windows PTY runner for workspace scripts and embedded terminals.
//!
//! Uses ConPTY (Win10 1809+) via the `portable-pty` crate. The public API
//! mirrors `scripts.rs` so callers stay cfg-free.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
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

type ProcessKey = (String, String, Option<String>);

const PROCESS_TERM_TIMEOUT: Duration = Duration::from_millis(200);
const PROCESS_KILL_TIMEOUT: Duration = Duration::from_millis(500);
const PTY_POLL_INTERVAL: Duration = Duration::from_millis(25);
const PTY_WRITE_RETRY: Duration = Duration::from_millis(5);
const PTY_WRITE_DEADLINE: Duration = Duration::from_millis(500);

#[derive(Clone)]
struct ProcessHandle {
    pid: u32,
    killed: Arc<AtomicBool>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
}

#[derive(Clone, Default)]
pub struct ScriptProcessManager {
    processes: Arc<Mutex<HashMap<ProcessKey, ProcessHandle>>>,
}

impl ScriptProcessManager {
    pub fn new() -> Self {
        Self::default()
    }

    fn register(
        &self,
        key: ProcessKey,
        pid: u32,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
        master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    ) -> Arc<AtomicBool> {
        let killed = Arc::new(AtomicBool::new(false));
        let handle = ProcessHandle {
            pid,
            killed: killed.clone(),
            writer,
            child,
            master,
        };
        let mut map = self.processes.lock().expect("process map poisoned");
        if let Some(old) = map.insert(key, handle) {
            old.killed.store(true, Ordering::Release);
            escalating_kill(old.pid, &old.child);
        }
        killed
    }

    fn unregister(&self, key: &ProcessKey, pid: u32) {
        let mut map = self.processes.lock().expect("process map poisoned");
        if let Some(h) = map.get(key) {
            if h.pid == pid {
                map.remove(key);
            }
        }
    }

    pub fn kill_others_in_repo(
        &self,
        repo_id: &str,
        script_type: &str,
        keep_workspace_id: Option<&str>,
    ) -> usize {
        let victims: Vec<ProcessHandle> = {
            let map = self.processes.lock().expect("process map poisoned");
            map.iter()
                .filter(|(k, _)| {
                    k.0 == repo_id && k.1 == script_type && k.2.as_deref() != keep_workspace_id
                })
                .map(|(_, h)| h.clone())
                .collect()
        };
        let count = victims.len();
        for h in victims {
            h.killed.store(true, Ordering::Release);
            escalating_kill(h.pid, &h.child);
        }
        count
    }

    pub fn kill_all(&self) -> usize {
        let victims: Vec<ProcessHandle> = {
            let map = self.processes.lock().expect("process map poisoned");
            map.values().cloned().collect()
        };
        let count = victims.len();
        for h in victims {
            h.killed.store(true, Ordering::Release);
            escalating_kill(h.pid, &h.child);
        }
        count
    }

    pub fn kill(&self, key: &ProcessKey) -> bool {
        let handle = {
            let map = self.processes.lock().expect("process map poisoned");
            map.get(key).cloned()
        };
        match handle {
            Some(h) => {
                h.killed.store(true, Ordering::Release);
                escalating_kill(h.pid, &h.child);
                true
            }
            None => false,
        }
    }

    pub fn write_stdin(&self, key: &ProcessKey, data: &[u8]) -> Result<bool> {
        let writer = {
            let map = self.processes.lock().expect("process map poisoned");
            map.get(key).map(|h| h.writer.clone())
        };
        let Some(writer) = writer else {
            return Ok(false);
        };

        let mut file = writer.lock().expect("stdin mutex poisoned");
        let deadline = Instant::now() + PTY_WRITE_DEADLINE;
        let mut remaining = data;
        while !remaining.is_empty() {
            match file.write(remaining) {
                Ok(0) => bail!("PTY master write returned 0"),
                Ok(n) => remaining = &remaining[n..],
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        bail!("PTY master write timed out");
                    }
                    std::thread::sleep(PTY_WRITE_RETRY);
                }
                Err(e) => return Err(e).context("PTY master write failed"),
            }
        }
        Ok(true)
    }

    pub fn resize(&self, key: &ProcessKey, cols: u16, rows: u16) -> Result<bool> {
        let master = {
            let map = self.processes.lock().expect("process map poisoned");
            map.get(key).map(|h| h.master.clone())
        };
        let Some(master) = master else {
            return Ok(false);
        };
        let master = master.lock().expect("master mutex poisoned");
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("PTY resize failed")?;
        Ok(true)
    }
}

fn escalating_kill(pid: u32, child: &Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>) {
    if let Ok(mut c) = child.lock() {
        let _ = c.kill();
    }
    let _ = std::process::Command::new("taskkill.exe")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .status();
    let deadline = Instant::now() + PROCESS_TERM_TIMEOUT + PROCESS_KILL_TIMEOUT;
    while Instant::now() < deadline {
        if is_pid_gone(pid) {
            return;
        }
        std::thread::sleep(PTY_POLL_INTERVAL);
    }
}

fn is_pid_gone(pid: u32) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("taskkill.exe")
        .args(["/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            stderr.contains("not found") || stderr.contains("找不到")
        }
        Err(_) => true,
    }
}

#[derive(Clone, Default)]
pub struct ScriptContext {
    pub root_path: String,
    pub workspace_path: Option<String>,
    pub workspace_name: Option<String>,
    pub default_branch: Option<String>,
    pub port_base: Option<u16>,
    pub port_count: Option<u16>,
}

fn shell_basename(shell_path: &str) -> &str {
    Path::new(shell_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(shell_path)
}

fn is_bash_like(shell_path: &str) -> bool {
    matches!(
        shell_basename(shell_path),
        "bash" | "bash.exe" | "sh" | "sh.exe" | "zsh" | "zsh.exe"
    )
}

fn is_fish(shell_path: &str) -> bool {
    matches!(shell_basename(shell_path), "fish" | "fish.exe")
}

fn resolve_interactive_shell() -> (String, Vec<String>) {
    if let Ok(shell) = std::env::var("SHELL") {
        let path = Path::new(&shell);
        if path.is_file() && (is_bash_like(&shell) || is_fish(&shell)) {
            return (shell, vec!["-i".to_string(), "-l".to_string()]);
        }
    }
    let comspec =
        std::env::var("ComSpec").unwrap_or_else(|_| String::from(r"C:\Windows\System32\cmd.exe"));
    (comspec, Vec::new())
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn fish_shell_escape(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('$', "\\$")
            .replace('"', "\\\"")
    )
}

fn wrapped_script_for_shell(shell_path: &str, script: &str) -> String {
    if is_fish(shell_path) {
        return format!(
            "eval {}; set -l __helmor_ec $status; printf '\\r\\n\\033[2m[Completed with exit code %d]\\033[0m\\r\\n' $__helmor_ec; exit $__helmor_ec\r\n",
            fish_shell_escape(script),
        );
    }
    if is_bash_like(shell_path) {
        return format!(
            "eval {}; __helmor_ec=$?; printf '\\r\\n\\033[2m[Completed with exit code %d]\\033[0m\\r\\n' $__helmor_ec; exit $__helmor_ec\r\n",
            shell_escape(script),
        );
    }
    format!(
        "{script}\r\necho.\r\necho [Completed with exit code %ERRORLEVEL%]\r\nexit /b %ERRORLEVEL%\r\n"
    )
}

fn apply_context(cmd: &mut CommandBuilder, context: &ScriptContext) {
    cmd.env("TERM", "xterm-256color");
    cmd.env("FORCE_COLOR", "1");
    cmd.env("CLICOLOR_FORCE", "1");
    cmd.env("HELMOR_ROOT_PATH", &context.root_path);
    if let Some(wp) = &context.workspace_path {
        cmd.env("HELMOR_WORKSPACE_PATH", wp);
    }
    if let Some(wn) = &context.workspace_name {
        cmd.env("HELMOR_WORKSPACE_NAME", wn);
    }
    if let Some(db) = &context.default_branch {
        cmd.env("HELMOR_DEFAULT_BRANCH", db);
    }
    if let (Some(base), Some(count)) = (context.port_base, context.port_count) {
        cmd.env("HELMOR_PORT", base.to_string());
        cmd.env("HELMOR_PORT_COUNT", count.to_string());
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_script(
    manager: &ScriptProcessManager,
    repo_id: &str,
    script_type: &str,
    workspace_id: Option<&str>,
    script: &str,
    working_dir: &str,
    context: &ScriptContext,
    channel: Channel<ScriptEvent>,
) -> Result<Option<i32>> {
    let (shell, args) = resolve_interactive_shell();
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_script_with_shell(
        manager,
        repo_id,
        script_type,
        workspace_id,
        Some(script),
        working_dir,
        context,
        channel,
        &shell,
        &arg_refs,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_terminal_session(
    manager: &ScriptProcessManager,
    repo_id: &str,
    script_type: &str,
    workspace_id: Option<&str>,
    working_dir: &str,
    context: &ScriptContext,
    channel: Channel<ScriptEvent>,
    boot_input: Option<&str>,
) -> Result<Option<i32>> {
    let (shell, args) = resolve_interactive_shell();
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_script_with_shell(
        manager,
        repo_id,
        script_type,
        workspace_id,
        None,
        working_dir,
        context,
        channel,
        &shell,
        &arg_refs,
        boot_input,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_script_with_shell(
    manager: &ScriptProcessManager,
    repo_id: &str,
    script_type: &str,
    workspace_id: Option<&str>,
    script: Option<&str>,
    working_dir: &str,
    context: &ScriptContext,
    channel: Channel<ScriptEvent>,
    shell_path: &str,
    shell_args: &[&str],
    boot_input: Option<&str>,
) -> Result<Option<i32>> {
    if let Some(s) = script {
        if s.trim().is_empty() {
            bail!("Script is empty");
        }
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("openpty failed")?;

    let mut cmd = CommandBuilder::new(shell_path);
    for arg in shell_args {
        cmd.arg(arg);
    }
    cmd.cwd(working_dir);
    apply_context(&mut cmd, context);

    let child = pair
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("Failed to spawn {shell_path}"))?;
    let pid = child.process_id().unwrap_or(0);

    let reader = pair
        .master
        .try_clone_reader()
        .context("PTY try_clone_reader failed")?;
    let writer = pair
        .master
        .take_writer()
        .context("PTY take_writer failed")?;
    let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
    let child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>> = Arc::new(Mutex::new(child));
    let master: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));

    let _ = channel.send(ScriptEvent::Started {
        pid,
        command: script
            .map(str::to_string)
            .unwrap_or_else(|| format!("{shell_path} {}", shell_args.join(" "))),
    });

    let key: ProcessKey = (
        repo_id.to_string(),
        script_type.to_string(),
        workspace_id.map(str::to_string),
    );
    let killed = manager.register(
        key.clone(),
        pid,
        writer.clone(),
        child.clone(),
        master.clone(),
    );

    let ch = channel.clone();
    let stop_reader = Arc::new(AtomicBool::new(false));
    let stop_reader_in_thread = stop_reader.clone();
    let reader_handle = std::thread::Builder::new()
        .name("script-pty".into())
        .spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            loop {
                if stop_reader_in_thread.load(Ordering::Relaxed) {
                    break;
                }
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = String::from_utf8_lossy(&buf[..n]).into_owned();
                        let _ = ch.send(ScriptEvent::Stdout { data });
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "PTY read ended");
                        break;
                    }
                }
            }
        })
        .ok();

    if let Some(script) = script {
        let wrapped = wrapped_script_for_shell(shell_path, script);
        let mut file = writer.lock().expect("stdin mutex poisoned");
        if let Err(e) = file.write_all(wrapped.as_bytes()) {
            tracing::warn!(error = %e, "initial PTY write failed");
        }
    } else if let Some(input) = boot_input {
        let mut file = writer.lock().expect("stdin mutex poisoned");
        if let Err(e) = file.write_all(input.as_bytes()) {
            tracing::warn!(error = %e, "boot_input PTY write failed");
        }
    }

    let exit_code = {
        let mut child_guard = child.lock().expect("child mutex poisoned");
        let status = child_guard.wait().ok();
        status.map(|s| s.exit_code() as i32)
    };

    manager.unregister(&key, pid);

    stop_reader.store(true, Ordering::Release);
    if let Some(h) = reader_handle {
        let _ = h.join();
    }

    let exit_code = if killed.load(Ordering::Acquire) {
        None
    } else {
        exit_code
    };

    let _ = channel.send(ScriptEvent::Exited { code: exit_code });
    Ok(exit_code)
}
