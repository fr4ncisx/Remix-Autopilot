use std::env::consts::OS;
use std::process::Command;

use super::OllamaClient;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformOs {
    Windows,
    MacOs,
    Linux,
    Other,
}

impl PlatformOs {
    pub fn detect() -> Self {
        match OS {
            "windows" => Self::Windows,
            "macos" => Self::MacOs,
            "linux" => Self::Linux,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::MacOs => "macOS",
            Self::Linux => "Linux",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Winget,
    Chocolatey,
    Homebrew,
    AptGet,
    Dnf,
    Pacman,
    Zypper,
}

impl PackageManager {
    pub fn label(self) -> &'static str {
        match self {
            Self::Winget => "winget",
            Self::Chocolatey => "choco",
            Self::Homebrew => "brew",
            Self::AptGet => "apt-get",
            Self::Dnf => "dnf",
            Self::Pacman => "pacman",
            Self::Zypper => "zypper",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformInfo {
    pub os: PlatformOs,
    pub package_manager: Option<PackageManager>,
    pub is_elevated: bool,
}

impl PlatformInfo {
    pub fn detect() -> Self {
        let os = PlatformOs::detect();
        let package_manager = detect_package_manager_with(os, |candidate| {
            let binary = match candidate {
                PackageManager::Winget => "winget",
                PackageManager::Chocolatey => "choco",
                PackageManager::Homebrew => "brew",
                PackageManager::AptGet => "apt-get",
                PackageManager::Dnf => "dnf",
                PackageManager::Pacman => "pacman",
                PackageManager::Zypper => "zypper",
            };
            probe_command(binary, &["--version"]).is_ok()
        });
        Self {
            os,
            package_manager,
            is_elevated: detect_elevation(os),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    Git,
    Ollama,
    GitHubCli,
    LlmProvider,
}

impl DependencyKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Git => "Git",
            Self::Ollama => "Ollama",
            Self::GitHubCli => "GitHub CLI",
            Self::LlmProvider => "AI provider",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyState {
    Ready,
    Missing,
    NotRunning,
    NotConfigured,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyStatus {
    pub kind: DependencyKind,
    pub state: DependencyState,
    pub version: Option<String>,
    pub detail: Option<String>,
    pub suggested_command: Option<String>,
    pub fallback_url: Option<&'static str>,
    pub platform: PlatformInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub display: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyActionKind {
    Install,
    StartService,
    PullModel,
    ManualAuth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyAction {
    pub kind: DependencyActionKind,
    pub title: String,
    pub command: Option<CommandSpec>,
    pub manual_command: Option<String>,
    pub requires_elevation: bool,
    pub runnable_in_cli: bool,
}

impl DependencyStatus {
    pub fn ready(kind: DependencyKind, platform: &PlatformInfo, version: Option<String>) -> Self {
        Self {
            kind,
            state: DependencyState::Ready,
            version,
            detail: None,
            suggested_command: None,
            fallback_url: fallback_url(kind, platform.os),
            platform: platform.clone(),
        }
    }

    pub fn missing(kind: DependencyKind, platform: &PlatformInfo, detail: Option<String>) -> Self {
        Self {
            kind,
            state: DependencyState::Missing,
            version: None,
            detail,
            suggested_command: install_command(kind, platform),
            fallback_url: fallback_url(kind, platform.os),
            platform: platform.clone(),
        }
    }

    pub fn ollama_not_running(
        platform: &PlatformInfo,
        version: Option<String>,
        detail: String,
    ) -> Self {
        Self {
            kind: DependencyKind::Ollama,
            state: DependencyState::NotRunning,
            version,
            detail: Some(detail),
            suggested_command: Some("ollama serve".to_string()),
            fallback_url: fallback_url(DependencyKind::Ollama, platform.os),
            platform: platform.clone(),
        }
    }

    pub fn ollama_no_models(platform: &PlatformInfo, version: Option<String>) -> Self {
        Self {
            kind: DependencyKind::Ollama,
            state: DependencyState::NotConfigured,
            version,
            detail: Some("No local Ollama models were found.".to_string()),
            suggested_command: Some("ollama pull qwen3.5:latest".to_string()),
            fallback_url: Some("https://ollama.com/library"),
            platform: platform.clone(),
        }
    }

    pub fn gh_auth_missing(platform: &PlatformInfo, version: Option<String>) -> Self {
        Self {
            kind: DependencyKind::GitHubCli,
            state: DependencyState::NotConfigured,
            version,
            detail: Some("GitHub CLI is installed, but it is not authenticated.".to_string()),
            suggested_command: Some("gh auth login".to_string()),
            fallback_url: Some("https://cli.github.com/manual/gh_auth_login"),
            platform: platform.clone(),
        }
    }

    pub fn llm_provider_not_configured(
        platform: &PlatformInfo,
        detail: String,
        fallback_url: Option<&'static str>,
    ) -> Self {
        Self {
            kind: DependencyKind::LlmProvider,
            state: DependencyState::NotConfigured,
            version: None,
            detail: Some(detail),
            suggested_command: None,
            fallback_url,
            platform: platform.clone(),
        }
    }

    pub fn llm_provider_not_running(
        platform: &PlatformInfo,
        detail: String,
        fallback_url: Option<&'static str>,
    ) -> Self {
        Self {
            kind: DependencyKind::LlmProvider,
            state: DependencyState::NotRunning,
            version: None,
            detail: Some(detail),
            suggested_command: None,
            fallback_url,
            platform: platform.clone(),
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.state, DependencyState::Ready)
    }

    pub fn is_blocking(&self) -> bool {
        self.kind == DependencyKind::Git && !self.is_ready()
    }

    pub fn recovery_action(&self) -> Option<DependencyAction> {
        match (self.kind, self.state) {
            (DependencyKind::Git, DependencyState::Missing) => {
                let command = install_command_spec(self.kind, &self.platform);
                Some(DependencyAction {
                    kind: DependencyActionKind::Install,
                    title: "Install Git".to_string(),
                    runnable_in_cli: self.platform.is_elevated && command.is_some(),
                    requires_elevation: !self.platform.is_elevated,
                    manual_command: self.suggested_command.clone(),
                    command,
                })
            }
            (DependencyKind::Ollama, DependencyState::Missing) => {
                let command = install_command_spec(self.kind, &self.platform);
                Some(DependencyAction {
                    kind: DependencyActionKind::Install,
                    title: "Install Ollama".to_string(),
                    runnable_in_cli: self.platform.is_elevated && command.is_some(),
                    requires_elevation: !self.platform.is_elevated,
                    manual_command: self.suggested_command.clone(),
                    command,
                })
            }
            (DependencyKind::GitHubCli, DependencyState::Missing) => {
                let command = install_command_spec(self.kind, &self.platform);
                Some(DependencyAction {
                    kind: DependencyActionKind::Install,
                    title: "Install GitHub CLI".to_string(),
                    runnable_in_cli: self.platform.is_elevated && command.is_some(),
                    requires_elevation: !self.platform.is_elevated,
                    manual_command: self.suggested_command.clone(),
                    command,
                })
            }
            (DependencyKind::Ollama, DependencyState::NotRunning) => Some(DependencyAction {
                kind: DependencyActionKind::StartService,
                title: "Start Ollama".to_string(),
                runnable_in_cli: true,
                requires_elevation: false,
                manual_command: self.suggested_command.clone(),
                command: Some(CommandSpec {
                    program: "ollama".to_string(),
                    args: vec!["serve".to_string()],
                    display: "ollama serve".to_string(),
                }),
            }),
            (DependencyKind::Ollama, DependencyState::NotConfigured) => Some(DependencyAction {
                kind: DependencyActionKind::PullModel,
                title: "Pull recommended model".to_string(),
                runnable_in_cli: true,
                requires_elevation: false,
                manual_command: self.suggested_command.clone(),
                command: Some(CommandSpec {
                    program: "ollama".to_string(),
                    args: vec!["pull".to_string(), "qwen3.5:latest".to_string()],
                    display: "ollama pull qwen3.5:latest".to_string(),
                }),
            }),
            (DependencyKind::GitHubCli, DependencyState::NotConfigured) => Some(DependencyAction {
                kind: DependencyActionKind::ManualAuth,
                title: "Authenticate GitHub CLI".to_string(),
                runnable_in_cli: false,
                requires_elevation: false,
                manual_command: Some("gh auth login".to_string()),
                command: None,
            }),
            (DependencyKind::LlmProvider, _) => None,
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyDoctor {
    pub platform: PlatformInfo,
    pub git: DependencyStatus,
    pub ollama: DependencyStatus,
    pub gh: DependencyStatus,
    pub llm_provider: DependencyStatus,
}

impl DependencyDoctor {
    pub async fn gather(ollama: &OllamaClient, llm_provider: DependencyStatus) -> Self {
        let platform = PlatformInfo::detect();
        let git = match probe_command("git", &["--version"]) {
            Ok(version) => DependencyStatus::ready(DependencyKind::Git, &platform, Some(version)),
            Err(detail) => DependencyStatus::missing(DependencyKind::Git, &platform, Some(detail)),
        };

        let gh = match probe_command("gh", &["--version"]) {
            Ok(version) => match probe_command("gh", &["auth", "status"]) {
                Ok(_) => {
                    DependencyStatus::ready(DependencyKind::GitHubCli, &platform, Some(version))
                }
                Err(_) => DependencyStatus::gh_auth_missing(&platform, Some(version)),
            },
            Err(detail) => {
                DependencyStatus::missing(DependencyKind::GitHubCli, &platform, Some(detail))
            }
        };

        let ollama = match probe_command("ollama", &["--version"]) {
            Ok(installed_version) => match ollama.version().await {
                Ok(runtime_version) => match ollama.models().await {
                    Ok(models) if models.is_empty() => {
                        DependencyStatus::ollama_no_models(&platform, Some(runtime_version))
                    }
                    Ok(_) => DependencyStatus::ready(
                        DependencyKind::Ollama,
                        &platform,
                        Some(runtime_version),
                    ),
                    Err(error) => DependencyStatus::ollama_not_running(
                        &platform,
                        Some(installed_version),
                        error.to_string(),
                    ),
                },
                Err(error) => DependencyStatus::ollama_not_running(
                    &platform,
                    Some(installed_version),
                    error.to_string(),
                ),
            },
            Err(detail) => {
                DependencyStatus::missing(DependencyKind::Ollama, &platform, Some(detail))
            }
        };

        Self {
            platform,
            git,
            ollama,
            gh,
            llm_provider,
        }
    }

    pub fn status(&self, kind: DependencyKind) -> &DependencyStatus {
        match kind {
            DependencyKind::Git => &self.git,
            DependencyKind::Ollama => &self.ollama,
            DependencyKind::GitHubCli => &self.gh,
            DependencyKind::LlmProvider => &self.llm_provider,
        }
    }
}

fn package_manager_candidates(os: PlatformOs) -> &'static [PackageManager] {
    match os {
        PlatformOs::Windows => &[PackageManager::Winget, PackageManager::Chocolatey],
        PlatformOs::MacOs => &[PackageManager::Homebrew],
        PlatformOs::Linux => &[
            PackageManager::AptGet,
            PackageManager::Dnf,
            PackageManager::Pacman,
            PackageManager::Zypper,
        ],
        PlatformOs::Other => &[],
    }
}

fn detect_package_manager_with<F>(os: PlatformOs, mut available: F) -> Option<PackageManager>
where
    F: FnMut(PackageManager) -> bool,
{
    package_manager_candidates(os)
        .iter()
        .copied()
        .find(|candidate| available(*candidate))
}

fn install_command(kind: DependencyKind, platform: &PlatformInfo) -> Option<String> {
    match (platform.os, platform.package_manager, kind) {
        (PlatformOs::Windows, Some(PackageManager::Winget), DependencyKind::Git) => {
            Some("winget install --id Git.Git --accept-source-agreements --accept-package-agreements".to_string())
        }
        (PlatformOs::Windows, Some(PackageManager::Winget), DependencyKind::GitHubCli) => {
            Some("winget install --id GitHub.cli --accept-source-agreements --accept-package-agreements".to_string())
        }
        (PlatformOs::Windows, Some(PackageManager::Winget), DependencyKind::Ollama) => {
            Some("winget install --id Ollama.Ollama --accept-source-agreements --accept-package-agreements".to_string())
        }
        (PlatformOs::Windows, Some(PackageManager::Chocolatey), DependencyKind::Git) => {
            Some("choco install git -y".to_string())
        }
        (PlatformOs::Windows, Some(PackageManager::Chocolatey), DependencyKind::GitHubCli) => {
            Some("choco install gh -y".to_string())
        }
        (PlatformOs::Windows, Some(PackageManager::Chocolatey), DependencyKind::Ollama) => {
            Some("choco install ollama -y".to_string())
        }
        (PlatformOs::MacOs, Some(PackageManager::Homebrew), DependencyKind::Git) => {
            Some("brew install git".to_string())
        }
        (PlatformOs::MacOs, Some(PackageManager::Homebrew), DependencyKind::GitHubCli) => {
            Some("brew install gh".to_string())
        }
        (PlatformOs::MacOs, Some(PackageManager::Homebrew), DependencyKind::Ollama) => {
            Some("brew install ollama".to_string())
        }
        (PlatformOs::Linux, Some(PackageManager::AptGet), DependencyKind::Git) => {
            Some("sudo apt-get install -y git".to_string())
        }
        (PlatformOs::Linux, Some(PackageManager::AptGet), DependencyKind::GitHubCli) => {
            Some("sudo apt-get install -y gh".to_string())
        }
        (PlatformOs::Linux, Some(PackageManager::Dnf), DependencyKind::Git) => {
            Some("sudo dnf install -y git".to_string())
        }
        (PlatformOs::Linux, Some(PackageManager::Dnf), DependencyKind::GitHubCli) => {
            Some("sudo dnf install -y gh".to_string())
        }
        (PlatformOs::Linux, Some(PackageManager::Pacman), DependencyKind::Git) => {
            Some("sudo pacman -S --noconfirm git".to_string())
        }
        (PlatformOs::Linux, Some(PackageManager::Pacman), DependencyKind::GitHubCli) => {
            Some("sudo pacman -S --noconfirm github-cli".to_string())
        }
        (PlatformOs::Linux, Some(PackageManager::Zypper), DependencyKind::Git) => {
            Some("sudo zypper install -y git".to_string())
        }
        (PlatformOs::Linux, Some(PackageManager::Zypper), DependencyKind::GitHubCli) => {
            Some("sudo zypper install -y gh".to_string())
        }
        (_, _, DependencyKind::LlmProvider) => None,
        _ => None,
    }
}

fn install_command_spec(kind: DependencyKind, platform: &PlatformInfo) -> Option<CommandSpec> {
    match (platform.os, platform.package_manager, kind) {
        (PlatformOs::Windows, Some(PackageManager::Winget), DependencyKind::Git) => Some(
            CommandSpec {
                program: "winget".to_string(),
                args: vec![
                    "install".to_string(),
                    "--id".to_string(),
                    "Git.Git".to_string(),
                    "--accept-source-agreements".to_string(),
                    "--accept-package-agreements".to_string(),
                ],
                display: "winget install --id Git.Git --accept-source-agreements --accept-package-agreements".to_string(),
            },
        ),
        (PlatformOs::Windows, Some(PackageManager::Winget), DependencyKind::GitHubCli) => Some(
            CommandSpec {
                program: "winget".to_string(),
                args: vec![
                    "install".to_string(),
                    "--id".to_string(),
                    "GitHub.cli".to_string(),
                    "--accept-source-agreements".to_string(),
                    "--accept-package-agreements".to_string(),
                ],
                display: "winget install --id GitHub.cli --accept-source-agreements --accept-package-agreements".to_string(),
            },
        ),
        (PlatformOs::Windows, Some(PackageManager::Winget), DependencyKind::Ollama) => Some(
            CommandSpec {
                program: "winget".to_string(),
                args: vec![
                    "install".to_string(),
                    "--id".to_string(),
                    "Ollama.Ollama".to_string(),
                    "--accept-source-agreements".to_string(),
                    "--accept-package-agreements".to_string(),
                ],
                display: "winget install --id Ollama.Ollama --accept-source-agreements --accept-package-agreements".to_string(),
            },
        ),
        (PlatformOs::Windows, Some(PackageManager::Chocolatey), DependencyKind::Git) => Some(
            CommandSpec {
                program: "choco".to_string(),
                args: vec!["install".to_string(), "git".to_string(), "-y".to_string()],
                display: "choco install git -y".to_string(),
            },
        ),
        (PlatformOs::Windows, Some(PackageManager::Chocolatey), DependencyKind::GitHubCli) => Some(
            CommandSpec {
                program: "choco".to_string(),
                args: vec!["install".to_string(), "gh".to_string(), "-y".to_string()],
                display: "choco install gh -y".to_string(),
            },
        ),
        (PlatformOs::Windows, Some(PackageManager::Chocolatey), DependencyKind::Ollama) => Some(
            CommandSpec {
                program: "choco".to_string(),
                args: vec!["install".to_string(), "ollama".to_string(), "-y".to_string()],
                display: "choco install ollama -y".to_string(),
            },
        ),
        (PlatformOs::MacOs, Some(PackageManager::Homebrew), DependencyKind::Git) => Some(
            CommandSpec {
                program: "brew".to_string(),
                args: vec!["install".to_string(), "git".to_string()],
                display: "brew install git".to_string(),
            },
        ),
        (PlatformOs::MacOs, Some(PackageManager::Homebrew), DependencyKind::GitHubCli) => Some(
            CommandSpec {
                program: "brew".to_string(),
                args: vec!["install".to_string(), "gh".to_string()],
                display: "brew install gh".to_string(),
            },
        ),
        (PlatformOs::MacOs, Some(PackageManager::Homebrew), DependencyKind::Ollama) => Some(
            CommandSpec {
                program: "brew".to_string(),
                args: vec!["install".to_string(), "ollama".to_string()],
                display: "brew install ollama".to_string(),
            },
        ),
        (PlatformOs::Linux, Some(PackageManager::AptGet), DependencyKind::Git) => Some(
            CommandSpec {
                program: "apt-get".to_string(),
                args: vec!["install".to_string(), "-y".to_string(), "git".to_string()],
                display: "apt-get install -y git".to_string(),
            },
        ),
        (PlatformOs::Linux, Some(PackageManager::AptGet), DependencyKind::GitHubCli) => Some(
            CommandSpec {
                program: "apt-get".to_string(),
                args: vec!["install".to_string(), "-y".to_string(), "gh".to_string()],
                display: "apt-get install -y gh".to_string(),
            },
        ),
        (PlatformOs::Linux, Some(PackageManager::Dnf), DependencyKind::Git) => Some(
            CommandSpec {
                program: "dnf".to_string(),
                args: vec!["install".to_string(), "-y".to_string(), "git".to_string()],
                display: "dnf install -y git".to_string(),
            },
        ),
        (PlatformOs::Linux, Some(PackageManager::Dnf), DependencyKind::GitHubCli) => Some(
            CommandSpec {
                program: "dnf".to_string(),
                args: vec!["install".to_string(), "-y".to_string(), "gh".to_string()],
                display: "dnf install -y gh".to_string(),
            },
        ),
        (PlatformOs::Linux, Some(PackageManager::Pacman), DependencyKind::Git) => Some(
            CommandSpec {
                program: "pacman".to_string(),
                args: vec![
                    "-S".to_string(),
                    "--noconfirm".to_string(),
                    "git".to_string(),
                ],
                display: "pacman -S --noconfirm git".to_string(),
            },
        ),
        (
            PlatformOs::Linux,
            Some(PackageManager::Pacman),
            DependencyKind::GitHubCli,
        ) => Some(CommandSpec {
            program: "pacman".to_string(),
            args: vec![
                "-S".to_string(),
                "--noconfirm".to_string(),
                "github-cli".to_string(),
            ],
            display: "pacman -S --noconfirm github-cli".to_string(),
        }),
        (PlatformOs::Linux, Some(PackageManager::Zypper), DependencyKind::Git) => Some(
            CommandSpec {
                program: "zypper".to_string(),
                args: vec!["install".to_string(), "-y".to_string(), "git".to_string()],
                display: "zypper install -y git".to_string(),
            },
        ),
        (PlatformOs::Linux, Some(PackageManager::Zypper), DependencyKind::GitHubCli) => Some(
            CommandSpec {
                program: "zypper".to_string(),
                args: vec!["install".to_string(), "-y".to_string(), "gh".to_string()],
                display: "zypper install -y gh".to_string(),
            },
        ),
        (_, _, DependencyKind::LlmProvider) => None,
        _ => None,
    }
}

fn detect_elevation(os: PlatformOs) -> bool {
    match os {
        PlatformOs::Windows => {
            let output = Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "[bool](([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))",
                ])
                .output();
            match output {
                Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .eq_ignore_ascii_case("true"),
                _ => false,
            }
        }
        PlatformOs::Linux | PlatformOs::MacOs => match Command::new("id").arg("-u").output() {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim() == "0"
            }
            _ => false,
        },
        PlatformOs::Other => false,
    }
}

fn fallback_url(kind: DependencyKind, os: PlatformOs) -> Option<&'static str> {
    match kind {
        DependencyKind::Git => Some(match os {
            PlatformOs::Windows => "https://git-scm.com/download/win",
            PlatformOs::MacOs => "https://git-scm.com/download/mac",
            PlatformOs::Linux | PlatformOs::Other => "https://git-scm.com/download/linux",
        }),
        DependencyKind::Ollama => Some("https://ollama.com/download"),
        DependencyKind::GitHubCli => Some("https://cli.github.com/"),
        DependencyKind::LlmProvider => None,
    }
}

fn probe_command(binary: &str, args: &[&str]) -> std::result::Result<String, String> {
    let output = Command::new(binary)
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stdout.is_empty() {
            Ok(stderr)
        } else {
            Ok(stdout)
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !stderr.is_empty() {
            Err(stderr)
        } else if !stdout.is_empty() {
            Err(stdout)
        } else {
            Err(format!("{} {} failed", binary, args.join(" ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_manager_prefers_winget_on_windows() {
        let detected = detect_package_manager_with(PlatformOs::Windows, |candidate| {
            matches!(
                candidate,
                PackageManager::Winget | PackageManager::Chocolatey
            )
        });

        assert_eq!(detected, Some(PackageManager::Winget));
    }

    #[test]
    fn package_manager_uses_brew_on_macos() {
        let detected = detect_package_manager_with(PlatformOs::MacOs, |candidate| {
            candidate == PackageManager::Homebrew
        });

        assert_eq!(detected, Some(PackageManager::Homebrew));
    }

    #[test]
    fn git_install_hint_uses_detected_package_manager() {
        let platform = PlatformInfo {
            os: PlatformOs::Windows,
            package_manager: Some(PackageManager::Chocolatey),
            is_elevated: false,
        };

        let status = DependencyStatus::missing(DependencyKind::Git, &platform, None);

        assert_eq!(
            status.suggested_command.as_deref(),
            Some("choco install git -y")
        );
    }

    #[test]
    fn ollama_no_models_guidance_uses_pull_command() {
        let platform = PlatformInfo {
            os: PlatformOs::Linux,
            package_manager: Some(PackageManager::AptGet),
            is_elevated: false,
        };

        let status = DependencyStatus::ollama_no_models(&platform, Some("0.9.0".to_string()));

        assert_eq!(
            status.suggested_command.as_deref(),
            Some("ollama pull qwen3.5:latest")
        );
        assert_eq!(status.state, DependencyState::NotConfigured);
    }

    #[test]
    fn github_auth_guidance_uses_login_command() {
        let platform = PlatformInfo {
            os: PlatformOs::Linux,
            package_manager: Some(PackageManager::AptGet),
            is_elevated: false,
        };

        let status = DependencyStatus::gh_auth_missing(&platform, Some("2.0.0".to_string()));

        assert_eq!(status.suggested_command.as_deref(), Some("gh auth login"));
        assert_eq!(status.state, DependencyState::NotConfigured);
    }

    #[test]
    fn install_action_is_runnable_only_when_elevated() {
        let platform = PlatformInfo {
            os: PlatformOs::Windows,
            package_manager: Some(PackageManager::Winget),
            is_elevated: true,
        };

        let status = DependencyStatus::missing(DependencyKind::Git, &platform, None);
        let action = status.recovery_action().unwrap();

        assert!(action.runnable_in_cli);
        assert!(action.command.is_some());
    }

    #[test]
    fn is_ready_true_for_ready_state() {
        let platform = PlatformInfo {
            os: PlatformOs::Linux,
            package_manager: Some(PackageManager::AptGet),
            is_elevated: false,
        };
        let status = DependencyStatus::ready(DependencyKind::Git, &platform, Some("2.40".into()));
        assert!(status.is_ready());
    }

    #[test]
    fn is_ready_false_for_missing_state() {
        let platform = PlatformInfo {
            os: PlatformOs::Windows,
            package_manager: Some(PackageManager::Winget),
            is_elevated: false,
        };
        let status = DependencyStatus::missing(DependencyKind::Git, &platform, None);
        assert!(!status.is_ready());
    }

    #[test]
    fn is_blocking_true_for_git_missing() {
        let platform = PlatformInfo {
            os: PlatformOs::Linux,
            package_manager: Some(PackageManager::AptGet),
            is_elevated: false,
        };
        let status = DependencyStatus::missing(DependencyKind::Git, &platform, None);
        assert!(status.is_blocking());
    }

    #[test]
    fn is_blocking_false_for_git_ready() {
        let platform = PlatformInfo {
            os: PlatformOs::Linux,
            package_manager: Some(PackageManager::AptGet),
            is_elevated: false,
        };
        let status = DependencyStatus::ready(DependencyKind::Git, &platform, Some("2.40".into()));
        assert!(!status.is_blocking());
    }

    #[test]
    fn is_blocking_false_for_non_git_missing() {
        let platform = PlatformInfo {
            os: PlatformOs::Linux,
            package_manager: Some(PackageManager::AptGet),
            is_elevated: false,
        };
        let status = DependencyStatus::missing(DependencyKind::GitHubCli, &platform, None);
        assert!(!status.is_blocking());
    }

    #[test]
    fn recovery_action_returns_install_for_git_missing() {
        let platform = PlatformInfo {
            os: PlatformOs::MacOs,
            package_manager: Some(PackageManager::Homebrew),
            is_elevated: false,
        };
        let status = DependencyStatus::missing(DependencyKind::Git, &platform, None);
        let action = status.recovery_action().unwrap();
        assert_eq!(action.kind, DependencyActionKind::Install);
        assert!(action.title.contains("Git"));
    }

    #[test]
    fn recovery_action_returns_pull_for_ollama_no_models() {
        let platform = PlatformInfo {
            os: PlatformOs::Linux,
            package_manager: Some(PackageManager::AptGet),
            is_elevated: false,
        };
        let status = DependencyStatus::ollama_no_models(&platform, None);
        let action = status.recovery_action().unwrap();
        assert_eq!(action.kind, DependencyActionKind::PullModel);
    }

    #[test]
    fn recovery_action_returns_manual_auth_for_gh_auth() {
        let platform = PlatformInfo {
            os: PlatformOs::Windows,
            package_manager: Some(PackageManager::Winget),
            is_elevated: false,
        };
        let status = DependencyStatus::gh_auth_missing(&platform, None);
        let action = status.recovery_action().unwrap();
        assert_eq!(action.kind, DependencyActionKind::ManualAuth);
    }

    #[test]
    fn recovery_action_returns_none_for_ready() {
        let platform = PlatformInfo {
            os: PlatformOs::Linux,
            package_manager: Some(PackageManager::AptGet),
            is_elevated: false,
        };
        let status = DependencyStatus::ready(DependencyKind::Git, &platform, None);
        assert!(status.recovery_action().is_none());
    }

    #[test]
    fn platform_os_label_matches_variant() {
        assert_eq!(PlatformOs::Windows.label(), "Windows");
        assert_eq!(PlatformOs::MacOs.label(), "macOS");
        assert_eq!(PlatformOs::Linux.label(), "Linux");
        assert_eq!(PlatformOs::Other.label(), "Other");
    }

    #[test]
    fn package_manager_label_matches_variant() {
        assert_eq!(PackageManager::Winget.label(), "winget");
        assert_eq!(PackageManager::Chocolatey.label(), "choco");
        assert_eq!(PackageManager::Homebrew.label(), "brew");
        assert_eq!(PackageManager::AptGet.label(), "apt-get");
        assert_eq!(PackageManager::Dnf.label(), "dnf");
        assert_eq!(PackageManager::Pacman.label(), "pacman");
        assert_eq!(PackageManager::Zypper.label(), "zypper");
    }

    #[test]
    fn ollama_not_running_suggests_serve_command() {
        let platform = PlatformInfo {
            os: PlatformOs::Linux,
            package_manager: Some(PackageManager::AptGet),
            is_elevated: false,
        };
        let status = DependencyStatus::ollama_not_running(
            &platform,
            Some("0.9.0".into()),
            "not running".into(),
        );
        assert_eq!(status.state, DependencyState::NotRunning);
        assert_eq!(status.suggested_command.as_deref(), Some("ollama serve"));
    }

    #[test]
    fn llm_provider_not_configured_has_detail() {
        let platform = PlatformInfo {
            os: PlatformOs::Windows,
            package_manager: Some(PackageManager::Winget),
            is_elevated: false,
        };
        let status = DependencyStatus::llm_provider_not_configured(
            &platform,
            "No API key configured".into(),
            Some("https://example.com"),
        );
        assert_eq!(status.state, DependencyState::NotConfigured);
        assert_eq!(status.kind, DependencyKind::LlmProvider);
        assert!(status.detail.unwrap().contains("No API key"));
        assert!(status.suggested_command.is_none());
    }

    #[test]
    fn install_command_uses_apt_get_on_linux() {
        let platform = PlatformInfo {
            os: PlatformOs::Linux,
            package_manager: Some(PackageManager::AptGet),
            is_elevated: false,
        };
        let status = DependencyStatus::missing(DependencyKind::Git, &platform, None);
        assert_eq!(
            status.suggested_command.as_deref(),
            Some("sudo apt-get install git")
        );
    }
}
