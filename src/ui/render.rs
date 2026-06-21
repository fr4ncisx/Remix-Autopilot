use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::domain::ThemeChoice;
use crate::infrastructure::{
    BranchOption, BranchSource, DependencyKind, DependencyState, DependencyStatus, PackageManager,
};

use super::state::{
    ChatRole, Modal, OnboardingStep, TextInputKind, TuiApp, onboarding_dependency_actions,
};

fn ollama_running(app: &TuiApp) -> bool {
    app.ollama_health
        .as_ref()
        .is_some_and(|health| health.running)
}

pub fn draw(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = frame.area();
    let colors = palette(app.core.config.theme);
    let has_suggestions = app.modal.is_none() && app.input.starts_with('/');
    let area_width = (area.width as usize).max(1);

    let max_status_height = max_status_height(area.height);
    let status_height = responsive_status_height(app, area_width, max_status_height);
    let footer_height = keyboard_footer_height(area.height);

    let suggestion_sub = 3 + status_height + footer_height + 1;
    let suggestion_height = if has_suggestions {
        (app.suggestions.len() as u16 + 2).min(area.height.saturating_sub(suggestion_sub) / 2)
    } else {
        0
    };

    let mut constraints = vec![Constraint::Min(1)];
    if has_suggestions {
        constraints.push(Constraint::Length(suggestion_height));
    }
    constraints.push(Constraint::Length(3));
    constraints.push(Constraint::Length(status_height));
    constraints.push(Constraint::Length(footer_height));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let history_block = Block::default()
        .title(" Remix Autopilot ")
        .title_alignment(Alignment::Left)
        .borders(Borders::TOP)
        .border_style(Style::default().fg(colors.muted));
    let history_inner = history_block.inner(chunks[0]);
    frame.render_widget(history_block, chunks[0]);
    render_history(frame, app, history_inner);

    let input_index = if has_suggestions { 2 } else { 1 };
    if has_suggestions {
        render_suggestions(frame, app, chunks[1]);
    }
    render_input(frame, app, chunks[input_index]);
    render_status(frame, app, chunks[input_index + 1], status_height);
    render_keyboard_footer(frame, app, chunks[input_index + 2]);

    if let Some(modal) = &app.modal {
        render_modal(frame, app, modal);
    }
}

fn render_history(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let colors = palette(app.core.config.theme);
    let available_width = area.width as usize;
    let box_width = if available_width < 12 {
        available_width.max(1)
    } else {
        available_width.saturating_sub(2).max(12)
    };
    let content_width = box_width.saturating_sub(6).max(1);

    let mut all_lines: Vec<Line<'static>> = Vec::new();

    for entry in &app.history {
        let (role_label, pcolor) = match entry.role {
            ChatRole::User => ("> You", colors.user),
            ChatRole::Assistant => match app.execution_mode {
                crate::ui::state::ExecutionMode::Scout => ("Scout", colors.assistant),
                crate::ui::state::ExecutionMode::Autopilot => ("Autopilot", colors.assistant),
            },
            ChatRole::System => ("System", colors.system),
            ChatRole::Error => ("Error", colors.warning),
        };

        all_lines.push(Line::from(vec![Span::styled(
            format!(" {}", role_label),
            Style::default().fg(pcolor).bold(),
        )]));

        match entry.role {
            ChatRole::Assistant | ChatRole::Error => {
                let border_color = if let ChatRole::Error = entry.role {
                    Color::Rgb(239, 68, 68) // Soft red border for errors
                } else {
                    colors.accent
                };
                let bg_color = if let ChatRole::Error = entry.role {
                    Color::Rgb(127, 29, 29) // Dark red background for errors
                } else {
                    colors.modal_bg
                };
                let text_color = if let ChatRole::Error = entry.role {
                    Color::Rgb(254, 226, 226) // Soft red text for errors
                } else {
                    colors.text
                };

                let box_style = Style::default().fg(border_color).bg(bg_color);
                let text_style = Style::default().fg(text_color).bg(bg_color);

                all_lines.push(Line::styled(
                    format!("  ┌{}┐", "─".repeat(box_width.saturating_sub(2))),
                    box_style,
                ));

                for line in display_message_lines(entry) {
                    if line.trim().is_empty() {
                        all_lines.push(Line::styled(
                            format!("  │ {} │", "─".repeat(content_width)),
                            box_style,
                        ));
                    } else {
                        let chars = line.chars().collect::<Vec<_>>();
                        if chars.is_empty() {
                            let padded = " ".repeat(content_width);
                            all_lines.push(Line::from(vec![
                                Span::styled("  │ ", box_style),
                                Span::styled(padded, text_style),
                                Span::styled(" │", box_style),
                            ]));
                        } else {
                            let mut start = 0;
                            while start < chars.len() {
                                let end = (start + content_width).min(chars.len());
                                let chunk: String = chars[start..end].iter().collect();
                                let padded = format!("{:<width$}", chunk, width = content_width);
                                all_lines.push(Line::from(vec![
                                    Span::styled("  │ ", box_style),
                                    Span::styled(padded, text_style),
                                    Span::styled(" │", box_style),
                                ]));
                                start = end;
                            }
                        }
                    }
                }

                all_lines.push(Line::styled(
                    format!("  └{}┘", "─".repeat(box_width.saturating_sub(2))),
                    box_style,
                ));
            }
            _ => {
                for line in display_message_lines(entry) {
                    all_lines.push(Line::from(vec![
                        Span::styled("  │ ", Style::default().fg(pcolor)),
                        Span::styled(line, Style::default().fg(colors.text)),
                    ]));
                }
            }
        }

        all_lines.push(Line::raw(""));
    }

    let visible_height = area.height as usize;
    if visible_height == 0 {
        return;
    }

    let max_scroll = all_lines.len().saturating_sub(visible_height);
    let scroll = app.history_scroll.min(max_scroll);
    let show_scroll_hint = scroll > 0 && visible_height > 1;
    let content_height = if show_scroll_hint {
        visible_height.saturating_sub(1)
    } else {
        visible_height
    };
    let start = all_lines.len().saturating_sub(content_height + scroll);
    let end = (start + content_height).min(all_lines.len());

    let mut visible_lines = Vec::new();
    if show_scroll_hint {
        let hint = if is_spanish_language(&app.core.config.language) {
            "Leyendo historial - End vuelve al final"
        } else {
            "Reading history - End jumps to latest"
        };
        visible_lines.push(Line::styled(
            format!(" {} ", hint),
            Style::default()
                .fg(colors.hint)
                .bg(colors.status_bg)
                .add_modifier(Modifier::BOLD),
        ));
    }
    visible_lines.extend(all_lines[start..end].iter().cloned());

    let items = visible_lines
        .into_iter()
        .map(ListItem::new)
        .collect::<Vec<_>>();
    frame.render_widget(List::new(items), area);
}

fn display_message_lines(entry: &crate::ui::state::ChatEntry) -> Vec<String> {
    match entry.role {
        ChatRole::Assistant => markdown_display_lines(&entry.message),
        _ => {
            let lines = entry
                .message
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>();
            if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            }
        }
    }
}

fn markdown_display_lines(message: &str) -> Vec<String> {
    let mut in_fence = false;
    let mut lines = Vec::new();

    for raw in message.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            lines.push(raw.to_string());
        } else {
            lines.push(clean_markdown_line(raw));
        }
    }

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn clean_markdown_line(raw: &str) -> String {
    let mut line = raw.to_string();
    let trimmed_start = line.trim_start();
    let leading_spaces = line.len().saturating_sub(trimmed_start.len());

    if trimmed_start.starts_with('#') {
        line = trimmed_start
            .trim_start_matches('#')
            .trim_start()
            .to_string();
    } else if let Some(rest) = trimmed_start.strip_prefix("> ") {
        line = format!("{}{}", " ".repeat(leading_spaces), rest);
    } else if let Some(rest) = trimmed_start.strip_prefix('>') {
        line = format!("{}{}", " ".repeat(leading_spaces), rest.trim_start());
    } else if let Some(rest) = trimmed_start
        .strip_prefix("- ")
        .or_else(|| trimmed_start.strip_prefix("* "))
    {
        line = format!("{}• {}", " ".repeat(leading_spaces), rest);
    }

    line = replace_markdown_links(&line);
    line.replace("**", "")
        .replace("__", "")
        .replace(['`', '*'], "")
}

fn replace_markdown_links(line: &str) -> String {
    let chars = line.chars().collect::<Vec<_>>();
    let mut output = String::new();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] == '['
            && let Some(close_label_rel) = chars[index + 1..].iter().position(|ch| *ch == ']')
        {
            let close_label = index + 1 + close_label_rel;
            if chars.get(close_label + 1) == Some(&'(')
                && let Some(close_url_rel) =
                    chars[close_label + 2..].iter().position(|ch| *ch == ')')
            {
                let close_url = close_label + 2 + close_url_rel;
                let label = chars[index + 1..close_label].iter().collect::<String>();
                let url = chars[close_label + 2..close_url].iter().collect::<String>();
                if url.is_empty() || url == label {
                    output.push_str(&label);
                } else {
                    output.push_str(&format!("{} ({})", label, url));
                }
                index = close_url + 1;
                continue;
            }
        }
        output.push(chars[index]);
        index += 1;
    }

    output
}

fn render_suggestions(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let colors = palette(app.core.config.theme);
    let visible_height = area.height.saturating_sub(2) as usize;
    let total = app.suggestions.len();
    if total == 0 {
        return;
    }
    let selected = app.selected_suggestion.min(total.saturating_sub(1));

    let scroll_offset = if total > visible_height && selected >= visible_height {
        selected - visible_height + 1
    } else {
        0
    };

    let items = app
        .suggestions
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(index, suggestion)| {
            let marker = if index == selected { ">" } else { " " };
            let style = if index == selected {
                Style::default()
                    .fg(colors.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(colors.text)
            };
            ListItem::new(Line::from(vec![
                Span::styled(marker, style),
                Span::raw(" "),
                Span::styled(suggestion.command, style),
                Span::raw("  "),
                Span::styled(suggestion.description, Style::default().fg(colors.muted)),
            ]))
        })
        .collect::<Vec<_>>();

    let lang = app.core.config.language.to_lowercase();
    let list_title = match lang.trim() {
        "spanish" | "español" | "espanol" => "/ comandos",
        _ => "/ commands",
    };

    let list = List::new(items).block(
        Block::default()
            .title(list_title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(colors.border)),
    );
    frame.render_widget(list, area);
}

fn render_input(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let colors = palette(app.core.config.theme);
    let model = app.core.config.model.as_deref().unwrap_or("model");
    let repo = app.core.status().repo;
    let prompt_len = repo.chars().count() as u16 + 1 + model.chars().count() as u16 + 3;

    let lang = app.core.config.language.to_lowercase();
    let is_checking = app.ollama_health.is_none();
    let is_busy = app.busy;
    let spinner = crate::ui::state::SPINNER[app.spinner_frame as usize].to_string();
    let activity = if is_checking {
        let provider = app.core.provider_label();
        let loading_msg = match lang.trim() {
            "spanish" | "español" | "espanol" => {
                format!("Verificando conexión de {}...", provider)
            }
            _ => format!("Checking {} connection...", provider),
        };
        Some((spinner.clone(), loading_msg))
    } else if is_busy {
        Some((spinner.clone(), app.busy_message.clone()))
    } else {
        None
    };
    let activity_len = activity
        .as_ref()
        .map(|(spinner, message)| {
            spinner.chars().count() as u16 + 1 + message.chars().count() as u16
        })
        .unwrap_or(0);
    let has_activity = activity.is_some();

    let mut line_spans = vec![
        Span::styled(repo, Style::default().fg(colors.success).bold()),
        Span::styled("@", Style::default().fg(colors.muted)),
        Span::styled(model, Style::default().fg(colors.info).bold()),
        Span::styled(" > ", Style::default().fg(colors.muted)),
    ];

    if let Some((spinner, message)) = &activity {
        line_spans.push(Span::styled(
            format!("{} {}", spinner, message),
            Style::default().fg(colors.hint),
        ));
        if !app.input.is_empty() {
            line_spans.push(Span::styled(" │ ", Style::default().fg(colors.muted)));
            line_spans.push(Span::raw(app.input.as_str()));
        }
    } else if app.input.is_empty() {
        let placeholder = match lang.trim() {
            "spanish" | "español" | "espanol" => "Pregunta lo que sea o usa / para comandos",
            _ => "Ask anything or use / for commands",
        };
        line_spans.push(Span::styled(placeholder, Style::default().fg(colors.muted)));
    } else {
        line_spans.push(Span::raw(app.input.as_str()));
    }

    let line = Line::from(line_spans);

    frame.render_widget(Paragraph::new(line), area);

    if app.modal.is_none() && !is_busy && !is_checking {
        let mut cursor_x = area.x + prompt_len + activity_len;
        if has_activity && !app.input.is_empty() {
            cursor_x += 3;
        }
        cursor_x += app.input.chars().count() as u16;
        frame.set_cursor_position((cursor_x.min(area.x + area.width.saturating_sub(1)), area.y));
    }
}

fn render_status(frame: &mut Frame<'_>, app: &TuiApp, area: Rect, _height: u16) {
    let colors = palette(app.core.config.theme);
    let bg = Style::default().bg(colors.status_bg);
    let lines = responsive_status_lines(app, area.width as usize, colors, _height);
    frame.render_widget(Paragraph::new(lines).style(bg), area);
}

fn render_keyboard_footer(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let colors = palette(app.core.config.theme);
    let bg = Style::default().bg(colors.modal_bg);
    let line = keyboard_footer_line(app, area.width as usize, colors);
    frame.render_widget(Paragraph::new(vec![line]).style(bg), area);
}

fn max_status_height(terminal_height: u16) -> u16 {
    terminal_height.saturating_sub(6).clamp(1, 1)
}

fn keyboard_footer_height(_terminal_height: u16) -> u16 {
    1
}

fn responsive_status_height(app: &TuiApp, width: usize, max_height: u16) -> u16 {
    responsive_status_lines(app, width, palette(app.core.config.theme), max_height)
        .len()
        .max(1)
        .min(max_height as usize) as u16
}

fn responsive_status_lines(
    app: &TuiApp,
    width: usize,
    colors: Palette,
    _max_height: u16,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let status = app.core.status();
    let lang = app.core.config.language.to_lowercase();
    let lang_str = lang.trim();
    let sep = Span::styled(" · ", Style::default().fg(colors.muted));
    let compact = width < 56;
    let val_exec_mode = app.execution_mode.label(&app.core.config.language);
    let mode_color = match app.execution_mode {
        crate::ui::state::ExecutionMode::Autopilot => colors.success,
        crate::ui::state::ExecutionMode::Scout => colors.accent,
    };
    let context_label = match lang_str {
        "spanish" | "español" | "espanol" => "Contexto",
        _ => "Context",
    };
    let show_context_usage = app.core.config.provider == crate::domain::LlmProviderKind::Ollama;
    let context_item = if show_context_usage && let Some(usage) = app.last_context_usage {
        let percent = usage.percent().unwrap_or(0);
        let context_color = if percent >= 85 {
            colors.warning
        } else if percent >= 60 {
            colors.system
        } else {
            colors.success
        };
        vec![Span::styled(
            format!("{} {}%", context_label, percent),
            Style::default().fg(context_color).bold(),
        )]
    } else {
        Vec::new()
    };

    let mut items = vec![
        vec![provider_status_span(app, colors, compact)],
        vec![Span::styled(
            val_exec_mode.to_string(),
            Style::default().fg(mode_color).bold(),
        )],
        vec![Span::styled(
            app.core.config.language.clone(),
            Style::default().fg(colors.accent).bold(),
        )],
    ];
    if provider_status_ready(app) {
        items.insert(
            1,
            git_status_item(status.is_repo, status.has_origin, lang_str, colors, compact),
        );
    }
    if !context_item.is_empty() {
        items.insert(items.len().saturating_sub(1), context_item);
    }
    items.push(branch_status_item(
        &status.branch,
        lang_str,
        colors,
        compact,
    ));
    items.extend(degraded_capability_items(app, lang_str, colors));
    if show_context_usage
        && let Some(usage) = app.last_context_usage
        && usage.truncated
    {
        let truncated = match lang_str {
            "spanish" | "español" | "espanol" => "Diff truncado",
            _ => "Diff truncated",
        };
        items.push(vec![Span::styled(
            truncated,
            Style::default().fg(colors.warning).bold(),
        )]);
    }
    vec![single_status_line(items, Vec::new(), width, &sep)]
}

