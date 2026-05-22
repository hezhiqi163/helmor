//! Third-party editor / terminal / Git GUI detection and launching.
//!
//! Detection runs a two-phase scan:
//!   1. **Fast path** — stat() each spec's known install paths (`/Applications/…`,
//!      `~/Applications/…`, system utilities). Covers the 99% case with no subprocesses.
//!   2. **mdfind fallback** (macOS only) — one batched Spotlight query against
//!      all bundle IDs in the catalog. Catches apps installed in non-standard
//!      locations (Setapp, custom directories, brew casks configured elsewhere).
//!
//! Launching reuses the same resolution so we hand `open -a` an absolute path
//! rather than relying on Launch Services' name mapping — more robust against
//! renamed `.app` bundles.

use anyhow::Context;
use serde::Serialize;

use super::common::{run_blocking, CmdResult};
use crate::models::workspaces as workspace_models;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedEditor {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// Static description of an editor/terminal/tool we can open a workspace in.
pub struct EditorSpec {
    pub id: &'static str,
    pub name: &'static str,
    /// macOS `CFBundleIdentifier`s. Multiple entries cover stable/preview/CE variants.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub bundle_ids: &'static [&'static str],
    /// Well-known install paths. `$HOME` is expanded at runtime.
    #[cfg_attr(windows, allow(dead_code))]
    pub known_paths: &'static [&'static str],
}

