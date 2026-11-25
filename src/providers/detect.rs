//! Detect which providers are available on this machine.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::ProviderKind;

#[derive(Clone, Debug, Default)]
pub struct Detected {
    /// Installed CLIs and the path found on PATH.
    pub clis: Vec<(ProviderKind, String)>,
    /// Is an Ollama server answering at the default address.
    pub ollama_running: bool,
    pub ollama_models: Vec<String>,
}

/// The executable name of a CLI provider.
pub fn binary_name(kind: ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::ClaudeCli => Some("claude"),
        ProviderKind::GeminiCli => Some("gemini"),
        ProviderKind::CodexCli => Some("codex"),
        _ => None,
    }
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    // TODO(windows): look for `.exe` / `.cmd` shims (npm installs `gemini.cmd`).
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Directories searched: PATH, then the usual per-user install locations that a desktop
/// launcher's PATH often lacks.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for extra in [".local/bin", ".npm-global/bin", ".claude/local", ".bun/bin", ".volta/bin", ".cargo/bin", "bin"] {
            dirs.push(home.join(extra));
        }
    }
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/usr/bin"));
    dirs
}

/// Find an executable by name on PATH and the extra user directories.
pub fn find_binary(name: &str) -> Option<PathBuf> {
    // Inside a Flatpak, look on the host (the CLIs are run there with flatpak-spawn).
    if std::path::Path::new("/.flatpak-info").exists() {
        let out = std::process::Command::new("flatpak-spawn").args(["--host", "sh", "-lc", &format!("command -v {name}")]).output().ok()?;
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return (out.status.success() && !p.is_empty()).then(|| PathBuf::from(p));
    }
    search_dirs().into_iter().map(|d| d.join(name)).find(|p| is_executable(p))
}

/// Look for `claude`, `gemini`, `codex` on PATH and probe Ollama at localhost:11434.
pub async fn detect() -> Detected {
    let mut out = Detected::default();
    for kind in [ProviderKind::ClaudeCli, ProviderKind::GeminiCli, ProviderKind::CodexCli] {
        if let Some(path) = binary_name(kind).and_then(find_binary) {
            out.clis.push((kind, path.to_string_lossy().to_string()));
        }
    }
    let base = ProviderKind::Ollama.default_base_url().unwrap_or("http://localhost:11434");
    if let Ok(models) = super::ollama::list_models_with_timeout(base, Duration::from_millis(1500)).await {
        out.ollama_running = true;
        out.ollama_models = models;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_sh() {
        assert!(find_binary("sh").is_some());
        assert!(find_binary("definitely-not-a-real-binary-xyz").is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn live_detect() {
        let d = detect().await;
        println!("{d:#?}");
    }
}
