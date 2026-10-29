//! Interface preferences, remembered between runs in `settings.txt` in the
//! user's configuration directory: one `key=value` per line.

use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    /// Whether the settings card is showing.
    pub panel: bool,
    /// How long zooms glide for, in milliseconds; 0 for no animation.
    pub glide_ms: u32,
    /// Palette entries used lately, most recent first.
    pub recent_commands: Vec<String>,
    /// Not saved: for test runs, which must not change the user's settings.
    pub read_only: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { panel: true, glide_ms: 250, recent_commands: Vec::new(), read_only: false }
    }
}

/// How many of each recent list to keep.
const RECENT: usize = 10;

/// The zoom animation choices, by name.
pub const GLIDES: [(&str, u32); 4] = [("Off", 0), ("Quick", 150), ("Smooth", 250), ("Slow", 400)];

pub fn glide_name(ms: u32) -> &'static str {
    GLIDES.iter().find(|(_, v)| *v == ms).map_or("Custom", |(name, _)| name)
}

fn path() -> Option<PathBuf> {
    crate::remote_config::dir().map(|d| d.join("settings.txt"))
}

impl Prefs {
    pub fn load() -> Prefs {
        path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Prefs::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) {
        if self.read_only {
            return;
        }
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.to_text());
    }

    /// Note a palette entry as just used.
    pub fn used(&mut self, key: &str) {
        remember(&mut self.recent_commands, key.to_string());
        self.save();
    }

    pub fn glide(&self) -> Duration {
        Duration::from_millis(self.glide_ms as u64)
    }

    fn parse(text: &str) -> Prefs {
        let mut p = Prefs::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "panel" => p.panel = v != "off",
                "glide_ms" => p.glide_ms = v.parse::<u32>().map_or(p.glide_ms, |ms| ms.min(2000)),
                "command" if !v.is_empty() && p.recent_commands.len() < RECENT => p.recent_commands.push(v.into()),
                _ => {}
            }
        }
        p
    }

    fn to_text(&self) -> String {
        let mut text = format!("panel={}\nglide_ms={}\n", if self.panel { "on" } else { "off" }, self.glide_ms);
        for key in &self.recent_commands {
            text += &format!("command={key}\n");
        }
        text
    }
}

/// Put `item` first in a recent list, without duplicates.
fn remember<T: PartialEq>(list: &mut Vec<T>, item: T) {
    list.retain(|x| *x != item);
    list.insert(0, item);
    list.truncate(RECENT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let p = Prefs {
            panel: false,
            glide_ms: 150,
            recent_commands: vec!["toggle:slopes".into(), "open".into()],
            ..Prefs::default()
        };
        assert_eq!(Prefs::parse(&p.to_text()), p);
        assert_eq!(Prefs::parse("junk\nunknown=1\nglide_ms=lots\n"), Prefs::default());
    }

    #[test]
    fn recent_lists_move_to_front_and_stay_short() {
        let mut list = vec![1, 2, 3];
        remember(&mut list, 3);
        assert_eq!(list, [3, 1, 2]);
        for i in 10..30 {
            remember(&mut list, i);
        }
        assert_eq!(list.len(), RECENT);
        assert_eq!(list[0], 29);
    }
}
