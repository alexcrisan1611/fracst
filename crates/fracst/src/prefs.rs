//! Interface preferences, remembered between runs in `settings.txt` in the
//! user's configuration directory: one `key=value` per line.

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    /// Whether the settings card is showing.
    pub panel: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { panel: true }
    }
}

fn path() -> Option<PathBuf> {
    crate::remote_config::dir().map(|d| d.join("settings.txt"))
}

impl Prefs {
    pub fn load() -> Prefs {
        path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Prefs::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.to_text());
    }

    fn parse(text: &str) -> Prefs {
        let mut p = Prefs::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            if k.trim() == "panel" {
                p.panel = v.trim() != "off";
            }
        }
        p
    }

    fn to_text(&self) -> String {
        format!("panel={}\n", if self.panel { "on" } else { "off" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let p = Prefs { panel: false };
        assert_eq!(Prefs::parse(&p.to_text()), p);
        assert_eq!(Prefs::parse("junk\nunknown=1\n"), Prefs::default());
    }
}