fn single_status_line(
    left_items: Vec<Vec<Span<'static>>>,
    right_items: Vec<Vec<Span<'static>>>,
    width: usize,
    sep: &Span<'static>,
) -> Line<'static> {
    let left = fit_status_items(&left_items, width, sep);
    let left_width = spans_width(&left);
    let mut right_count = right_items.len();
    let mut right = flatten_status_items(&right_items[..right_count], sep);

    while right_count > 0 && left_width + 1 + spans_width(&right) > width {
        right_count -= 1;
        right = flatten_status_items(&right_items[..right_count], sep);
    }

    let mut spans = left;
    let used = left_width + spans_width(&right);
    let spacer = if right.is_empty() {
        width.saturating_sub(used)
    } else {
        width.saturating_sub(used).max(1)
    };
    if spacer > 0 {
        spans.push(Span::raw(" ".repeat(spacer)));
    }
    spans.extend(right);
    Line::from(spans)
}

fn keyboard_footer_line(app: &TuiApp, width: usize, colors: Palette) -> Line<'static> {
    let is_spanish = is_spanish_language(&app.core.config.language);
    let shortcuts = match &app.modal {
        Some(Modal::OnboardingWizard { .. }) if is_spanish => {
            vec![
                "Setup",
                "↑↓ mover",
                "Enter seleccionar",
                "Esc idioma",
                "Ctrl+C salir",
            ]
        }
        Some(Modal::OnboardingWizard { .. }) => {
            vec![
                "Setup",
                "↑↓ move",
                "Enter select",
                "Esc language",
                "Ctrl+C quit",
            ]
        }
        Some(Modal::TextInput { .. }) if app.onboarding_active && is_spanish => vec![
            "Setup",
            "Escribir valor",
            "Enter confirmar",
            "Esc volver",
            "Ctrl+C salir",
        ],
        Some(Modal::TextInput { .. }) if app.onboarding_active => vec![
            "Setup",
            "Type value",
            "Enter confirm",
            "Esc back",
            "Ctrl+C quit",
        ],
        Some(Modal::Picker { .. }) if app.onboarding_active && is_spanish => vec![
            "Setup",
            "↑↓ mover",
            "Enter seleccionar",
            "Esc volver",
            "Ctrl+C salir",
        ],
        Some(Modal::Picker { .. }) if app.onboarding_active => {
            vec![
                "Setup",
                "↑↓ move",
                "Enter select",
                "Esc back",
                "Ctrl+C quit",
            ]
        }
        Some(Modal::CommandExecution { .. }) if app.onboarding_active && is_spanish => {
            vec!["Setup", "Enter revisar", "Ctrl+C salir"]
        }
        Some(Modal::CommandExecution { .. }) if app.onboarding_active => {
            vec!["Setup", "Enter review", "Ctrl+C quit"]
        }
        _ => vec![
            "F2 settings",
            "/ commands",
            "Shift+Tab mode",
            "Enter send",
            "Ctrl+C quit",
        ],
    };
    let items = shortcuts
        .into_iter()
        .map(|shortcut| {
            vec![Span::styled(
                shortcut.to_string(),
                Style::default().fg(colors.hint).bold(),
            )]
        })
        .collect::<Vec<_>>();
    single_status_line(
        items,
        Vec::new(),
        width,
        &Span::styled(" · ", Style::default().fg(colors.border)),
    )
}

fn is_spanish_language(language: &str) -> bool {
    matches!(
        language.to_lowercase().trim(),
        "spanish" | "español" | "espanol"
    )
}

fn fit_status_items(
    items: &[Vec<Span<'static>>],
    width: usize,
    sep: &Span<'static>,
) -> Vec<Span<'static>> {
    let mut fitted = Vec::new();
    let mut used = 0usize;
    for item in items {
        let item_width = spans_width(item);
        let sep_width = if fitted.is_empty() {
            0
        } else {
            span_width(sep)
        };
        if !fitted.is_empty() && used + sep_width + item_width > width {
            break;
        }
        if !fitted.is_empty() {
            fitted.push(sep.clone());
            used += sep_width;
        }
        fitted.extend(item.clone());
        used += item_width;
    }
    fitted
}

fn flatten_status_items(items: &[Vec<Span<'static>>], sep: &Span<'static>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for item in items {
        if !spans.is_empty() {
            spans.push(sep.clone());
        }
        spans.extend(item.clone());
    }
    spans
}

fn provider_status_span(app: &TuiApp, colors: Palette, compact: bool) -> Span<'static> {
    let chip_fg = Color::Rgb(15, 23, 42);
    let is_spanish = is_spanish_language(&app.core.config.language);
    if !app.core.config.provider.is_selected() {
        let label = if compact {
            "AI"
        } else if is_spanish {
            " configurar IA "
        } else {
            " set up AI "
        };
        return Span::styled(
            label,
            Style::default()
                .fg(chip_fg)
                .bg(Color::Rgb(250, 204, 21))
                .add_modifier(Modifier::BOLD),
        );
    }
    let provider = app.core.provider_label();
    let is_ollama = app.core.config.provider == crate::domain::LlmProviderKind::Ollama;
    if let Some(doctor) = app.dependency_doctor.as_ref() {
        match doctor.llm_provider.state {
            DependencyState::NotConfigured => {
                let label = provider_setup_label(provider, &doctor.llm_provider, is_spanish);
                return Span::styled(
                    format!(" {} ", label),
                    Style::default()
                        .fg(chip_fg)
                        .bg(Color::Rgb(250, 204, 21))
                        .add_modifier(Modifier::BOLD),
                );
            }
            DependencyState::Missing | DependencyState::NotRunning => {
                let label = provider_blocked_label(provider, &doctor.llm_provider, is_spanish);
                return Span::styled(
                    format!(" {} ", label),
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Rgb(220, 38, 38))
                        .add_modifier(Modifier::BOLD),
                );
            }
            DependencyState::Ready => {}
        }
    }
    if !is_ollama {
        let label = if compact {
            provider.to_string()
        } else if is_spanish {
            format!("Proveedor {}", provider)
        } else {
            format!("{} Provider", provider)
        };
        return Span::styled(
            format!(" {} ", label),
            Style::default()
                .fg(chip_fg)
                .bg(colors.success)
                .add_modifier(Modifier::BOLD),
        );
    }
    match app.ollama_health.as_ref() {
        None => Span::styled(
            format!(
                " {} ",
                if compact {
                    "Ollama".to_string()
                } else if is_spanish {
                    "validando Ollama".to_string()
                } else {
                    "checking Ollama".to_string()
                }
            ),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Some(health) if health.running => {
            let vram_info = if let Some(vram) = app.core.vram_mb {
                if vram >= 1024 {
                    format!(" ({:.1}GB)", vram as f64 / 1024.0)
                } else {
                    format!(" ({}MB)", vram)
                }
            } else {
                String::new()
            };
            let label = if compact {
                "Ollama".to_string()
            } else {
                format!("Ollama{}", vram_info)
            };
            Span::styled(
                format!(" {} ", label),
                Style::default()
                    .fg(chip_fg)
                    .bg(colors.success)
                    .add_modifier(Modifier::BOLD),
            )
        }
        Some(health) if health.installed => {
            let label = if compact {
                "Ollama off"
            } else if is_spanish {
                "Ollama no está activo"
            } else {
                "Ollama not running"
            };
            Span::styled(
                format!(" {} ", label),
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(220, 38, 38))
                    .add_modifier(Modifier::BOLD),
            )
        }
        Some(_) => Span::styled(
            format!(
                " {} ",
                if compact {
                    "Ollama"
                } else if is_spanish {
                    "instalar Ollama"
                } else {
                    "install Ollama"
                }
            ),
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(220, 38, 38))
                .add_modifier(Modifier::BOLD),
        ),
    }
}

fn provider_setup_label(provider: &str, issue: &DependencyStatus, is_spanish: bool) -> String {
    let detail = issue.detail.as_deref().unwrap_or_default().to_lowercase();
    if detail.contains("api key") || detail.contains("clave") {
        if is_spanish {
            format!("{} requiere clave API", provider)
        } else {
            format!("{} needs API key", provider)
        }
    } else if detail.contains("model") || detail.contains("modelo") {
        if is_spanish {
            format!("{} requiere modelo", provider)
        } else {
            format!("{} needs model", provider)
        }
    } else if provider.eq_ignore_ascii_case("Ollama") {
        if is_spanish {
            "Ollama requiere modelo".to_string()
        } else {
            "Ollama needs model".to_string()
        }
    } else if is_spanish {
        format!("{} requiere configuración", provider)
    } else {
        format!("{} needs setup", provider)
    }
}

fn provider_blocked_label(provider: &str, issue: &DependencyStatus, is_spanish: bool) -> String {
    let detail = issue.detail.as_deref().unwrap_or_default().to_lowercase();
    if detail.contains("401") || detail.contains("403") || detail.contains("unauthorized") {
        if is_spanish {
            "clave API rechazada".to_string()
        } else {
            "API key rejected".to_string()
        }
    } else if provider.eq_ignore_ascii_case("Ollama") {
        if is_spanish {
            "Ollama no está activo".to_string()
        } else {
            "Ollama not running".to_string()
        }
    } else if is_spanish {
        format!("{} no disponible", provider)
    } else {
        format!("{} unavailable", provider)
    }
}

fn provider_status_ready(app: &TuiApp) -> bool {
    app.dependency_doctor
        .as_ref()
        .map(|doctor| doctor.llm_provider.is_ready())
        .unwrap_or_else(|| app.core.config.provider.is_selected())
}

fn git_status_item(
    is_repo: bool,
    has_origin: bool,
    lang: &str,
    colors: Palette,
    compact: bool,
) -> Vec<Span<'static>> {
    if !is_repo {
        let label = if compact {
            match lang {
                "spanish" | "español" | "espanol" => " sin repo ",
                _ => " no repo ",
            }
        } else {
            match lang {
                "spanish" | "español" | "espanol" => " falta repo · usar /setup ",
                _ => " repo missing · run /setup ",
            }
        };
        return vec![Span::styled(
            label,
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(220, 38, 38))
                .add_modifier(Modifier::BOLD),
        )];
    }

    if has_origin {
        let label = if compact {
            match lang {
                "spanish" | "español" | "espanol" => " repo remoto ",
                _ => " repo remote ",
            }
        } else {
            match lang {
                "spanish" | "español" | "espanol" => " repo existe (remoto) ",
                _ => " repo exists (remote) ",
            }
        };
        return vec![Span::styled(
            label,
            Style::default()
                .fg(Color::Rgb(15, 23, 42))
                .bg(colors.success)
                .add_modifier(Modifier::BOLD),
        )];
    }

    let label = if compact {
        match lang {
            "spanish" | "español" | "espanol" => " repo local ",
            _ => " repo local ",
        }
    } else {
        match lang {
            "spanish" | "español" | "espanol" => " repo existe (local) ",
            _ => " repo exists (local) ",
        }
    };
    vec![Span::styled(
        label,
        Style::default()
            .fg(Color::Rgb(15, 23, 42))
            .bg(Color::Rgb(250, 204, 21))
            .add_modifier(Modifier::BOLD),
    )]
}

fn branch_status_item(
    branch: &str,
    lang: &str,
    colors: Palette,
    compact: bool,
) -> Vec<Span<'static>> {
    let label = if compact {
        ""
    } else {
        match lang {
            "spanish" | "español" | "espanol" => "rama",
            _ => "branch",
        }
    };
    let mut spans = Vec::new();
    if !label.is_empty() {
        spans.push(Span::styled(label, Style::default().fg(colors.info).bold()));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        format!(" {} ", branch),
        branch_name_style(branch, colors).add_modifier(Modifier::BOLD),
    ));
    spans
}

fn branch_name_style(branch: &str, colors: Palette) -> Style {
    if is_protected_branch(branch) {
        Style::default()
            .fg(Color::White)
            .bg(Color::Rgb(220, 38, 38))
    } else if matches!(branch.trim(), "" | "unknown") {
        Style::default().fg(colors.hint)
    } else {
        Style::default()
            .fg(Color::Rgb(15, 23, 42))
            .bg(colors.accent)
    }
}

fn degraded_capability_items(
    app: &TuiApp,
    lang: &str,
    _colors: Palette,
) -> Vec<Vec<Span<'static>>> {
    let Some(report) = app.dependency_doctor.as_ref() else {
        return Vec::new();
    };

    let mut items = Vec::new();

    match report.gh.state {
        DependencyState::Missing => items.push(capability_chip(
            match lang {
                "spanish" | "español" | "espanol" => " github no disponible ",
                _ => " github unavailable ",
            },
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(220, 38, 38))
                .add_modifier(Modifier::BOLD),
        )),
        DependencyState::NotConfigured => items.push(capability_chip(
            match lang {
                "spanish" | "español" | "espanol" => " PR requiere login ",
                _ => " PR auth needed ",
            },
            Style::default()
                .fg(Color::Rgb(15, 23, 42))
                .bg(Color::Rgb(250, 204, 21))
                .add_modifier(Modifier::BOLD),
        )),
        DependencyState::Ready | DependencyState::NotRunning => {}
    }

    items
}

fn capability_chip(label: &'static str, style: Style) -> Vec<Span<'static>> {
    vec![Span::styled(label, style)]
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(span_width).sum()
}

fn span_width(span: &Span<'_>) -> usize {
    span.content.chars().count()
}

fn branch_switch_item(
    branch: &BranchOption,
    is_selected: bool,
    current_suffix: &str,
    colors: Palette,
) -> ListItem<'static> {
    let marker_style = if is_selected {
        Style::default().fg(colors.accent).bold()
    } else {
        Style::default().fg(colors.hint)
    };
    let source_label = match branch.source {
        BranchSource::Remote => "origin/",
        BranchSource::Local => "",
    };
    let branch_style = branch_switch_branch_style(branch, colors).bold();

    let mut spans = vec![
        Span::styled(if is_selected { ">" } else { " " }, marker_style),
        Span::raw(" "),
        Span::styled(source_label.to_string(), Style::default().fg(colors.muted)),
        Span::styled(branch.name.clone(), branch_style),
    ];
    if branch.is_current {
        spans.push(Span::styled(
            current_suffix.to_string(),
            Style::default().fg(colors.accent).bold(),
        ));
    }

    ListItem::new(Line::from(spans))
}

fn branch_switch_branch_style(branch: &BranchOption, colors: Palette) -> Style {
    if branch.is_protected() {
        Style::default().fg(Color::Rgb(239, 68, 68))
    } else {
        Style::default().fg(colors.success)
    }
}

fn is_protected_branch(branch: &str) -> bool {
    matches!(branch.trim(), "main" | "master")
}

fn render_onboarding_modal(
    frame: &mut Frame<'_>,
    app: &TuiApp,
    area: Rect,
    step: &OnboardingStep,
    selected: usize,
    colors: Palette,
) {
    let is_spanish = is_spanish_language(&app.core.config.language);
    let outer = modal_block(
        if is_spanish {
            "Setup guiado"
        } else {
            "Guided setup"
        },
        colors,
    );
    frame.render_widget(outer.clone(), area);
    let inner = outer.inner(area);
    let content = onboarding_step_content(step, &app.core.config.language);

    if inner.width >= 72 && inner.height >= 14 {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(24),
                Constraint::Length(1),
                Constraint::Min(42),
            ])
            .split(inner);
        let sep = Paragraph::new((0..chunks[0].height).map(|_| "│\n").collect::<String>())
            .style(Style::default().fg(colors.border));
        frame.render_widget(sep, chunks[1]);
        frame.render_widget(
            Paragraph::new(onboarding_progress_lines(app, step, colors)),
            chunks[0],
        );
        render_onboarding_detail(frame, chunks[2], &content, selected, is_spanish, colors);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(inner.height.saturating_sub(9).clamp(4, 8)),
                Constraint::Length(1),
                Constraint::Min(8),
            ])
            .split(inner);
        frame.render_widget(
            Paragraph::new(onboarding_progress_lines(app, step, colors)),
            chunks[0],
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                "─".repeat(inner.width as usize),
                Style::default().fg(colors.border),
            )),
            chunks[1],
        );
        render_onboarding_detail(frame, chunks[2], &content, selected, is_spanish, colors);
    }
}