/// Catalog order controls the dropdown menu order in the UI.
pub const CATALOG: &[EditorSpec] = &[
    // --- AI-first editors -------------------------------------------------
    EditorSpec {
        id: "cursor",
        name: "Cursor",
        bundle_ids: &["com.todesktop.230313mzl4w4u92"],
        known_paths: &["/Applications/Cursor.app", "$HOME/Applications/Cursor.app"],
    },
    EditorSpec {
        id: "vscode",
        name: "VS Code",
        bundle_ids: &["com.microsoft.VSCode"],
        known_paths: &[
            "/Applications/Visual Studio Code.app",
            "$HOME/Applications/Visual Studio Code.app",
        ],
    },
    EditorSpec {
        id: "vscode-insiders",
        name: "VS Code Insiders",
        bundle_ids: &["com.microsoft.VSCodeInsiders"],
        known_paths: &[
            "/Applications/Visual Studio Code - Insiders.app",
            "$HOME/Applications/Visual Studio Code - Insiders.app",
        ],
    },
    EditorSpec {
        id: "windsurf",
        name: "Windsurf",
        bundle_ids: &["com.exafunction.windsurf", "com.codeium.windsurf"],
        known_paths: &[
            "/Applications/Windsurf.app",
            "$HOME/Applications/Windsurf.app",
        ],
    },
    EditorSpec {
        id: "zed",
        name: "Zed",
        bundle_ids: &["dev.zed.Zed", "dev.zed.Zed-Preview"],
        known_paths: &["/Applications/Zed.app", "$HOME/Applications/Zed.app"],
    },
    // --- JetBrains --------------------------------------------------------
    EditorSpec {
        id: "intellij",
        name: "IntelliJ IDEA",
        bundle_ids: &["com.jetbrains.intellij", "com.jetbrains.intellij.ce"],
        known_paths: &[
            "/Applications/IntelliJ IDEA.app",
            "/Applications/IntelliJ IDEA CE.app",
            "$HOME/Applications/IntelliJ IDEA.app",
            "$HOME/Applications/IntelliJ IDEA CE.app",
        ],
    },
    EditorSpec {
        id: "pycharm",
        name: "PyCharm",
        bundle_ids: &["com.jetbrains.pycharm", "com.jetbrains.pycharm.ce"],
        known_paths: &[
            "/Applications/PyCharm.app",
            "/Applications/PyCharm CE.app",
            "$HOME/Applications/PyCharm.app",
            "$HOME/Applications/PyCharm CE.app",
        ],
    },
    EditorSpec {
        id: "webstorm",
        name: "WebStorm",
        bundle_ids: &["com.jetbrains.WebStorm"],
        known_paths: &[
            "/Applications/WebStorm.app",
            "$HOME/Applications/WebStorm.app",
        ],
    },
    EditorSpec {
        id: "goland",
        name: "GoLand",
        bundle_ids: &["com.jetbrains.goland"],
        known_paths: &["/Applications/GoLand.app", "$HOME/Applications/GoLand.app"],
    },
    EditorSpec {
        id: "rubymine",
        name: "RubyMine",
        bundle_ids: &["com.jetbrains.rubymine"],
        known_paths: &[
            "/Applications/RubyMine.app",
            "$HOME/Applications/RubyMine.app",
        ],
    },
    EditorSpec {
        id: "phpstorm",
        name: "PhpStorm",
        bundle_ids: &["com.jetbrains.PhpStorm"],
        known_paths: &[
            "/Applications/PhpStorm.app",
            "$HOME/Applications/PhpStorm.app",
        ],
    },
    EditorSpec {
        id: "clion",
        name: "CLion",
        bundle_ids: &["com.jetbrains.CLion"],
        known_paths: &["/Applications/CLion.app", "$HOME/Applications/CLion.app"],
    },
    EditorSpec {
        id: "rider",
        name: "Rider",
        bundle_ids: &["com.jetbrains.rider"],
        known_paths: &["/Applications/Rider.app", "$HOME/Applications/Rider.app"],
    },
    // --- Apple + Google ---------------------------------------------------
    EditorSpec {
        id: "xcode",
        name: "Xcode",
        bundle_ids: &["com.apple.dt.Xcode"],
        known_paths: &["/Applications/Xcode.app", "$HOME/Applications/Xcode.app"],
    },
    EditorSpec {
        id: "android-studio",
        name: "Android Studio",
        bundle_ids: &["com.google.android.studio"],
        known_paths: &[
            "/Applications/Android Studio.app",
            "$HOME/Applications/Android Studio.app",
        ],
    },
    // --- Classic editors --------------------------------------------------
    EditorSpec {
        id: "sublime",
        name: "Sublime Text",
        bundle_ids: &["com.sublimetext.4", "com.sublimetext.3"],
        known_paths: &[
            "/Applications/Sublime Text.app",
            "$HOME/Applications/Sublime Text.app",
        ],
    },
    EditorSpec {
        id: "macvim",
        name: "MacVim",
        bundle_ids: &["org.vim.MacVim"],
        known_paths: &["/Applications/MacVim.app", "$HOME/Applications/MacVim.app"],
    },
    EditorSpec {
        id: "neovide",
        name: "Neovide",
        bundle_ids: &["com.neovide.neovide"],
        known_paths: &[
            "/Applications/Neovide.app",
            "$HOME/Applications/Neovide.app",
        ],
    },
    EditorSpec {
        id: "emacs",
        name: "GNU Emacs",
        bundle_ids: &["org.gnu.Emacs"],
        known_paths: &["/Applications/Emacs.app", "$HOME/Applications/Emacs.app"],
    },
    // --- Terminals --------------------------------------------------------
    EditorSpec {
        id: "terminal",
        name: "Terminal",
        bundle_ids: &["com.apple.Terminal"],
        known_paths: &[
            "/System/Applications/Utilities/Terminal.app",
            "/Applications/Utilities/Terminal.app",
        ],
    },
    EditorSpec {
        id: "iterm",
        name: "iTerm",
        bundle_ids: &["com.googlecode.iterm2"],
        known_paths: &["/Applications/iTerm.app", "$HOME/Applications/iTerm.app"],
    },
    EditorSpec {
        id: "warp",
        name: "Warp",
        bundle_ids: &["dev.warp.Warp-Stable"],
        known_paths: &["/Applications/Warp.app", "$HOME/Applications/Warp.app"],
    },
    EditorSpec {
        id: "ghostty",
        name: "Ghostty",
        bundle_ids: &["com.mitchellh.ghostty"],
        known_paths: &[
            "/Applications/Ghostty.app",
            "$HOME/Applications/Ghostty.app",
        ],
    },
    EditorSpec {
        id: "alacritty",
        name: "Alacritty",
        bundle_ids: &["org.alacritty"],
        known_paths: &[
            "/Applications/Alacritty.app",
            "$HOME/Applications/Alacritty.app",
        ],
    },
    EditorSpec {
        id: "wezterm",
        name: "WezTerm",
        bundle_ids: &["com.github.wez.wezterm"],
        known_paths: &[
            "/Applications/WezTerm.app",
            "$HOME/Applications/WezTerm.app",
        ],
    },
    EditorSpec {
        id: "hyper",
        name: "Hyper",
        bundle_ids: &["co.zeit.hyper"],
        known_paths: &["/Applications/Hyper.app", "$HOME/Applications/Hyper.app"],
    },
    // --- Git GUIs ---------------------------------------------------------
    EditorSpec {
        id: "tower",
        name: "Tower",
        bundle_ids: &["com.fournova.Tower3"],
        known_paths: &["/Applications/Tower.app", "$HOME/Applications/Tower.app"],
    },
    EditorSpec {
        id: "sourcetree",
        name: "Sourcetree",
        bundle_ids: &["com.torusknot.SourceTreeNotMAS"],
        known_paths: &[
            "/Applications/Sourcetree.app",
            "$HOME/Applications/Sourcetree.app",
        ],
    },
    EditorSpec {
        id: "gitkraken",
        name: "GitKraken",
        bundle_ids: &["com.axosoft.gitkraken"],
        known_paths: &[
            "/Applications/GitKraken.app",
            "$HOME/Applications/GitKraken.app",
        ],
    },
];

