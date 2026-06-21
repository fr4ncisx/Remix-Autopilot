#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Help,
    Exit,
    Config,
    Provider,
    Model,
    Lang(Option<String>),
    Switch,
    Staged,
    Diff,
    DeprecatedDryRun,
    Commit,
    Push,
    Pr,
    Explain,
    Review,
    Status,
    Log,
    Setup,
    Theme,
    Reset,
    Resolve,
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub command: &'static str,
    pub description: &'static str,
    pub intent: Intent,
    pub score: u8,
}

#[derive(Debug, Clone)]
pub enum IntentDecision {
    Certain(Intent),
    Unknown,
}

pub struct IntentParser;

impl IntentParser {
    pub fn parse(input: &str) -> IntentDecision {
        let trimmed = input.trim();
        if trimmed.is_empty() || !trimmed.starts_with('/') {
            return IntentDecision::Unknown;
        }
        if trimmed == "/dry-run" {
            return IntentDecision::Certain(Intent::DeprecatedDryRun);
        }

        command_specs()
            .iter()
            .find(|spec| spec.command == trimmed)
            .map(|spec| IntentDecision::Certain(spec.intent.clone()))
            .unwrap_or(IntentDecision::Unknown)
    }
}

pub fn slash_suggestions(input: &str) -> Vec<Suggestion> {
    let query = normalize_input(input.trim_start_matches('/'));
    let mut suggestions = command_specs()
        .iter()
        .map(|spec| Suggestion {
            command: spec.command,
            description: spec.description,
            intent: spec.intent.clone(),
            score: if query.is_empty() {
                80
            } else {
                spec.score(&query)
            },
        })
        .filter(|suggestion| suggestion.score >= 55)
        .collect::<Vec<_>>();
    suggestions.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.command.cmp(right.command))
    });
    suggestions
}

#[derive(Clone)]
struct CommandSpec {
    command: &'static str,
    description: &'static str,
    intent: Intent,
}

impl CommandSpec {
    fn score(&self, input: &str) -> u8 {
        let cmd_name = self.command.trim_start_matches('/');
        let query = input.trim_start_matches('/');
        if query.is_empty() {
            return 80;
        }
        if query == cmd_name {
            return 100;
        }
        if cmd_name.starts_with(query) {
            let match_len = query.len();
            let cmd_len = cmd_name.len().max(1);
            return 90 + ((match_len * 10) / cmd_len) as u8;
        }
        0
    }
}

fn command_specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            command: "/commit",
            description: "generate and create a commit",
            intent: Intent::Commit,
        },
        CommandSpec {
            command: "/diff",
            description: "show changed files",
            intent: Intent::Diff,
        },
        CommandSpec {
            command: "/provider",
            description: "choose the active AI provider",
            intent: Intent::Provider,
        },
        CommandSpec {
            command: "/model",
            description: "choose or set the active model",
            intent: Intent::Model,
        },
        CommandSpec {
            command: "/lang",
            description: "change UI and AI response language",
            intent: Intent::Lang(None),
        },
        CommandSpec {
            command: "/switch",
            description: "switch branches",
            intent: Intent::Switch,
        },
        CommandSpec {
            command: "/staged",
            description: "toggle staged-only mode",
            intent: Intent::Staged,
        },
        CommandSpec {
            command: "/push",
            description: "push current branch",
            intent: Intent::Push,
        },
        CommandSpec {
            command: "/pr",
            description: "create a pull request",
            intent: Intent::Pr,
        },
        CommandSpec {
            command: "/pull-request",
            description: "create a pull request",
            intent: Intent::Pr,
        },
        CommandSpec {
            command: "/explain",
            description: "explain the diff",
            intent: Intent::Explain,
        },
        CommandSpec {
            command: "/review",
            description: "review the diff",
            intent: Intent::Review,
        },
        CommandSpec {
            command: "/status",
            description: "summarize changed files with AI",
            intent: Intent::Status,
        },
        CommandSpec {
            command: "/log",
            description: "browse commit history",
            intent: Intent::Log,
        },
        CommandSpec {
            command: "/history",
            description: "browse commit history",
            intent: Intent::Log,
        },
        CommandSpec {
            command: "/setup",
            description: "initialize repo or remote",
            intent: Intent::Setup,
        },
        CommandSpec {
            command: "/theme",
            description: "change UI theme color palette",
            intent: Intent::Theme,
        },
        CommandSpec {
            command: "/reset",
            description: "reset configuration and disconnect origin safely",
            intent: Intent::Reset,
        },
        CommandSpec {
            command: "/resolve",
            description: "resolve the next pending setup or dependency issue",
            intent: Intent::Resolve,
        },
        CommandSpec {
            command: "/config",
            description: "open interactive settings",
            intent: Intent::Config,
        },
        CommandSpec {
            command: "/help",
            description: "show help",
            intent: Intent::Help,
        },
        CommandSpec {
            command: "/exit",
            description: "quit",
            intent: Intent::Exit,
        },
    ]
}

