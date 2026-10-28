//! The remote server and key last connected to, remembered between runs in
//! `remote.txt` in the user's configuration directory. The key is stored
//! as given (a revocable fracst key or server token).

use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Saved {
    pub url: String,
    pub key: String,
}

pub fn dir() -> Option<PathBuf> {
    let var = |k| std::env::var_os(k).map(PathBuf::from);
    #[cfg(windows)]
    let base = var("APPDATA");
    #[cfg(target_os = "macos")]
    let base = var("HOME").map(|h| h.join("Library/Application Support"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")));
    base.map(|b| b.join("fracst"))
}

fn path() -> Option<PathBuf> {
    dir().map(|d| d.join("remote.txt"))
}

pub fn load() -> Saved {
    let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return Saved::default();
    };
    let mut saved = Saved::default();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "url" => saved.url = v.trim().to_string(),
                "key" => saved.key = v.trim().to_string(),
                _ => {}
            }
        }
    }
    saved
}

pub fn save(saved: &Saved) {
    let Some(path) = path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, format!("url={}\nkey={}\n", saved.url, saved.key));
}