fn spec_by_id(id: &str) -> Option<&'static EditorSpec> {
    CATALOG.iter().find(|s| s.id == id)
}

#[cfg(target_os = "macos")]
fn expand(path: &str, home: &str) -> String {
    path.replace("$HOME", home)
}

/// Try the spec's well-known paths. Returns the first one that exists.
#[cfg(target_os = "macos")]
fn resolve_via_known_paths(spec: &EditorSpec, home: &str) -> Option<String> {
    for p in spec.known_paths {
        let resolved = expand(p, home);
        if std::path::Path::new(&resolved).exists() {
            return Some(resolved);
        }
    }
    None
}

/// macOS Spotlight fallback: one batched `mdfind` over every bundle ID in the
/// catalog's unresolved set. Returns a map from `.app` filename → resolved path.
///
/// Why filename-keyed: mdfind returns paths only (no attribute readout), so we
/// reconcile hits back to their spec by basename, which is the stable identifier
/// across install locations.
#[cfg(target_os = "macos")]
fn mdfind_candidate_paths(missing: &[&EditorSpec]) -> std::collections::HashMap<String, String> {
    use std::collections::HashMap;

    let mut bundle_ids: Vec<&str> = missing
        .iter()
        .flat_map(|s| s.bundle_ids.iter().copied())
        .collect();
    bundle_ids.sort_unstable();
    bundle_ids.dedup();

    if bundle_ids.is_empty() {
        return HashMap::new();
    }

    let query = bundle_ids
        .iter()
        .map(|bid| format!("kMDItemCFBundleIdentifier == '{bid}'"))
        .collect::<Vec<_>>()
        .join(" || ");

    let output = match std::process::Command::new("mdfind").arg(&query).output() {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            tracing::debug!(status=?o.status, "mdfind exited non-zero");
            return HashMap::new();
        }
        Err(e) => {
            tracing::debug!(error=%e, "mdfind spawn failed");
            return HashMap::new();
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut result: HashMap<String, String> = HashMap::new();
    for path in stdout.lines() {
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        let Some(name) = std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
        else {
            continue;
        };
        // Prefer /Applications/ over other locations if we see the same filename twice.
        let prefer = path.starts_with("/Applications/");
        match result.get(name) {
            Some(existing) if !prefer || existing.starts_with("/Applications/") => {}
            _ => {
                result.insert(name.to_string(), path.to_string());
            }
        }
    }
    result
}

/// Resolve a spec's path via its known app-bundle filenames against the mdfind index.
#[cfg(target_os = "macos")]
fn resolve_via_mdfind(
    spec: &EditorSpec,
    index: &std::collections::HashMap<String, String>,
) -> Option<String> {
    for p in spec.known_paths {
        if let Some(name) = std::path::Path::new(p).file_name().and_then(|s| s.to_str()) {
            if let Some(hit) = index.get(name) {
                return Some(hit.clone());
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
pub(crate) fn detect_installed_editors_blocking() -> anyhow::Result<Vec<DetectedEditor>> {
    let home = std::env::var("HOME").unwrap_or_default();

    // Phase 1 — fast path
    let mut detected: Vec<DetectedEditor> = Vec::with_capacity(CATALOG.len());
    let mut missing: Vec<&EditorSpec> = Vec::new();

    for spec in CATALOG {
        match resolve_via_known_paths(spec, &home) {
            Some(path) => detected.push(DetectedEditor {
                id: spec.id.to_string(),
                name: spec.name.to_string(),
                path,
            }),
            None => missing.push(spec),
        }
    }

    // Phase 2 — one batched mdfind over everything we missed.
    if !missing.is_empty() {
        let index = mdfind_candidate_paths(&missing);
        if !index.is_empty() {
            for spec in missing {
                if let Some(path) = resolve_via_mdfind(spec, &index) {
                    detected.push(DetectedEditor {
                        id: spec.id.to_string(),
                        name: spec.name.to_string(),
                        path,
                    });
                }
            }
        }
    }

    // Re-sort to catalog order so the UI dropdown stays stable regardless of
    // which phase each editor was found in.
    detected.sort_by_key(|d| {
        CATALOG
            .iter()
            .position(|s| s.id == d.id)
            .unwrap_or(usize::MAX)
    });

    Ok(detected)
}

#[cfg(windows)]
const WINDOWS_EDITOR_PATHS: &[(&str, &[&str])] = &[
    (
        "cursor",
        &[
            r"%LOCALAPPDATA%\Programs\cursor\Cursor.exe",
            r"%LOCALAPPDATA%\Programs\Cursor\Cursor.exe",
        ],
    ),
    (
        "vscode",
        &[
            r"%LOCALAPPDATA%\Programs\Microsoft VS Code\Code.exe",
            r"%ProgramFiles%\Microsoft VS Code\Code.exe",
        ],
    ),
    (
        "vscode-insiders",
        &[
            r"%LOCALAPPDATA%\Programs\Microsoft VS Code Insiders\Code - Insiders.exe",
            r"%ProgramFiles%\Microsoft VS Code Insiders\Code - Insiders.exe",
        ],
    ),
    (
        "windsurf",
        &[r"%LOCALAPPDATA%\Programs\Windsurf\Windsurf.exe"],
    ),
    (
        "zed",
        &[
            r"%LOCALAPPDATA%\Programs\Zed\zed.exe",
            r"%LOCALAPPDATA%\Programs\Zed\Zed.exe",
        ],
    ),
    (
        "sublime",
        &[
            r"%ProgramFiles%\Sublime Text\sublime_text.exe",
            r"%ProgramFiles%\Sublime Text 3\sublime_text.exe",
        ],
    ),
    (
        "intellij",
        &[
            r"%ProgramFiles%\JetBrains\IntelliJ IDEA\bin\idea64.exe",
            r"%LOCALAPPDATA%\Programs\IntelliJ IDEA\bin\idea64.exe",
        ],
    ),
    (
        "pycharm",
        &[
            r"%ProgramFiles%\JetBrains\PyCharm\bin\pycharm64.exe",
            r"%LOCALAPPDATA%\Programs\PyCharm\bin\pycharm64.exe",
        ],
    ),
    (
        "webstorm",
        &[
            r"%ProgramFiles%\JetBrains\WebStorm\bin\webstorm64.exe",
            r"%LOCALAPPDATA%\Programs\WebStorm\bin\webstorm64.exe",
        ],
    ),
    (
        "goland",
        &[
            r"%ProgramFiles%\JetBrains\GoLand\bin\goland64.exe",
            r"%LOCALAPPDATA%\Programs\GoLand\bin\goland64.exe",
        ],
    ),
    (
        "android-studio",
        &[
            r"%ProgramFiles%\Android\Android Studio\bin\studio64.exe",
            r"%LOCALAPPDATA%\Programs\Android Studio\bin\studio64.exe",
        ],
    ),
    ("warp", &[r"%LOCALAPPDATA%\Programs\Warp\warp.exe"]),
    (
        "wezterm",
        &[
            r"%ProgramFiles%\WezTerm\wezterm.exe",
            r"%LOCALAPPDATA%\Programs\WezTerm\wezterm.exe",
        ],
    ),
    (
        "alacritty",
        &[
            r"%ProgramFiles%\Alacritty\alacritty.exe",
            r"%LOCALAPPDATA%\Programs\Alacritty\alacritty.exe",
        ],
    ),
    ("hyper", &[r"%LOCALAPPDATA%\Programs\Hyper\Hyper.exe"]),
    (
        "gitkraken",
        &[
            r"%LOCALAPPDATA%\GitKraken\GitKraken.exe",
            r"%LOCALAPPDATA%\Programs\GitKraken\GitKraken.exe",
        ],
    ),
    (
        "sourcetree",
        &[
            r"%LOCALAPPDATA%\SourceTree\SourceTree.exe",
            r"%ProgramFiles%\Sourcetree\SourceTree.exe",
        ],
    ),
    ("terminal", &[]),
];

#[cfg(windows)]
fn resolve_via_windows_paths(editor_id: &str) -> Option<String> {
    if editor_id == "terminal" {
        return crate::windows_shell::resolve_windows_terminal()
            .ok()
            .map(|p| p.display().to_string());
    }

    let paths = WINDOWS_EDITOR_PATHS
        .iter()
        .find(|(id, _)| *id == editor_id)
        .map(|(_, paths)| *paths)?;

    for template in paths {
        let resolved = crate::windows_shell::expand_path_template(template);
        if std::path::Path::new(&resolved).is_file() {
            return Some(resolved);
        }
    }

    crate::windows_shell::resolve_jetbrains_toolbox(editor_id)
}

#[cfg(windows)]
pub(crate) fn detect_installed_editors_blocking() -> anyhow::Result<Vec<DetectedEditor>> {
    let mut detected: Vec<DetectedEditor> = Vec::new();
    for spec in CATALOG {
        if let Some(path) = resolve_via_windows_paths(spec.id) {
            detected.push(DetectedEditor {
                id: spec.id.to_string(),
                name: spec.name.to_string(),
                path,
            });
        }
    }
    detected.sort_by_key(|d| {
        CATALOG
            .iter()
            .position(|s| s.id == d.id)
            .unwrap_or(usize::MAX)
    });
    Ok(detected)
}

#[cfg(all(not(target_os = "macos"), not(windows)))]
pub(crate) fn detect_installed_editors_blocking() -> anyhow::Result<Vec<DetectedEditor>> {
    Ok(Vec::new())
}

/// Resolve a single spec's path on demand (used by the launcher).
#[cfg(target_os = "macos")]
fn resolve_single(spec: &EditorSpec) -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    if let Some(p) = resolve_via_known_paths(spec, &home) {
        return Some(p);
    }
    let index = mdfind_candidate_paths(&[spec]);
    resolve_via_mdfind(spec, &index)
}

#[cfg(windows)]
fn resolve_single(spec: &EditorSpec) -> Option<String> {
    resolve_via_windows_paths(spec.id)
}

#[cfg(all(not(target_os = "macos"), not(windows)))]
fn resolve_single(_spec: &EditorSpec) -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn launch_with_open(
    app_path: Option<&str>,
    app_name: &str,
    target: &std::path::Path,
) -> anyhow::Result<()> {
    let target_str = target.display().to_string();
    let mut cmd = std::process::Command::new("open");
    match app_path {
        Some(p) => cmd.args(["-a", p, &target_str]),
        None => cmd.args(["-a", app_name, &target_str]),
    };
    cmd.spawn().map(|_| ()).context("open command failed")
}

#[cfg(windows)]
fn launch_with_open(
    app_path: Option<&str>,
    editor_id: &str,
    target: &std::path::Path,
) -> anyhow::Result<()> {
    match app_path {
        Some(exe) => launch_windows_editor(exe, editor_id, target),
        None => crate::windows_shell::explorer_open_dir(target),
    }
}

#[cfg(windows)]
fn launch_windows_editor(
    exe: &str,
    editor_id: &str,
    target: &std::path::Path,
) -> anyhow::Result<()> {
    let target_arg = target.as_os_str();
    let mut cmd = std::process::Command::new(exe);
    crate::windows_subprocess::hide_console_window(&mut cmd);
    match editor_id {
        "terminal" => {
            cmd.arg("-d").arg(target_arg);
        }
        "wezterm" => {
            cmd.args(["start", "--cwd"]).arg(target_arg);
        }
        "alacritty" => {
            cmd.args(["--working-directory"]).arg(target_arg);
        }
        "warp" | "hyper" => {
            // These GUIs do not take a workspace path on the CLI; open the app.
        }
        _ => {
            cmd.arg(target_arg);
        }
    }
    cmd.spawn()
        .map(|_| ())
        .with_context(|| format!("failed to launch {exe}"))
}

#[cfg(all(not(target_os = "macos"), not(windows)))]
fn launch_with_open(
    _app_path: Option<&str>,
    _editor_id: &str,
    _target: &std::path::Path,
) -> anyhow::Result<()> {
    anyhow::bail!("Opening third-party editors is only supported on macOS and Windows")
}

#[cfg(target_os = "macos")]
fn reveal_in_finder(dir: &std::path::Path) -> anyhow::Result<()> {
    std::process::Command::new("open")
        .arg(dir)
        .spawn()
        .map(|_| ())
        .context("open command failed")
}

#[cfg(windows)]
fn reveal_in_finder(dir: &std::path::Path) -> anyhow::Result<()> {
    crate::windows_shell::explorer_open_dir(dir)
}

#[cfg(all(not(target_os = "macos"), not(windows)))]
fn reveal_in_finder(_dir: &std::path::Path) -> anyhow::Result<()> {
    anyhow::bail!("Opening Finder is only supported on macOS and Windows")
}

#[tauri::command]
pub async fn detect_installed_editors() -> CmdResult<Vec<DetectedEditor>> {
    run_blocking(detect_installed_editors_blocking).await
}

#[tauri::command]
pub async fn open_workspace_in_editor(workspace_id: String, editor: String) -> CmdResult<()> {
    run_blocking(move || {
        let spec =
            spec_by_id(&editor).ok_or_else(|| anyhow::anyhow!("Unsupported editor: {editor}"))?;

        let record = workspace_models::load_workspace_record_by_id(&workspace_id)?
            .with_context(|| format!("Workspace not found: {workspace_id}"))?;

        let workspace_dir = crate::workspace::helpers::workspace_path(&record)?;
        if !workspace_dir.is_dir() {
            return Err(anyhow::anyhow!(
                "Workspace directory not found: {}",
                workspace_dir.display()
            ));
        }

        // Prefer the absolute app path (bypasses Launch Services name resolution,
        // which trips on renamed bundles and ambiguous names).
        let resolved = resolve_single(spec);
        #[cfg(target_os = "macos")]
        launch_with_open(resolved.as_deref(), spec.name, &workspace_dir)
            .with_context(|| format!("Failed to open {}", spec.name))?;
        #[cfg(windows)]
        launch_with_open(resolved.as_deref(), spec.id, &workspace_dir)
            .with_context(|| format!("Failed to open {}", spec.name))?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn open_file_in_editor(path: String, editor: String) -> CmdResult<()> {
    run_blocking(move || {
        let spec =
            spec_by_id(&editor).ok_or_else(|| anyhow::anyhow!("Unsupported editor: {editor}"))?;

        let stat = crate::editor_files::stat_editor_file(&path)?;
        if !stat.exists || !stat.is_file {
            return Err(anyhow::anyhow!("Editor file not found: {path}"));
        }

        let resolved = resolve_single(spec);
        #[cfg(target_os = "macos")]
        launch_with_open(resolved.as_deref(), spec.name, std::path::Path::new(&path))
            .with_context(|| format!("Failed to open {}", spec.name))?;
        #[cfg(windows)]
        launch_with_open(resolved.as_deref(), spec.id, std::path::Path::new(&path))
            .with_context(|| format!("Failed to open {}", spec.name))?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn open_workspace_in_finder(workspace_id: String) -> CmdResult<()> {
    run_blocking(move || {
        let record = workspace_models::load_workspace_record_by_id(&workspace_id)?
            .with_context(|| format!("Workspace not found: {workspace_id}"))?;

        let workspace_dir = crate::workspace::helpers::workspace_path(&record)?;
        if !workspace_dir.is_dir() {
            return Err(anyhow::anyhow!(
                "Workspace directory not found: {}",
                workspace_dir.display()
            ));
        }

        reveal_in_finder(&workspace_dir).context("Failed to open file explorer")
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_are_unique_and_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for spec in CATALOG {
            assert!(!spec.id.is_empty(), "empty id in catalog");
            assert_eq!(
                spec.id,
                spec.id.to_lowercase(),
                "id `{}` not lowercase",
                spec.id
            );
            assert!(!spec.name.is_empty(), "empty name for id `{}`", spec.id);
            assert!(
                !spec.bundle_ids.is_empty(),
                "no bundle ids for `{}`",
                spec.id
            );
            assert!(
                !spec.known_paths.is_empty(),
                "no known paths for `{}`",
                spec.id
            );
            assert!(seen.insert(spec.id), "duplicate id `{}`", spec.id);
        }
    }

    #[test]
    fn catalog_known_paths_use_app_bundles() {
        for spec in CATALOG {
            for p in spec.known_paths {
                assert!(
                    p.ends_with(".app"),
                    "catalog `{}` path `{p}` should end with .app",
                    spec.id
                );
            }
        }
    }

    #[test]
    fn catalog_bundle_ids_are_non_empty_strings() {
        for spec in CATALOG {
            for bid in spec.bundle_ids {
                assert!(!bid.is_empty(), "empty bundle id in `{}`", spec.id);
                assert!(
                    !bid.contains(char::is_whitespace),
                    "bundle id `{bid}` contains whitespace"
                );
            }
        }
    }

    #[test]
    fn spec_by_id_is_case_sensitive_and_rejects_unknown() {
        assert!(spec_by_id("cursor").is_some());
        assert!(spec_by_id("Cursor").is_none());
        assert!(spec_by_id("unknown-editor").is_none());
    }

    #[test]
    fn detected_editor_serializes_flat_camel_case() {
        let editor = DetectedEditor {
            id: "vscode".into(),
            name: "VS Code".into(),
            path: "/Applications/Visual Studio Code.app".into(),
        };
        let value = serde_json::to_value(&editor).unwrap();
        assert_eq!(value["id"], "vscode");
        assert_eq!(value["name"], "VS Code");
        assert_eq!(value["path"], "/Applications/Visual Studio Code.app");
    }

    #[test]
    fn legacy_editor_ids_still_in_catalog() {
        // Safety net: the frontend and tests previously hard-coded these ids.
        // They must stay recognisable or existing user prefs break.
        for id in [
            "cursor",
            "vscode",
            "vscode-insiders",
            "windsurf",
            "zed",
            "webstorm",
            "sublime",
            "terminal",
            "warp",
        ] {
            assert!(
                spec_by_id(id).is_some(),
                "legacy id `{id}` missing from catalog"
            );
        }
    }
}
