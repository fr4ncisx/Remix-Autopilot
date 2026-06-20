use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub provider: LlmProviderKind,
    #[serde(default)]
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub language: String,
    pub staged_only: bool,
    #[serde(default = "default_true")]
    pub auto_setup_repo: bool,
    #[serde(default = "default_true")]
    pub prompt_push_after_commit: bool,
    #[serde(default)]
    pub theme: ThemeChoice,
    #[serde(default)]
    pub history_limit: HistoryLimit,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: LlmProviderKind::Unset,
            base_url: None,
            model: None,
            language: "English".to_string(),
            staged_only: false,
            auto_setup_repo: true,
            prompt_push_after_commit: true,
            theme: ThemeChoice::CodexDark,
            history_limit: HistoryLimit::Medium,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LlmProviderKind {
    #[default]
    Unset,
    Ollama,
    OpenAi,
    Gemini,
    Anthropic,
}

impl LlmProviderKind {
    pub fn all() -> &'static [Self] {
        &[Self::Ollama, Self::OpenAi, Self::Gemini, Self::Anthropic]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unset => "Not selected",
            Self::Ollama => "Ollama",
            Self::OpenAi => "OpenAI",
            Self::Gemini => "Gemini",
            Self::Anthropic => "Anthropic",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::Unset => "unset",
            Self::Ollama => "ollama",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
            Self::Anthropic => "anthropic",
        }
    }

    pub fn uses_api_key(self) -> bool {
        !matches!(self, Self::Unset | Self::Ollama)
    }

    pub fn supports_model_listing(self) -> bool {
        !matches!(self, Self::Unset)
    }

    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::Unset => None,
            Self::Ollama => Some("http://localhost:11434"),
            Self::OpenAi => Some("https://api.openai.com/v1"),
            Self::Gemini => Some("https://generativelanguage.googleapis.com/v1beta"),
            Self::Anthropic => Some("https://api.anthropic.com/v1"),
        }
    }

    pub fn is_selected(self) -> bool {
        !matches!(self, Self::Unset)
    }

    pub fn from_label(input: &str) -> Option<Self> {
        match input.to_lowercase().replace(['_', '-'], " ").trim() {
            "not selected" | "none" | "unset" => Some(Self::Unset),
            "ollama" => Some(Self::Ollama),
            "openai" | "open ai" => Some(Self::OpenAi),
            "gemini" | "google gemini" => Some(Self::Gemini),
            "anthropic" => Some(Self::Anthropic),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    #[default]
    CodexDark,
    Nord,
    Sunset,
    Dracula,
    HighContrast,
    Light,
}

impl ThemeChoice {
    pub fn all() -> &'static [Self] {
        &[
            Self::CodexDark,
            Self::Nord,
            Self::Sunset,
            Self::Dracula,
            Self::HighContrast,
            Self::Light,
        ]
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().replace(['_', '-'], " ").trim() {
            "codex dark" | "codex" | "codexdark" => Some(Self::CodexDark),
            "nord" => Some(Self::Nord),
            "sunset" => Some(Self::Sunset),
            "dracula" => Some(Self::Dracula),
            "high contrast" | "highcontrast" => Some(Self::HighContrast),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CodexDark => "Codex dark",
            Self::Nord => "Nord",
            Self::Sunset => "Sunset",
            Self::Dracula => "Dracula",
            Self::HighContrast => "High contrast",
            Self::Light => "Light",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::CodexDark => Self::Nord,
            Self::Nord => Self::Sunset,
            Self::Sunset => Self::Dracula,
            Self::Dracula => Self::HighContrast,
            Self::HighContrast => Self::Light,
            Self::Light => Self::CodexDark,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::CodexDark => Self::Light,
            Self::Nord => Self::CodexDark,
            Self::Sunset => Self::Nord,
            Self::Dracula => Self::Sunset,
            Self::HighContrast => Self::Dracula,
            Self::Light => Self::HighContrast,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum HistoryLimit {
    Small,
    #[default]
    Medium,
    Large,
}

impl HistoryLimit {
    pub fn value(self) -> usize {
        match self {
            Self::Small => 20,
            Self::Medium => 40,
            Self::Large => 80,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "20",
            Self::Medium => "40",
            Self::Large => "80",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Small => Self::Medium,
            Self::Medium => Self::Large,
            Self::Large => Self::Small,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Small => Self::Large,
            Self::Medium => Self::Small,
            Self::Large => Self::Medium,
        }
    }
}

fn default_true() -> bool {
    true
}
