//! Checks GitHub once at startup for a newer release. Only the release's
//! version is fetched; nothing about this computer is sent. Turned off from
//! the App section of the settings (remembered in `update-check.txt` in the configuration
//! directory) or with `FRACST_NO_UPDATE_CHECK=1`.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

const LATEST: &str = "https://api.github.com/repos/alexcrisan1611/fracst/releases/latest";

/// A release newer than this build.
pub struct Release {
    /// e.g. "0.2.2".
    pub version: String,
    /// The release page.
    pub url: String,
}

fn setting_path() -> Option<std::path::PathBuf> {
    crate::remote_config::dir().map(|d| d.join("update-check.txt"))
}

/// Whether to check at startup (the default).
pub fn enabled() -> bool {
    if std::env::var_os("FRACST_NO_UPDATE_CHECK").is_some_and(|v| !v.is_empty() && v != "0") {
        return false;
    }
    let off = setting_path().and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|s| s.trim() == "off");
    !off
}

pub fn set_enabled(on: bool) {
    let Some(path) = setting_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, if on { "on\n" } else { "off\n" });
}

/// Ask GitHub in the background; the answer arrives once, if there is a newer release.
pub fn spawn_check(ctx: eframe::egui::Context) -> Receiver<Release> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        if let Some(release) = newer_release() {
            let _ = tx.send(release);
            ctx.request_repaint();
        }
    });
    rx
}

fn newer_release() -> Option<Release> {
    let agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).build().new_agent();
    let body = agent
        .get(LATEST)
        .header("User-Agent", concat!("fracst/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let version = json["tag_name"].as_str()?.trim_start_matches('v').to_string();
    let url = json["html_url"].as_str()?.to_string();
    is_newer(&version, env!("CARGO_PKG_VERSION")).then_some(Release { version, url })
}

/// Compares dotted version numbers; anything unparsable counts as not newer.
fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| v.split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<_>>>();
    match (parse(candidate), parse(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.2", "0.2.1"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.2.1", "0.2.1"));
        assert!(!is_newer("0.2.0", "0.2.1"));
        assert!(!is_newer("0.3.0-beta", "0.2.1"));
    }
}