fn render_onboarding_detail(
    frame: &mut Frame<'_>,
    area: Rect,
    content: &OnboardingContent,
    selected: usize,
    is_spanish: bool,
    colors: Palette,
) {
    let compact = area.height <= 16;
    let title_height = if compact { 2 } else { 3 };
    let help_height = if compact { 1 } else { 2 };
    let action_height = if compact {
        (content.actions.len() as u16).saturating_add(1).clamp(2, 4)
    } else {
        ((content.actions.len() as u16) * 2)
            .saturating_add(1)
            .clamp(3, 8)
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(title_height),
            Constraint::Min(1),
            Constraint::Length(action_height),
            Constraint::Length(help_height),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                content.title.clone(),
                Style::default().fg(content.title_color(colors)).bold(),
            )),
            if compact {
                Line::from(Span::raw(""))
            } else {
                Line::from(Span::styled(
                    if is_spanish {
                        "Paso requerido para continuar."
                    } else {
                        "Required step to continue."
                    },
                    Style::default().fg(colors.muted),
                ))
            },
        ]),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(if compact {
            onboarding_detail_compact_lines(content, is_spanish, colors)
        } else {
            onboarding_detail_lines(content, is_spanish, colors)
        })
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(colors.text)),
        chunks[1],
    );
    frame.render_widget(
        if compact {
            onboarding_action_compact_list(&content.actions, selected, colors)
        } else {
            onboarding_action_list(&content.actions, selected, colors)
        },
        chunks[2],
    );
    frame.render_widget(
        Paragraph::new(onboarding_help_lines(is_spanish, colors, compact))
            .style(Style::default().fg(colors.hint)),
        chunks[3],
    );
}

fn onboarding_help_lines(is_spanish: bool, colors: Palette, compact: bool) -> Vec<Line<'static>> {
    let help = if is_spanish {
        "↑↓ elegir opción   Enter confirmar   Esc volver a idioma   Ctrl+C salir"
    } else {
        "↑↓ choose option   Enter confirm   Esc back to language   Ctrl+C quit"
    };
    if compact {
        return vec![Line::from(Span::raw(help))];
    }
    vec![
        Line::from(Span::styled(
            if is_spanish { "Atajos" } else { "Shortcuts" },
            Style::default().fg(colors.accent).bold(),
        )),
        Line::from(Span::raw(help)),
    ]
}

