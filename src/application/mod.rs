use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

use reqwest::Client;
use tokio::sync::mpsc;

use crate::domain::commit::{
    CommitGroup, FileEntry, commit_plan_prompt, explain_prompt, pr_prompt,
    resolve_conflict_prompt, review_prompt, scout_question_prompt, status_prompt,
};
use crate::domain::{
    CommitMessage, CommitPlan, Config, DiffContext, Intent, LlmContextUsage, LlmProviderKind,
    PrInfo, PullRequestDraft,
};
use crate::error::{AppError, Result};
use crate::infrastructure::{
    BranchOption, CommitLogEntry, ConfigRepository, DependencyDoctor, DependencyKind,
    DependencyStatus, Git, GitHubCli, LlmClient, OllamaClient, PlatformInfo, RepoStatus,
    SecretsRepository, SwitchBranches,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaHealth {
    pub installed: bool,
    pub running: bool,
    pub version: Option<String>,
    pub install_message: Option<String>,
    pub runtime_message: Option<String>,
}

impl OllamaHealth {
    pub fn ready(version: String) -> Self {
        Self {
            installed: true,
            running: true,
            version: Some(version),
            install_message: None,
            runtime_message: None,
        }
    }

    pub fn not_installed(message: String) -> Self {
        Self {
            installed: false,
            running: false,
            version: None,
            install_message: Some(message),
            runtime_message: None,
        }
    }

    pub fn not_running(version: Option<String>, message: String) -> Self {
        Self {
            installed: true,
            running: false,
            version,
            install_message: None,
            runtime_message: Some(message),
        }
    }
}

#[derive(Clone)]
pub struct AppCore {
    pub config: Config,
    pub vram_mb: Option<usize>,
    git: Git,
    github: GitHubCli,
    llm: LlmClient,
    ollama: OllamaClient,
}

impl AppCore {
    pub fn new(cwd: PathBuf, config: Config, client: Client) -> Self {
        Self {
            config,
            vram_mb: None,
            git: Git::new(cwd.clone()),
            github: GitHubCli::new(cwd),
            llm: LlmClient::new(client.clone()),
            ollama: OllamaClient::new(client),
        }
    }

    pub fn save_config(&self) -> Result<()> {
        ConfigRepository::save(&self.config)
    }

    pub fn status(&self) -> RepoStatus {
        self.git.status()
    }

    pub async fn preflight_messages(&self) -> Vec<String> {
        let mut messages = Vec::new();
        let lang = self.config.language.to_lowercase();
        let lang_str = lang.trim();

        if self.git.ensure_installed().is_err() {
            return messages;
        }
        if !self.git.is_repo() {
            let msg = match lang_str {
                "spanish" | "español" | "espanol" => {
                    "Este directorio no es un repositorio Git. Usa /setup."
                }
                _ => "This directory is not a Git repository. Use /setup.",
            };
            messages.push(msg.to_string());
        } else if !self.git.has_origin() {
            let msg = match lang_str {
                "spanish" | "español" | "espanol" => {
                    "Este repositorio no tiene un servidor remoto origin. Usa /setup."
                }
                _ => "This repository has no origin remote. Use /setup.",
            };
            messages.push(msg.to_string());
        }
        messages
    }

    pub fn ensure_repo(&self) -> Result<()> {
        self.git.ensure_installed()?;
        self.git.ensure_repo()
    }

    pub fn ensure_origin(&self) -> Result<()> {
        if self.git.has_origin() {
            Ok(())
        } else {
            Err(AppError::OriginMissing)
        }
    }

    pub fn init_repo(&self) -> Result<String> {
        self.git.ensure_installed()?;
        self.git.init()
    }

    pub fn add_origin(&self, url: &str) -> Result<String> {
        self.git.ensure_repo()?;
        self.git.add_origin(url)
    }

    pub fn reset_safe(&mut self) -> Result<bool> {
        SecretsRepository::clear_api_keys()?;
        let removed_origin = if self.git.is_repo() {
            self.git.remove_origin_if_exists()?
        } else {
            false
        };
        self.config = Config::default();
        self.save_config()?;
        Ok(removed_origin)
    }

    pub fn create_github_repo(&self, name: &str, private: bool) -> Result<String> {
        if !self.git.is_repo() {
            self.git.init()?;
        }
        self.github.create_repo(name, private)
    }

    pub async fn models(&self) -> Result<Vec<String>> {
        let models = self
            .llm
            .models(&self.config, self.api_key()?.as_deref())
            .await?;
        if models.is_empty() {
            Err(AppError::NoOllamaModels)
        } else {
            Ok(models)
        }
    }

    pub async fn provider_health(&self) -> OllamaHealth {
        if !self.config.provider.is_selected() {
            return OllamaHealth::not_running(None, "No AI provider selected.".to_string());
        }
        if self.config.provider == LlmProviderKind::Ollama {
            let installed_version = match detect_ollama_installation() {
                Ok(version) => version,
                Err(message) => return OllamaHealth::not_installed(message),
            };

            return match self.ollama.version().await {
                Ok(version) => OllamaHealth::ready(version),
                Err(error) => OllamaHealth::not_running(Some(installed_version), error.to_string()),
            };
        }

        match self
            .llm
            .health(&self.config, self.api_key().ok().flatten().as_deref())
            .await
        {
            Ok(detail) => OllamaHealth::ready(detail),
            Err(AppError::MissingApiKey { provider })
            | Err(AppError::MissingBaseUrl { provider })
            | Err(AppError::MissingModel { provider }) => {
                OllamaHealth::not_running(None, format!("{} is not fully configured.", provider))
            }
            Err(error) => OllamaHealth::not_running(None, error.to_string()),
        }
    }

    pub async fn dependency_doctor(&self) -> DependencyDoctor {
        let provider = self.provider_dependency_status().await;
        DependencyDoctor::gather(&self.ollama, provider).await
    }

    pub fn set_model(&mut self, model: String) -> Result<()> {
        self.config.model = Some(model);
        self.save_config()
    }

    pub fn set_provider(&mut self, provider: LlmProviderKind) -> Result<()> {
        self.config.provider = provider;
        if provider == LlmProviderKind::Ollama {
            if self
                .config
                .base_url
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                self.config.base_url = provider.default_base_url().map(str::to_string);
            }
        } else {
            self.config.base_url = None;
        }
        self.save_config()
    }

    pub fn set_base_url(&mut self, base_url: String) -> Result<()> {
        let trimmed = base_url.trim();
        self.config.base_url = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.trim_end_matches('/').to_string())
        };
        self.save_config()
    }

    pub fn set_api_key(&self, api_key: &str) -> Result<()> {
        if !self.config.provider.is_selected() {
            return Err(AppError::ProviderNotSelected);
        }
        SecretsRepository::save_api_key(self.config.provider, api_key)
    }

    pub fn api_key_configured(&self) -> Result<bool> {
        if !self.config.provider.is_selected() {
            return Ok(false);
        }
        Ok(self.api_key()?.is_some())
    }

    pub fn provider_label(&self) -> &'static str {
        self.config.provider.label()
    }

    pub fn set_language(&mut self, language: String) -> Result<()> {
        self.config.language = language;
        self.save_config()
    }

    pub fn toggle_staged(&mut self) -> Result<bool> {
        self.config.staged_only = !self.config.staged_only;
        self.save_config()?;
        Ok(self.config.staged_only)
    }

    pub fn diff_limits(&self) -> (usize, u32) {
        match self.vram_mb {
            None => (12_000, 1),
            Some(vram) if vram < 3000 => (8_000, 1),
            Some(vram) if vram < 6000 => (16_000, 2),
            Some(vram) if vram < 10000 => (24_000, 2),
            _ => (40_000, 3),
        }
    }

    pub fn calculate_num_ctx(&self, prompt_len: usize) -> Option<usize> {
        let prompt_tokens = (prompt_len / 3) + 200;
        let max_ctx = match self.vram_mb {
            None => 2048,
            Some(vram) if vram < 3000 => 2048,
            Some(vram) if vram < 6000 => 4096,
            Some(vram) if vram < 10000 => 8192,
            _ => 16384,
        };
        let required_ctx = prompt_tokens + 1024;
        Some(required_ctx.clamp(2048, max_ctx))
    }

    pub fn estimate_context_usage(&self, prompt_len: usize, truncated: bool) -> LlmContextUsage {
        let estimated_tokens = (prompt_len / 3) + 200;
        let limit = self.calculate_num_ctx(prompt_len).unwrap_or(0);
        LlmContextUsage {
            estimated_tokens,
            limit,
            truncated,
        }
    }

    pub fn diff_context(&self) -> Result<DiffContext> {
        self.ensure_repo()?;
        let (max_chars, context_lines) = self.diff_limits();
        self.git
            .diff_context(self.config.staged_only, max_chars, context_lines)
    }

    pub fn staged_context(&self) -> Result<DiffContext> {
        self.ensure_repo()?;
        let (max_chars, context_lines) = self.diff_limits();
        self.git.diff_context(true, max_chars, context_lines)
    }

    pub async fn draft_commit_plan(&mut self) -> Result<(CommitPlan, LlmContextUsage)> {
        self.ensure_commit_repo()?;
        let (max_chars, context_lines) = self.diff_limits();
        let context = if self.config.staged_only {
            self.staged_context()?
        } else {
            self.git.all_context(max_chars, context_lines)?
        };
        if context.is_empty() {
            return Err(AppError::NoChanges);
        }
        let model = self.ensure_model().await?;
        let prompt = commit_plan_prompt(&self.config.language, &context);
        let usage = self.estimate_context_usage(prompt.len(), context.truncated);
        let response = self
            .llm
            .generate_json(
                &self.config,
                self.api_key()?.as_deref(),
                &model,
                &prompt,
                Some(usage.limit),
            )
            .await?;
        let mut plan = CommitPlan::from_llm_response(&response)?;
        complete_commit_plan(&mut plan, &context);
        Ok((plan, usage))
    }

    fn ensure_commit_repo(&self) -> Result<()> {
        self.git.ensure_installed()?;
        if self.git.is_repo() {
            return Ok(());
        }
        if self.config.auto_setup_repo {
            self.git.init()?;
            Ok(())
        } else {
            self.git.ensure_repo()
        }
    }

    pub fn create_commit(&self, message: &CommitMessage) -> Result<String> {
        self.ensure_repo()?;
        self.git.ensure_identity()?;
        if !self.config.staged_only {
            self.git.add_all()?;
        }
        let (max_chars, context_lines) = self.diff_limits();
        let staged = self.git.diff_context(true, max_chars, context_lines)?;
        if staged.is_empty() {
            return Err(AppError::NoChanges);
        }
        self.git.commit(message)
    }

    pub fn execute_commit_plan(&self, plan: &CommitPlan) -> Result<String> {
        self.ensure_repo()?;
        self.git.ensure_identity()?;

        let mut outputs = Vec::new();
        let (max_chars, context_lines) = self.diff_limits();
        let mut plan = plan.clone();
        let context = if self.config.staged_only {
            self.staged_context()?
        } else {
            self.git.all_context(max_chars, context_lines)?
        };
        complete_commit_plan(&mut plan, &context);
        dedupe_commit_plan_files(&mut plan);

        for group in &plan.groups {
            if group.files.is_empty() {
                continue;
            }
            self.git.reset_index()?;

            let paths = group
                .files
                .iter()
                .filter(|file| file.patch.as_deref().is_none_or(str::is_empty))
                .map(|file| file.path.clone())
                .collect::<Vec<_>>();
            if !paths.is_empty() {
                self.git.add_paths(&paths)?;
            }
            for file_entry in group
                .files
                .iter()
                .filter(|f| f.patch.as_deref().is_some_and(|p| !p.trim().is_empty()))
            {
                if let Some(patch) = &file_entry.patch {
                    self.git.apply_patch_to_index(&file_entry.path, patch)?;
                }
            }

            let staged = self.git.diff_context(true, max_chars, context_lines)?;
            if staged.is_empty() {
                let _ = self.git.reset_index();
                return Err(AppError::NoChanges);
            }

            match self.git.commit_index(&group.commit) {
                Ok(output) => {
                    outputs.push(format!(
                        "{}. {}",
                        outputs.len() + 1,
                        if output.trim().is_empty() {
                            group.commit.title()
                        } else {
                            output
                        }
                    ));
                }
                Err(error) => {
                    let _ = self.git.reset_index();
                    return Err(error);
                }
            }
        }

        if !self.config.staged_only && !self.git.all_context(max_chars, context_lines)?.is_empty() {
            self.git.reset_index()?;
            self.git.add_all()?;
            let fallback = CommitMessage {
                commit_type: "chore".to_string(),
                scope: String::new(),
                subject: "include remaining changes".to_string(),
                body:
                    "Add files that were still pending after executing the generated commit plan."
                        .to_string(),
            };
            match self.git.commit_index(&fallback) {
                Ok(output) => outputs.push(format!(
                    "{}. {}",
                    outputs.len() + 1,
                    if output.trim().is_empty() {
                        fallback.title()
                    } else {
                        output
                    }
                )),
                Err(error) => {
                    let _ = self.git.reset_index();
                    return Err(error);
                }
            }
        }

        if outputs.is_empty() {
            return Err(AppError::NoChanges);
        }

        Ok(outputs.join("\n"))
    }

    pub fn push(&self) -> Result<String> {
        self.ensure_repo()?;
        self.ensure_origin()?;
        self.git.push_current()
    }

    pub async fn explain(&mut self) -> Result<String> {
        let context = self.diff_context()?;
        if context.is_empty() {
            return Err(AppError::NoChanges);
        }
        let model = self.ensure_model().await?;
        let prompt = explain_prompt(&self.config.language, &context);
        let num_ctx = self.calculate_num_ctx(prompt.len());
        self.llm
            .generate(
                &self.config,
                self.api_key()?.as_deref(),
                &model,
                &prompt,
                num_ctx,
            )
            .await
    }

    pub async fn review(&mut self) -> Result<String> {
        let context = self.diff_context()?;
        if context.is_empty() {
            return Err(AppError::NoChanges);
        }
        let model = self.ensure_model().await?;
        let prompt = review_prompt(&self.config.language, &context);
        let num_ctx = self.calculate_num_ctx(prompt.len());
        self.llm
            .generate(
                &self.config,
                self.api_key()?.as_deref(),
                &model,
                &prompt,
                num_ctx,
            )
            .await
    }

    pub async fn explain_stream(&mut self, tx: mpsc::UnboundedSender<String>) -> Result<String> {
        let context = self.diff_context()?;
        if context.is_empty() {
            return Err(AppError::NoChanges);
        }
        let model = self.ensure_model().await?;
        let prompt = explain_prompt(&self.config.language, &context);
        let num_ctx = self.calculate_num_ctx(prompt.len());
        self.llm
            .generate_stream(
                &self.config,
                self.api_key()?.as_deref(),
                &model,
                &prompt,
                num_ctx,
                tx,
            )
            .await
    }

    pub async fn review_stream(&mut self, tx: mpsc::UnboundedSender<String>) -> Result<String> {
        let context = self.diff_context()?;
        if context.is_empty() {
            return Err(AppError::NoChanges);
        }
        let model = self.ensure_model().await?;
        let prompt = review_prompt(&self.config.language, &context);
        let num_ctx = self.calculate_num_ctx(prompt.len());
        self.llm
            .generate_stream(
                &self.config,
                self.api_key()?.as_deref(),
                &model,
                &prompt,
                num_ctx,
                tx,
            )
            .await
    }

    pub async fn status_summary_stream(
        &mut self,
        tx: mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        if let Some(message) = self.clean_status_summary()? {
            let _ = tx.send(message);
            return Ok(String::new());
        }

        let context = self.diff_context()?;
        if context.is_empty() {
            return Err(AppError::NoChanges);
        }
        let model = self.ensure_model().await?;
        let prompt = status_prompt(&self.config.language, &context);
        let num_ctx = self.calculate_num_ctx(prompt.len());
        self.llm
            .generate_stream(
                &self.config,
                self.api_key()?.as_deref(),
                &model,
                &prompt,
                num_ctx,
                tx,
            )
            .await
    }

    fn clean_status_summary(&self) -> Result<Option<String>> {
        self.ensure_repo()?;
        let sync = self.git.working_tree_sync()?;
        if sync.has_changes {
            return Ok(None);
        }

        let branch = self
            .git
            .current_branch()
            .unwrap_or_else(|_| "unknown".to_string());
        let is_spanish = matches!(
            self.config.language.to_lowercase().trim(),
            "spanish" | "español" | "espanol"
        );

        let message = if !sync.has_upstream {
            if is_spanish {
                format!(
                    "No hay cambios locales en `{branch}`. La rama no tiene upstream configurado; no usé IA."
                )
            } else {
                format!(
                    "No local changes on `{branch}`. This branch has no upstream configured; AI was not used."
                )
            }
        } else if sync.ahead == 0 && sync.behind == 0 {
            if is_spanish {
                format!(
                    "No hay cambios locales en `{branch}` y la rama está sincronizada con el remoto. No usé IA."
                )
            } else {
                format!(
                    "No local changes on `{branch}` and the branch is synced with remote. AI was not used."
                )
            }
        } else if sync.ahead > 0 && sync.behind > 0 {
            if is_spanish {
                format!(
                    "No hay cambios locales en `{branch}`, pero la rama diverge del remoto: {} commit(s) por subir y {} por traer. No usé IA.",
                    sync.ahead, sync.behind
                )
            } else {
                format!(
                    "No local changes on `{branch}`, but the branch diverged from remote: {} commit(s) to push and {} to pull. AI was not used.",
                    sync.ahead, sync.behind
                )
            }
        } else if sync.ahead > 0 {
            if is_spanish {
                format!(
                    "No hay cambios locales en `{branch}`, pero hay {} commit(s) pendientes de push. No usé IA.",
                    sync.ahead
                )
            } else {
                format!(
                    "No local changes on `{branch}`, but there are {} commit(s) pending push. AI was not used.",
                    sync.ahead
                )
            }
        } else if is_spanish {
            format!(
                "No hay cambios locales en `{branch}`, pero hay {} commit(s) pendientes de pull. No usé IA.",
                sync.behind
            )
        } else {
            format!(
                "No local changes on `{branch}`, but there are {} commit(s) pending pull. AI was not used.",
                sync.behind
            )
        };

        Ok(Some(message))
    }

    pub async fn scout_question_stream(
        &mut self,
        question: &str,
        tx: mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        let context = self.diff_context()?;
        if context.is_empty() {
            return Err(AppError::NoChanges);
        }
        let model = self.ensure_model().await?;
        let prompt = scout_question_prompt(&self.config.language, &context, question);
        let num_ctx = self.calculate_num_ctx(prompt.len());
        self.llm
            .generate_stream(
                &self.config,
                self.api_key()?.as_deref(),
                &model,
                &prompt,
                num_ctx,
                tx,
            )
            .await
    }

    pub async fn remote_branches_async(&self) -> Result<Vec<String>> {
        let git = self.git.clone();
        tokio::task::spawn_blocking(move || {
            git.ensure_repo()?;
            if !git.has_origin() {
                return Err(AppError::OriginMissing);
            }
            git.fetch_origin()?;
            let branches = git.remote_branches()?;
            if branches.is_empty() {
                Err(AppError::NoRemoteBranches)
            } else {
                Ok(branches)
            }
        })
        .await
        .map_err(|e| AppError::Custom(format!("Background task failed: {}", e)))?
    }

    pub async fn switch_branches_async(&self) -> Result<SwitchBranches> {
        let git = self.git.clone();
        tokio::task::spawn_blocking(move || {
            git.ensure_repo()?;
            if git.has_origin() {
                git.fetch_origin()?;
            }
            let branches = git.switch_branches()?;
            if branches.is_empty() {
                Err(AppError::Custom(
                    "No branches are available to switch.".to_string(),
                ))
            } else {
                Ok(branches)
            }
        })
        .await
        .map_err(|e| AppError::Custom(format!("Background task failed: {}", e)))?
    }

    pub fn switch_branch(&self, branch: &BranchOption) -> Result<String> {
        self.ensure_repo()?;
        self.git.switch_to_branch(branch)
    }

    pub fn commit_log(&self) -> Result<Vec<CommitLogEntry>> {
        self.ensure_repo()?;
        self.git.commit_log(30)
    }

    pub fn reset_soft_to_commit(&self, hash: &str) -> Result<String> {
        self.ensure_repo()?;
        self.git.reset_soft_to(hash)
    }

    pub fn create_and_switch_branch(&self, name: &str) -> Result<String> {
        self.ensure_repo()?;
        self.git.create_and_switch_branch(name)
    }

    pub async fn draft_pr(&mut self, base: &str) -> Result<PullRequestDraft> {
        let current = self.git.current_branch()?;
        let (max_chars, context_lines) = self.diff_limits();
        let context = if self.config.staged_only {
            self.staged_context()?
        } else {
            self.git.all_context(max_chars, context_lines)?
        };
        if context.is_empty() {
            return Ok(PullRequestDraft {
                title: format!("Merge {}", current),
                body: "Automated pull request created by Remix Autopilot.".to_string(),
            });
        }

        let template_paths = [
            ".github/pull_request_template.md",
            ".github/PULL_REQUEST_TEMPLATE.md",
            "pull_request_template.md",
            "PULL_REQUEST_TEMPLATE.md",
            ".github/pull_request_template.txt",
        ];
        let mut template = None;
        for path in &template_paths {
            if let Some(content) = self.git.read_file(path) {
                template = Some(content);
                break;
            }
        }

        let model = self.ensure_model().await?;
        let commits = self.git.commit_log_between(base, &current).unwrap_or_default();
        let commits_text = commits
            .iter()
            .map(|c| format!("{} - {}", c.short_hash, c.subject))
            .collect::<Vec<_>>()
            .join("\n");
        let prompt = pr_prompt(
            &self.config.language,
            &context,
            &commits_text,
            base,
            &current,
            template.as_deref(),
        );
        let num_ctx = self.calculate_num_ctx(prompt.len());
        let response = self
            .llm
            .generate(
                &self.config,
                self.api_key()?.as_deref(),
                &model,
                &prompt,
                num_ctx,
            )
            .await?;
        PullRequestDraft::from_llm_response(&response)
    }

    pub fn create_pr(&self, base: &str, draft: &PullRequestDraft) -> Result<String> {
        let head = self.git.current_branch()?;
        self.github.create_pr(base, &head, draft)
    }

    pub fn edit_pr(&self, number: i64, draft: &PullRequestDraft) -> Result<String> {
        self.github.edit_pr(number, draft)
    }

    pub fn close_pr(&self, number: i64) -> Result<String> {
        self.github.close_pr(number)
    }

    #[allow(dead_code)]
    pub async fn resolve_conflict(&mut self, file: &str) -> Result<String> {
        let conflict = self.git.read_file(file).unwrap_or_default();
        let model = self.ensure_model().await?;
        let prompt = resolve_conflict_prompt(&self.config.language, file, &conflict);
        let num_ctx = self.calculate_num_ctx(prompt.len());
        self.llm.generate(&self.config, self.api_key()?.as_deref(), &model, &prompt, num_ctx).await
    }

    pub fn current_branch(&self) -> Result<String> {
        self.git.current_branch()
    }

    pub fn can_merge(&self, base: &str) -> bool {
        self.git.can_merge(base)
    }

    pub fn list_open_prs(&self, head: &str, base: &str) -> Result<Vec<PrInfo>> {
        self.github.list_open_prs(head, base)
    }

    pub async fn execute_simple(&mut self, intent: &Intent) -> Result<String> {
        match intent {
            Intent::Config => Ok(self.config_summary()),
            Intent::Provider => Ok(format!("Active provider: {}", self.config.provider.label())),
            Intent::Staged => {
                let staged = self.toggle_staged()?;
                let lang = self.config.language.to_lowercase();
                let msg = match lang.trim() {
                    "spanish" | "español" | "espanol" => format!(
                        "Modo solo-preparados (staged) está {}.",
                        if staged { "activado" } else { "desactivado" }
                    ),
                    _ => format!(
                        "Staged-only mode is {}.",
                        if staged { "enabled" } else { "disabled" }
                    ),
                };
                Ok(msg)
            }
            Intent::Diff => {
                let context = self.diff_context()?;
                if context.is_empty() {
                    Err(AppError::NoChanges)
                } else {
                    Ok(context.summary(&self.config.language))
                }
            }
            Intent::DeprecatedDryRun => Ok(match self.config.language.to_lowercase().trim() {
                "spanish" | "español" | "espanol" => {
                    "/dry-run fue eliminado. Usa /commit: primero muestra una vista previa y solo crea commits si confirmas el modal.".to_string()
                }
                _ => {
                    "/dry-run was removed. Use /commit: it previews the plan first and only creates commits after you confirm the modal.".to_string()
                }
            }),
            Intent::Explain => self.explain().await,
            Intent::Review => self.review().await,
            _ => Ok(help_text(&self.config.language)),
        }
    }

    pub fn config_summary(&self) -> String {
        format!(
            "provider: {}\nbase_url: {}\napi_key: {}\nmodel: {}\nlanguage: {}\nstaged_only: {}\nauto_setup_repo: {}\nprompt_push_after_commit: {}\ntheme: {}\nhistory_limit: {}",
            self.config.provider.label(),
            self.config.base_url.as_deref().unwrap_or("default"),
            if self.api_key_configured().unwrap_or(false) {
                "configured"
            } else {
                "not configured"
            },
            self.config.model.as_deref().unwrap_or("not selected"),
            self.config.language,
            self.config.staged_only,
            self.config.auto_setup_repo,
            self.config.prompt_push_after_commit,
            self.config.theme.label(),
            self.config.history_limit.label()
        )
    }

    async fn ensure_model(&mut self) -> Result<String> {
        if let Some(model) = self
            .config
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if !self.config.provider.supports_model_listing() {
                return Ok(model.to_string());
            }
            let models = self.models().await?;
            if models.iter().any(|candidate| candidate == model) {
                return Ok(model.to_string());
            }
        }
        let first = if self.config.provider.supports_model_listing() {
            self.models()
                .await?
                .into_iter()
                .next()
                .ok_or(AppError::NoOllamaModels)?
        } else {
            return Err(AppError::MissingModel {
                provider: self.config.provider.label().to_string(),
            });
        };
        self.config.model = Some(first.clone());
        self.save_config()?;
        Ok(first)
    }

    fn api_key(&self) -> Result<Option<String>> {
        if !self.config.provider.is_selected() {
            return Ok(None);
        }
        SecretsRepository::load_api_key(self.config.provider)
    }

    async fn provider_dependency_status(&self) -> DependencyStatus {
        let platform = PlatformInfo::detect();
        match self.config.provider {
            LlmProviderKind::Unset => DependencyStatus::llm_provider_not_configured(
                &platform,
                "Select an AI provider in /config before using AI features.".to_string(),
                None,
            ),
            LlmProviderKind::Ollama => match detect_ollama_installation() {
                Err(detail) => DependencyStatus::llm_provider_not_running(
                    &platform,
                    detail,
                    provider_fallback_url(self.config.provider),
                ),
                Ok(version) => match self.ollama.version().await {
                    Ok(_) => match self.ollama.models().await {
                        Ok(models) if models.is_empty() => {
                            DependencyStatus::llm_provider_not_configured(
                                &platform,
                                "Ollama is running, but no local models are available.".to_string(),
                                provider_fallback_url(self.config.provider),
                            )
                        }
                        Ok(_) => DependencyStatus::ready(
                            DependencyKind::LlmProvider,
                            &platform,
                            Some(version),
                        ),
                        Err(error) => DependencyStatus::llm_provider_not_running(
                            &platform,
                            error.to_string(),
                            provider_fallback_url(self.config.provider),
                        ),
                    },
                    Err(error) => DependencyStatus::llm_provider_not_running(
                        &platform,
                        error.to_string(),
                        provider_fallback_url(self.config.provider),
                    ),
                },
            },
            _ => match self
                .llm
                .health(&self.config, self.api_key().ok().flatten().as_deref())
                .await
            {
                Ok(detail) => {
                    DependencyStatus::ready(DependencyKind::LlmProvider, &platform, Some(detail))
                }
                Err(AppError::ProviderNotSelected) => {
                    DependencyStatus::llm_provider_not_configured(
                        &platform,
                        "Select an AI provider in /config before using AI features.".to_string(),
                        None,
                    )
                }
                Err(AppError::MissingApiKey { .. })
                | Err(AppError::MissingBaseUrl { .. })
                | Err(AppError::MissingModel { .. }) => {
                    DependencyStatus::llm_provider_not_configured(
                        &platform,
                        "The active AI provider still needs configuration.".to_string(),
                        provider_fallback_url(self.config.provider),
                    )
                }
                Err(error) => DependencyStatus::llm_provider_not_running(
                    &platform,
                    error.to_string(),
                    provider_fallback_url(self.config.provider),
                ),
            },
        }
    }
}

