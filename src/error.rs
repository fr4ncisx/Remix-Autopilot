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
