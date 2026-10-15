//! Settings (`settings.json`) and the session playlist (`session.m3u`) in the
//! player's config directory. The appearance keys mean what they mean in
//! Orchard Commander's settings.

use std::fs;
use std::path::{Path, PathBuf};

use orchard_theme::schemes::Appearance;
use serde::{Deserialize, Serialize};

/// `$ORCHARD_MUSIC_PLAYER_CONFIG_DIR`, or the platform's config folder.
pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("ORCHARD_MUSIC_PLAYER_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let name = if cfg!(target_os = "linux") { "orchard-music-player" } else { "Orchard Music Player" };
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join(name)
}

/// The player's font size unless changed (Commander's is 13: the player's
/// grid has fewer, larger things in it).
pub const DEFAULT_FONT_SIZE: i64 = 14;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub color_scheme: String,
    /// Use `light_scheme` / `dark_scheme` by the system's light or dark mode.
    pub follow_system_appearance: bool,
    pub light_scheme: String,
    pub dark_scheme: String,
    pub frame_style: String,
    pub font_family: String,
    pub font_style: String,
    pub font_size: i64,
    pub line_spacing: f64,
    pub volume: f32,
    pub shuffle: bool,
    pub repeat: String,
    pub visualizer: String,
    /// ReplayGain: "off", "track" or "album".
    pub replay_gain: String,
    /// Where the file browser opens.
    pub last_dir: Option<PathBuf>,
    /// The playlist entry that played when the player quit.
    pub current: Option<usize>,
    pub window_size: Option<(u32, u32)>,
}

impl Default for Settings {
    fn default() -> Self {
        let a = Appearance::default();
        Settings {
            color_scheme: a.color_scheme,
            follow_system_appearance: a.follow_system_appearance,
            light_scheme: a.light_scheme,
            dark_scheme: a.dark_scheme,
            frame_style: a.frame_style,
            font_family: a.font_family,
            font_style: a.font_style,
            font_size: DEFAULT_FONT_SIZE,
            line_spacing: a.line_spacing,
            volume: 0.8,
            shuffle: false,
            repeat: "off".into(),
            visualizer: "bars".into(),
            replay_gain: "track".into(),
            last_dir: None,
            current: None,
            window_size: None,
        }
    }
}

impl Settings {
    pub fn appearance(&self) -> Appearance {
        Appearance {
            color_scheme: self.color_scheme.clone(),
            follow_system_appearance: self.follow_system_appearance,
            light_scheme: self.light_scheme.clone(),
            dark_scheme: self.dark_scheme.clone(),
            frame_style: self.frame_style.clone(),
            font_family: self.font_family.clone(),
            font_style: self.font_style.clone(),
            font_size: self.font_size,
            line_spacing: self.line_spacing,
        }
    }

    pub fn load(dir: &Path) -> Settings {
        fs::read_to_string(dir.join("settings.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        let tmp = dir.join("settings.json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        fs::rename(tmp, dir.join("settings.json"))
    }
}

pub fn session_file(dir: &Path) -> PathBuf {
    dir.join("session.m3u")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults_for_missing_keys() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings { volume: 0.5, shuffle: true, color_scheme: "nord".into(), ..Settings::default() };
        s.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()), s);
        fs::write(dir.path().join("settings.json"), "{\"volume\": 0.3}").unwrap();
        let s = Settings::load(dir.path());
        assert_eq!(s.volume, 0.3);
        assert_eq!(s.repeat, "off");
    }
}