fn complete_commit_plan(plan: &mut CommitPlan, context: &DiffContext) {
    let changed_files = changed_files_from_status(&context.status);
    if changed_files.is_empty() {
        return;
    }

    let mut planned = plan
        .groups
        .iter()
        .flat_map(|group| group.files.iter())
        .map(|file| normalize_path_for_compare(&file.path))
        .collect::<HashSet<_>>();

    let missing = changed_files
        .into_iter()
        .filter(|file| planned.insert(normalize_path_for_compare(&file.path)))
        .collect::<Vec<_>>();

    if missing.is_empty() {
        return;
    }

    if let Some(group) = plan.groups.last_mut() {
        group.files.extend(missing);
        if group.rationale.trim().is_empty() {
            group.rationale =
                "Includes all remaining changed files so the commit plan covers the full working tree."
                    .to_string();
        }
    } else {
        plan.groups.push(CommitGroup {
            commit: CommitMessage {
                commit_type: "chore".to_string(),
                scope: String::new(),
                subject: "include remaining changes".to_string(),
                body: "Commit files that were not assigned to a more specific group.".to_string(),
            },
            files: missing,
            rationale: "Fallback group for changed files not assigned by the model.".to_string(),
        });
    }
}

fn dedupe_commit_plan_files(plan: &mut CommitPlan) {
    let mut seen = HashSet::new();
    for group in &mut plan.groups {
        group
            .files
            .retain(|file| seen.insert(normalize_path_for_compare(&file.path)));
    }
}