fn render_modal(frame: &mut Frame<'_>, app: &TuiApp, modal: &Modal) {
    let colors = palette(app.core.config.theme);
    let area = match modal {
        Modal::OnboardingWizard { .. } => centered_rect(82, 78, frame.area()),
        _ => centered_rect(68, 60, frame.area()),
    };
    if area.width < 10 || area.height < 5 {
        return;
    }
    frame.render_widget(Clear, area);
    let lang = app.core.config.language.to_lowercase();

    match modal {
        Modal::OnboardingWizard { step, selected } => {
            render_onboarding_modal(frame, app, area, step, *selected, colors);
        }
        Modal::Settings { selected } => {
            let outer_block = modal_block(
                match lang.trim() {
                    "spanish" | "español" | "espanol" => "Configuración",
                    _ => "Settings",
                },
                colors,
            );
            frame.render_widget(outer_block.clone(), area);
            let inner_area = outer_block.inner(area);

            // Split inner_area vertically:
            // - Top: settings list and shortcuts
            // - Divider
            // - Bottom: description
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(5),
                    Constraint::Length(1), // separator
                    Constraint::Length(3), // description
                ])
                .split(inner_area);

            // Draw horizontal line separator
            let sep_line = Paragraph::new(Span::styled(
                "─".repeat(inner_area.width as usize),
                Style::default().fg(colors.border),
            ));
            frame.render_widget(sep_line, chunks[1]);

            // Split top vertically:
            // - Left 55% / Top: settings list
            // - Separator
            // - Right 45% / Bottom: shortcuts
            let top_chunks = if inner_area.width >= 75 {
                Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(55),
                        Constraint::Length(1),
                        Constraint::Percentage(45),
                    ])
                    .split(chunks[0])
            } else {
                Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(6),
                        Constraint::Length(1),
                        Constraint::Min(5),
                    ])
                    .split(chunks[0])
            };

            // Draw separator
            if inner_area.width >= 75 {
                let v_sep_text = (0..top_chunks[0].height).map(|_| "│\n").collect::<String>();
                let v_sep = Paragraph::new(v_sep_text).style(Style::default().fg(colors.border));
                frame.render_widget(v_sep, top_chunks[1]);
            } else {
                let h_sep = Paragraph::new(Span::styled(
                    "─".repeat(top_chunks[1].width as usize),
                    Style::default().fg(colors.border),
                ));
                frame.render_widget(h_sep, top_chunks[1]);
            }

            // Render Left: Settings List
            let rows = settings_rows(app);
            let items = rows
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    let is_selected = index == *selected;
                    let marker = if is_selected { ">" } else { " " };
                    let style = if is_selected {
                        Style::default().fg(colors.accent).bold()
                    } else {
                        Style::default().fg(colors.text)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(marker, style),
                        Span::raw(" "),
                        Span::styled(row.clone(), style),
                    ]))
                })
                .collect::<Vec<_>>();
            let list = List::new(items).block(Block::default().borders(Borders::NONE));
            frame.render_widget(list, top_chunks[0]);

            // Render Right: Shortcuts Info
            let shortcut_title = match lang.trim() {
                "spanish" | "español" | "espanol" => "Atajos de Teclado",
                _ => "Keyboard Shortcuts",
            };

            let key_style = Style::default().fg(colors.accent).bold();
            let desc_style = Style::default().fg(colors.text);

            let shortcut_lines = match lang.trim() {
                "spanish" | "español" | "espanol" => vec![
                    Line::from(vec![
                        Span::styled("  \u{2191}\u{2195} Flechas  ", key_style),
                        Span::styled("Navegar", desc_style),
                    ]),
                    Line::from(vec![
                        Span::styled("  \u{2190}\u{2192} Flechas  ", key_style),
                        Span::styled("Cambiar valor", desc_style),
                    ]),
                    Line::from(vec![
                        Span::styled("  Enter      ", key_style),
                        Span::styled("Seleccionar / Abrir", desc_style),
                    ]),
                    Line::from(vec![
                        Span::styled("  Esc        ", key_style),
                        Span::styled("Salir y Guardar", desc_style),
                    ]),
                ],
                _ => vec![
                    Line::from(vec![
                        Span::styled("  \u{2191}\u{2195} Arrows   ", key_style),
                        Span::styled("Navigate (wraps)", desc_style),
                    ]),
                    Line::from(vec![
                        Span::styled("  \u{2190}\u{2192} Arrows   ", key_style),
                        Span::styled("Change value / cycle", desc_style),
                    ]),
                    Line::from(vec![
                        Span::styled("  Enter      ", key_style),
                        Span::styled("Select / Open picker", desc_style),
                    ]),
                    Line::from(vec![
                        Span::styled("  Esc        ", key_style),
                        Span::styled("Save and Exit", desc_style),
                    ]),
                ],
            };

            let mut shortcut_content = vec![
                Line::from(Span::styled(
                    format!("  {}", shortcut_title),
                    Style::default().fg(colors.warning).bold(),
                )),
                Line::from(""),
            ];
            shortcut_content.extend(shortcut_lines);

            let shortcuts_paragraph =
                Paragraph::new(shortcut_content).block(Block::default().borders(Borders::NONE));
            frame.render_widget(shortcuts_paragraph, top_chunks[2]);

            // Render Bottom: Description
            let desc_text = settings_description(*selected, &app.core.config.language);
            let desc_paragraph = Paragraph::new(desc_text)
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(Color::Rgb(156, 163, 175))); // Gris claro
            frame.render_widget(desc_paragraph, chunks[2]);
        }
        Modal::Picker {
            title,
            items,
            selected,
        } => {
            let rows = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let is_selected = index == *selected;
                    let marker = if is_selected { ">" } else { " " };
                    let style = if is_selected {
                        Style::default().fg(colors.accent).bold()
                    } else {
                        Style::default().fg(colors.text)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(marker, style),
                        Span::raw(" "),
                        Span::styled(item.label.clone(), style),
                    ]))
                })
                .collect::<Vec<_>>();
            let hint = match (lang.trim(), app.onboarding_active) {
                ("spanish" | "español" | "espanol", true) => "selecc  Esc atrás",
                ("spanish" | "español" | "espanol", false) => "selecc  Esc cerrar",
                (_, true) => "select  Esc back",
                (_, false) => "select  Esc close",
            };
            let mut state = ListState::default();
            state.select(Some(*selected));
            frame.render_stateful_widget(
                List::new(rows).block(modal_block(
                    &format!("{} \u{2502} \u{2191}\u{2195} Enter {}", title, hint),
                    colors,
                )),
                area,
                &mut state,
            );
        }
        Modal::Confirm {
            title,
            message,
            selected,
            kind,
        } => {
            let border_color = if matches!(kind, crate::ui::state::ConfirmKind::ResetConfiguration)
            {
                colors.system
            } else {
                colors.border
            };
            let outer = modal_block_with_border(title, colors, border_color);
            frame.render_widget(outer.clone(), area);
            let inner_area = outer.inner(area);
            let options = match (lang.trim(), kind) {
                (
                    "spanish" | "español" | "espanol",
                    crate::ui::state::ConfirmKind::ResetConfiguration,
                ) => ["Resetear configuración", "Cancelar"],
                (_, crate::ui::state::ConfirmKind::ResetConfiguration) => {
                    ["Reset settings", "Cancel"]
                }
                ("spanish" | "español" | "espanol", _) => ["Sí", "No"],
                _ => ["Yes", "No"],
            };
            let rows: Vec<ListItem> = options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    let is_selected = index == *selected;
                    let marker = if is_selected { ">" } else { " " };
                    let style = if is_selected {
                        Style::default().fg(colors.accent).bold()
                    } else {
                        Style::default().fg(colors.text)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(marker, style),
                        Span::raw(" "),
                        Span::styled(*option, style),
                    ]))
                })
                .collect();

            let msg_lines = message.lines().count() as u16;
            let inner = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length((msg_lines + 4).min(inner_area.height.saturating_sub(4))),
                    Constraint::Length(rows.len() as u16 + 2),
                ])
                .split(inner_area);

            let question_label = match lang.trim() {
                "spanish" | "español" | "espanol" => "Pregunta",
                _ => "Question",
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        question_label,
                        Style::default().fg(colors.accent).bold(),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        message.clone(),
                        Style::default().fg(colors.text).bold(),
                    )),
                ])
                .wrap(Wrap { trim: true })
                .style(Style::default().bg(colors.modal_bg)),
                inner[0],
            );
            let hint = match lang.trim() {
                "spanish" | "español" | "espanol" => "selecc  Esc cancelar",
                _ => "select  Esc cancel",
            };
            frame.render_widget(
                List::new(rows)
                    .style(Style::default().bg(colors.modal_bg))
                    .block(modal_block(
                        &format!("\u{2191}\u{2195} Enter {}", hint),
                        colors,
                    )),
                inner[1],
            );
        }
        Modal::BranchSwitch { branches, selected } => {
            let title = match lang.trim() {
                "spanish" | "español" | "espanol" => "Cambiar de rama",
                _ => "Switch branch",
            };
            let origin_header = "origin";
            let local_header = match lang.trim() {
                "spanish" | "español" | "espanol" => "local",
                _ => "local",
            };
            let current_suffix = match lang.trim() {
                "spanish" | "español" | "espanol" => " actual",
                _ => " current",
            };

            let mut rows = Vec::new();
            let mut selected_row = None;
            let mut item_index = 0usize;

            if !branches.remote.is_empty() {
                rows.push(ListItem::new(Line::from(vec![Span::styled(
                    origin_header,
                    Style::default().fg(colors.warning).bold(),
                )])));
                for branch in &branches.remote {
                    if item_index == *selected {
                        selected_row = Some(rows.len());
                    }
                    rows.push(branch_switch_item(
                        branch,
                        item_index == *selected,
                        current_suffix,
                        colors,
                    ));
                    item_index += 1;
                }
            }

            if !branches.local.is_empty() {
                rows.push(ListItem::new(Line::from(vec![Span::styled(
                    local_header,
                    Style::default().fg(colors.warning).bold(),
                )])));
                for branch in &branches.local {
                    if item_index == *selected {
                        selected_row = Some(rows.len());
                    }
                    rows.push(branch_switch_item(
                        branch,
                        item_index == *selected,
                        current_suffix,
                        colors,
                    ));
                    item_index += 1;
                }
            }

            let hint = match lang.trim() {
                "spanish" | "español" | "espanol" => "selecc  Esc cerrar",
                _ => "select  Esc close",
            };
            let mut state = ListState::default();
            state.select(selected_row);
            frame.render_stateful_widget(
                List::new(rows).block(modal_block(
                    &format!("{} │ ↑↓ Enter {}", title, hint),
                    colors,
                )),
                area,
                &mut state,
            );
        }
        Modal::CommitLog {
            entries,
            selected,
            action,
            scroll,
        } => {
            let is_spanish = matches!(lang.trim(), "spanish" | "español" | "espanol");
            let title = if is_spanish {
                "Historial Git"
            } else {
                "Git history"
            };
            let hint = if is_spanish {
                "↑↓ commit  Tab acción  Enter confirmar  Esc cerrar"
            } else {
                "↑↓ commit  Tab action  Enter confirm  Esc close"
            };
            let block_title = format!("{title} | {hint}");
            let block = modal_block(&block_title, colors);
            frame.render_widget(block.clone(), area);
            let inner = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(8), Constraint::Length(3)])
                .split(block.inner(area));
            let visible = inner[0].height.saturating_sub(1).max(1) as usize;
            let start = (*scroll).min(entries.len().saturating_sub(1));
            let rows = entries
                .iter()
                .enumerate()
                .skip(start)
                .take(visible)
                .map(|(index, entry)| {
                    let is_selected = index == *selected;
                    let marker = if is_selected { ">" } else { " " };
                    let decorations = if entry.decorations.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", entry.decorations)
                    };
                    let style = if is_selected {
                        Style::default().fg(colors.text).bg(colors.info).bold()
                    } else {
                        Style::default().fg(colors.text).bg(colors.modal_bg)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(marker, style),
                        Span::raw(" "),
                        Span::styled(entry.short_hash.clone(), style.fg(colors.accent)),
                        Span::styled(decorations, style.fg(colors.warning)),
                        Span::raw(" "),
                        Span::styled(entry.subject.clone(), style),
                        Span::styled(
                            format!("  {} · {}", entry.author, entry.relative_time),
                            style.fg(colors.muted),
                        ),
                    ]))
                })
                .collect::<Vec<_>>();
            frame.render_widget(
                List::new(rows).style(Style::default().bg(colors.modal_bg)),
                inner[0],
            );
            let actions = if is_spanish {
                ["Reset soft hasta este commit", "Cerrar"]
            } else {
                ["Soft reset to this commit", "Close"]
            };
            let action_line = actions
                .iter()
                .enumerate()
                .flat_map(|(index, label)| {
                    let style = if index == *action {
                        Style::default().fg(colors.text).bg(colors.info).bold()
                    } else {
                        Style::default().fg(colors.muted).bg(colors.modal_bg)
                    };
                    [Span::styled(format!(" {} ", label), style), Span::raw("  ")]
                })
                .collect::<Vec<_>>();
            frame.render_widget(
                Paragraph::new(Line::from(action_line))
                    .style(Style::default().bg(colors.modal_bg))
                    .wrap(Wrap { trim: true }),
                inner[1],
            );
        }
        Modal::ProtectedBranchCommit {
            branch,
            branches,
            selected,
            new_branch,
            editing_new_branch,
        } => {
            let is_spanish = matches!(lang.trim(), "spanish" | "español" | "espanol");
            let title = if is_spanish {
                "Rama protegida"
            } else {
                "Protected branch"
            };
            let warning_bg = Color::Rgb(254, 226, 226);
            let warning_fg = Color::Rgb(127, 29, 29);
            let warning_border = Color::Rgb(248, 113, 113);
            let available = branches.total_count();
            let actions = [
                if is_spanish {
                    format!("Cambiar de rama ({available} disponibles)")
                } else {
                    format!("Switch branch ({available} available)")
                },
                if is_spanish {
                    "Crear rama nueva".to_string()
                } else {
                    "Create new branch".to_string()
                },
                if is_spanish {
                    "Continuar en esta rama".to_string()
                } else {
                    "Continue on this branch".to_string()
                },
            ];
            let branch_value = if new_branch.is_empty() {
                if is_spanish {
                    "feature/nueva-rama"
                } else {
                    "feature/new-branch"
                }
            } else {
                new_branch.as_str()
            };
            let branch_style = if *editing_new_branch {
                Style::default()
                    .fg(warning_fg)
                    .bg(Color::Rgb(255, 245, 245))
                    .bold()
            } else {
                Style::default().fg(warning_fg).bg(warning_bg)
            };
            let rows = actions
                .iter()
                .enumerate()
                .map(|(index, action)| {
                    let is_selected = index == *selected;
                    let marker = if is_selected { ">" } else { " " };
                    let style = if is_selected {
                        Style::default().fg(warning_fg).bg(warning_bg).bold()
                    } else {
                        Style::default().fg(warning_fg).bg(warning_bg)
                    };
                    if index == 1 {
                        ListItem::new(Line::from(vec![
                            Span::styled(marker, style),
                            Span::raw(" "),
                            Span::styled(action.clone(), style),
                            Span::raw("  "),
                            Span::styled(branch_value.to_string(), branch_style),
                        ]))
                    } else {
                        ListItem::new(Line::from(vec![
                            Span::styled(marker, style),
                            Span::raw(" "),
                            Span::styled(action.clone(), style),
                        ]))
                    }
                })
                .collect::<Vec<_>>();
            let message = if is_spanish {
                format!(
                    "Estás en `{branch}`. Para evitar commits directos en main/master, cambia o crea una rama y el plan de commits se ejecuta automáticamente."
                )
            } else {
                format!(
                    "You are on `{branch}`. To avoid direct commits on main/master, switch or create a branch and the commit plan will start automatically."
                )
            };
            let esc_hint = if *editing_new_branch {
                if is_spanish { "Esc volver" } else { "Esc back" }
            } else if is_spanish {
                "Esc cancelar"
            } else {
                "Esc cancel"
            };
            let hint = if is_spanish {
                format!("↑↓ acción  Tab escribir rama  Enter confirmar  {esc_hint}")
            } else {
                format!("↑↓ action  Tab edit branch  Enter confirm  {esc_hint}")
            };
            let block = Block::default()
                .title(format!("{title} | {hint}"))
                .title_alignment(ratatui::layout::Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(warning_border))
                .style(Style::default().fg(warning_fg).bg(warning_bg));
            frame.render_widget(block.clone(), area);
            let inner = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(4), Constraint::Min(4)])
                .split(block.inner(area));
            frame.render_widget(
                Paragraph::new(message)
                    .wrap(Wrap { trim: true })
                    .style(Style::default().fg(warning_fg).bg(warning_bg).bold()),
                inner[0].inner(ratatui::layout::Margin {
                    horizontal: 2,
                    vertical: 1,
                }),
            );
            frame.render_widget(
                List::new(rows).style(Style::default().fg(warning_fg).bg(warning_bg)),
                inner[1].inner(ratatui::layout::Margin {
                    horizontal: 2,
                    vertical: 0,
                }),
            );
        }
        Modal::TextInput { title, value, kind } => {
            let hint = match (lang.trim(), app.onboarding_active) {
                ("spanish" | "español" | "espanol", true) => "confirmar  Esc atrás",
                ("spanish" | "español" | "espanol", false) => "confirmar  Esc cerrar",
                (_, true) => "confirm  Esc back",
                (_, false) => "confirm  Esc close",
            };
            let block_title = format!("{} \u{2502} Enter {}", title, hint);
            let display_value = if matches!(kind, TextInputKind::ApiKey) {
                "*".repeat(value.chars().count())
            } else {
                value.clone()
            };
            let paragraph = Paragraph::new(Line::from(vec![
                Span::styled(">", Style::default().fg(colors.user).bold()),
                Span::raw(" "),
                Span::raw(display_value),
            ]))
            .block(modal_block(&block_title, colors));
            frame.render_widget(paragraph, area);
            let x = area.x + 3 + value.chars().count() as u16;
            frame.set_cursor_position((x.min(area.x + area.width.saturating_sub(2)), area.y + 1));
        }
        Modal::CommitPlanReview {
            plan,
            selected,
            scroll,
        } => {
            let mut text = String::new();
            let is_spanish = matches!(lang.trim(), "spanish" | "español" | "espanol");
            let title = match lang.trim() {
                "spanish" | "español" | "espanol" => "Plan de commits",
                _ => "Commit plan",
            };
            let files_label = if is_spanish { "Archivos" } else { "Files" };
            let scope_label = if is_spanish { "Alcance" } else { "Scope" };
            let scope_value = if app.core.config.staged_only {
                if is_spanish {
                    "Solo archivos staged"
                } else {
                    "Staged changes only"
                }
            } else if is_spanish {
                "Todos los cambios: staged, unstaged y untracked"
            } else {
                "All changes: staged, unstaged, and untracked"
            };
            let mut unique_files: Vec<&str> = Vec::new();
            for group in &plan.groups {
                for file in &group.files {
                    if !unique_files.iter().any(|path| *path == file.path) {
                        unique_files.push(file.path.as_str());
                    }
                }
            }

            text.push_str(&format!("{}: {}\n", scope_label, scope_value));
            text.push_str(&format!(
                "{}: {} | {}: {}\n\n",
                if is_spanish {
                    "Commits propuestos"
                } else {
                    "Proposed commits"
                },
                plan.groups.len(),
                files_label,
                unique_files.len()
            ));

            for (index, group) in plan.groups.iter().enumerate() {
                text.push_str(&format!("{}. {}\n", index + 1, group.commit.title()));
                text.push_str(&format!("   {}:\n", files_label));
                for file in &group.files {
                    text.push_str(&format!("   - {} ({})\n", file.path, file.status));
                }
                text.push('\n');
            }

            let actions = [
                if matches!(lang.trim(), "spanish" | "español" | "espanol") {
                    "Ejecutar plan"
                } else {
                    "Execute plan"
                },
                if matches!(lang.trim(), "spanish" | "español" | "espanol") {
                    "Regenerar"
                } else {
                    "Regenerate"
                },
                if matches!(lang.trim(), "spanish" | "español" | "espanol") {
                    "Cancelar"
                } else {
                    "Cancel"
                },
            ];

            let hint = match lang.trim() {
                "spanish" | "español" | "espanol" => {
                    "↑↓/Tab acción  ←→ acción  PgUp/PgDn scroll  Enter confirmar  Esc cancelar"
                }
                _ => "↑↓/Tab action  ←→ action  PgUp/PgDn scroll  Enter confirm  Esc cancel",
            };
            let inner_plan = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(4)])
                .split(area);

            frame.render_widget(
                Block::default()
                    .title(format!("{} | {}", title, hint))
                    .title_alignment(ratatui::layout::Alignment::Center)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(colors.accent))
                    .style(Style::default().bg(colors.modal_bg).fg(colors.text)),
                area,
            );
            let text_area = inner_plan[0].inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 1,
            });
            let max_scroll = text
                .lines()
                .count()
                .saturating_sub(text_area.height as usize);
            let effective_scroll = (*scroll).min(max_scroll);
            frame.render_widget(
                Paragraph::new(text)
                    .scroll((effective_scroll as u16, 0))
                    .wrap(Wrap { trim: false })
                    .style(Style::default().bg(colors.modal_bg).fg(colors.text)),
                text_area,
            );
            frame.render_widget(
                plain_action_list(&actions, *selected, colors),
                inner_plan[1].inner(ratatui::layout::Margin {
                    horizontal: 2,
                    vertical: 0,
                }),
            );
        }
        Modal::CommitReview {
            message,
            files,
            selected,
            scroll,
        } => {
            let mut text = format!("{}\n\n{}", message.title(), message.body);
            if !files.is_empty() {
                let files_label = match lang.trim() {
                    "spanish" | "español" | "espanol" => {
                        "\n\n\u{2500}\u{2500} Archivos \u{2500}\u{2500}\n"
                    }
                    _ => "\n\n\u{2500}\u{2500} Files \u{2500}\u{2500}\n",
                };
                text.push_str(files_label);
                for f in files {
                    text.push_str(&format!(
                        "\n\u{2022} {} ({}) - {}",
                        f.path, f.status, f.description
                    ));
                }
            }
            let actions = match lang.trim() {
                "spanish" | "español" | "espanol" => {
                    ["Crear commit", "Editar asunto", "Regenerar", "Cancelar"]
                }
                _ => ["Create commit", "Edit subject", "Regenerate", "Cancel"],
            };
            let inner_commit = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(4)])
                .split(area);
            let commit_title = match lang.trim() {
                "spanish" | "español" | "espanol" => {
                    "Revisar commit \u{2502} \u{2191}\u{2195} Enter  PgUp/PgDn scroll  Esc atrás"
                }
                _ => "Commit review \u{2502} \u{2191}\u{2195} Enter  PgUp/PgDn scroll  Esc back",
            };
            frame.render_widget(
                Block::default()
                    .title(commit_title)
                    .title_alignment(ratatui::layout::Alignment::Center)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(colors.border))
                    .style(Style::default().bg(colors.modal_bg)),
                area,
            );
            frame.render_widget(
                Paragraph::new(text)
                    .scroll((*scroll as u16, 0))
                    .wrap(Wrap { trim: true }),
                inner_commit[0],
            );
            frame.render_widget(
                plain_action_list(&actions, *selected, colors),
                inner_commit[1],
            );
        }
        Modal::PrDraft {
            base,
            draft,
            selected,
            scroll,
        } => {
            let text = format!("base: {}\n\ntitle: {}\n\n{}", base, draft.title, draft.body);
            let actions = match lang.trim() {
                "spanish" | "español" | "espanol" => ["Crear PR", "Cancelar"],
                _ => ["Create PR", "Cancel"],
            };
            let inner_pr = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(4)])
                .split(area);
            let pr_title = match lang.trim() {
                "spanish" | "español" | "espanol" => {
                    "Pull Request \u{2502} \u{2191}\u{2195} Enter  PgUp/PgDn scroll  Esc atrás"
                }
                _ => "Pull Request \u{2502} \u{2191}\u{2195} Enter  PgUp/PgDn scroll  Esc back",
            };
            frame.render_widget(
                Block::default()
                    .title(pr_title)
                    .title_alignment(ratatui::layout::Alignment::Center)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(colors.border))
                    .style(Style::default().bg(colors.modal_bg)),
                area,
            );
            frame.render_widget(
                Paragraph::new(text)
                    .scroll((*scroll as u16, 0))
                    .wrap(Wrap { trim: true }),
                inner_pr[0],
            );
            frame.render_widget(plain_action_list(&actions, *selected, colors), inner_pr[1]);
        }
        Modal::ExistingPrs {
            prs,
            selected,
            ..
        } => {
            let title = match lang.trim() {
                "spanish" | "español" | "espanol" => "PR existente detectado",
                _ => "Existing PR detected",
            };
            let actions = match lang.trim() {
                "spanish" | "español" | "espanol" => ["Actualizar PR", "Cerrar y Recrear", "Cancelar"],
                _ => ["Update PR", "Close and Recreate", "Cancel"],
            };
            let inner = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(5)])
                .split(area);

            frame.render_widget(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(colors.border))
                    .style(Style::default().bg(colors.modal_bg)),
                area,
            );

            let pr_list: Vec<ListItem> = prs
                .iter()
                .enumerate()
                .map(|(i, pr)| {
                    let style = if i == *selected {
                        Style::default().fg(colors.accent)
                    } else {
                        Style::default()
                    };
                    ListItem::new(format!("#{} - {}", pr.number, pr.title)).style(style)
                })
                .collect();

            frame.render_widget(
                List::new(pr_list).block(Block::default().borders(Borders::NONE)),
                inner[0],
            );
            frame.render_widget(plain_action_list(&actions, *selected, colors), inner[1]);
        }
        Modal::ConflictResolution { selected, .. } => {
            let title = match lang.trim() {
                "spanish" | "español" | "espanol" => "Conflictos de fusión detectados",
                _ => "Merge conflicts detected",
            };
            let msg = match lang.trim() {
                "spanish" | "español" | "espanol" => {
                    "Hay conflictos entre las ramas. ¿Cómo deseas resolverlos?"
                }
                _ => "There are conflicts between branches. How would you like to resolve them?",
            };
            let actions = match lang.trim() {
                "spanish" | "español" | "espanol" => {
                    ["Resolver con IA (Recomendado)", "Resolver manualmente", "Cancelar"]
                }
                _ => ["Resolve with AI (Recommended)", "Resolve manually", "Cancel"],
            };
            let inner = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(5)])
                .split(area);

            frame.render_widget(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(colors.border))
                    .style(Style::default().bg(colors.modal_bg)),
                area,
            );
            frame.render_widget(
                Paragraph::new(msg).wrap(Wrap { trim: true }),
                inner[0],
            );
            frame.render_widget(plain_action_list(&actions, *selected, colors), inner[1]);
        }
        Modal::Setup { selected } => {
            let is_spanish = matches!(lang.trim(), "spanish" | "español" | "espanol");
            let actions = if is_spanish {
                vec![
                    OnboardingAction {
                        label: "Trabajar localmente".to_string(),
                        help: "Inicializa Git para usar diff, review y commits locales."
                            .to_string(),
                    },
                    OnboardingAction {
                        label: "Conectar remote existente".to_string(),
                        help: "Pide la URL y la guarda como origin.".to_string(),
                    },
                    OnboardingAction {
                        label: "Crear repo en GitHub".to_string(),
                        help: "Usa GitHub CLI para crear el repositorio remoto.".to_string(),
                    },
                    OnboardingAction {
                        label: "Salir del setup".to_string(),
                        help: "Cierra este asistente sin cambiar el repositorio.".to_string(),
                    },
                ]
            } else {
                vec![
                    OnboardingAction {
                        label: "Work locally".to_string(),
                        help: "Initializes Git for local diff, review, and commits.".to_string(),
                    },
                    OnboardingAction {
                        label: "Connect existing remote".to_string(),
                        help: "Asks for the URL and saves it as origin.".to_string(),
                    },
                    OnboardingAction {
                        label: "Create GitHub repo".to_string(),
                        help: "Uses GitHub CLI to create the remote repository.".to_string(),
                    },
                    OnboardingAction {
                        label: "Exit setup".to_string(),
                        help: "Closes this assistant without changing the repository.".to_string(),
                    },
                ]
            };
            let setup_title = if is_spanish {
                "Setup del proyecto"
            } else {
                "Project setup"
            };
            let status = app.core.status();
            let state_line = if is_spanish {
                format!(
                    "Estado: repo {} · remote {}",
                    if status.is_repo {
                        "detectado"
                    } else {
                        "faltante"
                    },
                    if status.has_origin {
                        "conectado"
                    } else {
                        "pendiente"
                    }
                )
            } else {
                format!(
                    "State: repo {} · remote {}",
                    if status.is_repo {
                        "detected"
                    } else {
                        "missing"
                    },
                    if status.has_origin {
                        "connected"
                    } else {
                        "pending"
                    }
                )
            };
            let intro = if is_spanish {
                "Elegí cómo querés preparar este directorio. Cada opción explica el resultado antes de ejecutarse."
            } else {
                "Choose how to prepare this directory. Each option explains what it will do before it runs."
            };
            let help = if is_spanish {
                "↑↓ mover   Enter seleccionar   Esc cerrar"
            } else {
                "↑↓ move   Enter select   Esc close"
            };
            let inner_setup = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(5),
                    Constraint::Length(1),
                    Constraint::Min(8),
                    Constraint::Length(2),
                ])
                .split(area.inner(ratatui::layout::Margin {
                    horizontal: 3,
                    vertical: 2,
                }));
            frame.render_widget(
                Block::default()
                    .title(setup_title)
                    .title_alignment(ratatui::layout::Alignment::Center)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(colors.accent))
                    .style(Style::default().bg(colors.modal_bg)),
                area,
            );
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        if is_spanish {
                            "Preparación guiada"
                        } else {
                            "Guided preparation"
                        },
                        Style::default().fg(colors.accent).bold(),
                    )),
                    Line::from(Span::styled(state_line, Style::default().fg(colors.text))),
                    Line::from(""),
                    Line::from(Span::styled(intro, Style::default().fg(colors.muted))),
                ])
                .wrap(Wrap { trim: true }),
                inner_setup[0],
            );
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "─".repeat(inner_setup[1].width as usize),
                    Style::default().fg(colors.border),
                )),
                inner_setup[1],
            );
            frame.render_widget(
                onboarding_action_list(&actions, *selected, colors),
                inner_setup[2],
            );
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        if is_spanish { "Atajos" } else { "Shortcuts" },
                        Style::default().fg(colors.accent).bold(),
                    )),
                    Line::from(Span::styled(help, Style::default().fg(colors.hint))),
                ]),
                inner_setup[3],
            );
        }
        Modal::DependencyIssue {
            issue,
            actions,
            selected,
            blocking,
            notice,
        } => {
            let title = dependency_modal_title(issue, *blocking, lang.trim());
            let mut msg = dependency_modal_message(issue, lang.trim());
            if let Some(notice) = notice {
                msg.push_str("\n\n");
                msg.push_str(notice);
            }
            let rows: Vec<ListItem> = actions
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    let is_selected = index == *selected;
                    let marker = if is_selected { ">" } else { " " };
                    let style = if is_selected {
                        Style::default().fg(colors.accent).bold()
                    } else {
                        Style::default().fg(colors.text)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(marker, style),
                        Span::raw(" "),
                        Span::styled(option.label.clone(), style),
                    ]))
                })
                .collect();

            let inner = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(rows.len() as u16 + 2),
                ])
                .split(area);

            frame.render_widget(
                Block::default()
                    .title(title)
                    .title_alignment(ratatui::layout::Alignment::Center)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(colors.warning))
                    .style(Style::default().bg(colors.modal_bg)),
                area,
            );

            let text_area = inner[0].inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 1,
            });
            frame.render_widget(
                Paragraph::new(msg)
                    .wrap(Wrap { trim: true })
                    .style(Style::default().fg(colors.text)),
                text_area,
            );

            let list_area = inner[1].inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 0,
            });
            frame.render_widget(
                List::new(rows).block(Block::default().borders(Borders::NONE)),
                list_area,
            );
        }
        Modal::CommandExecution {
            title,
            command,
            logs,
            ..
        } => {
            let command_title = match lang.trim() {
                "spanish" | "español" | "espanol" => {
                    format!("{title} │ Enter volver  Esc atrás")
                }
                _ => format!("{title} │ Enter back  Esc close"),
            };
            let body = if logs.is_empty() {
                command.clone()
            } else {
                logs.join("\n")
            };
            let inner = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(1)])
                .split(area);

            frame.render_widget(
                Block::default()
                    .title(command_title)
                    .title_alignment(ratatui::layout::Alignment::Center)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(colors.warning))
                    .style(Style::default().bg(colors.modal_bg)),
                area,
            );

            let command_area = inner[0].inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 1,
            });
            frame.render_widget(
                Paragraph::new(format!("$ {command}")).style(Style::default().fg(colors.accent)),
                command_area,
            );

            let log_area = inner[1].inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 0,
            });
            frame.render_widget(
                Paragraph::new(body)
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(colors.text)),
                log_area,
            );
        }
        Modal::ScoutDecision { selected } => {
            let title = match lang.trim() {
                "spanish" | "español" | "espanol" => "Modo Scout - Decisiones",
                _ => "Scout Mode - Decisions",
            };
            let subtitle = match lang.trim() {
                "spanish" | "español" | "espanol" => "Selecciona una opción sobre el git diff:",
                _ => "Select an option about the git diff:",
            };
            let options = match lang.trim() {
                "spanish" | "español" | "espanol" => [
                    "Explicar cambios detalladamente",
                    "Realizar una Revisión de Código (Code Review)",
                    "Hacer una pregunta personalizada",
                    "Cerrar sesión Scout",
                ],
                _ => [
                    "Explain changes in detail",
                    "Perform a Code Review",
                    "Ask a custom question",
                    "Close Scout session",
                ],
            };

            let rows: Vec<ListItem> = options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    let is_selected = index == *selected;
                    let marker = if is_selected { ">" } else { " " };
                    let style = if is_selected {
                        Style::default().fg(colors.accent).bold()
                    } else {
                        Style::default().fg(colors.text)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(marker, style),
                        Span::raw(" "),
                        Span::styled(*option, style),
                    ]))
                })
                .collect();

            let inner = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Min(4)])
                .split(area);

            frame.render_widget(
                Block::default()
                    .title(title)
                    .title_alignment(ratatui::layout::Alignment::Center)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(colors.accent))
                    .style(Style::default().bg(colors.modal_bg)),
                area,
            );

            let header_area = inner[0].inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 1,
            });
            frame.render_widget(
                Paragraph::new(subtitle).style(Style::default().fg(colors.text).bold()),
                header_area,
            );

            let list_area = inner[1].inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 0,
            });
            frame.render_widget(
                List::new(rows).block(Block::default().borders(Borders::NONE)),
                list_area,
            );
        }
    }
}

