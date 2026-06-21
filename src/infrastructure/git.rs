use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::domain::diff::truncate_diff;
use crate::domain::{CommitMessage, DiffContext};
use crate::error::{AppError, Result};

#[derive(Debug, Clone)]
pub struct RepoStatus {
    pub repo: String,
    pub branch: String,
    pub has_origin: bool,
    pub is_repo: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkingTreeSync {
    pub has_changes: bool,
    pub has_upstream: bool,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchSource {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchOption {
    pub name: String,
    pub source: BranchSource,
    pub last_commit_unix: Option<i64>,
    pub is_current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitLogEntry {
    pub hash: String,
    pub short_hash: String,
    pub decorations: String,
    pub subject: String,
    pub author: String,
    pub relative_time: String,
}

impl BranchOption {
    pub fn is_protected(&self) -> bool {
        is_protected_branch_name(&self.name)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SwitchBranches {
    pub remote: Vec<BranchOption>,
    pub local: Vec<BranchOption>,
}

impl SwitchBranches {
    pub fn is_empty(&self) -> bool {
        self.remote.is_empty() && self.local.is_empty()
    }

    pub fn total_count(&self) -> usize {
        self.remote.len() + self.local.len()
    }

    pub fn get(&self, index: usize) -> Option<&BranchOption> {
        if index < self.remote.len() {
            self.remote.get(index)
        } else {
            self.local.get(index.saturating_sub(self.remote.len()))
        }
    }
}

#[derive(Clone)]
pub struct Git {
    cwd: PathBuf,
    cache: Arc<Mutex<GitCache>>,
}

#[derive(Default)]
struct GitCache {
    status: Option<(RepoStatus, Instant)>,
    status_ttl: Duration,
}

impl Git {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            cache: Arc::new(Mutex::new(GitCache {
                status: None,
                status_ttl: Duration::from_secs(3),
            })),
        }
    }

    pub fn status(&self) -> RepoStatus {
        let mut cache = self.cache.lock().expect("git status cache lock poisoned");
        if let Some((status, time)) = &cache.status
            && time.elapsed() < cache.status_ttl
        {
            return status.clone();
        }
        let status = RepoStatus {
            repo: repo_label(&self.cwd),
            branch: self
                .current_branch()
                .unwrap_or_else(|_| "unknown".to_string()),
            has_origin: self.has_origin(),
            is_repo: self.is_repo(),
        };
        cache.status = Some((status.clone(), Instant::now()));
        status
    }

    pub fn ensure_installed(&self) -> Result<String> {
        let output = Command::new("git")
            .arg("--version")
            .output()
            .map_err(|_| AppError::GitMissing)?;
        command_stdout(output.status, output.stdout, output.stderr, "git --version")
    }

    pub fn is_repo(&self) -> bool {
        self.ensure_repo().is_ok()
    }

    pub fn ensure_repo(&self) -> Result<()> {
        let output = self.output(["rev-parse", "--is-inside-work-tree"])?;
        if output.trim() == "true" {
            Ok(())
        } else {
            Err(AppError::NotGitRepo)
        }
    }

    pub fn working_tree_sync(&self) -> Result<WorkingTreeSync> {
        let output = self.output(["status", "--porcelain=v1", "--branch"])?;
        Ok(parse_working_tree_sync(&output))
    }

    pub fn init(&self) -> Result<String> {
        let output = self.output(["init"])?;
        self.invalidate_status_cache();
        Ok(output)
    }

    pub fn ensure_identity(&self) -> Result<()> {
        let name = self
            .output(["config", "--get", "user.name"])
            .unwrap_or_default();
        let email = self
            .output(["config", "--get", "user.email"])
            .unwrap_or_default();
        if name.trim().is_empty() || email.trim().is_empty() {
            return Err(AppError::GitIdentityMissing);
        }
        Ok(())
    }

    pub fn has_origin(&self) -> bool {
        self.origin_url()
            .is_ok_and(|origin| !origin.trim().is_empty())
    }

    pub fn origin_url(&self) -> Result<String> {
        self.output(["remote", "get-url", "origin"])
    }

    pub fn add_origin(&self, url: &str) -> Result<String> {
        if url.trim().is_empty() {
            return Err(AppError::EmptyRemoteUrl);
        }
        let output = self.output(["remote", "add", "origin", url])?;
        self.invalidate_status_cache();
        Ok(output)
    }

    pub fn remove_origin_if_exists(&self) -> Result<bool> {
        if !self.has_origin() {
            self.invalidate_status_cache();
            return Ok(false);
        }
        self.output(["remote", "remove", "origin"])?;
        self.invalidate_status_cache();
        Ok(true)
    }

    pub fn current_branch(&self) -> Result<String> {
        let branch = self.output(["branch", "--show-current"])?;
        let branch = branch.trim();
        if branch.is_empty() {
            return Err(AppError::DetachedHead);
        }
        Ok(branch.to_string())
    }

    pub fn has_upstream(&self) -> Result<bool> {
        match self.output(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]) {
            Ok(value) => Ok(!value.trim().is_empty()),
            Err(AppError::GitCommand { .. }) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn add_all(&self) -> Result<String> {
        self.output(["add", "-A"])
    }

    pub fn add_paths(&self, paths: &[String]) -> Result<String> {
        let paths = validate_relative_paths(paths)?;
        let mut args = vec!["add".to_string(), "--".to_string()];
        args.extend(paths);
        self.output_vec(args)
    }

    pub fn reset_index(&self) -> Result<String> {
        self.output(["reset", "-q"])
    }

    pub fn apply_patch_to_index(&self, file_path: &str, patch: &str) -> Result<String> {
        // Normalize CRLF → LF: LLMs on Windows often emit \r\n which git apply rejects
        let patch = patch.replace("\r\n", "\n").replace('\r', "\n");
        let patch = patch.trim();
        if patch.is_empty() {
            return Err(AppError::Custom(
                "commit group patch cannot be empty".to_string(),
            ));
        }

        // Ensure the file path uses forward slashes (git expects POSIX paths)
        let git_path = file_path.replace('\\', "/");

        let patch_with_header = if patch.starts_with("--- ") {
            patch.to_string()
        } else {
            format!("--- a/{git_path}\n+++ b/{git_path}\n{patch}")
        };

        let mut child = Command::new("git")
            .current_dir(&self.cwd)
            .args(["apply", "--cached", "--recount", "--whitespace=nowarn", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| AppError::GitMissing)?;

        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(patch_with_header.as_bytes())
                .map_err(|error| {
                    AppError::Custom(format!("failed to write patch to git: {}", error))
                })?;
        }

        let output = child
            .wait_with_output()
            .map_err(|error| AppError::Custom(format!("failed to apply patch: {}", error)))?;

        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
        }

        // Patch failed — fall back to staging the whole file.
        // This is safe because the commit plan groups already scope what gets committed;
        // staging the full file here is better than aborting the whole commit plan.
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let lower = stderr.to_lowercase();
        if lower.contains("permission denied") || lower.contains("access denied") {
            return Err(AppError::GitPermissionDenied(stderr));
        }

        // Try whole-file add as fallback
        self.add_paths(&[git_path])
            .map_err(|_| AppError::GitCommand {
                args: "git apply --cached --recount --whitespace=nowarn -".to_string(),
                stderr,
            })
    }

    pub fn commit(&self, message: &CommitMessage) -> Result<String> {
        self.output(["commit", "-m", &message.title(), "-m", message.body.trim()])
    }

    pub fn commit_index(&self, message: &CommitMessage) -> Result<String> {
        self.output(["commit", "-m", &message.title(), "-m", message.body.trim()])
    }

    pub fn push_current(&self) -> Result<String> {
        let branch = self.current_branch()?;
        if self.has_upstream()? {
            self.output_push(["push"])
        } else {
            self.output_push(["push", "--set-upstream", "origin", &branch])
        }
    }

    pub fn commit_log(&self, limit: usize) -> Result<Vec<CommitLogEntry>> {
        let limit = limit.clamp(1, 100).to_string();
        let output = self.output([
            "log",
            "--date=relative",
            "--decorate=short",
            "--format=%H%x1f%h%x1f%D%x1f%s%x1f%an%x1f%cr",
            "-n",
            &limit,
        ])?;
        Ok(parse_commit_log(&output))
    }

    pub fn commit_log_between(&self, base: &str, head: &str) -> Result<Vec<CommitLogEntry>> {
        let range = format!("{}..{}", base, head);
        let output = self.output([
            "log",
            "--date=relative",
            "--decorate=short",
            "--format=%H%x1f%h%x1f%D%x1f%s%x1f%an%x1f%cr",
            &range,
        ])?;
        Ok(parse_commit_log(&output))
    }

    pub fn can_merge(&self, base: &str) -> bool {
        let status = Command::new("git")
            .current_dir(&self.cwd)
            .args(["merge-tree", base, "HEAD"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        match status {
            Ok(s) => s.success(),
            Err(_) => false,
        }
    }

    pub fn diff_between(
        &self,
        base: &str,
        head: &str,
        max_chars: usize,
        context_lines: u32,
    ) -> Result<DiffContext> {
        let u_arg = format!("-U{}", context_lines);
        let range = format!("{}...{}", base, head);
        let status = self.output(["diff", "--name-status", &range])?;
        let stat = self.output(["diff", "--stat", &range])?;
        let diff = self.output(["diff", &u_arg, &range])?;
        let (diff, truncated) = truncate_diff(diff, max_chars);
        Ok(DiffContext {
            status,
            stat,
            diff,
            truncated,
        })
    }

    pub fn reset_soft_to(&self, hash: &str) -> Result<String> {
        self.output(["reset", "--soft", hash])
    }

    pub fn fetch_origin(&self) -> Result<String> {
        self.output(["fetch", "origin", "--prune"])
    }

    pub fn remote_branches(&self) -> Result<Vec<String>> {
        let output = self.output(["branch", "-r"])?;
        let mut branches = output
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("origin/"))
            .filter(|line| !line.contains("HEAD ->"))
            .map(|line| line.trim_start_matches("origin/").to_string())
            .collect::<Vec<_>>();
        branches.sort();
        branches.dedup();
        Ok(branches)
    }

    pub fn switch_branches(&self) -> Result<SwitchBranches> {
        self.ensure_repo()?;
        let current = self.current_branch().ok();
        Ok(SwitchBranches {
            remote: if self.has_origin() {
                self.branch_refs("refs/remotes/origin", BranchSource::Remote, None)?
            } else {
                Vec::new()
            },
            local: self.branch_refs("refs/heads", BranchSource::Local, current.as_deref())?,
        })
    }

    pub fn switch_to_branch(&self, branch: &BranchOption) -> Result<String> {
        self.ensure_repo()?;
        let output = match branch.source {
            BranchSource::Local => self.output(["checkout", &branch.name])?,
            BranchSource::Remote => {
                if self.local_branch_exists(&branch.name)? {
                    self.output(["checkout", &branch.name])?
                } else {
                    let remote_ref = format!("origin/{}", branch.name);
                    self.output(["checkout", "-b", &branch.name, "--track", &remote_ref])?
                }
            }
        };
        self.invalidate_status_cache();
        Ok(output)
    }

    pub fn create_and_switch_branch(&self, name: &str) -> Result<String> {
        self.ensure_repo()?;
        let branch = name.trim();
        if branch.is_empty() {
            return Err(AppError::Custom("Branch name is required.".to_string()));
        }
        let output = self.output(["checkout", "-b", branch])?;
        self.invalidate_status_cache();
        Ok(output)
    }

    fn invalidate_status_cache(&self) {
        self.cache.lock().expect("git cache lock poisoned").status = None;
    }

    pub fn diff_context(
        &self,
        staged_only: bool,
        max_chars: usize,
        context_lines: u32,
    ) -> Result<DiffContext> {
        let u_arg = format!("-U{}", context_lines);
        let status = if staged_only {
            self.output(["diff", "--cached", "--name-status"])?
        } else {
            self.output(["status", "--short"])?
        };
        let stat = if staged_only {
            self.output(["diff", "--cached", "--stat"])?
        } else {
            self.output(["diff", "--stat"])?
        };
        let mut diff = if staged_only {
            self.output(["diff", "--cached", &u_arg])?
        } else {
            self.output(["diff", &u_arg])?
        };

        if !staged_only {
            let untracked = self
                .output(["ls-files", "--others", "--exclude-standard"])
                .unwrap_or_default();
            diff = self.append_untracked_diffs(diff, &untracked);
        }

        let (diff, truncated) = truncate_diff(diff, max_chars);
        Ok(DiffContext {
            status,
            stat,
            diff,
            truncated,
        })
    }

    pub fn branch_diff_context(
        &self,
        branch: &str,
        max_chars: usize,
        context_lines: u32,
    ) -> Result<DiffContext> {
        let u_arg = format!("-U{}", context_lines);
        let status = self.output(["diff", "--name-status", branch])?;
        let stat = self.output(["diff", "--stat", branch])?;
        let diff = self.output(["diff", &u_arg, branch])?;

        let (diff, truncated) = truncate_diff(diff, max_chars);
        Ok(DiffContext {
            status,
            stat,
            diff,
            truncated,
        })
    }

    pub fn all_context(&self, max_chars: usize, context_lines: u32) -> Result<DiffContext> {
        let u_arg = format!("-U{}", context_lines);
        let status = self.output(["status", "--porcelain"])?;
        let stat = self.output(["status", "--short"])?;
        let staged = self
            .output(["diff", "--cached", &u_arg])
            .unwrap_or_default();
        let unstaged = self.output(["diff", &u_arg]).unwrap_or_default();
        let untracked = self
            .output(["ls-files", "--others", "--exclude-standard"])
            .unwrap_or_default();

        let mut diff = String::new();
        if !staged.is_empty() {
            diff.push_str("STAGED:\n");
            diff.push_str(&staged);
            diff.push('\n');
        }
        if !unstaged.is_empty() {
            diff.push_str("UNSTAGED:\n");
            diff.push_str(&unstaged);
            diff.push('\n');
        }
        if !untracked.is_empty() {
            diff.push_str("UNTRACKED:\n");
            diff.push_str(&untracked);
            diff.push('\n');
            diff = self.append_untracked_diffs(diff, &untracked);
        }

        let (diff, truncated) = truncate_diff(diff, max_chars);
        Ok(DiffContext {
            status,
            stat,
            diff,
            truncated,
        })
    }

    fn append_untracked_diffs(&self, mut diff: String, untracked_list: &str) -> String {
        let mut added_header = false;
        for file_path in untracked_list.lines() {
            let file_path = file_path.trim();
            if file_path.is_empty() {
                continue;
            }
            let full_path = self.cwd.join(file_path);
            if is_text_file_and_small(&full_path) {
                if !added_header {
                    diff.push_str("\nUNTRACKED FILES CONTENT:\n");
                    added_header = true;
                }
                if let Ok(content) = std::fs::read_to_string(&full_path) {
                    diff.push_str(&format!("--- /dev/null\n+++ b/{}\n", file_path));
                    for line in content.lines() {
                        diff.push('+');
                        diff.push_str(line);
                        diff.push('\n');
                    }
                }
            }
        }
        diff
    }

    fn output<const N: usize>(&self, args: [&str; N]) -> Result<String> {
        let output = Command::new("git")
            .current_dir(&self.cwd)
            .args(args)
            .output()
            .map_err(|_| AppError::GitMissing)?;
        command_stdout(
            output.status,
            output.stdout,
            output.stderr,
            &format!("git {}", args.join(" ")),
        )
    }

    fn output_vec(&self, args: Vec<String>) -> Result<String> {
        let output = Command::new("git")
            .current_dir(&self.cwd)
            .args(&args)
            .output()
            .map_err(|_| AppError::GitMissing)?;
        command_stdout(
            output.status,
            output.stdout,
            output.stderr,
            &format!("git {}", args.join(" ")),
        )
    }

    fn output_push<const N: usize>(&self, args: [&str; N]) -> Result<String> {
        self.output(args).map_err(classify_push_error)
    }

    pub fn read_file(&self, rel_path: &str) -> Option<String> {
        let full_path = self.cwd.join(rel_path);
        std::fs::read_to_string(full_path).ok()
    }

    fn branch_refs(
        &self,
        ref_prefix: &str,
        source: BranchSource,
        current_branch: Option<&str>,
    ) -> Result<Vec<BranchOption>> {
        let output = self.output_vec(vec![
            "for-each-ref".to_string(),
            "--format=%(refname:short)\t%(committerdate:unix)".to_string(),
            ref_prefix.to_string(),
        ])?;
        let mut branches = output
            .lines()
            .filter_map(|line| parse_branch_ref_line(line, source, current_branch))
            .collect::<Vec<_>>();
        sort_branch_options(&mut branches);
        Ok(branches)
    }

    fn local_branch_exists(&self, branch: &str) -> Result<bool> {
        let ref_name = format!("refs/heads/{}", branch);
        let status = Command::new("git")
            .current_dir(&self.cwd)
            .args(["show-ref", "--verify", "--quiet", &ref_name])
            .status()
            .map_err(|_| AppError::GitMissing)?;
        if status.success() {
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

fn validate_relative_paths(paths: &[String]) -> Result<Vec<String>> {
    if paths.is_empty() {
        return Err(AppError::Custom(
            "commit group does not contain any files".to_string(),
        ));
    }

    let mut validated = Vec::with_capacity(paths.len());
    for path in paths {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(AppError::Custom(
                "commit group contains an empty file path".to_string(),
            ));
        }

        let path_obj = Path::new(trimmed);
        if path_obj.is_absolute() {
            return Err(AppError::Custom(format!(
                "commit group path must be relative: {}",
                trimmed
            )));
        }
        if path_obj.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(AppError::Custom(format!(
                "commit group path cannot leave the repository: {}",
                trimmed
            )));
        }

        validated.push(trimmed.replace('\\', "/"));
    }
    Ok(validated)
}

fn repo_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "workspace".to_string())
}

fn parse_branch_ref_line(
    line: &str,
    source: BranchSource,
    current_branch: Option<&str>,
) -> Option<BranchOption> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut parts = trimmed.splitn(2, '\t');
    let raw_name = parts.next()?.trim();
    let timestamp = parts
        .next()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0);

    let name = match source {
        BranchSource::Local => raw_name.to_string(),
        BranchSource::Remote => {
            if raw_name == "origin"
                || raw_name == "origin/HEAD"
                || raw_name.starts_with("origin/HEAD ->")
            {
                return None;
            }
            raw_name.trim_start_matches("origin/").to_string()
        }
    };

    if name.is_empty() {
        return None;
    }

    Some(BranchOption {
        is_current: matches!(source, BranchSource::Local) && current_branch == Some(name.as_str()),
        last_commit_unix: timestamp,
        name,
        source,
    })
}

fn sort_branch_options(branches: &mut [BranchOption]) {
    branches.sort_by(|left, right| {
        right
            .last_commit_unix
            .cmp(&left.last_commit_unix)
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn is_protected_branch_name(branch: &str) -> bool {
    matches!(branch.trim(), "main" | "master")
}

fn parse_working_tree_sync(output: &str) -> WorkingTreeSync {
    let mut sync = WorkingTreeSync::default();
    for line in output.lines() {
        if let Some(branch) = line.strip_prefix("## ") {
            sync.has_upstream = branch.contains("...");
            if let Some(metadata) = branch
                .split('[')
                .nth(1)
                .and_then(|part| part.split(']').next())
            {
                for item in metadata.split(',') {
                    let item = item.trim();
                    if let Some(value) = item.strip_prefix("ahead ") {
                        sync.ahead = value.parse().unwrap_or(0);
                    } else if let Some(value) = item.strip_prefix("behind ") {
                        sync.behind = value.parse().unwrap_or(0);
                    }
                }
            }
        } else if !line.trim().is_empty() {
            sync.has_changes = true;
        }
    }
    sync
}

fn parse_commit_log(output: &str) -> Vec<CommitLogEntry> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\x1f');
            let hash = parts.next()?.trim();
            let short_hash = parts.next()?.trim();
            let decorations = parts.next().unwrap_or("").trim();
            let subject = parts.next().unwrap_or("").trim();
            let author = parts.next().unwrap_or("").trim();
            let relative_time = parts.next().unwrap_or("").trim();
            if hash.is_empty() || short_hash.is_empty() {
                return None;
            }
            Some(CommitLogEntry {
                hash: hash.to_string(),
                short_hash: short_hash.to_string(),
                decorations: decorations.to_string(),
                subject: subject.to_string(),
                author: author.to_string(),
                relative_time: relative_time.to_string(),
            })
        })
        .collect()
}

fn classify_push_error(error: AppError) -> AppError {
    let AppError::GitCommand { args, stderr } = error else {
        return error;
    };

    let normalized = stderr.to_lowercase();
    if normalized.contains("authentication failed")
        || normalized.contains("permission denied")
        || normalized.contains("could not read username")
        || normalized.contains("repository not found")
        || normalized.contains("403")
        || normalized.contains("401")
    {
        return AppError::GitPushAuth { args, stderr };
    }
    if normalized.contains("no upstream branch") {
        return AppError::GitPushNoUpstream { args, stderr };
    }
    if normalized.contains("remote origin does not exist") || normalized.contains("no such remote")
    {
        return AppError::OriginMissing;
    }

    AppError::GitCommand { args, stderr }
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

    let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
    let lower = stderr.to_lowercase();
    if lower.contains("permission denied") || lower.contains("access denied") {
        return Err(AppError::GitPermissionDenied(stderr));
    }

    Err(AppError::GitCommand {
        args: command.to_string(),
        stderr,
    })
}

fn is_text_file_and_small(path: &Path) -> bool {
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if !metadata.is_file() || metadata.len() > 16_384 {
        return false;
    }
    // Read first 1024 bytes to check for null bytes (indicating binary content)
    use std::io::Read;
    if let Ok(mut file) = std::fs::File::open(path) {
        let mut buffer = [0; 1024];
        if let Ok(bytes_read) = file.read(&mut buffer)
            && buffer[..bytes_read].contains(&0)
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn git_in(path: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(path)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
        if args.first() == Some(&"init") {
            let _ = Command::new("git")
                .current_dir(path)
                .args(["config", "commit.gpgsign", "false"])
                .status();
        }
    }

    fn git_output_in(path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {:?} failed", args);
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn write_file(path: &Path, name: &str, body: &str) {
        std::fs::write(path.join(name), body).unwrap();
    }

    #[test]
    fn parses_clean_synced_branch_status() {
        let sync = parse_working_tree_sync("## main...origin/main\n");

        assert!(!sync.has_changes);
        assert!(sync.has_upstream);
        assert_eq!(sync.ahead, 0);
        assert_eq!(sync.behind, 0);
    }

    #[test]
    fn parses_ahead_behind_and_local_changes() {
        let sync = parse_working_tree_sync(
            "## feature/api...origin/feature/api [ahead 2, behind 1]\n M src/main.rs\n?? docs/new.md\n",
        );

        assert!(sync.has_changes);
        assert!(sync.has_upstream);
        assert_eq!(sync.ahead, 2);
        assert_eq!(sync.behind, 1);
    }

    #[test]
    fn parses_branch_without_upstream() {
        let sync = parse_working_tree_sync("## local-only\n");

        assert!(!sync.has_changes);
        assert!(!sync.has_upstream);
    }

    #[test]
    fn parses_decorated_commit_log_entries() {
        let entries = parse_commit_log(
            "abc123456789\x1fabc1234\x1fHEAD -> main, origin/main\x1fadd log modal\x1fAda\x1f2 minutes ago\n",
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].hash, "abc123456789");
        assert_eq!(entries[0].short_hash, "abc1234");
        assert_eq!(entries[0].decorations, "HEAD -> main, origin/main");
        assert_eq!(entries[0].subject, "add log modal");
        assert_eq!(entries[0].author, "Ada");
        assert_eq!(entries[0].relative_time, "2 minutes ago");
    }

    fn git_commit_with_dates(path: &Path, message: &str, author_date: &str, committer_date: &str) {
        let status = Command::new("git")
            .current_dir(path)
            .env("GIT_AUTHOR_DATE", author_date)
            .env("GIT_COMMITTER_DATE", committer_date)
            .args(["commit", "-m", message])
            .status()
            .unwrap();
        assert!(status.success(), "git commit {message} failed");
    }

    #[test]
    fn classify_push_error_auth() {
        let err = classify_push_error(AppError::GitCommand {
            args: "push".to_string(),
            stderr: "authentication failed".to_string(),
        });
        assert!(matches!(err, AppError::GitPushAuth { .. }));
    }

    #[test]
    fn classify_push_error_no_upstream() {
        let err = classify_push_error(AppError::GitCommand {
            args: "push".to_string(),
            stderr: "no upstream branch".to_string(),
        });
        assert!(matches!(err, AppError::GitPushNoUpstream { .. }));
    }

    #[test]
    fn classify_push_error_generic() {
        let err = classify_push_error(AppError::GitCommand {
            args: "push".to_string(),
            stderr: "some other error".to_string(),
        });
        assert!(matches!(err, AppError::GitCommand { .. }));
    }

    #[test]
    fn repo_label_from_path() {
        let path = std::path::PathBuf::from("/home/user/my-repo");
        let label = repo_label(&path);
        assert_eq!(label, "my-repo");
    }

    #[test]
    fn validates_relative_paths() {
        let paths = vec!["src/main.rs".to_string(), "README.md".to_string()];
        let validated = validate_relative_paths(&paths).unwrap();
        assert_eq!(validated, paths);
    }

    #[test]
    fn rejects_paths_outside_repository() {
        let paths = vec!["../secret.txt".to_string()];
        assert!(validate_relative_paths(&paths).is_err());
    }

    #[test]
    fn parse_branch_ref_line_skips_origin_head_alias() {
        let parsed = parse_branch_ref_line("origin/HEAD\t1710000000", BranchSource::Remote, None);
        assert!(parsed.is_none());
    }

    #[test]
    fn branch_option_marks_only_main_and_master_as_protected() {
        let main = BranchOption {
            name: "main".to_string(),
            source: BranchSource::Local,
            last_commit_unix: Some(1),
            is_current: false,
        };
        let master = BranchOption {
            name: "master".to_string(),
            source: BranchSource::Local,
            last_commit_unix: Some(1),
            is_current: false,
        };
        let feature = BranchOption {
            name: "main-fix".to_string(),
            source: BranchSource::Local,
            last_commit_unix: Some(1),
            is_current: false,
        };

        assert!(main.is_protected());
        assert!(master.is_protected());
        assert!(!feature.is_protected());
    }

    #[test]
    fn switch_branches_orders_sections_by_latest_commit_then_name() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        git_in(dir.path(), &["config", "user.name", "Tester"]);
        git_in(dir.path(), &["config", "user.email", "tester@example.com"]);

        write_file(dir.path(), "shared.txt", "one\n");
        git_in(dir.path(), &["add", "."]);
        git_commit_with_dates(
            dir.path(),
            "base",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        );

        git_in(dir.path(), &["checkout", "-b", "alpha"]);
        write_file(dir.path(), "shared.txt", "two\n");
        git_in(dir.path(), &["add", "."]);
        git_commit_with_dates(
            dir.path(),
            "alpha",
            "2026-01-03T00:00:00Z",
            "2026-01-03T00:00:00Z",
        );

        git_in(dir.path(), &["checkout", "main"]);
        git_in(dir.path(), &["checkout", "-b", "zeta"]);
        write_file(dir.path(), "shared.txt", "three\n");
        git_in(dir.path(), &["add", "."]);
        git_commit_with_dates(
            dir.path(),
            "zeta",
            "2026-01-02T00:00:00Z",
            "2026-01-02T00:00:00Z",
        );

        git_in(dir.path(), &["checkout", "main"]);
        git_in(
            dir.path(),
            &["remote", "add", "origin", "https://example.com/repo.git"],
        );
        git_in(
            dir.path(),
            &["update-ref", "refs/remotes/origin/main", "main"],
        );
        git_in(
            dir.path(),
            &["update-ref", "refs/remotes/origin/alpha", "alpha"],
        );
        git_in(
            dir.path(),
            &["update-ref", "refs/remotes/origin/zeta", "zeta"],
        );

        let git = Git::new(dir.path().to_path_buf());
        let branches = git.switch_branches().unwrap();

        assert_eq!(
            branches
                .local
                .iter()
                .map(|branch| branch.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta", "main"]
        );
        assert_eq!(
            branches
                .remote
                .iter()
                .map(|branch| branch.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta", "main"]
        );
    }

    #[test]
    fn switch_to_remote_branch_creates_tracking_local_branch() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        git_in(dir.path(), &["config", "user.name", "Tester"]);
        git_in(dir.path(), &["config", "user.email", "tester@example.com"]);
        write_file(dir.path(), "file.txt", "base\n");
        git_in(dir.path(), &["add", "."]);
        git_in(dir.path(), &["commit", "-m", "base"]);

        git_in(dir.path(), &["checkout", "-b", "feature"]);
        write_file(dir.path(), "file.txt", "feature\n");
        git_in(dir.path(), &["add", "."]);
        git_in(dir.path(), &["commit", "-m", "feature"]);
        git_in(dir.path(), &["checkout", "main"]);

        git_in(
            dir.path(),
            &["remote", "add", "origin", "https://example.com/repo.git"],
        );
        git_in(
            dir.path(),
            &["update-ref", "refs/remotes/origin/main", "main"],
        );
        git_in(
            dir.path(),
            &["update-ref", "refs/remotes/origin/feature", "feature"],
        );
        git_in(dir.path(), &["branch", "-D", "feature"]);

        let git = Git::new(dir.path().to_path_buf());
        let remote_feature = git
            .switch_branches()
            .unwrap()
            .remote
            .into_iter()
            .find(|branch| branch.name == "feature")
            .unwrap();

        git.switch_to_branch(&remote_feature).unwrap();

        assert_eq!(git.current_branch().unwrap(), "feature");
        let upstream = git_output_in(
            dir.path(),
            &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        );
        assert_eq!(upstream, "origin/feature");
    }

    #[test]
    fn test_can_merge_and_diff_between() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        git_in(dir.path(), &["config", "user.name", "Tester"]);
        git_in(dir.path(), &["config", "user.email", "tester@example.com"]);
        write_file(dir.path(), "file.txt", "base content\n");
        git_in(dir.path(), &["add", "."]);
        git_in(dir.path(), &["commit", "-m", "commit 1"]);

        git_in(dir.path(), &["checkout", "-b", "feature"]);
        write_file(dir.path(), "file.txt", "base content\nfeature content\n");
        git_in(dir.path(), &["add", "."]);
        git_in(dir.path(), &["commit", "-m", "commit 2"]);

        let git = Git::new(dir.path().to_path_buf());
        assert!(git.can_merge("main"));

        let diff = git.diff_between("main", "feature", 1000, 3).unwrap();
        assert!(diff.status.contains("M\tfile.txt") || diff.status.contains("M file.txt"));
        assert!(diff.diff.contains("+feature content"));

        // Create conflict
        git_in(dir.path(), &["checkout", "main"]);
        write_file(dir.path(), "file.txt", "conflicting content\n");
        git_in(dir.path(), &["add", "."]);
        git_in(dir.path(), &["commit", "-m", "commit 3"]);

        git_in(dir.path(), &["checkout", "feature"]);
        assert!(!git.can_merge("main"));
    }
}
