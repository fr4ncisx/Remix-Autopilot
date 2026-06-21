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

    pub fn file_line_stats(&self) -> std::collections::HashMap<String, (usize, usize)> {
        let mut stats = std::collections::HashMap::new();
        let mut current_file = None;

        for line in self.diff.lines() {
            if line.starts_with("diff --git ") {
                if let Some(pos) = line.find(" b/") {
                    let file = line[pos + 3..].to_string();
                    current_file = Some(file.clone());
                    stats.entry(file).or_insert((0, 0));
                }
            } else if let Some(path) = line.strip_prefix("+++ b/") {
                let path = path.trim().to_string();
                if path != "/dev/null" {
                    current_file = Some(path.clone());
                    stats.entry(path).or_insert((0, 0));
                }
            } else if let Some(path) = line.strip_prefix("--- a/") {
                let path = path.trim().to_string();
                if path != "/dev/null" {
                    current_file = Some(path.clone());
                    stats.entry(path).or_insert((0, 0));
                }
            } else if let Some(cf) = &current_file {
                if line.starts_with('+') && !line.starts_with("+++") {
                    stats.entry(cf.clone()).and_modify(|(add, _)| *add += 1).or_insert((1, 0));
                } else if line.starts_with('-') && !line.starts_with("---") {
                    stats.entry(cf.clone()).and_modify(|(_, del)| *del += 1).or_insert((0, 1));
                }
            }
        }
        stats
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
    fn file_line_stats_parses_diff_correctly() {
        let context = DiffContext {
            diff: "\
diff --git a/src/main.rs b/src/main.rs
index 123456..789012 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
+added line 1
+added line 2
-removed line 1
 unmodified line
--- /dev/null
+++ b/src/new_file.rs
@@ -0,0 +1 @@
+untracked added line
".to_string(),
            ..Default::default()
        };

        let stats = context.file_line_stats();
        assert_eq!(stats.get("src/main.rs"), Some(&(2, 1)));
        assert_eq!(stats.get("src/new_file.rs"), Some(&(1, 0)));
    }
}
