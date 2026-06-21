use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("failed to resolve current directory: {0}")]
    CurrentDir(std::io::Error),
    #[error("terminal error: {0}")]
    Terminal(#[from] std::io::Error),
    #[error("could not find a user config directory")]
    ConfigDir,
    #[error("failed to read config `{path}`: {source}")]
    ConfigRead {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write config `{path}`: {source}")]
    ConfigWrite {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse config: {0}")]
    ConfigParse(serde_json::Error),
    #[error("failed to serialize JSON: {0}")]
    JsonSerialize(#[from] serde_json::Error),
    #[error("`git` is not installed or is not available in PATH")]
    GitMissing,
    #[error("Git command failed: git {args}\n{stderr}")]
    GitCommand { args: String, stderr: String },
    #[error(
        "Git push authentication or permission failed: git {args}\n{stderr}\nCheck your GitHub credentials or run `gh auth login`."
    )]
    GitPushAuth { args: String, stderr: String },
    #[error("Git push failed because the branch has no upstream: git {args}\n{stderr}")]
    GitPushNoUpstream { args: String, stderr: String },
    #[error("this directory is not a Git repository")]
    NotGitRepo,
    #[error("Git user.name and user.email must be configured before committing")]
    GitIdentityMissing,
    #[error("could not detect current branch; detached HEAD is not supported")]
    DetachedHead,
    #[error("remote `origin` is required for push and PR")]
    OriginMissing,
    #[error("there are no changes to commit")]
    NoChanges,
    #[error("Git permission denied: {0}")]
    GitPermissionDenied(String),
    #[error("Ollama is not responding at http://localhost:11434; start Ollama and try again")]
    OllamaUnavailable { source: reqwest::Error },
    #[error("Ollama returned HTTP status {0}")]
    OllamaHttp(u16),
    #[error("failed to decode Ollama response: {0}")]
    OllamaDecode(reqwest::Error),
    #[error("no Ollama models are available; pull a model first")]
    NoOllamaModels,
    #[error("an AI provider must be selected first")]
    ProviderNotSelected,
    #[error("{provider} API key is not configured")]
    MissingApiKey { provider: String },
    #[error("{provider} base URL is required")]
    MissingBaseUrl { provider: String },
    #[error("a model must be configured for {provider}")]
    MissingModel { provider: String },
    #[error("{provider} is not responding: {source}")]
    ProviderUnavailable {
        provider: String,
        source: reqwest::Error,
    },
    #[error("{provider} returned HTTP status {status}")]
    ProviderHttp { provider: String, status: u16 },
    #[error("failed to decode {provider} response: {source}")]
    ProviderDecode {
        provider: String,
        source: reqwest::Error,
    },
    #[error("secret store error: {0}")]
    SecretStore(String),
    #[error("invalid LLM response: {0}")]
    InvalidLlmResponse(String),
    #[error("invalid JSON returned by LLM: {source}\nvalue: {value}")]
    InvalidJson {
        value: String,
        source: serde_json::Error,
    },
    #[error("origin URL cannot be empty")]
    EmptyRemoteUrl,
    #[error("GitHub repository name cannot be empty")]
    EmptyRepositoryName,
    #[error("`gh` is not installed. Install GitHub CLI and run `gh auth login` to create PRs.")]
    GhMissing,
    #[error("GitHub CLI is not authenticated. Run `gh auth login` and try again.")]
    GhAuthMissing,
    #[error("GitHub CLI command failed: gh {args}\n{stderr}")]
    GhCommand { args: String, stderr: String },
    #[error("no remote branches were found for origin")]
    NoRemoteBranches,
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Custom(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_config_dir() {
        let error = AppError::ConfigDir;
        assert!(error.to_string().contains("config directory"));
    }

    #[test]
    fn display_git_missing() {
        let error = AppError::GitMissing;
        assert!(error.to_string().contains("git"));
        assert!(error.to_string().contains("not installed"));
    }

    #[test]
    fn display_not_git_repo() {
        let error = AppError::NotGitRepo;
        assert!(error.to_string().contains("not a Git repository"));
    }

    #[test]
    fn display_origin_missing() {
        let error = AppError::OriginMissing;
        assert!(error.to_string().contains("origin"));
    }

    #[test]
    fn display_no_changes() {
        let error = AppError::NoChanges;
        assert!(error.to_string().contains("no changes"));
    }

    #[test]
    fn display_provider_not_selected() {
        let error = AppError::ProviderNotSelected;
        assert!(error.to_string().contains("provider"));
    }

    #[test]
    fn display_missing_model_shows_provider() {
        let error = AppError::MissingModel {
            provider: "Gemini".to_string(),
        };
        assert!(error.to_string().contains("Gemini"));
        assert!(error.to_string().contains("model"));
    }

    #[test]
    fn display_missing_api_key_shows_provider() {
        let error = AppError::MissingApiKey {
            provider: "OpenAI".to_string(),
        };
        assert!(error.to_string().contains("OpenAI"));
        assert!(error.to_string().contains("API key"));
    }

    #[test]
    fn display_custom_shows_message() {
        let error = AppError::Custom("test error".into());
        assert_eq!(error.to_string(), "test error");
    }

    #[test]
    fn display_empty_remote_url() {
        let error = AppError::EmptyRemoteUrl;
        assert!(error.to_string().contains("empty"));
    }

    #[test]
    fn display_empty_repository_name() {
        let error = AppError::EmptyRepositoryName;
        assert!(error.to_string().contains("empty"));
    }

    #[test]
    fn display_gh_missing() {
        let error = AppError::GhMissing;
        assert!(error.to_string().contains("gh"));
        assert!(error.to_string().contains("not installed"));
    }

    #[test]
    fn display_gh_auth_missing() {
        let error = AppError::GhAuthMissing;
        assert!(error.to_string().contains("not authenticated"));
    }

    #[test]
    fn display_no_ollama_models() {
        let error = AppError::NoOllamaModels;
        assert!(error.to_string().contains("no Ollama models"));
    }

    #[test]
    fn display_invalid_llm_response() {
        let error = AppError::InvalidLlmResponse("bad response".into());
        assert!(error.to_string().contains("invalid LLM response"));
        assert!(error.to_string().contains("bad response"));
    }

    #[test]
    fn display_secret_store() {
        let error = AppError::SecretStore("keyring failed".into());
        assert!(error.to_string().contains("secret store"));
        assert!(error.to_string().contains("keyring failed"));
    }

    #[test]
    fn display_detached_head() {
        let error = AppError::DetachedHead;
        assert!(error.to_string().contains("detached HEAD"));
    }

    #[test]
    fn display_git_identity_missing() {
        let error = AppError::GitIdentityMissing;
        assert!(error.to_string().contains("user.name"));
    }

    #[test]
    fn display_git_command_shows_args_and_stderr() {
        let error = AppError::GitCommand {
            args: "status".to_string(),
            stderr: "fatal: not a git repository".to_string(),
        };
        let msg = error.to_string();
        assert!(msg.contains("status"));
        assert!(msg.contains("fatal: not a git repository"));
    }

    #[test]
    fn display_git_push_auth_shows_credentials_hint() {
        let error = AppError::GitPushAuth {
            args: "origin main".to_string(),
            stderr: "authentication failed".to_string(),
        };
        let msg = error.to_string();
        assert!(msg.contains("authentication"));
        assert!(msg.contains("gh auth login"));
    }

    #[test]
    fn display_git_push_no_upstream() {
        let error = AppError::GitPushNoUpstream {
            args: "origin main".to_string(),
            stderr: "no upstream branch".to_string(),
        };
        let msg = error.to_string();
        assert!(msg.contains("no upstream"));
    }

    #[test]
    fn display_ollama_http() {
        let error = AppError::OllamaHttp(503);
        assert!(error.to_string().contains("503"));
    }

    #[test]
    fn display_provider_http_shows_provider_and_status() {
        let error = AppError::ProviderHttp {
            provider: "Gemini".to_string(),
            status: 429,
        };
        let msg = error.to_string();
        assert!(msg.contains("Gemini"));
        assert!(msg.contains("429"));
    }

    #[test]
    fn display_missing_base_url_shows_provider() {
        let error = AppError::MissingBaseUrl {
            provider: "Anthropic".to_string(),
        };
        assert!(error.to_string().contains("Anthropic"));
        assert!(error.to_string().contains("base URL"));
    }

    #[test]
    fn display_config_parse_shows_error() {
        let json_error = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let error = AppError::ConfigParse(json_error);
        assert!(error.to_string().contains("failed to parse config"));
    }

    #[test]
    fn display_json_serialize_shows_error() {
        let mut map = std::collections::HashMap::new();
        map.insert(vec![1, 2], 3);
        let json_err = serde_json::to_string(&map).unwrap_err();
        let error = AppError::JsonSerialize(json_err);
        assert!(error.to_string().contains("failed to serialize JSON"));
    }
}
