//! Interface preferences, remembered between runs in `settings.txt` in the
//! user's configuration directory: one `key=value` per line.

use crate::theme::Appearance;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    /// Whether the settings card is showing.
    pub panel: bool,
    /// How long zooms glide for, in milliseconds; 0 for no animation.
    pub glide_ms: u32,
    /// Dark, Light, or as the system is.
    pub appearance: Appearance,
    /// Whether the dark appearance is true black.
    pub midnight: bool,
    /// Palette entries used lately, most recent first.
    pub recent_commands: Vec<String>,
    /// Location files opened or saved lately, most recent first.
    pub recent_locations: Vec<PathBuf>,
    /// Video project folders used lately, most recent first.
    pub recent_videos: Vec<PathBuf>,
    /// Not saved: for test runs, which must not change the user's settings.
    pub read_only: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            panel: true,
            glide_ms: 250,
            appearance: Appearance::Dark,
            midnight: false,
            recent_commands: Vec::new(),
            recent_locations: Vec::new(),
            recent_videos: Vec::new(),
            read_only: false,
        }
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

    /// Note a location file as just used.
    pub fn used_location(&mut self, path: &Path) {
        remember(&mut self.recent_locations, absolute(path));
        self.save();
    }

    /// Note a video project folder as just used.
    pub fn used_video(&mut self, dir: &Path) {
        remember(&mut self.recent_videos, absolute(dir));
        self.save();
    }

    /// Forget a recent file or folder that has gone.
    pub fn forget(&mut self, path: &Path) {
        self.recent_locations.retain(|p| p != path);
        self.recent_videos.retain(|p| p != path);
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
                // Midnight was briefly an appearance of its own.
                "appearance" if v == "midnight" => (p.appearance, p.midnight) = (Appearance::Dark, true),
                "appearance" => p.appearance = Appearance::from_key(v).unwrap_or(p.appearance),
                "midnight" => p.midnight = v == "on",
                "command" if !v.is_empty() && p.recent_commands.len() < RECENT => p.recent_commands.push(v.into()),
                "location" if !v.is_empty() && p.recent_locations.len() < RECENT => p.recent_locations.push(v.into()),
                "video" if !v.is_empty() && p.recent_videos.len() < RECENT => p.recent_videos.push(v.into()),
                _ => {}
            }
        }
        p
    }

    fn to_text(&self) -> String {
        let mut text = format!(
            "panel={}\nglide_ms={}\nappearance={}\nmidnight={}\n",
            if self.panel { "on" } else { "off" },
            self.glide_ms,
            self.appearance.key(),
            if self.midnight { "on" } else { "off" }
        );
        for key in &self.recent_commands {
            text += &format!("command={key}\n");
        }
        for path in &self.recent_locations {
            text += &format!("location={}\n", path.display());
        }
        for dir in &self.recent_videos {
            text += &format!("video={}\n", dir.display());
        }
        text
    }
}

/// Recent places are kept whole, so they open from anywhere.
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
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
            appearance: Appearance::System,
            midnight: true,
            recent_commands: vec!["toggle:slopes".into(), "open".into()],
            recent_locations: vec![PathBuf::from("C:\\deep places\\spiral.kfr"), PathBuf::from("/home/me/a=b.png")],
            recent_videos: vec![PathBuf::from("/home/me/videos/dive")],
            ..Prefs::default()
        };
        assert_eq!(Prefs::parse(&p.to_text()), p);
        assert_eq!(Prefs::parse("junk\nunknown=1\nglide_ms=lots\nappearance=sepia\n"), Prefs::default());
        let old = Prefs::parse("appearance=midnight\n");
        assert_eq!((old.appearance, old.midnight), (Appearance::Dark, true));
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