fn plain_action_list<T: AsRef<str>>(
    actions: &[T],
    selected: usize,
    colors: Palette,
) -> List<'static> {
    let items = actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let is_selected = index == selected;
            let marker = if is_selected { ">" } else { " " };
            let style = if is_selected {
                Style::default().fg(colors.accent).bold()
            } else {
                Style::default().fg(colors.text)
            };
            ListItem::new(Line::from(vec![
                Span::styled(marker.to_string(), style),
                Span::raw(" "),
                Span::styled(action.as_ref().to_string(), style),
            ]))
        })
        .collect::<Vec<_>>();
    List::new(items).style(Style::default().bg(colors.modal_bg))
}

fn onboarding_action_list(
    actions: &[OnboardingAction],
    selected: usize,
    colors: Palette,
) -> List<'static> {
    let items = actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let is_selected = index == selected;
            let marker = if is_selected { ">" } else { " " };
            let label_style = if is_selected {
                Style::default().fg(colors.accent).bold()
            } else {
                Style::default().fg(colors.text).bold()
            };
            let help_style = Style::default().fg(colors.muted);
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(marker.to_string(), label_style),
                    Span::raw(" "),
                    Span::styled(action.label.clone(), label_style),
                ]),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(action.help.clone(), help_style),
                ]),
            ])
        })
        .collect::<Vec<_>>();
    List::new(items)
}

fn onboarding_action_compact_list(
    actions: &[OnboardingAction],
    selected: usize,
    colors: Palette,
) -> List<'static> {
    let items = actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let is_selected = index == selected;
            let marker = if is_selected { ">" } else { " " };
            let style = if is_selected {
                Style::default().fg(colors.accent).bold()
            } else {
                Style::default().fg(colors.text).bold()
            };
            ListItem::new(Line::from(vec![
                Span::styled(marker.to_string(), style),
                Span::raw(" "),
                Span::styled(action.label.clone(), style),
            ]))
        })
        .collect::<Vec<_>>();
    List::new(items)
}

fn dependency_modal_title(issue: &DependencyStatus, blocking: bool, lang: &str) -> String {
    match (lang, issue.kind, blocking) {
        ("spanish" | "español" | "espanol", DependencyKind::Git, true) => {
            "Git requerido".to_string()
        }
        ("spanish" | "español" | "espanol", DependencyKind::Ollama, _) => {
            "Ollama requerido".to_string()
        }
        ("spanish" | "español" | "espanol", DependencyKind::GitHubCli, _) => {
            "GitHub CLI requerido".to_string()
        }
        ("spanish" | "español" | "espanol", DependencyKind::LlmProvider, _) => {
            "Proveedor IA requerido".to_string()
        }
        (_, DependencyKind::Git, true) => "Git required".to_string(),
        (_, DependencyKind::Ollama, _) => "Ollama required".to_string(),
        (_, DependencyKind::GitHubCli, _) => "GitHub CLI required".to_string(),
        (_, DependencyKind::LlmProvider, _) => "AI provider required".to_string(),
        _ => issue.kind.label().to_string(),
    }
}

fn dependency_modal_message(issue: &DependencyStatus, lang: &str) -> String {
    let package_manager = issue
        .platform
        .package_manager
        .map(package_manager_label)
        .unwrap_or_else(|| match lang {
            "spanish" | "español" | "espanol" => "ninguno detectado",
            _ => "none detected",
        });

    let intro = match (lang, issue.kind, issue.state) {
        ("spanish" | "español" | "espanol", DependencyKind::Git, DependencyState::Missing) => {
            "Git es obligatorio para usar Remix Autopilot."
        }
        ("spanish" | "español" | "espanol", DependencyKind::Ollama, DependencyState::Missing) => {
            "Ollama no está instalado. Las funciones de IA no estarán disponibles."
        }
        (
            "spanish" | "español" | "espanol",
            DependencyKind::Ollama,
            DependencyState::NotRunning,
        ) => "Ollama está instalado, pero no responde en localhost:11434.",
        (
            "spanish" | "español" | "espanol",
            DependencyKind::Ollama,
            DependencyState::NotConfigured,
        ) => "Ollama está activo, pero no hay modelos locales disponibles.",
        (
            "spanish" | "español" | "espanol",
            DependencyKind::GitHubCli,
            DependencyState::Missing,
        ) => {
            "GitHub CLI no está instalado. /pr y la creación de repositorios GitHub estarán deshabilitados."
        }
        (
            "spanish" | "español" | "espanol",
            DependencyKind::GitHubCli,
            DependencyState::NotConfigured,
        ) => "GitHub CLI está instalado, pero falta autenticación.",
        (
            "spanish" | "español" | "espanol",
            DependencyKind::LlmProvider,
            DependencyState::NotConfigured,
        ) => "El proveedor de IA activo requiere configuración adicional.",
        (
            "spanish" | "español" | "espanol",
            DependencyKind::LlmProvider,
            DependencyState::NotRunning,
        ) => "El proveedor de IA activo no responde correctamente.",
        (_, DependencyKind::Git, DependencyState::Missing) => {
            "Git is required to use Remix Autopilot."
        }
        (_, DependencyKind::Ollama, DependencyState::Missing) => {
            "Ollama is not installed. AI features are unavailable until you install it."
        }
        (_, DependencyKind::Ollama, DependencyState::NotRunning) => {
            "Ollama is installed, but it is not responding on localhost:11434."
        }
        (_, DependencyKind::Ollama, DependencyState::NotConfigured) => {
            "Ollama is running, but no local models are available."
        }
        (_, DependencyKind::LlmProvider, DependencyState::NotConfigured) => {
            "The active AI provider still needs configuration."
        }
        (_, DependencyKind::LlmProvider, DependencyState::NotRunning) => {
            "The active AI provider is not responding correctly."
        }
        (_, DependencyKind::GitHubCli, DependencyState::Missing) => {
            "GitHub CLI is not installed. /pr and GitHub repository creation stay unavailable."
        }
        (_, DependencyKind::GitHubCli, DependencyState::NotConfigured) => {
            "GitHub CLI is installed, but authentication is missing."
        }
        _ => "",
    };

    let detected_os = match lang {
        "spanish" | "español" | "espanol" => "Sistema detectado",
        _ => "Detected OS",
    };
    let detected_pm = match lang {
        "spanish" | "español" | "espanol" => "Gestor de paquetes detectado",
        _ => "Detected package manager",
    };
    let version_label = match lang {
        "spanish" | "español" | "espanol" => "Versión detectada",
        _ => "Detected version",
    };
    let detail_label = match lang {
        "spanish" | "español" | "espanol" => "Detalle",
        _ => "Detail",
    };
    let command_label = match lang {
        "spanish" | "español" | "espanol" => "Comando recomendado",
        _ => "Recommended command",
    };
    let fallback_label = match lang {
        "spanish" | "español" | "espanol" => "Alternativa",
        _ => "Fallback",
    };

    let mut lines = vec![
        intro.to_string(),
        String::new(),
        format!("{detected_os}: {}", issue.platform.os.label()),
        format!("{detected_pm}: {package_manager}"),
    ];
    if let Some(version) = &issue.version
        && !version.trim().is_empty()
    {
        lines.push(format!("{version_label}: {version}"));
    }
    if let Some(detail) = &issue.detail
        && !detail.trim().is_empty()
    {
        lines.push(format!("{detail_label}: {detail}"));
    }
    if let Some(command) = &issue.suggested_command {
        lines.push(String::new());
        lines.push(format!("{command_label}:"));
        lines.push(command.clone());
    }
    if let Some(url) = issue.fallback_url {
        lines.push(String::new());
        lines.push(format!("{fallback_label}:"));
        lines.push(url.to_string());
    }
    lines.join("\n")
}

fn package_manager_label(manager: PackageManager) -> &'static str {
    manager.label()
}

fn onboarding_progress_lines(
    app: &TuiApp,
    current: &OnboardingStep,
    colors: Palette,
) -> Vec<Line<'static>> {
    let is_spanish = is_spanish_language(&app.core.config.language);
    let mut lines = vec![Line::from(Span::styled(
        if is_spanish {
            "  Progreso"
        } else {
            "  Progress"
        },
        Style::default().fg(colors.accent).bold(),
    ))];
    let steps = onboarding_progress_items(app);
    for (label, state) in steps {
        let is_current = step_matches_label(current, &label);
        let marker = if is_current {
            ">"
        } else if state == ProgressState::Done {
            "✓"
        } else {
            "•"
        };
        let state_label = match (is_spanish, is_current, state) {
            (_, true, _) => {
                if is_spanish {
                    "actual"
                } else {
                    "current"
                }
            }
            (true, false, ProgressState::Done) => "listo",
            (false, false, ProgressState::Done) => "done",
            (true, false, ProgressState::Pending) => "pendiente",
            (false, false, ProgressState::Pending) => "pending",
        };
        let style = if is_current {
            Style::default().fg(colors.accent).bold()
        } else if state == ProgressState::Done {
            Style::default().fg(colors.success)
        } else {
            Style::default().fg(colors.muted)
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", marker), style),
            Span::styled(label, style),
            Span::styled(format!(" ({})", state_label), style),
        ]));
    }
    lines
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProgressState {
    Done,
    Pending,
}

fn onboarding_progress_items(app: &TuiApp) -> Vec<(String, ProgressState)> {
    let is_spanish = is_spanish_language(&app.core.config.language);
    let status = app.core.status();
    let provider_selected = app.core.config.provider.is_selected();
    let api_key_ready =
        !app.core.config.provider.uses_api_key() || app.core.api_key_configured().unwrap_or(false);
    let model_ready = app
        .core
        .config
        .model
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    let provider_ready = app
        .dependency_doctor
        .as_ref()
        .map(|doctor| doctor.llm_provider.is_ready())
        .unwrap_or(provider_selected);
    let git_ready = app
        .dependency_doctor
        .as_ref()
        .map(|doctor| doctor.git.is_ready())
        .unwrap_or(true);

    vec![
        (
            if is_spanish { "Idioma" } else { "Language" }.to_string(),
            if app.onboarding_language_confirmed {
                ProgressState::Done
            } else {
                ProgressState::Pending
            },
        ),
        (
            "Git".to_string(),
            if git_ready {
                ProgressState::Done
            } else {
                ProgressState::Pending
            },
        ),
        (
            if is_spanish {
                "Repositorio"
            } else {
                "Repository"
            }
            .to_string(),
            if status.is_repo {
                ProgressState::Done
            } else {
                ProgressState::Pending
            },
        ),
        (
            if is_spanish { "Remoto" } else { "Remote" }.to_string(),
            if status.has_origin || app.onboarding_remote_deferred {
                ProgressState::Done
            } else {
                ProgressState::Pending
            },
        ),
        (
            if is_spanish { "Proveedor" } else { "Provider" }.to_string(),
            if provider_selected && provider_ready {
                ProgressState::Done
            } else {
                ProgressState::Pending
            },
        ),
        (
            if is_spanish { "Clave API" } else { "API key" }.to_string(),
            if api_key_ready {
                ProgressState::Done
            } else {
                ProgressState::Pending
            },
        ),
        (
            if is_spanish { "Modelo" } else { "Model" }.to_string(),
            if model_ready {
                ProgressState::Done
            } else {
                ProgressState::Pending
            },
        ),
    ]
}

fn step_matches_label(step: &OnboardingStep, label: &str) -> bool {
    match step {
        OnboardingStep::LanguageSelection => label == "Language" || label == "Idioma",
        OnboardingStep::GitDependency(_) => label == "Git",
        OnboardingStep::RepoSetupChoice => label == "Repository" || label == "Repositorio",
        OnboardingStep::RemoteSetupChoice => label == "Remote" || label == "Remoto",
        OnboardingStep::ProviderSelection => label == "Provider" || label == "Proveedor",
        OnboardingStep::ProviderDependency(_) => {
            label == "Provider"
                || label == "Proveedor"
                || label == "Provider ready"
                || label == "Proveedor listo"
        }
        OnboardingStep::ModelConfiguration => label == "Model" || label == "Modelo",
        OnboardingStep::ApiKeyConfiguration => label == "API key" || label == "Clave API",
    }
}

