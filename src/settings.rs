use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub fullscreen: bool,
    pub hd: bool,
    pub effect: u8,
    pub interval: u8,
    pub album_flip: bool,
    pub reflection: bool,
    pub density: u8,
    pub frame_rate: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            fullscreen: false,
            hd: true,
            effect: 0,
            interval: 1,
            album_flip: true,
            reflection: true,
            density: 1,
            frame_rate: 1,
        }
    }
}
impl Settings {
    fn path() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/tera/settings.json"))
    }
    pub fn load() -> Self {
        let mut settings: Self = Self::path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        settings.effect = settings.effect.min(4);
        settings.interval = settings.interval.min(3);
        settings.density = settings.density.min(2);
        settings.frame_rate = settings.frame_rate.min(2);
        settings
    }
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(path) = Self::path() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        }
        Ok(())
    }
    pub fn seconds(&self) -> f32 {
        [1.5, 3.0, 5.0, 10.0][self.interval as usize]
    }
    pub fn fps(&self) -> u32 {
        [30, 60, 120][self.frame_rate as usize]
    }
    pub fn density_name(&self) -> &'static str {
        ["Spacious", "Balanced", "Dense"][self.density as usize]
    }
    pub fn effect_name(&self) -> &'static str {
        ["Flip", "Zoom", "Fold", "Random", "Off"][self.effect as usize]
    }
}