fn changed_files_from_status(status: &str) -> Vec<FileEntry> {
    status
        .lines()
        .filter_map(changed_file_from_status_line)
        .collect()
}

fn changed_file_from_status_line(line: &str) -> Option<FileEntry> {
    let trimmed = line.trim_end();
    if trimmed.is_empty() {
        return None;
    }

    let (code, raw_path) = parse_status_code_and_path(trimmed)?;
    if raw_path.is_empty() || code == "!!" {
        return None;
    }

    let path = raw_path
        .rsplit_once(" -> ")
        .map(|(_, new_path)| new_path)
        .unwrap_or(raw_path)
        .trim()
        .trim_matches('"')
        .to_string();
    if path.is_empty() {
        return None;
    }

    let status = if code == "??" {
        "untracked"
    } else if code.contains('D') {
        "deleted"
    } else if code.contains('R') {
        "renamed"
    } else if code.contains('A') {
        "added"
    } else {
        "modified"
    };

    Some(FileEntry {
        id: path.clone(),
        path: path.clone(),
        status: status.to_string(),
        description: "Included automatically because it is present in git status.".to_string(),
        patch: None,
    })
}

fn parse_status_code_and_path(line: &str) -> Option<(&str, &str)> {
    if line.len() >= 3 && line.as_bytes().get(2).is_some_and(u8::is_ascii_whitespace) {
        return Some((&line[..2], line[3..].trim()));
    }
    line.split_once(char::is_whitespace)
}