struct OnboardingContent {
    title: String,
    missing: String,
    why: String,
    next: String,
    tip: String,
    actions: Vec<OnboardingAction>,
    alert: bool,
}

struct OnboardingAction {
    label: String,
    help: String,
}

impl OnboardingContent {
    fn new(
        title: String,
        missing: String,
        why: String,
        next: String,
        tip: String,
        actions: Vec<OnboardingAction>,
    ) -> Self {
        Self {
            title,
            missing,
            why,
            next,
            tip,
            actions,
            alert: false,
        }
    }

    fn alert(
        title: String,
        missing: String,
        why: String,
        next: String,
        tip: String,
        actions: Vec<OnboardingAction>,
    ) -> Self {
        Self {
            title,
            missing,
            why,
            next,
            tip,
            actions,
            alert: true,
        }
    }

    fn title_color(&self, colors: Palette) -> Color {
        if self.alert {
            colors.warning
        } else {
            colors.info
        }
    }
}

fn onboarding_action(label: &str, help: &str) -> OnboardingAction {
    OnboardingAction {
        label: label.to_string(),
        help: help.to_string(),
    }
}

fn onboarding_detail_lines(
    content: &OnboardingContent,
    is_spanish: bool,
    colors: Palette,
) -> Vec<Line<'static>> {
    let labels = if is_spanish {
        ["Falta", "Por qué importa", "Qué hacer ahora", "Consejo"]
    } else {
        ["Missing", "Why it matters", "What to do now", "Tip"]
    };
    let values = [
        content.missing.as_str(),
        content.why.as_str(),
        content.next.as_str(),
        content.tip.as_str(),
    ];
    let mut lines = Vec::new();
    for (index, (label, value)) in labels.iter().zip(values.iter()).enumerate() {
        if index > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            (*label).to_string(),
            Style::default().fg(colors.accent).bold(),
        )));
        lines.push(Line::from(Span::styled(
            (*value).to_string(),
            Style::default().fg(colors.text),
        )));
    }
    lines
}

fn onboarding_detail_compact_lines(
    content: &OnboardingContent,
    is_spanish: bool,
    colors: Palette,
) -> Vec<Line<'static>> {
    let labels = if is_spanish {
        ["Falta", "Por qué importa", "Ahora", "Consejo"]
    } else {
        ["Missing", "Why it matters", "Now", "Tip"]
    };
    let values = [
        content.missing.as_str(),
        content.why.as_str(),
        content.next.as_str(),
        content.tip.as_str(),
    ];
    labels
        .iter()
        .zip(values.iter())
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(
                    format!("{}: ", label),
                    Style::default().fg(colors.accent).bold(),
                ),
                Span::styled((*value).to_string(), Style::default().fg(colors.text)),
            ])
        })
        .collect()
}

fn onboarding_step_content(step: &OnboardingStep, language: &str) -> OnboardingContent {
    let is_spanish = matches!(
        language.to_lowercase().trim(),
        "spanish" | "español" | "espanol"
    );
    match step {
        OnboardingStep::LanguageSelection => OnboardingContent::new(
            if is_spanish {
                "Elegir idioma".to_string()
            } else {
                "Choose language".to_string()
            },
            if is_spanish {
                "Confirmar el idioma antes de configurar la app.".to_string()
            } else {
                "Confirm the language before configuring the app.".to_string()
            },
            if is_spanish {
                "El idioma se usa en la interfaz, este asistente de setup y las respuestas de IA."
                    .to_string()
            } else {
                "The language is used for the interface, this setup guide, and AI responses."
                    .to_string()
            },
            if is_spanish {
                "Elegí el idioma en el que querés completar el resto del setup.".to_string()
            } else {
                "Choose the language you want to use for the rest of setup.".to_string()
            },
            if is_spanish {
                "Los pasos que ya estén configurados se saltan automáticamente.".to_string()
            } else {
                "Steps that are already configured are skipped automatically.".to_string()
            },
            if is_spanish {
                vec![
                    onboarding_action("Usar inglés", "Cambia el resto del setup a inglés."),
                    onboarding_action(
                        "Continuar en español",
                        "Usa español para el wizard, la UI y las respuestas de IA.",
                    ),
                ]
            } else {
                vec![
                    onboarding_action(
                        "Continue in English",
                        "Use English for the wizard, UI, and AI responses.",
                    ),
                    onboarding_action("Usar español", "Switch the rest of setup to Spanish."),
                ]
            },
        ),
        OnboardingStep::GitDependency(issue) | OnboardingStep::ProviderDependency(issue) => {
            let action_help = if is_spanish {
                "Ejecutá esta opción o corregí el entorno y usá Retry."
            } else {
                "Run this option or fix the environment and use Retry."
            };
            OnboardingContent::alert(
                if is_spanish {
                    "Resolver dependencia".to_string()
                } else {
                    "Resolve dependency".to_string()
                },
                dependency_modal_message(issue, if is_spanish { "spanish" } else { "english" }),
                if is_spanish {
                    "Este requisito bloquea el setup porque la app no puede completar el flujo sin esta pieza.".to_string()
                } else {
                    "This blocks setup because the app cannot complete the flow without this piece."
                        .to_string()
                },
                if is_spanish {
                    "Corregí la dependencia y volvé a intentar el chequeo.".to_string()
                } else {
                    "Fix the dependency and retry the check.".to_string()
                },
                if is_spanish {
                    "El color de alerta aparece solo cuando hay una dependencia o error real."
                        .to_string()
                } else {
                    "Alert color appears only for a real dependency or error.".to_string()
                },
                onboarding_dependency_actions(issue, language)
                    .into_iter()
                    .map(|action| onboarding_action(&action.label, action_help))
                    .collect(),
            )
        }
        OnboardingStep::RepoSetupChoice => OnboardingContent::new(
            if is_spanish {
                "Crear repositorio".to_string()
            } else {
                "Create repository".to_string()
            },
            if is_spanish {
                "Este directorio todavía no tiene `.git`.".to_string()
            } else {
                "This directory does not have `.git` yet.".to_string()
            },
            if is_spanish {
                "Git permite leer cambios, preparar commits y proteger ramas principales."
                    .to_string()
            } else {
                "Git lets Autopilot read changes, prepare commits, and protect main branches."
                    .to_string()
            },
            if is_spanish {
                "Inicializá Git para trabajar localmente o agregá `origin` si ya tenés un remote."
                    .to_string()
            } else {
                "Initialize Git for local work, or add `origin` if you already have a remote."
                    .to_string()
            },
            if is_spanish {
                "Podés configurar el remote más tarde si solo querés usar diff, review o commit local.".to_string()
            } else {
                "You can configure the remote later if you only need local diff, review, or commit."
                    .to_string()
            },
            if is_spanish {
                vec![
                    onboarding_action("Inicializar Git", "Crea `.git` y habilita flujos locales."),
                    onboarding_action(
                        "Inicializar Git + origin",
                        "Crea `.git` y te pide la URL del remote.",
                    ),
                ]
            } else {
                vec![
                    onboarding_action("Initialize Git", "Creates `.git` and enables local flows."),
                    onboarding_action(
                        "Initialize Git + origin",
                        "Creates `.git` and asks for the remote URL.",
                    ),
                ]
            },
        ),
        OnboardingStep::RemoteSetupChoice => OnboardingContent::new(
            if is_spanish {
                "Configurar remote".to_string()
            } else {
                "Configure remote".to_string()
            },
            if is_spanish {
                "Este repositorio no tiene `origin`.".to_string()
            } else {
                "This repository does not have `origin`.".to_string()
            },
            if is_spanish {
                "`push`, `/pr` y setup remoto necesitan un remote. Los flujos locales pueden seguir sin `origin`.".to_string()
            } else {
                "`push`, `/pr`, and remote setup need a remote. Local flows can continue without `origin`.".to_string()
            },
            if is_spanish {
                "Agregá la URL ahora o dejá el remote pendiente para terminar el setup local."
                    .to_string()
            } else {
                "Add the URL now or defer the remote to finish local setup.".to_string()
            },
            if is_spanish {
                "Si no usás GitHub todavía, continuar sin remote es válido.".to_string()
            } else {
                "If you don't use GitHub yet, continuing without a remote is valid.".to_string()
            },
            if is_spanish {
                vec![
                    onboarding_action(
                        "Agregar origin",
                        "Pide la URL y la guarda como remote `origin`.",
                    ),
                    onboarding_action(
                        "Seguir sin remote",
                        "Termina el setup local sin bloquearte.",
                    ),
                ]
            } else {
                vec![
                    onboarding_action("Add origin", "Asks for the URL and saves it as `origin`."),
                    onboarding_action(
                        "Continue without remote",
                        "Finishes local setup without blocking you.",
                    ),
                ]
            },
        ),
        OnboardingStep::ProviderSelection => OnboardingContent::new(
            if is_spanish {
                "Elegir proveedor".to_string()
            } else {
                "Choose provider".to_string()
            },
            if is_spanish {
                "Todavía no hay proveedor de IA activo.".to_string()
            } else {
                "There is no active AI provider yet.".to_string()
            },
            if is_spanish {
                "El proveedor define si usás Ollama local o una API como OpenAI, Gemini o Anthropic.".to_string()
            } else {
                "The provider defines whether you use local Ollama or an API such as OpenAI, Gemini, or Anthropic.".to_string()
            },
            if is_spanish {
                "Elegí proveedor. El wizard pedirá solo los datos que falten para esa opción."
                    .to_string()
            } else {
                "Choose a provider. The wizard asks only for the details that option needs."
                    .to_string()
            },
            if is_spanish {
                "Podés cambiar de proveedor más tarde desde settings o desde pasos de modelo/API key.".to_string()
            } else {
                "You can change provider later from settings or from model/API key steps."
                    .to_string()
            },
            if is_spanish {
                vec![onboarding_action(
                    "Seleccionar proveedor",
                    "Abre la lista de proveedores disponibles.",
                )]
            } else {
                vec![onboarding_action(
                    "Select provider",
                    "Opens the list of available providers.",
                )]
            },
        ),
        OnboardingStep::ModelConfiguration => OnboardingContent::new(
            if is_spanish {
                "Configurar modelo".to_string()
            } else {
                "Configure model".to_string()
            },
            if is_spanish {
                "Falta el modelo que va a responder.".to_string()
            } else {
                "The model that should answer is missing.".to_string()
            },
            if is_spanish {
                "`/commit`, `/review`, `/explain` y `/pr` necesitan saber qué modelo usar."
                    .to_string()
            } else {
                "`/commit`, `/review`, `/explain`, and `/pr` need to know which model to use."
                    .to_string()
            },
            if is_spanish {
                "Cargá la lista desde el proveedor, escribí el modelo manualmente o cambiá de proveedor."
                    .to_string()
            } else {
                "Load the provider list, enter the model manually, or change provider.".to_string()
            },
            if is_spanish {
                "La lista usa la clave API guardada. Si el proveedor no lista modelos, usá entrada manual.".to_string()
            } else {
                "The list uses the saved API key. If the provider cannot list models, use manual entry.".to_string()
            },
            if is_spanish {
                vec![
                    onboarding_action(
                        "Cargar modelos",
                        "Consulta modelos disponibles con la clave API.",
                    ),
                    onboarding_action(
                        "Escribir modelo manualmente",
                        "Permite pegar el nombre exacto si la API no puede listar modelos.",
                    ),
                    onboarding_action(
                        "Cambiar proveedor",
                        "Vuelve a elegir proveedor sin salir del setup.",
                    ),
                ]
            } else {
                vec![
                    onboarding_action("Load models", "Fetches available models with the API key."),
                    onboarding_action(
                        "Enter model manually",
                        "Paste the exact model name if the API cannot list models.",
                    ),
                    onboarding_action(
                        "Change provider",
                        "Choose another provider without leaving setup.",
                    ),
                ]
            },
        ),
        OnboardingStep::ApiKeyConfiguration => OnboardingContent::new(
            if is_spanish {
                "Configurar API key".to_string()
            } else {
                "Configure API key".to_string()
            },
            if is_spanish {
                "El proveedor seleccionado necesita una clave API.".to_string()
            } else {
                "The selected provider needs an API key.".to_string()
            },
            if is_spanish {
                "La clave autentica las llamadas y se guarda en el almacén seguro del sistema."
                    .to_string()
            } else {
                "The key authenticates provider calls and is stored in the OS secret store."
                    .to_string()
            },
            if is_spanish {
                "Pegá la clave. Después el setup intentará cargar los modelos del proveedor."
                    .to_string()
            } else {
                "Paste the key. Setup will then try to load the provider models.".to_string()
            },
            if is_spanish {
                "La clave no se guarda como texto plano en `config.json`.".to_string()
            } else {
                "The key is not stored as plain text in `config.json`.".to_string()
            },
            if is_spanish {
                vec![
                    onboarding_action(
                        "Ingresar clave API",
                        "Abre un input seguro para guardar la clave.",
                    ),
                    onboarding_action(
                        "Cambiar proveedor",
                        "Permite volver a Ollama u otra opción.",
                    ),
                ]
            } else {
                vec![
                    onboarding_action("Enter API key", "Opens a secure input to save the key."),
                    onboarding_action(
                        "Change provider",
                        "Switch back to Ollama or another option.",
                    ),
                ]
            },
        ),
    }
}

fn settings_rows(app: &TuiApp) -> Vec<String> {
    let config = &app.core.config;
    let lang = config.language.to_lowercase();
    let lang_str = lang.trim();

    let label_lang = match lang_str {
        "spanish" | "español" | "espanol" => "Idioma",
        _ => "Language",
    };
    let label_provider = match lang_str {
        "spanish" | "español" | "espanol" => "Proveedor",
        _ => "Provider",
    };
    let label_model = match lang_str {
        "spanish" | "español" | "espanol" => "Modelo",
        _ => "Model",
    };
    let label_base_url = match lang_str {
        "spanish" | "español" | "espanol" => "Base URL",
        _ => "Base URL",
    };
    let label_api_key = match lang_str {
        "spanish" | "español" | "espanol" => "API key",
        _ => "API key",
    };
    let label_staged = match lang_str {
        "spanish" | "español" | "espanol" => "Solo staged",
        _ => "Staged only",
    };
    let label_setup = match lang_str {
        "spanish" | "español" | "espanol" => "Auto configurar",
        _ => "Auto setup",
    };
    let label_push = match lang_str {
        "spanish" | "español" | "espanol" => "Preguntar push",
        _ => "Prompt push",
    };
    let label_theme = match lang_str {
        "spanish" | "español" | "espanol" => "Tema",
        _ => "Theme",
    };
    let label_history = match lang_str {
        "spanish" | "español" | "espanol" => "Historial",
        _ => "History limit",
    };

    let val_model = if app.core.config.provider == crate::domain::LlmProviderKind::Ollama
        && app.core.config.provider.supports_model_listing()
        && !ollama_running(app)
    {
        match lang_str {
            "spanish" | "español" | "espanol" => "N/A (desconectado)",
            _ => "N/A (offline)",
        }
    } else {
        config.model.as_deref().unwrap_or(match lang_str {
            "spanish" | "español" | "espanol" => "no seleccionado",
            _ => "not selected",
        })
    };

    let val_lang = match config.language.to_lowercase().as_str() {
        "spanish" | "español" | "espanol" => "Español",
        _ => "English",
    };

    let val_history = match lang_str {
        "spanish" | "español" | "espanol" => match config.history_limit {
            crate::domain::HistoryLimit::Small => "20 mensajes".to_string(),
            crate::domain::HistoryLimit::Medium => "40 mensajes".to_string(),
            crate::domain::HistoryLimit::Large => "80 mensajes".to_string(),
        },
        _ => config.history_limit.label().to_string(),
    };

    let val_provider = app.core.provider_label();
    let val_base_url = if !config.provider.is_selected() {
        match lang_str {
            "spanish" | "español" | "espanol" => "sin seleccionar",
            _ => "not selected",
        }
    } else if config.provider == crate::domain::LlmProviderKind::Ollama {
        app.core
            .config
            .base_url
            .as_deref()
            .unwrap_or(match lang_str {
                "spanish" | "español" | "espanol" => "predeterminada",
                _ => "default",
            })
    } else {
        match lang_str {
            "spanish" | "español" | "espanol" => "fijo",
            _ => "fixed",
        }
    };
    let val_api_key = if app.core.api_key_configured().unwrap_or(false) {
        match lang_str {
            "spanish" | "español" | "espanol" => "configurada",
            _ => "configured",
        }
    } else {
        match lang_str {
            "spanish" | "español" | "espanol" => "sin configurar",
            _ => "not configured",
        }
    };

    vec![
        format!("{:<16} {}", label_lang, val_lang),
        format!("{:<16} {}", label_provider, val_provider),
        format!("{:<16} {}", label_model, val_model),
        format!("{:<16} {}", label_base_url, val_base_url),
        format!("{:<16} {}", label_api_key, val_api_key),
        format!(
            "{:<16} {}",
            label_staged,
            translate_boolean(config.staged_only, &config.language)
        ),
        format!(
            "{:<16} {}",
            label_setup,
            translate_boolean(config.auto_setup_repo, &config.language)
        ),
        format!(
            "{:<16} {}",
            label_push,
            translate_boolean(config.prompt_push_after_commit, &config.language)
        ),
        format!("{:<16} {}", label_theme, config.theme.label()),
        format!("{:<16} {}", label_history, val_history),
    ]
}

