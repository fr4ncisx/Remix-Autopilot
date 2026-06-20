#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffContext {
    pub status: String,
    pub stat: String,
    pub diff: String,
    pub truncated: bool,
}

impl DiffContext {
    pub fn is_empty(&self) -> bool {
        self.status.trim().is_empty() && self.stat.trim().is_empty() && self.diff.trim().is_empty()
    }

    pub fn truncation_warning(&self, language: &str) -> String {
        if !self.truncated {
            return String::new();
        }
        let lang = language.to_lowercase();
        match lang.trim() {
            "spanish" | "español" | "espanol" => "\n\n[¡ADVERTENCIA!: El diff de código detallado fue truncado por límites de tamaño de la ventana de contexto. Considera la lista completa de archivos modificados en las secciones STATUS y STAT para guiar tu descripción general, aunque el diff detallado de algunos archivos no se muestre por completo.]\n".to_string(),
            _ => "\n\n[WARNING: The detailed code diff was truncated due to context window size limits. Rely on the complete file list in the STATUS and STAT sections to guide your general description, even if the detailed code diff for some files is not fully shown.]\n".to_string(),
        }
    }

    pub fn summary(&self, language: &str) -> String {
        let mut output = String::new();
        if !self.status.trim().is_empty() {
            output.push_str(&humanized_status(&self.status, language));
            output.push('\n');
        }
        if !self.stat.trim().is_empty() {
            output.push_str(self.stat.trim());
            output.push('\n');
        }
        if self.truncated {
            let lang = language.to_lowercase();
            let warning = match lang.trim() {
                "spanish" | "español" | "espanol" => {
                    "\n[ADVERTENCIA: Los cambios detallados (diff) fueron truncados debido al límite de tamaño de la ventana de contexto.]"
                }
                _ => {
                    "\n[WARNING: Detailed changes (diff) were truncated due to context size limits.]"
                }
            };
            output.push_str(warning);
        }
        output.trim().to_string()
    }
}

pub fn humanized_status(status: &str, language: &str) -> String {
    let spanish = matches!(
        language.to_lowercase().trim(),
        "spanish" | "español" | "espanol"
    );
    status
        .lines()
        .filter_map(|line| humanize_status_line(line, spanish))
        .collect::<Vec<_>>()
        .join("\n")
}

fn humanize_status_line(line: &str, spanish: bool) -> Option<String> {
    let trimmed = line.trim_end();
    if trimmed.is_empty() {
        return None;
    }

    let (code, path) = if trimmed.len() >= 3 {
        trimmed.split_at(2)
    } else {
        (trimmed, "")
    };
    let path = path.trim();
    let label = match code {
        "??" => {
            if spanish {
                "sin tracking"
            } else {
                "untracked"
            }
        }
        "!!" => {
            if spanish {
                "ignorado"
            } else {
                "ignored"
            }
        }
        " M" | "M " | "MM" => {
            if spanish {
                "modificado"
            } else {
                "modified"
            }
        }
        "A " | " A" | "AM" => {
            if spanish {
                "agregado"
            } else {
                "added"
            }
        }
        "D " | " D" | "MD" => {
            if spanish {
                "eliminado"
            } else {
                "deleted"
            }
        }
        "R " | " R" | "RM" => {
            if spanish {
                "renombrado"
            } else {
                "renamed"
            }
        }
        "C " | " C" => {
            if spanish {
                "copiado"
            } else {
                "copied"
            }
        }
        "UU" | "AA" | "DD" | "AU" | "UA" | "DU" | "UD" => {
            if spanish {
                "conflicto"
            } else {
                "conflict"
            }
        }
        _ => {
            if spanish {
                "cambiado"
            } else {
                "changed"
            }
        }
    };

    let path = if path.is_empty() { trimmed } else { path };
    Some(format!("- {}: {}", label, path))
}

pub fn truncate_diff(diff: String, max_chars: usize) -> (String, bool) {
    if diff.chars().count() <= max_chars {
        return (diff, false);
    }

    (diff.chars().take(max_chars).collect(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_large_diff() {
        let (diff, truncated) = truncate_diff("abcdef".to_string(), 3);
        assert_eq!(diff, "abc");
        assert!(truncated);
    }

    #[test]
    fn leaves_small_diff_intact() {
        let (diff, truncated) = truncate_diff("abc".to_string(), 3);
        assert_eq!(diff, "abc");
        assert!(!truncated);
    }

    #[test]
    fn summary_humanizes_untracked_status() {
        let context = DiffContext {
            status: "?? src/new.rs".to_string(),
            ..Default::default()
        };

        assert!(
            context
                .summary("English")
                .contains("- untracked: src/new.rs")
        );
    }
}
