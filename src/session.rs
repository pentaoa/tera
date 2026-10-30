use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub grid: bool,
    pub album: Option<String>,
    pub artist: Option<String>,
}

impl Session {
    fn path(synthetic: bool) -> Option<PathBuf> {
        let filename = if synthetic {
            "session-synthetic.json"
        } else {
            "session.json"
        };
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/tera").join(filename))
    }

    pub fn load(synthetic: bool) -> Self {
        Self::path(synthetic)
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, synthetic: bool) -> std::io::Result<()> {
        if let Some(path) = Self::path(synthetic) {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        }
        Ok(())
    }
}