fn normalize_path_for_compare(path: &str) -> String {
    path.trim().trim_matches('"').replace('\\', "/")
}

fn detect_ollama_installation() -> std::result::Result<String, String> {
    let output = Command::new("ollama")
        .arg("--version")
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
        Err(if stderr.is_empty() {
            "ollama --version failed".to_string()
        } else {
            stderr
        })
    }
}

fn provider_fallback_url(provider: LlmProviderKind) -> Option<&'static str> {
    match provider {
        LlmProviderKind::Unset => None,
        LlmProviderKind::Ollama => Some("https://ollama.com/download"),
        LlmProviderKind::OpenAi => Some("https://platform.openai.com/docs"),
        LlmProviderKind::Gemini => Some("https://ai.google.dev/gemini-api/docs"),
        LlmProviderKind::Anthropic => Some("https://docs.anthropic.com/en/api/overview"),
    }
}

pub fn help_text(language: &str) -> String {
    match language.to_lowercase().trim() {
        "spanish" | "español" | "espanol" => [
            "Usa comandos slash:",
            "/commit, /switch, /diff, /provider, /model, /lang, /staged, /push",
            "/pr, /pull-request, /explain, /review, /status, /log, /history, /setup, /theme, /config, /reset, /resolve, /help, /exit",
            "",
            "Teclas: F2 configuración, Shift+Tab cambiar modo, / comandos, Enter enviar, Esc cancelar",
        ]
        .join("\n"),
        _ => [
            "Use slash commands:",
            "/commit, /switch, /diff, /provider, /model, /lang, /staged, /push",
            "/pr, /pull-request, /explain, /review, /status, /log, /history, /setup, /theme, /config, /reset, /resolve, /help, /exit",
            "",
            "Keys: F2 settings, Shift+Tab switch mode, / commands, Enter send, Esc cancel",
        ]
        .join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::DiffContext;
    use reqwest::Client;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::tempdir;

    fn make_core() -> AppCore {
        let config = crate::domain::Config::default();
        AppCore::new(PathBuf::from("."), config, Client::new())
    }

    fn git_in(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
        assert!(status.success(), "git {args:?} failed");
    }

    fn git_output_in(dir: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    #[test]
    fn clean_status_summary_reports_no_ai_usage_without_local_changes() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        git_in(dir.path(), &["config", "commit.gpgsign", "false"]);
        git_in(dir.path(), &["config", "user.name", "Test User"]);
        git_in(dir.path(), &["config", "user.email", "test@example.com"]);
        fs::write(dir.path().join("README.md"), "base\n").unwrap();
        git_in(dir.path(), &["add", "README.md"]);
        git_in(dir.path(), &["commit", "-m", "base"]);

        let core = AppCore::new(
            dir.path().to_path_buf(),
            crate::domain::Config::default(),
            Client::new(),
        );

        let message = core
            .clean_status_summary()
            .unwrap()
            .expect("expected local status summary");

        assert!(message.contains("No local changes on `main`"), "{message}");
        assert!(message.contains("AI was not used"), "{message}");
    }

    #[test]
    fn diff_limits_by_vram() {
        let mut core = make_core();
        core.vram_mb = Some(2000);
        let (chars, lines) = core.diff_limits();
        assert_eq!((chars, lines), (8_000, 1));

        core.vram_mb = Some(5000);
        let (chars, lines) = core.diff_limits();
        assert_eq!((chars, lines), (16_000, 2));

        core.vram_mb = Some(8000);
        let (chars, lines) = core.diff_limits();
        assert_eq!((chars, lines), (24_000, 2));

        core.vram_mb = Some(20000);
        let (chars, lines) = core.diff_limits();
        assert_eq!((chars, lines), (40_000, 3));
    }

    #[test]
    fn calculate_num_ctx_clamped() {
        let mut core = make_core();
        core.vram_mb = Some(2000); // max 2048
        // Large prompt should be clamped to max_ctx
        let ctx = core.calculate_num_ctx(1_000_000);
        assert!(ctx.unwrap() <= 2048);
    }

    #[test]
    fn config_summary_includes_all_fields() {
        let core = make_core();
        let summary = core.config_summary();
        assert!(summary.contains("provider:"));
        assert!(summary.contains("api_key:"));
        assert!(summary.contains("model:"));
        assert!(summary.contains("language:"));
        assert!(summary.contains("staged_only:"));
    }

    #[test]
    fn ensure_model_first_available() {
        let _core = make_core();
        // This would require a mock Ollama, so just test the error path
        // In real tests we'd use a mock server
    }

    #[test]
    fn diff_context_empty() {
        let context = DiffContext::default();
        assert!(context.is_empty());
    }

    #[test]
    fn diff_context_truncation_warning() {
        let context = DiffContext {
            truncated: true,
            ..Default::default()
        };
        let warning = context.truncation_warning("English");
        assert!(warning.contains("WARNING"));
        assert!(warning.contains("truncated"));
    }

    #[test]
    fn complete_commit_plan_adds_changed_files_missing_from_model_output() {
        let mut plan = CommitPlan {
            summary: "Plan".to_string(),
            groups: vec![CommitGroup {
                commit: CommitMessage {
                    commit_type: "feat".to_string(),
                    scope: String::new(),
                    subject: "update tracked file".to_string(),
                    body: String::new(),
                },
                files: vec![FileEntry {
                    id: "src/app.rs".to_string(),
                    path: "src/app.rs".to_string(),
                    status: "modified".to_string(),
                    description: "Updated app".to_string(),
                    patch: None,
                }],
                rationale: String::new(),
            }],
        };
        let context = DiffContext {
            status: " M src/app.rs\n?? README.md\nA  src/new.rs\n".to_string(),
            ..Default::default()
        };

        complete_commit_plan(&mut plan, &context);

        let paths = plan.groups[0]
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["src/app.rs", "README.md", "src/new.rs"]);
    }

    #[test]
    fn execute_commit_plan_commits_tracked_and_untracked_files_without_leftovers() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        git_in(dir.path(), &["config", "user.name", "Test User"]);
        git_in(dir.path(), &["config", "user.email", "test@example.com"]);
        git_in(dir.path(), &["config", "commit.gpgsign", "false"]);

        fs::write(dir.path().join("tracked.txt"), "before\n").unwrap();
        git_in(dir.path(), &["add", "tracked.txt"]);
        git_in(dir.path(), &["commit", "-m", "base"]);

        fs::write(dir.path().join("tracked.txt"), "after\n").unwrap();
        fs::write(dir.path().join("untracked.txt"), "new\n").unwrap();

        let core = AppCore::new(
            dir.path().to_path_buf(),
            crate::domain::Config::default(),
            Client::new(),
        );
        let plan = CommitPlan {
            summary: String::new(),
            groups: vec![CommitGroup {
                commit: CommitMessage {
                    commit_type: "fix".to_string(),
                    scope: String::new(),
                    subject: "update tracked file".to_string(),
                    body: String::new(),
                },
                files: vec![FileEntry {
                    id: "tracked.txt".to_string(),
                    path: "tracked.txt".to_string(),
                    status: "modified".to_string(),
                    description: String::new(),
                    patch: None,
                }],
                rationale: String::new(),
            }],
        };

        core.execute_commit_plan(&plan).unwrap();

        assert_eq!(git_output_in(dir.path(), &["status", "--porcelain"]), "");
        assert!(git_output_in(dir.path(), &["log", "--oneline"]).contains("update tracked file"));
    }

    #[test]
    fn execute_commit_plan_respects_staged_only_scope() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        git_in(dir.path(), &["config", "user.name", "Test User"]);
        git_in(dir.path(), &["config", "user.email", "test@example.com"]);
        git_in(dir.path(), &["config", "commit.gpgsign", "false"]);

        fs::write(dir.path().join("staged.txt"), "before\n").unwrap();
        fs::write(dir.path().join("unstaged.txt"), "before\n").unwrap();
        git_in(dir.path(), &["add", "."]);
        git_in(dir.path(), &["commit", "-m", "base"]);

        fs::write(dir.path().join("staged.txt"), "after\n").unwrap();
        fs::write(dir.path().join("unstaged.txt"), "after\n").unwrap();
        fs::write(dir.path().join("untracked.txt"), "new\n").unwrap();
        git_in(dir.path(), &["add", "staged.txt"]);

        let config = crate::domain::Config {
            staged_only: true,
            ..Default::default()
        };
        let core = AppCore::new(dir.path().to_path_buf(), config, Client::new());
        let plan = CommitPlan {
            summary: String::new(),
            groups: vec![CommitGroup {
                commit: CommitMessage {
                    commit_type: "fix".to_string(),
                    scope: String::new(),
                    subject: "commit staged file".to_string(),
                    body: String::new(),
                },
                files: vec![FileEntry {
                    id: "staged.txt".to_string(),
                    path: "staged.txt".to_string(),
                    status: "modified".to_string(),
                    description: String::new(),
                    patch: None,
                }],
                rationale: String::new(),
            }],
        };

        core.execute_commit_plan(&plan).unwrap();

        let status = git_output_in(dir.path(), &["status", "--porcelain"]);
        let status_lines = status.lines().collect::<Vec<_>>();
        assert!(status_lines.contains(&" M unstaged.txt"), "{status}");
        assert!(status_lines.contains(&"?? untracked.txt"), "{status}");
        assert!(
            !status_lines
                .iter()
                .any(|line| line.ends_with("staged.txt") && !line.ends_with("unstaged.txt")),
            "{status}"
        );
    }

    #[test]
    fn execute_commit_plan_skips_duplicate_files_in_later_groups() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        git_in(dir.path(), &["config", "user.name", "Test User"]);
        git_in(dir.path(), &["config", "user.email", "test@example.com"]);
        git_in(dir.path(), &["config", "commit.gpgsign", "false"]);

        fs::write(dir.path().join("shared.txt"), "before\n").unwrap();
        git_in(dir.path(), &["add", "shared.txt"]);
        git_in(dir.path(), &["commit", "-m", "base"]);
        fs::write(dir.path().join("shared.txt"), "after\n").unwrap();

        let core = AppCore::new(
            dir.path().to_path_buf(),
            crate::domain::Config::default(),
            Client::new(),
        );
        let duplicate = FileEntry {
            id: "shared.txt".to_string(),
            path: "shared.txt".to_string(),
            status: "modified".to_string(),
            description: String::new(),
            patch: None,
        };
        let plan = CommitPlan {
            summary: String::new(),
            groups: vec![
                CommitGroup {
                    commit: CommitMessage {
                        commit_type: "fix".to_string(),
                        scope: String::new(),
                        subject: "first group".to_string(),
                        body: String::new(),
                    },
                    files: vec![duplicate.clone()],
                    rationale: String::new(),
                },
                CommitGroup {
                    commit: CommitMessage {
                        commit_type: "chore".to_string(),
                        scope: String::new(),
                        subject: "duplicate group".to_string(),
                        body: String::new(),
                    },
                    files: vec![duplicate],
                    rationale: String::new(),
                },
            ],
        };

        let output = core.execute_commit_plan(&plan).unwrap();

        assert!(output.contains("first group"), "{output}");
        assert!(!output.contains("duplicate group"), "{output}");
        assert_eq!(git_output_in(dir.path(), &["status", "--porcelain"]), "");
    }

    #[test]
    fn changed_files_from_status_handles_renames_and_untracked_files() {
        let files = changed_files_from_status("R  old.rs -> new.rs\n?? docs/guide.md\n D gone.rs");
        let paths = files
            .iter()
            .map(|file| (file.path.as_str(), file.status.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![
                ("new.rs", "renamed"),
                ("docs/guide.md", "untracked"),
                ("gone.rs", "deleted")
            ]
        );
    }

    #[test]
    fn estimates_context_usage_percent() {
        let core = make_core();
        let usage = core.estimate_context_usage(3_000, false);
        assert!(usage.limit >= usage.estimated_tokens);
        assert!(usage.percent().is_some());
    }

    #[test]
    fn ollama_health_ready_marks_cli_usable() {
        let health = OllamaHealth::ready("0.9.0".to_string());

        assert!(health.installed);
        assert!(health.running);
        assert_eq!(health.version.as_deref(), Some("0.9.0"));
    }

    #[test]
    fn ollama_health_not_installed_blocks_cli() {
        let health = OllamaHealth::not_installed("not found".to_string());

        assert!(!health.installed);
        assert!(!health.running);
        assert_eq!(health.install_message.as_deref(), Some("not found"));
    }

    #[test]
    fn ollama_health_not_running_keeps_installed_version() {
        let health = OllamaHealth::not_running(
            Some("ollama version is 0.9.0".to_string()),
            "offline".to_string(),
        );

        assert!(health.installed);
        assert!(!health.running);
        assert_eq!(health.runtime_message.as_deref(), Some("offline"));
    }
}
