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
        #[cfg(test)]
        {
            let pid = std::process::id();
            let thread_id = format!("{:?}", std::thread::current().id());
            let thread_clean: String = thread_id.chars().filter(|c| c.is_alphanumeric()).collect();
            Ok(std::env::temp_dir().join(format!("remix-autopilot-test-{}-{}.json", pid, thread_clean)))
        }
        #[cfg(not(test))]
        {
            let base = dirs::config_dir().ok_or(AppError::ConfigDir)?;
            Ok(base.join("remix-autopilot").join("config.json"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{HistoryLimit, LlmProviderKind, ThemeChoice};
    use tempfile::tempdir;

    fn test_config_path(dir: &std::path::Path) -> PathBuf {
        dir.join("remix-autopilot").join("config.json")
    }

    #[test]
    fn load_returns_default_when_file_missing() {
        let dir = tempdir().unwrap();
        let path = test_config_path(dir.path());
        assert!(!path.exists());
        let result = load_from_path(&path).unwrap();
        assert_eq!(result.provider, LlmProviderKind::Unset);
        assert_eq!(result.language, "English");
        assert_eq!(result.theme, ThemeChoice::CodexDark);
        assert_eq!(result.history_limit, HistoryLimit::Medium);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempdir().unwrap();
        let path = test_config_path(dir.path());
        let mut config = crate::domain::Config::default();
        config.provider = LlmProviderKind::Gemini;
        config.model = Some("gemini-2.0-flash".to_string());
        config.language = "Spanish".to_string();
        config.theme = ThemeChoice::Dracula;
        config.history_limit = HistoryLimit::Large;

        save_to_path(&config, &path).unwrap();
        let loaded = load_from_path(&path).unwrap();

        assert_eq!(loaded.provider, LlmProviderKind::Gemini);
        assert_eq!(loaded.model, Some("gemini-2.0-flash".to_string()));
        assert_eq!(loaded.language, "Spanish");
        assert_eq!(loaded.theme, ThemeChoice::Dracula);
        assert_eq!(loaded.history_limit, HistoryLimit::Large);
    }

    #[test]
    fn save_creates_parent_directories() {
        let dir = tempdir().unwrap();
        let path = dir
            .path()
            .join("deeply")
            .join("nested")
            .join("remix-autopilot")
            .join("config.json");
        let config = crate::domain::Config::default();

        save_to_path(&config, &path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn load_returns_error_for_corrupt_json() {
        let dir = tempdir().unwrap();
        let path = test_config_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not valid json }}}").unwrap();

        let result = load_from_path(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_returns_error_for_empty_file() {
        let dir = tempdir().unwrap();
        let path = test_config_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();

        let result = load_from_path(&path);
        assert!(result.is_err());
    }

    #[test]
    fn save_overwrites_existing_config() {
        let dir = tempdir().unwrap();
        let path = test_config_path(dir.path());

        let mut config1 = crate::domain::Config::default();
        config1.language = "English".to_string();
        save_to_path(&config1, &path).unwrap();

        let mut config2 = crate::domain::Config::default();
        config2.language = "Spanish".to_string();
        config2.provider = LlmProviderKind::OpenAi;
        save_to_path(&config2, &path).unwrap();

        let loaded = load_from_path(&path).unwrap();
        assert_eq!(loaded.language, "Spanish");
        assert_eq!(loaded.provider, LlmProviderKind::OpenAi);
    }

    fn load_from_path(path: &std::path::Path) -> Result<crate::domain::Config> {
        if !path.exists() {
            return Ok(crate::domain::Config::default());
        }
        let contents = fs::read_to_string(path).map_err(|source| AppError::ConfigRead {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::from_str(&contents).map_err(AppError::ConfigParse)
    }

    fn save_to_path(config: &crate::domain::Config, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| AppError::ConfigWrite {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let contents = serde_json::to_string_pretty(config)?;
        fs::write(path, contents).map_err(|source| AppError::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })
    }
}
