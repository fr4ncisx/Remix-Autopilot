use std::path::PathBuf;
use std::process::{Command, ExitStatus};

use crate::domain::{PrInfo, PullRequestDraft};
use crate::error::{AppError, Result};

#[derive(Clone)]
pub struct GitHubCli {
    cwd: PathBuf,
}

impl GitHubCli {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }

    pub fn ensure_ready(&self) -> Result<()> {
        let version = Command::new("gh").arg("--version").output();
        match version {
            Ok(output) if output.status.success() => {}
            _ => return Err(AppError::GhMissing),
        }

        let auth = Command::new("gh")
            .args(["auth", "status"])
            .output()
            .map_err(|_| AppError::GhMissing)?;
        if !auth.status.success() {
            return Err(AppError::GhAuthMissing);
        }
        Ok(())
    }

    pub fn create_repo(&self, name: &str, private: bool) -> Result<String> {
        if name.trim().is_empty() {
            return Err(AppError::EmptyRepositoryName);
        }
        self.ensure_ready()?;
        let visibility = if private { "--private" } else { "--public" };
        self.output([
            "repo", "create", name, "--source", ".", "--remote", "origin", visibility,
        ])
    }

    pub fn create_pr(&self, base: &str, head: &str, draft: &PullRequestDraft) -> Result<String> {
        self.ensure_ready()?;
        self.output([
            "pr",
            "create",
            "--base",
            base,
            "--head",
            head,
            "--title",
            &draft.title,
            "--body",
            &draft.body,
        ])
    }

    pub fn edit_pr(&self, number: i64, draft: &PullRequestDraft) -> Result<String> {
        self.ensure_ready()?;
        self.output([
            "pr",
            "edit",
            &number.to_string(),
            "--title",
            &draft.title,
            "--body",
            &draft.body,
        ])
    }

    pub fn close_pr(&self, number: i64) -> Result<String> {
        self.ensure_ready()?;
        self.output(["pr", "close", &number.to_string()])
    }

    pub fn list_open_prs(&self, head: &str, base: &str) -> Result<Vec<PrInfo>> {
        self.ensure_ready()?;
        let output = self.output([
            "pr",
            "list",
            "--head",
            head,
            "--base",
            base,
            "--state",
            "open",
            "--json",
            "number,title,url,author,body",
        ])?;
        serde_json::from_str(&output)
            .map_err(|e| AppError::Custom(format!("Failed to parse PR list: {}", e)))
    }

    fn output<const N: usize>(&self, args: [&str; N]) -> Result<String> {
        let output = Command::new("gh")
            .current_dir(&self.cwd)
            .args(args)
            .output()
            .map_err(|_| AppError::GhMissing)?;
        command_stdout(
            output.status,
            output.stdout,
            output.stderr,
            &format!("gh {}", args.join(" ")),
        )
    }
}

fn command_stdout(
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    command: &str,
) -> Result<String> {
    if status.success() {
        return Ok(String::from_utf8_lossy(&stdout).trim().to_string());
    }

    Err(AppError::GhCommand {
        args: command.to_string(),
        stderr: String::from_utf8_lossy(&stderr).trim().to_string(),
    })
}