fn translate_boolean(value: bool, language: &str) -> &'static str {
    let lang = language.to_lowercase();
    match lang.trim() {
        "spanish" | "español" | "espanol" => {
            if value {
                "Sí"
            } else {
                "No"
            }
        }
        _ => {
            if value {
                "Yes"
            } else {
                "No"
            }
        }
    }
}

fn settings_description(selected: usize, language: &str) -> &'static str {
    let lang = language.to_lowercase();
    let lang_str = lang.trim();
    match lang_str {
        "spanish" | "español" | "espanol" => match selected {
            0 => "Cambia el idioma de la interfaz del CLI y de las respuestas generadas por la IA.",
            1 => "Selecciona el proveedor de IA activo para comandos como /commit, /review y /pr.",
            2 => "Configura el modelo activo del proveedor seleccionado.",
            3 => "Configura el host local de Ollama si no usas localhost:11434.",
            4 => {
                "Guarda o reemplaza la API key del proveedor activo en el almacén seguro del sistema."
            }
            5 => "Si se activa, el análisis de cambios se limita únicamente a los archivos staged.",
            6 => "Inicializa automáticamente Git si el directorio actual no es un repositorio.",
            7 => "Pregunta si deseas hacer 'git push' a la rama remota tras realizar un commit.",
            8 => "Elige la paleta de colores para los mensajes del chat y las burbujas de diálogo.",
            9 => "Establece el número máximo de mensajes conservados en la ventana de chat.",
            _ => "",
        },
        _ => match selected {
            0 => "Change the language of the CLI interface and the AI-generated responses.",
            1 => "Select the active AI provider used by /commit, /review, /explain, and /pr.",
            2 => "Configure the active model for the selected provider.",
            3 => "Configure the local Ollama host if you are not using localhost:11434.",
            4 => "Store or replace the active provider API key in the OS secure secret store.",
            5 => "If enabled, changes analysis is restricted only to staged files.",
            6 => "Automatically initialize Git if the current directory is not a repository.",
            7 => "Prompt to push the current branch to remote after committing changes.",
            8 => "Choose the color palette for chat messages and dialog bubbles.",
            9 => "Set the maximum number of messages preserved in the chat window.",
            _ => "",
        },
    }
}

fn modal_block(title: &str, colors: Palette) -> Block<'_> {
    modal_block_with_border(title, colors, colors.border)
}

fn modal_block_with_border(title: &str, colors: Palette, border: Color) -> Block<'_> {
    Block::default()
        .title(title)
        .title_alignment(ratatui::layout::Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(colors.modal_bg))
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let pct_x = if area.width < 70 {
        98u16
    } else if area.width < 100 {
        90u16
    } else if area.width < 140 {
        80u16
    } else {
        percent_x.min(90)
    };

    let pct_y = if area.height < 25 {
        95u16
    } else if area.height < 40 {
        85u16
    } else {
        percent_y.min(90)
    };

    let vert_padding = (100u16.saturating_sub(pct_y)) / 2;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(vert_padding),
            Constraint::Percentage(pct_y),
            Constraint::Percentage(100u16.saturating_sub(pct_y).saturating_sub(vert_padding)),
        ])
        .split(area);

    let horiz_padding = (100u16.saturating_sub(pct_x)) / 2;
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(horiz_padding),
            Constraint::Percentage(pct_x),
            Constraint::Percentage(100u16.saturating_sub(pct_x).saturating_sub(horiz_padding)),
        ])
        .split(vertical[1])[1]
}

#[derive(Clone, Copy)]
struct Palette {
    text: Color,
    muted: Color,
    hint: Color,
    border: Color,
    status_bg: Color,
    modal_bg: Color,
    user: Color,
    assistant: Color,
    system: Color,
    accent: Color,
    warning: Color,
    success: Color,
    info: Color,
}

