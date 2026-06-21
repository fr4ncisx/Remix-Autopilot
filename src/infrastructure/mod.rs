pub mod config;
pub mod dependencies;
pub mod git;
pub mod github;
pub mod llm;
pub mod ollama;
pub mod secrets;

pub use config::ConfigRepository;
pub use dependencies::{
    DependencyAction, DependencyActionKind, DependencyDoctor, DependencyKind, DependencyState,
    DependencyStatus, PackageManager, PlatformInfo,
};
pub use git::{BranchOption, BranchSource, CommitLogEntry, Git, RepoStatus, SwitchBranches};
pub use github::GitHubCli;
pub use llm::LlmClient;
pub use ollama::{OllamaClient, detect_vram};
pub use secrets::SecretsRepository;
