use keyring::Entry;

use crate::domain::LlmProviderKind;
use crate::error::{AppError, Result};

const SERVICE_NAME: &str = "remix-autopilot";

pub struct SecretsRepository;

impl SecretsRepository {
    pub fn load_api_key(provider: LlmProviderKind) -> Result<Option<String>> {
        match Self::entry(provider)?.get_password() {
            Ok(value) if value.trim().is_empty() => Ok(None),
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(AppError::SecretStore(error.to_string())),
        }
    }

    pub fn save_api_key(provider: LlmProviderKind, api_key: &str) -> Result<()> {
        let entry = Self::entry(provider)?;
        if api_key.trim().is_empty() {
            match entry.delete_credential() {
                Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(AppError::SecretStore(error.to_string())),
            }
        } else {
            entry
                .set_password(api_key.trim())
                .map_err(|error| AppError::SecretStore(error.to_string()))
        }
    }

    pub fn clear_api_keys() -> Result<()> {
        for provider in LlmProviderKind::all()
            .iter()
            .copied()
            .filter(|provider| provider.uses_api_key())
        {
            Self::save_api_key(provider, "")?;
        }
        Ok(())
    }

    fn entry(provider: LlmProviderKind) -> Result<Entry> {
        Entry::new(
            SERVICE_NAME,
            &format!("provider:{}:api_key", provider.slug()),
        )
        .map_err(|error| AppError::SecretStore(error.to_string()))
    }
}
