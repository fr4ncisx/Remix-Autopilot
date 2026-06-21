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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_all_returns_four_variants() {
        assert_eq!(LlmProviderKind::all().len(), 4);
    }

    #[test]
    fn provider_kind_label_matches_name() {
        assert_eq!(LlmProviderKind::Unset.label(), "Not selected");
        assert_eq!(LlmProviderKind::Ollama.label(), "Ollama");
        assert_eq!(LlmProviderKind::OpenAi.label(), "OpenAI");
        assert_eq!(LlmProviderKind::Gemini.label(), "Gemini");
        assert_eq!(LlmProviderKind::Anthropic.label(), "Anthropic");
    }

    #[test]
    fn provider_kind_slug_is_lowercase() {
        assert_eq!(LlmProviderKind::Unset.slug(), "unset");
        assert_eq!(LlmProviderKind::Ollama.slug(), "ollama");
        assert_eq!(LlmProviderKind::OpenAi.slug(), "openai");
        assert_eq!(LlmProviderKind::Gemini.slug(), "gemini");
        assert_eq!(LlmProviderKind::Anthropic.slug(), "anthropic");
    }

    #[test]
    fn provider_kind_uses_api_key_only_for_cloud() {
        assert!(!LlmProviderKind::Unset.uses_api_key());
        assert!(!LlmProviderKind::Ollama.uses_api_key());
        assert!(LlmProviderKind::OpenAi.uses_api_key());
        assert!(LlmProviderKind::Gemini.uses_api_key());
        assert!(LlmProviderKind::Anthropic.uses_api_key());
    }

    #[test]
    fn provider_kind_supports_model_listing_for_non_unset() {
        assert!(!LlmProviderKind::Unset.supports_model_listing());
        assert!(LlmProviderKind::Ollama.supports_model_listing());
        assert!(LlmProviderKind::OpenAi.supports_model_listing());
        assert!(LlmProviderKind::Gemini.supports_model_listing());
        assert!(LlmProviderKind::Anthropic.supports_model_listing());
    }

    #[test]
    fn provider_kind_default_base_url_for_each() {
        assert_eq!(LlmProviderKind::Unset.default_base_url(), None);
        assert_eq!(
            LlmProviderKind::Ollama.default_base_url(),
            Some("http://localhost:11434")
        );
        assert_eq!(
            LlmProviderKind::OpenAi.default_base_url(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(
            LlmProviderKind::Gemini.default_base_url(),
            Some("https://generativelanguage.googleapis.com/v1beta")
        );
        assert_eq!(
            LlmProviderKind::Anthropic.default_base_url(),
            Some("https://api.anthropic.com/v1")
        );
    }

    #[test]
    fn provider_kind_is_selected_for_non_unset() {
        assert!(!LlmProviderKind::Unset.is_selected());
        assert!(LlmProviderKind::Ollama.is_selected());
        assert!(LlmProviderKind::OpenAi.is_selected());
        assert!(LlmProviderKind::Gemini.is_selected());
        assert!(LlmProviderKind::Anthropic.is_selected());
    }

    #[test]
    fn provider_kind_from_label_normalizes_case() {
        assert_eq!(
            LlmProviderKind::from_label("OLLAMA"),
            Some(LlmProviderKind::Ollama)
        );
        assert_eq!(
            LlmProviderKind::from_label("OpenAi"),
            Some(LlmProviderKind::OpenAi)
        );
        assert_eq!(
            LlmProviderKind::from_label("open_ai"),
            Some(LlmProviderKind::OpenAi)
        );
        assert_eq!(
            LlmProviderKind::from_label("open-ai"),
            Some(LlmProviderKind::OpenAi)
        );
        assert_eq!(
            LlmProviderKind::from_label("GEMINI"),
            Some(LlmProviderKind::Gemini)
        );
        assert_eq!(
            LlmProviderKind::from_label("Anthropic"),
            Some(LlmProviderKind::Anthropic)
        );
    }

    #[test]
    fn provider_kind_from_label_alias_none_unset() {
        assert_eq!(
            LlmProviderKind::from_label("none"),
            Some(LlmProviderKind::Unset)
        );
        assert_eq!(
            LlmProviderKind::from_label("unset"),
            Some(LlmProviderKind::Unset)
        );
        assert_eq!(
            LlmProviderKind::from_label("not selected"),
            Some(LlmProviderKind::Unset)
        );
    }

    #[test]
    fn provider_kind_from_label_alias_google_gemini() {
        assert_eq!(
            LlmProviderKind::from_label("google gemini"),
            Some(LlmProviderKind::Gemini)
        );
    }

    #[test]
    fn provider_kind_from_label_invalid_returns_none() {
        assert_eq!(LlmProviderKind::from_label("chatgpt"), None);
        assert_eq!(LlmProviderKind::from_label(""), None);
        assert_eq!(LlmProviderKind::from_label("invalid"), None);
    }

    #[test]
    fn theme_choice_all_returns_six_variants() {
        assert_eq!(ThemeChoice::all().len(), 6);
    }

    #[test]
    fn theme_choice_next_wraps_around() {
        assert_eq!(ThemeChoice::CodexDark.next(), ThemeChoice::Nord);
        assert_eq!(ThemeChoice::Nord.next(), ThemeChoice::Sunset);
        assert_eq!(ThemeChoice::Sunset.next(), ThemeChoice::Dracula);
        assert_eq!(ThemeChoice::Dracula.next(), ThemeChoice::HighContrast);
        assert_eq!(ThemeChoice::HighContrast.next(), ThemeChoice::Light);
        assert_eq!(ThemeChoice::Light.next(), ThemeChoice::CodexDark);
    }

    #[test]
    fn theme_choice_previous_wraps_around() {
        assert_eq!(ThemeChoice::Light.previous(), ThemeChoice::HighContrast);
        assert_eq!(ThemeChoice::CodexDark.previous(), ThemeChoice::Light);
        assert_eq!(ThemeChoice::Nord.previous(), ThemeChoice::CodexDark);
    }

    #[test]
    fn theme_choice_from_str_aliases() {
        assert_eq!(ThemeChoice::from_str("codex"), Some(ThemeChoice::CodexDark));
        assert_eq!(
            ThemeChoice::from_str("codexdark"),
            Some(ThemeChoice::CodexDark)
        );
        assert_eq!(
            ThemeChoice::from_str("codex dark"),
            Some(ThemeChoice::CodexDark)
        );
        assert_eq!(
            ThemeChoice::from_str("highcontrast"),
            Some(ThemeChoice::HighContrast)
        );
        assert_eq!(
            ThemeChoice::from_str("high contrast"),
            Some(ThemeChoice::HighContrast)
        );
        assert_eq!(ThemeChoice::from_str("NORD"), Some(ThemeChoice::Nord));
        assert_eq!(ThemeChoice::from_str("Dracula"), Some(ThemeChoice::Dracula));
        assert_eq!(ThemeChoice::from_str("light"), Some(ThemeChoice::Light));
    }

    #[test]
    fn theme_choice_from_str_invalid_returns_none() {
        assert_eq!(ThemeChoice::from_str("neon"), None);
        assert_eq!(ThemeChoice::from_str(""), None);
        assert_eq!(ThemeChoice::from_str("monokai"), None);
    }

    #[test]
    fn theme_choice_label_matches_name() {
        assert_eq!(ThemeChoice::CodexDark.label(), "Codex dark");
        assert_eq!(ThemeChoice::Nord.label(), "Nord");
        assert_eq!(ThemeChoice::Sunset.label(), "Sunset");
        assert_eq!(ThemeChoice::Dracula.label(), "Dracula");
        assert_eq!(ThemeChoice::HighContrast.label(), "High contrast");
        assert_eq!(ThemeChoice::Light.label(), "Light");
    }

    #[test]
    fn history_limit_value_matches_label() {
        assert_eq!(HistoryLimit::Small.value(), 20);
        assert_eq!(HistoryLimit::Small.label(), "20");
        assert_eq!(HistoryLimit::Medium.value(), 40);
        assert_eq!(HistoryLimit::Medium.label(), "40");
        assert_eq!(HistoryLimit::Large.value(), 80);
        assert_eq!(HistoryLimit::Large.label(), "80");
    }

    #[test]
    fn history_limit_next_wraps_around() {
        assert_eq!(HistoryLimit::Small.next(), HistoryLimit::Medium);
        assert_eq!(HistoryLimit::Medium.next(), HistoryLimit::Large);
        assert_eq!(HistoryLimit::Large.next(), HistoryLimit::Small);
    }

    #[test]
    fn history_limit_previous_wraps_around() {
        assert_eq!(HistoryLimit::Small.previous(), HistoryLimit::Large);
        assert_eq!(HistoryLimit::Medium.previous(), HistoryLimit::Small);
        assert_eq!(HistoryLimit::Large.previous(), HistoryLimit::Medium);
    }

    #[test]
    fn config_default_has_expected_values() {
        let config = Config::default();
        assert_eq!(config.provider, LlmProviderKind::Unset);
        assert_eq!(config.base_url, None);
        assert_eq!(config.model, None);
        assert_eq!(config.language, "English");
        assert!(!config.staged_only);
        assert!(config.auto_setup_repo);
        assert!(config.prompt_push_after_commit);
        assert_eq!(config.theme, ThemeChoice::CodexDark);
        assert_eq!(config.history_limit, HistoryLimit::Medium);
    }

    #[test]
    fn config_serde_round_trip() {
        let config = Config::default();
        let json = serde_json::to_string(&config).unwrap();
        let restored: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.provider, config.provider);
        assert_eq!(restored.language, config.language);
        assert_eq!(restored.theme, config.theme);
        assert_eq!(restored.history_limit, config.history_limit);
    }
}
