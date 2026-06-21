use serde::Deserialize;

use crate::domain::DiffContext;
use crate::error::{AppError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitMessage {
    pub commit_type: String,
    pub scope: String,
    pub subject: String,
    pub body: String,
}

impl CommitMessage {
    pub fn title(&self) -> String {
        let scope = self.scope.trim();
        if scope.is_empty() {
            format!(
                "{}: {}",
                self.commit_type.trim(),
                normalize_subject(&self.subject)
            )
        } else {
            format!(
                "{}({}): {}",
                self.commit_type.trim(),
                scope,
                normalize_subject(&self.subject)
            )
        }
    }

    #[cfg(test)]
    pub fn from_llm_response(response: &str) -> Result<Self> {
        let json = extract_json(response).ok_or_else(|| {
            AppError::InvalidLlmResponse(no_json_error("commit message", response))
        })?;
        let parsed: CommitMessageResponse =
            serde_json::from_str(json).map_err(|source| AppError::InvalidJson {
                value: json.to_string(),
                source,
            })?;

        let message = Self {
            commit_type: normalize_commit_type(&parsed.commit_type),
            scope: normalize_scope(&parsed.scope),
            subject: normalize_subject(&strip_emoji(&parsed.subject)),
            body: strip_emoji(parsed.body.trim()).trim().to_string(),
        };
        message.validate()?;
        Ok(message)
    }

    pub fn validate(&self) -> Result<()> {
        if !is_valid_commit_type(&self.commit_type) {
            return Err(AppError::InvalidLlmResponse(format!(
                "invalid conventional commit type `{}`",
                self.commit_type
            )));
        }
        if self.subject.trim().is_empty() {
            return Err(AppError::InvalidLlmResponse(
                "commit subject is required".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct CommitMessageResponse {
    #[serde(rename = "type")]
    commit_type: String,
    scope: String,
    subject: String,
    body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestDraft {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PrInfo {
    pub number: i64,
    pub title: String,
    pub url: String,
}

impl PullRequestDraft {
    pub fn from_llm_response(response: &str) -> Result<Self> {
        let json = extract_json(response).ok_or_else(|| {
            AppError::InvalidLlmResponse(no_json_error("pull request draft", response))
        })?;
        let parsed: PullRequestDraftResponse =
            serde_json::from_str(json).map_err(|source| AppError::InvalidJson {
                value: json.to_string(),
                source,
            })?;

        if parsed.title.trim().is_empty() || parsed.body.trim().is_empty() {
            return Err(AppError::InvalidLlmResponse(
                "PR title and body are required".to_string(),
            ));
        }

        Ok(Self {
            title: strip_emoji(parsed.title.trim()).trim().to_string(),
            body: strip_emoji(parsed.body.trim()).trim().to_string(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct PullRequestDraftResponse {
    title: String,
    body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub id: String,
    pub path: String,
    pub status: String,
    pub description: String,
    pub patch: Option<String>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftChanges {
    pub commit: CommitMessage,
    pub files: Vec<FileEntry>,
}

#[cfg(test)]
impl DraftChanges {
    pub fn from_llm_response(response: &str) -> Result<Self> {
        let json = extract_json(response).ok_or_else(|| {
            AppError::InvalidLlmResponse(no_json_error("draft changes", response))
        })?;
        let parsed: DraftChangesResponse =
            serde_json::from_str(json).map_err(|source| AppError::InvalidJson {
                value: json.to_string(),
                source,
            })?;

        let commit = CommitMessage {
            commit_type: normalize_commit_type(&parsed.commit_type),
            scope: normalize_scope(&parsed.scope),
            subject: normalize_subject(&parsed.subject),
            body: parsed.body.trim().to_string(),
        };
        commit.validate()?;

        let files = parsed
            .files
            .into_iter()
            .map(|f| FileEntry {
                id: f.id.unwrap_or_else(|| f.path.trim().to_string()),
                path: f.path.trim().to_string(),
                status: f.status.trim().to_string(),
                description: strip_emoji(f.description.trim()).trim().to_string(),
                patch: f
                    .patch
                    .map(|patch| patch.trim().to_string())
                    .filter(|patch| !patch.is_empty()),
            })
            .collect();

        Ok(Self { commit, files })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitGroup {
    pub commit: CommitMessage,
    pub files: Vec<FileEntry>,
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitPlan {
    pub summary: String,
    pub groups: Vec<CommitGroup>,
}

impl CommitPlan {
    pub fn from_llm_response(response: &str) -> Result<Self> {
        let json = extract_json(response)
            .ok_or_else(|| AppError::InvalidLlmResponse(no_json_error("commit plan", response)))?;
        let parsed: CommitPlanResponse = serde_json::from_str(json).map_err(|source| {
            AppError::InvalidLlmResponse(malformed_commit_plan_json_error(source, json))
        })?;

        let mut groups = Vec::new();
        for group in parsed.groups {
            let commit_type = group
                .commit
                .as_ref()
                .and_then(|commit| non_empty(&commit.commit_type))
                .or_else(|| non_empty(&group.commit_type))
                .unwrap_or_default();
            let scope = group
                .commit
                .as_ref()
                .and_then(|commit| non_empty(&commit.scope))
                .or_else(|| non_empty(&group.scope))
                .unwrap_or_default();
            let subject = group
                .commit
                .as_ref()
                .and_then(|commit| non_empty(&commit.subject))
                .or_else(|| non_empty(&group.subject))
                .unwrap_or_default();
            let body = group
                .commit
                .as_ref()
                .and_then(|commit| non_empty(&commit.body))
                .or_else(|| non_empty(&group.body))
                .unwrap_or_default();
            let commit = CommitMessage {
                commit_type: normalize_commit_type(commit_type),
                scope: normalize_scope(scope),
                subject: normalize_subject(&strip_emoji(subject)),
                body: strip_emoji(body.trim()).trim().to_string(),
            };
            commit.validate()?;

            let files = group
                .files
                .into_iter()
                .map(|file| FileEntry {
                    id: file.id.unwrap_or_else(|| file.path.trim().to_string()),
                    path: file.path.trim().to_string(),
                    status: normalize_file_status(&file.status),
                    description: normalize_file_description(&file.path, &file.description),
                    patch: file
                        .patch
                        .map(|patch| patch.trim().to_string())
                        .filter(|patch| !patch.is_empty()),
                })
                .collect::<Vec<_>>();

            if files.is_empty() {
                return Err(AppError::InvalidLlmResponse(
                    "commit groups must include at least one file".to_string(),
                ));
            }
            if files.iter().any(|file| file.path.trim().is_empty()) {
                return Err(AppError::InvalidLlmResponse(
                    "commit group files must include a path".to_string(),
                ));
            }

            groups.push(CommitGroup {
                commit,
                files,
                rationale: group.rationale.trim().to_string(),
            });
        }

        if groups.is_empty() {
            return Err(AppError::InvalidLlmResponse(
                "commit plan must include at least one group".to_string(),
            ));
        }

        Ok(Self {
            summary: parsed.summary.trim().to_string(),
            groups,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LlmContextUsage {
    pub estimated_tokens: usize,
    pub limit: usize,
    pub truncated: bool,
}

impl LlmContextUsage {
    pub fn percent(self) -> Option<u8> {
        if self.limit == 0 {
            return None;
        }
        let percent = (self.estimated_tokens.saturating_mul(100) / self.limit).min(100);
        Some(percent as u8)
    }
}

#[derive(Debug, Deserialize)]
struct CommitPlanResponse {
    summary: String,
    groups: Vec<CommitGroupResponse>,
}

#[derive(Debug, Deserialize)]
struct CommitGroupResponse {
    #[serde(rename = "type")]
    #[serde(default)]
    commit_type: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    commit: Option<CommitFieldsResponse>,
    #[serde(default)]
    rationale: String,
    files: Vec<FileEntryResponse>,
}

#[derive(Debug, Deserialize)]
struct CommitFieldsResponse {
    #[serde(rename = "type")]
    #[serde(default)]
    commit_type: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    body: String,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct DraftChangesResponse {
    #[serde(rename = "type")]
    commit_type: String,
    scope: String,
    subject: String,
    body: String,
    files: Vec<FileEntryResponse>,
}

#[derive(Debug, Deserialize)]
struct FileEntryResponse {
    #[serde(default)]
    id: Option<String>,
    path: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    patch: Option<String>,
}

#[cfg(test)]
pub fn commit_prompt(language: &str, context: &DiffContext) -> String {
    format!(
        "You are an expert Git commit assistant. Analyze ALL changes below and return ONLY a valid JSON object.\n\n\
         JSON Schema:\n\
         {{\n\
           \"type\": \"feat|fix|docs|style|refactor|test|chore|build|ci|perf|revert\",\n\
           \"scope\": \"optional-lowercase-word-or-empty-string\",\n\
           \"subject\": \"imperative present-tense description, no trailing period\",\n\
           \"body\": \"optional concise description of why/what changed\"\n\
         }}\n\n\
         Rules:\n\
         - Use Conventional Commit types only.\n\
         - Scope is OPTIONAL. Set to empty string \"\" if no specific module is affected.\n\
         - Subject must be imperative, no trailing period.\n\
         - Body is OPTIONAL. Set to empty string \"\" if not needed.\n\
         - Write subject and body in {}.\n\n\
         STATUS:\n{}\n\nSTAT:\n{}\n\nDIFF:\n{}{}{}",
        language,
        context.status,
        context.stat,
        context.diff,
        context.truncation_warning(language),
        language_reinforcement(language)
    )
}

pub fn commit_plan_prompt(language: &str, context: &DiffContext) -> String {
    format!(
        "You are an expert Git commit planner. Analyze ALL changes below and split them into coherent Conventional Commits. Return ONLY one valid JSON object.\n\n\
         JSON Schema:\n\
         {{\n\
           \"summary\": \"one concise sentence describing the whole plan\",\n\
           \"groups\": [\n\
             {{\n\
               \"type\": \"feat|fix|docs|style|refactor|test|chore|build|ci|perf|revert\",\n\
               \"scope\": \"optional-lowercase-word-or-empty-string\",\n\
               \"subject\": \"imperative present-tense description, no trailing period\",\n\
               \"body\": \"optional concise description of why/what changed\",\n\
               \"rationale\": \"brief reason these files belong together\",\n\
               \"files\": [\n\
                 {{\n\
                   \"id\": \"stable-change-id, for example src/lib.rs#hunk-1 or src/lib.rs\",\n\
                   \"path\": \"relative/path/to/file\",\n\
                   \"status\": \"modified|added|deleted|renamed|untracked|hunk\",\n\
                   \"description\": \"extremely brief description of what changed in this file or hunk\",\n\
                   \"patch\": \"optional unified diff patch for this hunk only, or empty string for whole-file staging\"\n\
                 }}\n\
               ]\n\
             }}\n\
           ]\n\
         }}\n\n\
          Rules:\n\
          - Output raw JSON only. Do not include markdown fences, prose, comments, explanations, or thinking text.\n\
          - The first character of the response must be {{ and the last character must be }}.\n\
          - Decide the groups yourself based on functional context and developer intent.\n\
         - Prefer the smallest independently revertible coherent commits. More focused commits are better when they remain correct.\n\
         - Do NOT create one giant commit unless all changes clearly belong to the same change.\n\
         - Split unrelated hunks inside the same file when the patch can be applied independently and safely.\n\
          - Do NOT split hunks or files that must compile or work together.\n\
          - Every changed file or hunk must appear in exactly one group.\n\
          - Every file object MUST include path, status, description, and patch.\n\
          - status MUST be one of: modified, added, deleted, renamed, untracked, hunk.\n\
          - Use patch only for independently applicable unified diff hunks; otherwise set patch to \"\".\n\
          - Do not invent generated files, directories, dependency names, or patches that are not present in the diff.\n\
          - Use Conventional Commit types only.\n\
         - Scope is OPTIONAL. Set it to \"\" if no specific module is affected.\n\
         - Subject must be imperative and have no trailing period.\n\
         - Body is OPTIONAL. Set it to \"\" if not needed.\n\
         - Do not use emojis anywhere in the JSON values.\n\
         - Write summary, subject, body, rationale, and descriptions in {}.\n\n\
         STATUS:\n{}\n\nSTAT:\n{}\n\nDIFF + UNTRACKED:\n{}{}{}",
        language,
        context.status,
        context.stat,
        context.diff,
        context.truncation_warning(language),
        language_reinforcement(language)
    )
}

pub fn pr_prompt(
    language: &str,
    context: &DiffContext,
    commits_text: &str,
    base: &str,
    head: &str,
    template: Option<&str>,
) -> String {
    let template_instruction = match template {
        Some(tpl) => format!(
            "You MUST format the PR description using the following Pull Request template. Do not delete the markdown headers or structure of the template, just fill in the appropriate information based on the diff:\n\nTEMPLATE:\n{}\n\nAt the end of the PR body, add the watermark: 'Powered by Autopilot CLI'.\n\n",
            tpl
        ),
        None => "Create a highly professional GitHub pull request description with production-ready standards (open-source style). The PR body must include the following sections: '### Description', '### Key Changes', '### How to Test', and '### Checklist' (markdown checklist). At the end of the PR body, add the watermark: 'Powered by Autopilot CLI'.\n\n".to_string()
    };

    format!(
        "Create a GitHub pull request draft in {} for head branch `{}` into base `{}`. Return only JSON with string fields `title` and `body`.\n\n\
         {}\
         Do not use emojis anywhere in generated text.\n\n\
         COMMITS:\n{}\n\n\
         STATUS:\n{}\n\nSTAT:\n{}\n\nDIFF:\n{}{}{}",
        language,
        head,
        base,
        template_instruction,
        commits_text,
        context.status,
        context.stat,
        context.diff,
        context.truncation_warning(language),
        language_reinforcement(language)
    )
}

pub fn explain_prompt(language: &str, context: &DiffContext) -> String {
    format!(
        "Explain these Git changes in {}. Be concise and practical. Do not use emojis.\n\nSTATUS:\n{}\n\nSTAT:\n{}\n\nDIFF:\n{}{}{}",
        language,
        context.status,
        context.stat,
        context.diff,
        context.truncation_warning(language),
        language_reinforcement(language)
    )
}

pub fn review_prompt(language: &str, context: &DiffContext) -> String {
    format!(
        "Review these Git changes in {}. Focus on bugs, risky behavior, missing tests, and security issues. Keep it short. Do not use emojis.\n\nSTATUS:\n{}\n\nSTAT:\n{}\n\nDIFF:\n{}{}{}",
        language,
        context.status,
        context.stat,
        context.diff,
        context.truncation_warning(language),
        language_reinforcement(language)
    )
}

pub fn status_prompt(language: &str, context: &DiffContext) -> String {
    format!(
        "Summarize the current Git working tree in {} for a CLI user. Do not use emojis. Be direct and practical.\n\n\
         Required format:\n\
         - Start with one short sentence saying whether there are changes to handle.\n\
         - Then list each changed file as `file path: lines/range - what changed`.\n\
         - Use line numbers or hunk ranges from the diff when available, for example `@@ -10,2 +10,5 @@`; if exact lines are not available, say `lines unknown`.\n\
         - Mention untracked, staged, unstaged, deleted, or renamed state when visible from STATUS.\n\
         - Do not include commit suggestions unless the user asks.\n\n\
         STATUS:\n{}\n\nSTAT:\n{}\n\nDIFF:\n{}{}{}",
        language,
        context.status,
        context.stat,
        context.diff,
        context.truncation_warning(language),
        language_reinforcement(language)
    )
}

fn language_reinforcement(language: &str) -> String {
    match language.to_lowercase().trim() {
        "spanish" | "español" | "espanol" => {
            "\n\nCRITICAL: Escribe todos los textos generados (como subject, body, description, title, etc.) completamente en ESPAÑOL.\n\
             No uses inglés para el contenido redactado. Los nombres de archivos, los comandos Git y los tipos/scopes convencionales (feat, fix, etc.) no deben ser de otra forma. No uses emojis."
        }
        _ => {
            "\n\nCRITICAL: Write all generated texts (like subject, body, description, title, etc.) completely in ENGLISH. Do not use emojis."
        }
    }.to_string()
}

fn extract_json(response: &str) -> Option<&str> {
    let start = response.find('{')?;
    let end = response.rfind('}')?;
    (end > start).then_some(&response[start..=end])
}

fn no_json_error(context: &str, response: &str) -> String {
    let preview = sanitize_llm_preview(response);
    if preview.is_empty() {
        format!(
            "The selected AI provider returned no valid JSON for the {}. Try Regenerate or switch models.",
            context
        )
    } else {
        format!(
            "The selected AI provider returned no valid JSON for the {}. Try Regenerate or switch models.\nPreview: {}",
            context, preview
        )
    }
}

fn sanitize_llm_preview(response: &str) -> String {
    const LIMIT: usize = 180;
    let mut preview = response
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(LIMIT)
        .collect::<String>();
    if response.chars().count() > LIMIT {
        preview.push_str("...");
    }
    preview
}

fn malformed_commit_plan_json_error(source: serde_json::Error, json: &str) -> String {
    let preview = sanitize_llm_preview(json);
    format!(
        "The selected AI provider returned malformed commit-plan JSON: {}. Try Regenerate, /staged, or another model. Preview: {}",
        source, preview
    )
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn normalize_file_status(value: &str) -> String {
    let status = value.trim().to_lowercase();
    match status.as_str() {
        "modified" | "added" | "deleted" | "renamed" | "untracked" | "hunk" => status,
        _ => "modified".to_string(),
    }
}

fn normalize_file_description(path: &str, description: &str) -> String {
    let description = strip_emoji(description.trim()).trim().to_string();
    if description.is_empty() {
        format!("changes in {}", path.trim())
    } else {
        description
    }
}

pub fn strip_emoji(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !is_emoji_like(*ch))
        .collect::<String>()
}

fn is_emoji_like(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1F000..=0x1FAFF
            | 0x2600..=0x27BF
            | 0xFE00..=0xFE0F
            | 0x200D
    )
}

fn normalize_commit_type(value: &str) -> String {
    value.trim().to_lowercase()
}

fn normalize_scope(value: &str) -> String {
    let normalized = value
        .trim()
        .to_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    normalized.trim_matches('-').to_string()
}

fn normalize_subject(value: &str) -> String {
    value.trim().trim_end_matches('.').to_string()
}

fn is_valid_commit_type(value: &str) -> bool {
    matches!(
        value,
        "feat"
            | "fix"
            | "docs"
            | "style"
            | "refactor"
            | "test"
            | "chore"
            | "build"
            | "ci"
            | "perf"
            | "revert"
    )
}

#[allow(dead_code)]
pub fn resolve_conflict_prompt(language: &str, file: &str, conflict: &str) -> String {
    format!(
        "You are an expert developer helping to resolve git merge conflicts in {}. \n\
         File: `{}`\n\n\
         Conflicts:\n{}\n\n\
         Provide the resolved content for this file. Return ONLY the content of the resolved file. Do not include markdown code fences, prose, or explanation.",
        language, file, conflict
    )
}

pub fn scout_question_prompt(language: &str, context: &DiffContext, question: &str) -> String {
    let lang_reinforce = match language.to_lowercase().trim() {
        "spanish" | "español" | "espanol" => "Responde obligatoriamente en Español.",
        _ => "Respond in English.",
    };
    format!(
        "You are an expert developer analyzing a git diff.\n\
         Here is the git diff and status context:\n\n\
         {}\n\n\
         {}{}\n\n\
         The user asks this specific question about these changes:\n\
         \"{}\"\n\n\
         Provide a professional, clear, and detailed answer. Do not use emojis. {}\n",
        context.status,
        context.diff,
        context.truncation_warning(language),
        question,
        lang_reinforce
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_prompt_requests_file_line_ranges_and_change_summary() {
        let context = DiffContext {
            status: " M src/main.rs\n?? docs/new.md".to_string(),
            stat: " src/main.rs | 4 ++--".to_string(),
            diff: "diff --git a/src/main.rs b/src/main.rs\n@@ -10,2 +10,4 @@ fn main() {}\n+println!(\"hi\");".to_string(),
            truncated: false,
        };

        let prompt = status_prompt("Spanish", &context);

        assert!(prompt.contains("file path: lines/range - what changed"));
        assert!(prompt.contains("lines unknown"));
        assert!(prompt.contains("STATUS:"));
        assert!(prompt.contains("@@ -10,2 +10,4 @@"));
        assert!(prompt.contains("CRITICAL"));
    }

    #[test]
    fn parses_commit_message_from_json_response() {
        let response = "Here:\n{\"type\":\"feat\",\"scope\":\"cli\",\"subject\":\"add prompt\",\"body\":\"Adds an interactive prompt.\"}\n";
        let message = CommitMessage::from_llm_response(response).unwrap();
        assert_eq!(message.title(), "feat(cli): add prompt");
        assert_eq!(message.body, "Adds an interactive prompt.");
    }

    #[test]
    fn rejects_invalid_commit_type() {
        let response = "{\"type\":\"feature\",\"scope\":\"cli\",\"subject\":\"add prompt\",\"body\":\"Adds prompt.\"}";
        assert!(matches!(
            CommitMessage::from_llm_response(response),
            Err(AppError::InvalidLlmResponse(_))
        ));
    }

    #[test]
    fn normalizes_scope_and_subject() {
        let response = "{\"type\":\"FIX\",\"scope\":\"Core CLI\",\"subject\":\"Handle failures.\",\"body\":\"Handles failures.\"}";
        let message = CommitMessage::from_llm_response(response).unwrap();
        assert_eq!(message.title(), "fix(core-cli): Handle failures");
    }

    #[test]
    fn builds_commit_prompt_with_language() {
        let context = DiffContext {
            status: " M src/main.rs".to_string(),
            stat: "src/main.rs | 1 +".to_string(),
            diff: "+hello".to_string(),
            truncated: false,
        };
        let prompt = commit_prompt("Spanish", &context);
        assert!(prompt.contains("Write subject and body in Spanish."));
        assert!(prompt.contains("src/main.rs"));
    }

    #[test]
    fn parses_draft_changes_from_json_response() {
        let response = r#"
            {
                "type": "feat",
                "scope": "tui",
                "subject": "localize text",
                "body": "Localized all user visible texts.",
                "files": [
                    {
                        "path": "src/ui/state.rs",
                        "status": "modified",
                        "description": "added translate helper"
                    }
                ]
            }
        "#;
        let draft = DraftChanges::from_llm_response(response).unwrap();
        assert_eq!(draft.commit.title(), "feat(tui): localize text");
        assert_eq!(draft.commit.body, "Localized all user visible texts.");
        assert_eq!(draft.files.len(), 1);
        assert_eq!(draft.files[0].path, "src/ui/state.rs");
        assert_eq!(draft.files[0].status, "modified");
        assert_eq!(draft.files[0].description, "added translate helper");
    }

    #[test]
    fn parses_commit_plan_from_json_response() {
        let response = r#"
            {
                "summary": "Split login and tests into focused commits",
                "groups": [
                    {
                        "type": "feat",
                        "scope": "auth",
                        "subject": "add login form",
                        "body": "",
                        "rationale": "Auth UI files implement one feature",
                        "files": [
                    {
                        "id": "src/auth.rs",
                        "path": "src/auth.rs",
                        "status": "modified",
                        "description": "adds login handling",
                        "patch": ""
                    }
                        ]
                    }
                ]
            }
        "#;
        let plan = CommitPlan::from_llm_response(response).unwrap();
        assert_eq!(plan.groups.len(), 1);
        assert_eq!(plan.groups[0].commit.title(), "feat(auth): add login form");
        assert_eq!(plan.groups[0].files[0].path, "src/auth.rs");
        assert_eq!(plan.groups[0].files[0].id, "src/auth.rs");
    }

    #[test]
    fn parses_commit_plan_with_nested_commit_object() {
        let response = r#"
            {
                "summary": "Split UI fixes",
                "groups": [
                    {
                        "commit": {
                            "type": "fix",
                            "scope": "tui",
                            "subject": "keep status visible",
                            "body": ""
                        },
                        "rationale": "UI layout fix",
                        "files": [
                            {
                                "path": "src/ui/render.rs",
                                "status": "modified",
                                "description": "updates responsive status bar"
                            }
                        ]
                    }
                ]
            }
        "#;
        let plan = CommitPlan::from_llm_response(response).unwrap();

        assert_eq!(
            plan.groups[0].commit.title(),
            "fix(tui): keep status visible"
        );
    }

    #[test]
    fn parses_commit_plan_with_missing_file_status_and_description() {
        let response = r#"
            {
                "summary": "Update UI tests",
                "groups": [
                    {
                        "type": "test",
                        "scope": "tui",
                        "subject": "cover narrow status bar",
                        "body": "",
                        "rationale": "tests protect layout behavior",
                        "files": [
                            {
                                "path": "src/ui/render.rs"
                            }
                        ]
                    }
                ]
            }
        "#;
        let plan = CommitPlan::from_llm_response(response).unwrap();

        assert_eq!(plan.groups[0].files[0].status, "modified");
        assert_eq!(
            plan.groups[0].files[0].description,
            "changes in src/ui/render.rs"
        );
    }

    #[test]
    fn malformed_commit_plan_json_uses_short_actionable_error() {
        let response = r#"
            {
                "summary": "broken",
                "groups": [
                    {"type": "fix", "files": [{"path": "README.md", "status: ": "}"}
                ]
            }
        "#;
        let error = CommitPlan::from_llm_response(response).unwrap_err();

        let AppError::InvalidLlmResponse(message) = error else {
            panic!("expected invalid LLM response");
        };
        assert!(message.contains("The selected AI provider returned malformed commit-plan JSON"));
        assert!(message.contains("Try Regenerate, /staged, or another model"));
        assert!(message.chars().count() < 420);
    }

    #[test]
    fn commit_plan_rejects_prose_only_response_with_actionable_error() {
        let response = "Sure, I would create a feat commit for the UI and a test commit.";
        let error = CommitPlan::from_llm_response(response).unwrap_err();

        let AppError::InvalidLlmResponse(message) = error else {
            panic!("expected invalid LLM response");
        };
        assert!(
            message.contains("The selected AI provider returned no valid JSON for the commit plan")
        );
        assert!(message.contains("Try Regenerate or switch models"));
        assert!(message.contains("Preview: Sure"));
    }

    #[test]
    fn validates_language_reinforcement() {
        assert!(language_reinforcement("Spanish").contains("ESPAÑOL"));
        assert!(language_reinforcement("español").contains("ESPAÑOL"));
        assert!(language_reinforcement("espanol").contains("ESPAÑOL"));
        assert!(language_reinforcement("English").contains("ENGLISH"));
        assert!(language_reinforcement("French").contains("ENGLISH")); // Fallback
    }

    #[test]
    fn strips_emoji_from_generated_text() {
        assert_eq!(strip_emoji("fix bug ✅"), "fix bug ");
    }
}
