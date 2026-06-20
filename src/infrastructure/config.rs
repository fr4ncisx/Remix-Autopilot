use std::fs;
use std::path::PathBuf;

use crate::domain::Config;
use crate::error::{AppError, Result};

pub struct ConfigRepository;

impl ConfigRepository {
    pub fn load() -> Result<Config> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Config::default());
        }

        let contents = fs::read_to_string(&path).map_err(|source| AppError::ConfigRead {
            path: path.clone(),
            source,
        })?;
        serde_json::from_str(&contents).map_err(AppError::ConfigParse)
    }

    pub fn save(config: &Config) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| AppError::ConfigWrite {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let contents = serde_json::to_string_pretty(config)?;
        fs::write(&path, contents).map_err(|source| AppError::ConfigWrite { path, source })
    }

    pub fn path() -> Result<PathBuf> {
        let base = dirs::config_dir().ok_or(AppError::ConfigDir)?;
        Ok(base.join("remix-autopilot").join("config.json"))
    }
}