fn palette(theme: ThemeChoice) -> Palette {
    match theme {
        ThemeChoice::CodexDark => Palette {
            text: Color::Rgb(241, 245, 249),
            muted: Color::Rgb(100, 116, 139),
            hint: Color::Rgb(148, 163, 184),
            border: Color::Rgb(51, 65, 85),
            status_bg: Color::Rgb(15, 23, 42),
            modal_bg: Color::Rgb(30, 41, 59),
            user: Color::Rgb(52, 211, 153),
            assistant: Color::Rgb(129, 140, 248),
            system: Color::Rgb(251, 146, 60),
            accent: Color::Rgb(56, 189, 248),
            warning: Color::Rgb(239, 68, 68),
            success: Color::Rgb(16, 185, 129),
            info: Color::Rgb(99, 102, 241),
        },
        ThemeChoice::Nord => Palette {
            text: Color::Rgb(236, 239, 244),
            muted: Color::Rgb(129, 161, 193),
            hint: Color::Rgb(143, 188, 187),
            border: Color::Rgb(76, 86, 106),
            status_bg: Color::Rgb(46, 52, 64),
            modal_bg: Color::Rgb(59, 66, 82),
            user: Color::Rgb(163, 190, 140),
            assistant: Color::Rgb(136, 192, 208),
            system: Color::Rgb(208, 135, 112),
            accent: Color::Rgb(129, 161, 193),
            warning: Color::Rgb(191, 97, 106),
            success: Color::Rgb(163, 190, 140),
            info: Color::Rgb(94, 129, 172),
        },
        ThemeChoice::Sunset => Palette {
            text: Color::Rgb(253, 244, 245),
            muted: Color::Rgb(214, 90, 90),
            hint: Color::Rgb(245, 117, 73),
            border: Color::Rgb(100, 20, 30),
            status_bg: Color::Rgb(45, 0, 28),
            modal_bg: Color::Rgb(29, 1, 32),
            user: Color::Rgb(250, 188, 42),
            assistant: Color::Rgb(240, 90, 126),
            system: Color::Rgb(255, 107, 107),
            accent: Color::Rgb(248, 150, 30),
            warning: Color::Rgb(249, 65, 68),
            success: Color::Rgb(144, 190, 109),
            info: Color::Rgb(247, 37, 133),
        },
        ThemeChoice::Dracula => Palette {
            text: Color::Rgb(248, 248, 242),
            muted: Color::Rgb(98, 114, 164),
            hint: Color::Rgb(139, 150, 180),
            border: Color::Rgb(68, 71, 90),
            status_bg: Color::Rgb(40, 42, 54),
            modal_bg: Color::Rgb(35, 37, 49),
            user: Color::Rgb(80, 250, 123),
            assistant: Color::Rgb(189, 147, 249),
            system: Color::Rgb(255, 184, 108),
            accent: Color::Rgb(139, 233, 253),
            warning: Color::Rgb(255, 85, 85),
            success: Color::Rgb(80, 250, 123),
            info: Color::Rgb(189, 147, 249),
        },
        ThemeChoice::HighContrast => Palette {
            text: Color::White,
            muted: Color::Gray,
            hint: Color::Rgb(180, 180, 180),
            border: Color::White,
            status_bg: Color::Blue,
            modal_bg: Color::Black,
            user: Color::LightGreen,
            assistant: Color::LightCyan,
            system: Color::LightMagenta,
            accent: Color::Yellow,
            warning: Color::LightYellow,
            success: Color::LightGreen,
            info: Color::LightBlue,
        },
        ThemeChoice::Light => Palette {
            text: Color::Black,
            muted: Color::Rgb(100, 100, 100),
            hint: Color::Rgb(140, 140, 140),
            border: Color::Gray,
            status_bg: Color::Rgb(220, 220, 225),
            modal_bg: Color::White,
            user: Color::Rgb(0, 160, 40),
            assistant: Color::Rgb(0, 120, 200),
            system: Color::Rgb(180, 40, 200),
            accent: Color::Rgb(0, 120, 200),
            warning: Color::Rgb(200, 160, 0),
            success: Color::Rgb(0, 160, 40),
            info: Color::Rgb(40, 100, 200),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::Instant;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use reqwest::Client;
    use tempfile::tempdir;

    use super::*;
    use crate::application::{AppCore, OllamaHealth};
    use crate::domain::commit::CommitGroup;
    use crate::domain::{CommitMessage, CommitPlan, Config, FileEntry, LlmContextUsage};
    use crate::infrastructure::dependencies::PlatformOs;
    use crate::infrastructure::{
        BranchOption, BranchSource, DependencyDoctor, DependencyKind, DependencyState,
        DependencyStatus, PackageManager, PlatformInfo, SwitchBranches,
    };
    use crate::ui::state::ExecutionMode;

    fn make_app_in(cwd: PathBuf) -> TuiApp {
        let config = Config::default();
        let core = AppCore::new(cwd, config, Client::new());
        let mut app = TuiApp::new(core);
        app.last_spinner_tick = Instant::now();
        app.ollama_health = Some(OllamaHealth::ready("0.9.0".to_string()));
        app.execution_mode = ExecutionMode::Autopilot;
        app
    }

    fn make_app() -> TuiApp {
        make_app_in(PathBuf::from("."))
    }

    fn platform() -> PlatformInfo {
        PlatformInfo {
            os: PlatformOs::Windows,
            package_manager: Some(PackageManager::Winget),
            is_elevated: false,
        }
    }

    fn doctor_with(ollama: DependencyState, gh: DependencyState) -> DependencyDoctor {
        let platform = platform();
        let ollama_status = match ollama {
            DependencyState::Ready => DependencyStatus::ready(
                DependencyKind::Ollama,
                &platform,
                Some("0.9.0".to_string()),
            ),
            DependencyState::Missing => {
                DependencyStatus::missing(DependencyKind::Ollama, &platform, None)
            }
            DependencyState::NotRunning => DependencyStatus::ollama_not_running(
                &platform,
                Some("0.9.0".to_string()),
                "down".to_string(),
            ),
            DependencyState::NotConfigured => {
                DependencyStatus::ollama_no_models(&platform, Some("0.9.0".to_string()))
            }
        };
        let gh_status = match gh {
            DependencyState::Ready => DependencyStatus::ready(
                DependencyKind::GitHubCli,
                &platform,
                Some("2.60".to_string()),
            ),
            DependencyState::Missing => {
                DependencyStatus::missing(DependencyKind::GitHubCli, &platform, None)
            }
            DependencyState::NotConfigured => {
                DependencyStatus::gh_auth_missing(&platform, Some("2.60".to_string()))
            }
            DependencyState::NotRunning => DependencyStatus::missing(
                DependencyKind::GitHubCli,
                &platform,
                Some("unsupported".to_string()),
            ),
        };
        DependencyDoctor {
            platform: platform.clone(),
            git: DependencyStatus::ready(DependencyKind::Git, &platform, Some("2.47".to_string())),
            llm_provider: match ollama {
                DependencyState::Ready => DependencyStatus::ready(
                    DependencyKind::LlmProvider,
                    &platform,
                    Some("ok".to_string()),
                ),
                DependencyState::Missing | DependencyState::NotRunning => {
                    DependencyStatus::llm_provider_not_running(
                        &platform,
                        "provider unavailable".to_string(),
                        Some("https://ollama.com/download"),
                    )
                }
                DependencyState::NotConfigured => DependencyStatus::llm_provider_not_configured(
                    &platform,
                    "provider incomplete".to_string(),
                    Some("https://ollama.com/download"),
                ),
            },
            ollama: ollama_status,
            gh: gh_status,
        }
    }

    fn lines_text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("")
    }

    fn render_to_text(app: &TuiApp, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .join("")
    }

    fn commit_plan_with_groups(count: usize) -> CommitPlan {
        CommitPlan {
            summary: "grouped changes".to_string(),
            groups: (0..count)
                .map(|index| CommitGroup {
                    commit: CommitMessage {
                        commit_type: "fix".to_string(),
                        scope: "tui".to_string(),
                        subject: format!("compact plan {}", index + 1),
                        body: "body should not dominate the review modal".to_string(),
                    },
                    files: vec![FileEntry {
                        id: format!("src/file{}.rs", index + 1),
                        path: format!("src/file{}.rs", index + 1),
                        status: "modified".to_string(),
                        description: "test file".to_string(),
                        patch: None,
                    }],
                    rationale: "same change context".to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn commit_plan_review_shows_all_changes_scope() {
        let mut app = make_app();
        app.core.config.staged_only = false;
        app.modal = Some(Modal::CommitPlanReview {
            plan: commit_plan_with_groups(2),
            selected: 0,
            scroll: 0,
        });

        let text = render_to_text(&app, 120, 28);

        assert!(
            text.contains("All changes: staged, unstaged, and untracked"),
            "{text}"
        );
        assert!(text.contains("Proposed commits"), "{text}");
        assert!(text.contains("Files: 2"), "{text}");
        assert!(text.contains("src/file1.rs (modified)"), "{text}");
        assert!(text.contains("src/file2.rs (modified)"), "{text}");
        assert!(!text.contains("Summary"), "{text}");
        assert!(!text.contains("grouped changes"), "{text}");
        assert!(!text.contains("same change context"), "{text}");
        assert!(!text.contains("body should not dominate"), "{text}");
    }

    #[test]
    fn commit_plan_review_shows_staged_only_scope() {
        let mut app = make_app();
        app.core.config.staged_only = true;
        app.modal = Some(Modal::CommitPlanReview {
            plan: commit_plan_with_groups(1),
            selected: 0,
            scroll: 0,
        });

        let text = render_to_text(&app, 120, 28);

        assert!(text.contains("Staged changes only"), "{text}");
    }

    #[test]
    fn long_assistant_message_scrolls_without_truncation_marker() {
        let mut app = make_app();
        let message = (1..=40)
            .map(|index| format!("history-line-{index:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.push_assistant(message);

        let latest = render_to_text(&app, 100, 16);

        assert!(latest.contains("history-line-40"), "{latest}");
        assert!(!latest.contains("history-line-01"), "{latest}");
        assert!(!latest.contains("[truncado]"), "{latest}");
        assert!(!latest.contains("[truncated]"), "{latest}");

        app.history_scroll = 30;
        let earlier = render_to_text(&app, 100, 16);

        assert!(earlier.contains("history-line-10"), "{earlier}");
        assert!(earlier.contains("Reading history"), "{earlier}");
    }

    #[test]
    fn assistant_markdown_is_rendered_as_readable_text() {
        let mut app = make_app();
        app.push_assistant(
            "# Title\n\n**Bold** and `code`\n```rust\nfn main() {}\n```\n[OpenAI](https://openai.com)\n- item",
        );

        let text = render_to_text(&app, 100, 22);

        assert!(text.contains("Title"), "{text}");
        assert!(text.contains("Bold and code"), "{text}");
        assert!(text.contains("fn main() {}"), "{text}");
        assert!(text.contains("OpenAI (https://openai.com)"), "{text}");
        assert!(text.contains("• item"), "{text}");
        assert!(!text.contains("# Title"), "{text}");
        assert!(!text.contains("**"), "{text}");
        assert!(!text.contains("```"), "{text}");
        assert!(!text.contains("`code`"), "{text}");
        assert!(!text.contains("[OpenAI]("), "{text}");
    }

    #[test]
    fn scout_mode_labels_assistant_messages_as_scout() {
        let mut app = make_app();
        app.execution_mode = ExecutionMode::Scout;
        app.push_assistant("Scout analysis ready.");

        let text = render_to_text(&app, 100, 18);

        assert!(text.contains("Scout"), "{text}");
        assert_eq!(text.matches("Autopilot").count(), 1, "{text}");
    }

    fn mock_branch_modal() -> Modal {
        Modal::BranchSwitch {
            branches: SwitchBranches {
                remote: vec![
                    BranchOption {
                        name: "main".to_string(),
                        source: BranchSource::Remote,
                        last_commit_unix: Some(20),
                        is_current: false,
                    },
                    BranchOption {
                        name: "feature/api".to_string(),
                        source: BranchSource::Remote,
                        last_commit_unix: Some(10),
                        is_current: false,
                    },
                ],
                local: vec![
                    BranchOption {
                        name: "feature/api".to_string(),
                        source: BranchSource::Local,
                        last_commit_unix: Some(30),
                        is_current: true,
                    },
                    BranchOption {
                        name: "release".to_string(),
                        source: BranchSource::Local,
                        last_commit_unix: Some(5),
                        is_current: false,
                    },
                ],
            },
            selected: 0,
        }
    }

    #[test]
    fn responsive_status_keeps_single_line_on_narrow_width() {
        let mut app = make_app();
        app.core.config.provider = crate::domain::LlmProviderKind::Ollama;
        app.ollama_health = Some(OllamaHealth::ready("0.9.0".to_string()));
        app.dependency_doctor = Some(doctor_with(DependencyState::Ready, DependencyState::Ready));
        app.last_context_usage = Some(LlmContextUsage {
            estimated_tokens: 900,
            limit: 1000,
            truncated: true,
        });

        let lines = responsive_status_lines(&app, 28, palette(app.core.config.theme), 7);
        let text = lines_text(&lines);

        assert_eq!(lines.len(), 1);
        assert!(text.contains("Ollama"));
    }

    #[test]
    fn responsive_status_hides_context_before_first_llm_usage() {
        let app = make_app();

        let lines = responsive_status_lines(&app, 28, palette(app.core.config.theme), 12);
        let text = lines_text(&lines);

        assert!(!text.contains("Context"));
    }

    #[test]
    fn responsive_status_marks_missing_local_repo() {
        let dir = tempdir().unwrap();
        let mut app = make_app_in(dir.path().to_path_buf());
        app.core.config.provider = crate::domain::LlmProviderKind::Ollama;
        app.ollama_health = Some(OllamaHealth::ready("0.9.0".to_string()));
        app.dependency_doctor = Some(doctor_with(DependencyState::Ready, DependencyState::Ready));

        let lines = responsive_status_lines(&app, 80, palette(app.core.config.theme), 12);
        let text = lines_text(&lines);

        assert!(text.contains("repo missing"));
        assert!(text.contains("run /setup"));
        assert!(!text.contains("remote no repo"));
    }

    #[test]
    fn responsive_status_marks_missing_remote_origin() {
        let dir = tempdir().unwrap();
        let status = Command::new("git")
            .current_dir(dir.path())
            .arg("init")
            .status()
            .unwrap();
        assert!(status.success());
        let mut app = make_app_in(dir.path().to_path_buf());
        app.core.config.provider = crate::domain::LlmProviderKind::Ollama;
        app.ollama_health = Some(OllamaHealth::ready("0.9.0".to_string()));
        app.dependency_doctor = Some(doctor_with(DependencyState::Ready, DependencyState::Ready));

        let lines = responsive_status_lines(&app, 80, palette(app.core.config.theme), 12);
        let text = lines_text(&lines);

        assert!(text.contains("repo exists (local)"));
        assert!(!text.contains("repo ok"));
        assert!(!text.contains("remote missing"));
    }

    #[test]
    fn onboarding_modal_renders_core_content_on_standard_terminal() {
        let mut app = make_app();
        app.onboarding_active = true;
        app.modal = Some(Modal::OnboardingWizard {
            step: OnboardingStep::LanguageSelection,
            selected: 0,
        });

        let text = render_to_text(&app, 80, 24);

        assert!(text.contains("Guided setup"));
        assert!(text.contains("Choose language"));
        assert!(text.contains("Missing"));
        assert!(text.contains("Why it matters"));
        assert!(text.contains("Continue in English"));
        assert!(text.contains("Enter select"));
        assert!(!text.contains("Required: complete this step to continue."));
    }

    #[test]
    fn onboarding_modal_renders_spanish_content_on_narrow_terminal() {
        let mut app = make_app();
        app.core.config.language = "Spanish".to_string();
        app.onboarding_active = true;
        app.modal = Some(Modal::OnboardingWizard {
            step: OnboardingStep::ApiKeyConfiguration,
            selected: 1,
        });

        let text = render_to_text(&app, 60, 20);

        assert!(text.contains("Setup guiado"));
        assert!(text.contains("Configurar API"));
        assert!(text.contains("Falta"));
        assert!(text.contains("Por qué importa"));
        assert!(text.contains("Cambiar proveedor"));
        assert!(text.contains("Enter seleccionar"));
    }

    #[test]
    fn onboarding_language_actions_match_saved_selection_order() {
        let mut app = make_app();
        app.core.config.language = "Spanish".to_string();
        app.onboarding_active = true;
        app.modal = Some(Modal::OnboardingWizard {
            step: OnboardingStep::LanguageSelection,
            selected: 0,
        });

        let text = render_to_text(&app, 90, 24);

        assert!(text.contains("> Usar inglés"), "{text}");
        assert!(text.contains("Continuar en español"), "{text}");
        assert!(text.contains("Esc idioma"), "{text}");
    }

    #[test]
    fn settings_child_picker_says_escape_closes_not_back() {
        let mut app = make_app();
        app.modal = Some(Modal::Picker {
            title: "Select language".to_string(),
            items: vec![
                crate::ui::state::PickerItem {
                    label: "English".to_string(),
                    value: crate::ui::state::PickerValue::Language("English".to_string()),
                },
                crate::ui::state::PickerItem {
                    label: "Spanish".to_string(),
                    value: crate::ui::state::PickerValue::Language("Spanish".to_string()),
                },
            ],
            selected: 0,
        });

        let text = render_to_text(&app, 80, 18);

        assert!(text.contains("Esc close"), "{text}");
        assert!(!text.contains("Esc back"), "{text}");
    }

    #[test]
    fn reset_confirmation_explains_safe_scope() {
        let mut app = make_app();
        app.modal = Some(Modal::Confirm {
            title: "Reset configuration".to_string(),
            message: crate::ui::state::reset_confirmation_message("English"),
            selected: 1,
            kind: crate::ui::state::ConfirmKind::ResetConfiguration,
        });

        let text = render_to_text(&app, 100, 24);

        assert!(text.contains("Reset configuration"), "{text}");
        assert!(text.contains("Not deleted: .git"), "{text}");
        assert!(text.contains("Reset settings"), "{text}");
        assert!(text.contains("Cancel"), "{text}");
    }

    #[test]
    fn onboarding_progress_uses_accent_not_warning_for_current_normal_step() {
        let mut app = make_app();
        app.onboarding_active = true;
        let colors = palette(app.core.config.theme);

        let lines = onboarding_progress_lines(&app, &OnboardingStep::LanguageSelection, colors);
        let current_style = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.contains(">"))
            .expect("expected current progress marker")
            .style;

        assert_eq!(current_style.fg, Some(colors.accent));
        assert_ne!(current_style.fg, Some(colors.warning));
    }

    #[test]
    fn onboarding_dependency_content_keeps_alert_tone_for_real_issue() {
        let platform = PlatformInfo {
            os: PlatformOs::Windows,
            package_manager: None,
            is_elevated: false,
        };
        let issue = DependencyStatus::missing(
            DependencyKind::Git,
            &platform,
            Some("git not found".to_string()),
        );

        let content = onboarding_step_content(&OnboardingStep::GitDependency(issue), "English");
        let colors = palette(ThemeChoice::CodexDark);

        assert!(content.alert);
        assert_eq!(content.title_color(colors), colors.warning);
    }

    #[test]
    fn api_key_text_input_masks_secret_value() {
        let mut app = make_app();
        app.modal = Some(Modal::TextInput {
            title: "API key".to_string(),
            value: "sk-secret-value".to_string(),
            kind: TextInputKind::ApiKey,
        });

        let text = render_to_text(&app, 80, 20);

        assert!(!text.contains("sk-secret-value"));
        assert!(text.contains("***************"));
    }

    #[test]
    fn setup_modal_renders_guided_cards_with_spacing() {
        let mut app = make_app();
        app.modal = Some(Modal::Setup { selected: 0 });

        let text = render_to_text(&app, 90, 24);

        assert!(text.contains("Project setup"), "{text}");
        assert!(text.contains("Guided preparation"), "{text}");
        assert!(text.contains("Work locally"), "{text}");
        assert!(text.contains("Connect existing remote"), "{text}");
        assert!(text.contains("Shortcuts"), "{text}");
        assert!(!text.contains("Initialize Git only"), "{text}");
        assert!(!text.contains("Setup │"), "{text}");
    }

    #[test]
    fn full_render_keeps_core_status_visible_on_mobile_width() {
        let dir = tempdir().unwrap();
        let status = Command::new("git")
            .current_dir(dir.path())
            .arg("init")
            .status()
            .unwrap();
        assert!(status.success());
        let mut app = make_app_in(dir.path().to_path_buf());
        app.core.config.provider = crate::domain::LlmProviderKind::Ollama;
        app.ollama_health = Some(OllamaHealth::ready("0.9.0".to_string()));
        app.dependency_doctor = Some(doctor_with(DependencyState::Ready, DependencyState::Ready));
        app.last_context_usage = Some(LlmContextUsage {
            estimated_tokens: 850,
            limit: 1000,
            truncated: false,
        });

        let text = render_to_text(&app, 40, 16);

        assert!(text.contains("Ollama"), "{text}");
        assert!(text.contains("repo local"), "{text}");
        assert!(text.contains("F2 settings"), "{text}");
    }

    #[test]
    fn status_uses_middle_dot_separators() {
        let app = make_app();
        let lines = responsive_status_lines(&app, 120, palette(app.core.config.theme), 4);
        let text = lines_text(&lines);

        assert!(text.contains(" · "));
        assert!(!text.contains("F2"));
    }

    #[test]
    fn responsive_status_stays_on_single_line() {
        let app = make_app();
        let lines = responsive_status_lines(&app, 120, palette(app.core.config.theme), 4);

        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn full_render_shows_keyboard_footer_separate_from_status() {
        let app = make_app();
        let text = render_to_text(&app, 100, 18);

        assert!(text.contains("F2 settings"), "{text}");
        assert!(text.contains("Shift+Tab mode"), "{text}");
    }

    #[test]
    fn branch_status_item_uses_distinct_styles_for_label_and_name() {
        let colors = palette(ThemeChoice::CodexDark);
        let item = branch_status_item("main", "english", colors, false);

        assert_ne!(item[0].style.fg, item[2].style.fg);
        assert_eq!(item[2].style.fg, Some(Color::White));
        assert_eq!(item[2].style.bg, Some(Color::Rgb(220, 38, 38)));
    }

    #[test]
    fn degraded_dependency_status_uses_capability_chips_instead_of_history_text() {
        let mut app = make_app();
        app.core.config.provider = crate::domain::LlmProviderKind::Ollama;
        app.dependency_doctor = Some(doctor_with(
            DependencyState::NotConfigured,
            DependencyState::NotConfigured,
        ));

        let text = lines_text(&responsive_status_lines(
            &app,
            120,
            palette(app.core.config.theme),
            4,
        ));

        assert!(text.contains("Ollama needs model"), "{text}");
        assert!(text.contains("PR auth needed"), "{text}");
        assert!(!text.contains("GitHub needs login"), "{text}");
        assert!(!text.contains("github auth"), "{text}");
    }

    #[test]
    fn api_provider_status_avoids_online_and_local_vram_residue() {
        let mut app = make_app();
        app.core.config.provider = crate::domain::LlmProviderKind::Gemini;
        app.core.vram_mb = Some(10 * 1024);
        app.ollama_health = Some(OllamaHealth::ready("0.9.0".to_string()));
        app.dependency_doctor = Some(doctor_with(DependencyState::Ready, DependencyState::Ready));
        app.last_context_usage = Some(LlmContextUsage {
            estimated_tokens: 900,
            limit: 1_000,
            truncated: true,
        });

        let text = lines_text(&responsive_status_lines(
            &app,
            120,
            palette(app.core.config.theme),
            4,
        ));

        assert!(text.contains("Gemini Provider"), "{text}");
        assert!(!text.contains("online"), "{text}");
        assert!(!text.contains("GB"), "{text}");
        assert!(!text.contains("MB"), "{text}");
        assert!(!text.contains("Ollama Provider"), "{text}");
        assert!(!text.contains("Context"), "{text}");
        assert!(!text.contains("Diff truncated"), "{text}");
    }

    #[test]
    fn spanish_status_uses_natural_localized_git_and_provider_text() {
        let dir = tempdir().unwrap();
        let status = Command::new("git")
            .current_dir(dir.path())
            .arg("init")
            .status()
            .unwrap();
        assert!(status.success());
        let mut app = make_app_in(dir.path().to_path_buf());
        app.core.config.language = "Spanish".to_string();
        app.core.config.provider = crate::domain::LlmProviderKind::Gemini;
        app.dependency_doctor = Some(doctor_with(DependencyState::Ready, DependencyState::Ready));

        let text = lines_text(&responsive_status_lines(
            &app,
            120,
            palette(app.core.config.theme),
            4,
        ));

        assert!(text.contains("Proveedor Gemini"), "{text}");
        assert!(text.contains("repo existe (local)"), "{text}");
        assert!(text.contains("rama"), "{text}");
        assert!(!text.contains("online"), "{text}");
        assert!(!text.contains("missing"), "{text}");
        assert!(!text.contains("repo ok"), "{text}");
    }

    #[test]
    fn full_render_shows_commit_busy_message_only_in_input_row() {
        let mut app = make_app();
        app.busy = true;
        app.busy_message = "Generating a structured commit plan...".to_string();
        app.ollama_health = Some(OllamaHealth::ready("0.9.0".to_string()));

        let text = render_to_text(&app, 100, 24);
        let occurrences = text
            .matches("Generating a structured commit plan...")
            .count();

        assert_eq!(occurrences, 1, "{text}");
        assert!(!text.contains("Ollama is generating"), "{text}");
    }

    #[test]
    fn status_uses_expected_provider_colors_for_health_states() {
        let mut app = make_app();
        app.core.config.provider = crate::domain::LlmProviderKind::Ollama;
        let colors = palette(app.core.config.theme);

        app.ollama_health = None;
        let checking = provider_status_span(&app, colors, false);
        assert_eq!(checking.style.fg, Some(Color::White));

        app.ollama_health = Some(OllamaHealth::ready("0.9.0".to_string()));
        let ready = provider_status_span(&app, colors, false);
        assert_eq!(ready.style.bg, Some(colors.success));

        app.ollama_health = Some(OllamaHealth::not_running(
            Some("0.9.0".to_string()),
            "connection refused".to_string(),
        ));
        let warning = provider_status_span(&app, colors, false);
        assert_eq!(warning.style.bg, Some(Color::Rgb(220, 38, 38)));

        app.ollama_health = Some(OllamaHealth::not_installed("missing".to_string()));
        let offline = provider_status_span(&app, colors, false);
        assert_eq!(offline.style.bg, Some(Color::Rgb(220, 38, 38)));
    }

    #[test]
    fn branch_switch_item_uses_red_for_protected_and_success_for_regular_branches() {
        let colors = palette(ThemeChoice::CodexDark);
        let protected = BranchOption {
            name: "main".to_string(),
            source: BranchSource::Remote,
            last_commit_unix: Some(1),
            is_current: false,
        };
        let regular = BranchOption {
            name: "feature/api".to_string(),
            source: BranchSource::Local,
            last_commit_unix: Some(1),
            is_current: false,
        };

        assert_eq!(
            branch_switch_branch_style(&protected, colors).fg,
            Some(Color::Rgb(239, 68, 68))
        );
        assert_eq!(
            branch_switch_branch_style(&regular, colors).fg,
            Some(colors.success)
        );
    }

    #[test]
    fn branch_switch_modal_renders_origin_and_local_sections() {
        let mut app = make_app();
        app.modal = Some(mock_branch_modal());

        let text = render_to_text(&app, 90, 24);

        assert!(text.contains("origin"), "{text}");
        assert!(text.contains("local"), "{text}");
        assert!(text.contains("main"), "{text}");
        assert!(text.contains("feature/api current"), "{text}");
    }

    #[test]
    fn local_repo_chip_uses_pending_background_and_dark_text() {
        let colors = palette(ThemeChoice::CodexDark);

        let item = git_status_item(true, false, "english", colors, false);
        let style = item[0].style;

        assert_eq!(style.fg, Some(Color::Rgb(15, 23, 42)));
        assert_eq!(style.bg, Some(Color::Rgb(250, 204, 21)));
    }
}