fn normalize_input(input: &str) -> String {
    input
        .trim()
        .trim_start_matches('/')
        .to_lowercase()
        .replace('á', "a")
        .replace('é', "e")
        .replace('í', "i")
        .replace('ó', "o")
        .replace('ú', "u")
        .replace('ñ', "n")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch.is_whitespace() {
                ch
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_slash_intents() {
        assert!(matches!(
            IntentParser::parse("/commit"),
            IntentDecision::Certain(Intent::Commit)
        ));
        assert!(matches!(
            IntentParser::parse("/switch"),
            IntentDecision::Certain(Intent::Switch)
        ));
        assert!(matches!(
            IntentParser::parse("/review"),
            IntentDecision::Certain(Intent::Review)
        ));
        assert!(matches!(
            IntentParser::parse("/status"),
            IntentDecision::Certain(Intent::Status)
        ));
        assert!(matches!(
            IntentParser::parse("/log"),
            IntentDecision::Certain(Intent::Log)
        ));
        assert!(matches!(
            IntentParser::parse("/history"),
            IntentDecision::Certain(Intent::Log)
        ));
        assert!(matches!(
            IntentParser::parse("/pr"),
            IntentDecision::Certain(Intent::Pr)
        ));
        assert!(matches!(
            IntentParser::parse("/pull-request"),
            IntentDecision::Certain(Intent::Pr)
        ));
        assert!(matches!(
            IntentParser::parse("/reset"),
            IntentDecision::Certain(Intent::Reset)
        ));
        assert!(matches!(
            IntentParser::parse("/resolve"),
            IntentDecision::Certain(Intent::Resolve)
        ));
    }

    #[test]
    fn parses_lang_slash_intent() {
        assert!(matches!(
            IntentParser::parse("/lang"),
            IntentDecision::Certain(Intent::Lang(None))
        ));
    }

    #[test]
    fn rejects_natural_language_input() {
        assert!(matches!(
            IntentParser::parse("commit"),
            IntentDecision::Unknown
        ));
        assert!(matches!(
            IntentParser::parse("revisa cambios"),
            IntentDecision::Unknown
        ));
        assert!(matches!(
            IntentParser::parse("create pr"),
            IntentDecision::Unknown
        ));
        assert!(matches!(
            IntentParser::parse("cambia el idioma a español"),
            IntentDecision::Unknown
        ));
    }

    #[test]
    fn ranks_slash_suggestions_by_query() {
        let suggestions = slash_suggestions("/com");
        assert_eq!(suggestions[0].command, "/commit");

        let suggestions = slash_suggestions("/sw");
        assert_eq!(suggestions[0].command, "/switch");

        let suggestions = slash_suggestions("/res");
        assert!(matches!(suggestions[0].command, "/reset" | "/resolve"));
    }

    #[test]
    fn simulate_command_is_not_available() {
        let suggestions = slash_suggestions("/simulate");
        assert!(suggestions.iter().all(|item| item.command != "/simulate"));
        assert!(!matches!(
            IntentParser::parse("/simulate"),
            IntentDecision::Certain(_)
        ));
    }

    #[test]
    fn dry_run_is_deprecated_and_hidden_from_suggestions() {
        assert!(matches!(
            IntentParser::parse("/dry-run"),
            IntentDecision::Certain(Intent::DeprecatedDryRun)
        ));
        assert!(
            slash_suggestions("/")
                .iter()
                .all(|item| item.command != "/dry-run")
        );
    }

    #[test]
    fn bare_slash_lists_all_available_commands() {
        let suggestions = slash_suggestions("/");
        assert!(suggestions.len() > 8);
        assert!(suggestions.iter().any(|item| item.command == "/commit"));
        assert!(suggestions.iter().any(|item| item.command == "/config"));
        assert!(suggestions.iter().any(|item| item.command == "/resolve"));
        assert!(suggestions.iter().any(|item| item.command == "/status"));
        assert!(suggestions.iter().any(|item| item.command == "/log"));
        assert!(suggestions.iter().any(|item| item.command == "/exit"));
    }
}
