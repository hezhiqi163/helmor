//! Windows shell integration (Explorer, `cmd start`, Windows Terminal).
//!
//! macOS continues to use `open` / AppleScript in `commands/*`; this module is
//! compiled only on Windows so Unix paths stay byte-identical.

use anyhow::Context;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Expand `%VAR%` tokens and `$HOME` using the current process environment.
pub fn expand_path_template(raw: &str) -> String {
    let mut out = raw.to_string();
    let pairs: &[(&str, &str)] = &[
        ("%USERPROFILE%", "USERPROFILE"),
        ("%LOCALAPPDATA%", "LOCALAPPDATA"),
        ("%APPDATA%", "APPDATA"),
        ("%ProgramFiles%", "ProgramFiles"),
        ("%ProgramFiles(x86)%", "ProgramFiles(x86)"),
    ];
    for (token, var) in pairs {
        if let Ok(v) = std::env::var(var) {
            out = out.replace(token, &v);
        }
    }
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = home.to_string_lossy();
        out = out.replace("$HOME", &home);
    }
    out
}

pub fn explorer_open_dir(path: &Path) -> anyhow::Result<()> {
    Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .context("explorer.exe failed to open directory")
}

pub fn explorer_select(path: &Path) -> anyhow::Result<()> {
    let arg = format!("/select,{}", path.display());
    Command::new("explorer.exe")
        .arg(arg)
        .spawn()
        .map(|_| ())
        .context("explorer.exe failed to reveal file")
}

/// Open a new console window running `command` (agent OAuth / CLI login).
pub fn open_login_console(command: &str) -> anyhow::Result<()> {
    if let Ok(wt) = resolve_windows_terminal() {
        return Command::new(&wt)
            .args(["-w", "0", "nt", "cmd.exe", "/k", command])
            .spawn()
            .map(|_| ())
            .context("Windows Terminal failed to start login shell");
    }

    Command::new("cmd.exe")
        .args([
            "/c",
            "start",
            "Helmor Agent Login",
            "cmd.exe",
            "/k",
            command,
        ])
        .spawn()
        .map(|_| ())
        .context("cmd.exe failed to start login shell")
}

pub fn resolve_windows_terminal() -> anyhow::Result<PathBuf> {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let candidate = PathBuf::from(local).join("Microsoft\\WindowsApps\\wt.exe");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    let mut where_cmd = Command::new("where.exe");
    where_cmd.arg("wt");
    crate::windows_subprocess::hide_console_window(&mut where_cmd);
    let output = where_cmd.output().context("where.exe wt failed")?;
    if !output.status.success() {
        anyhow::bail!("Windows Terminal (wt.exe) not found");
    }
    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if line.is_empty() {
        anyhow::bail!("Windows Terminal (wt.exe) not found");
    }
    Ok(PathBuf::from(line))
}

/// JetBrains Toolbox layout: `%LOCALAPPDATA%\JetBrains\Toolbox\apps\<AppId>\ch-0\<build>\bin\<exe>`.
pub fn resolve_jetbrains_toolbox(editor_id: &str) -> Option<String> {
    let (folder_prefixes, bin_name) = match editor_id {
        "intellij" => (&["IDEA"][..], "idea64.exe"),
        "pycharm" => (&["PyCharm", "PC"][..], "pycharm64.exe"),
        "webstorm" => (&["WebStorm"][..], "webstorm64.exe"),
        "goland" => (&["GoLand"][..], "goland64.exe"),
        "rubymine" => (&["RubyMine"][..], "rubymine64.exe"),
        "phpstorm" => (&["PhpStorm"][..], "phpstorm64.exe"),
        "clion" => (&["CLion"][..], "clion64.exe"),
        "rider" => (&["Rider"][..], "rider64.exe"),
        _ => return None,
    };

    let local = std::env::var_os("LOCALAPPDATA")?;
    let apps_root = PathBuf::from(local)
        .join("JetBrains")
        .join("Toolbox")
        .join("apps");
    if !apps_root.is_dir() {
        return None;
    }

    let mut best: Option<PathBuf> = None;
    let entries = std::fs::read_dir(&apps_root).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !folder_prefixes
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        if let Some(candidate) = newest_toolbox_build_bin(&entry.path(), bin_name) {
            match &best {
                Some(current) if current >= &candidate => {}
                _ => best = Some(candidate),
            }
        }
    }

    best.map(|p| p.display().to_string())
}

fn newest_toolbox_build_bin(app_dir: &Path, bin_name: &str) -> Option<PathBuf> {
    let ch0 = app_dir.join("ch-0");
    if !ch0.is_dir() {
        return None;
    }

    let mut builds: Vec<PathBuf> = std::fs::read_dir(&ch0)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    builds.sort();

    for build_dir in builds.into_iter().rev() {
        let exe = build_dir.join("bin").join(bin_name);
        if exe.is_file() {
            return Some(exe);
        }
    }
    None
}

/// Append `dir` to the current user's PATH when it is not already present.
pub fn ensure_user_path_contains(dir: &Path) -> anyhow::Result<()> {
    let dir_str = dir.to_string_lossy().replace('\'', "''");
    let script = format!(
        r#"$dir = '{dir_str}'; $path = [Environment]::GetEnvironmentVariable('Path', 'User'); if ([string]::IsNullOrWhiteSpace($path)) {{ [Environment]::SetEnvironmentVariable('Path', $dir, 'User'); exit 0 }}; $parts = $path -split ';' | Where-Object {{ $_ -and $_.Trim() -ne '' }}; if ($parts -contains $dir) {{ exit 0 }}; [Environment]::SetEnvironmentVariable('Path', ($path.TrimEnd(';') + ';' + $dir), 'User')"#,
    );
    let mut ps = Command::new("powershell.exe");
    ps.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    crate::windows_subprocess::hide_console_window(&mut ps);
    let output = ps.output().context("powershell PATH update failed")?;
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

pub fn copy_image_to_clipboard(path: &Path) -> anyhow::Result<()> {
    let escaped = path.to_string_lossy().replace('\'', "''");
    let script = format!("Set-Clipboard -Path '{escaped}'");
    let mut ps = Command::new("powershell.exe");
    ps.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    crate::windows_subprocess::hide_console_window(&mut ps);
    let output = ps.output().context("powershell Set-Clipboard failed")?;
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}
