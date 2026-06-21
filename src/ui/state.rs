use std::future::Future;
use std::io::{self, Stdout};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arboard::Clipboard;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode, size,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TokioCommand;
use tokio::sync::mpsc;

use crate::application::{AppCore, OllamaHealth, help_text};
use crate::domain::commit::strip_emoji;
use crate::domain::{
    CommitMessage, CommitPlan, FileEntry, Intent, IntentDecision, IntentParser, LlmContextUsage,
    LlmProviderKind, PullRequestDraft, PrInfo, Suggestion, slash_suggestions,
};
use crate::error::{AppError, Result};
use crate::infrastructure::{
    CommitLogEntry, DependencyAction, DependencyActionKind, DependencyDoctor, DependencyKind,
    DependencyState, DependencyStatus, PlatformInfo, SwitchBranches,
};

use super::render::draw;

type Term = Terminal<CrosstermBackend<Stdout>>;

pub const SPINNER: &[char] = &['|', '/', '-', '\\'];
const COMMIT_PLAN_TIMEOUT_SECS: u64 = 180;

pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Tick,
    AsyncOutcome(Box<AsyncOutcome>),
}

pub enum AsyncOutcome {
    Message(String),
    Error(AppError),
    DependencyDoctor(DependencyDoctor),
    DependencyCommandOutput(String),
    DependencyCommandFinished {
        report: DependencyDoctor,
        issue: DependencyStatus,
        blocking: bool,
        resume_intent: Option<Intent>,
        succeeded: bool,
    },
    CommitPlanReady(CommitPlan, LlmContextUsage),
    CommitLogReady(Vec<CommitLogEntry>),
    CommitLogReset(String),
    SwitchBranchesReady(SwitchBranches),
    PrDraft(String, PullRequestDraft),
    ExistingPrs(Vec<PrInfo>, String, String),
    MergeCheckResult(bool, String),
    ModelPicker(Vec<String>),
    PushCompleted(String),
    CommitCreated(CommitMessage),
    CommitPlanExecuted(String),
    ModelFetchError(String),
    OllamaStatus(OllamaHealth),
    InternetStatus(bool),
    PreflightMessages(Vec<String>),
    RemoteBranches(Vec<String>),
    StreamingChunk(String),
    StreamEnd,
}

async fn check_internet() -> bool {
    if let Ok(mut addrs) = tokio::net::lookup_host("google.com:80").await
        && addrs.next().is_some()
    {
        return true;
    }
    if let Ok(addr) = "1.1.1.1:53".parse::<std::net::SocketAddr>()
        && tokio::time::timeout(
            Duration::from_millis(1500),
            tokio::net::TcpStream::connect(&addr),
        )
        .await
        .is_ok()
    {
        return true;
    }
    false
}

pub async fn run_tui(core: AppCore) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_loop(&mut terminal, core).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

async fn run_loop(terminal: &mut Term, core: AppCore) -> Result<()> {
    let mut app = TuiApp::new(core);
    app.maybe_open_onboarding();

    const EVENT_CHANNEL_CAP: usize = 1024;
    let (tx, mut rx) = mpsc::channel(EVENT_CHANNEL_CAP);

    let mut background_tasks = Vec::new();

    let tx_tick = tx.clone();
    background_tasks.push(
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(16)).await;
                if tx_tick.send(AppEvent::Tick).await.is_err() {
                    break;
                }
            }
        })
        .abort_handle(),
    );

    let tx_input = tx.clone();
    std::thread::spawn(move || {
        loop {
            match crossterm::event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    if tx_input.blocking_send(AppEvent::Key(key)).is_err() {
                        break;
                    }
                }
                Ok(Event::Mouse(mouse)) => {
                    if tx_input.blocking_send(AppEvent::Mouse(mouse)).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    let tx_preflight = tx.clone();
    let core_preflight = app.core.clone();
    background_tasks.push(
        tokio::spawn(async move {
            let messages = core_preflight.preflight_messages().await;
            let _ = tx_preflight
                .send(AppEvent::AsyncOutcome(Box::new(
                    AsyncOutcome::PreflightMessages(messages),
                )))
                .await;
        })
        .abort_handle(),
    );

    let tx_doctor = tx.clone();
    let core_doctor = app.core.clone();
    background_tasks.push(
        tokio::spawn(async move {
            let doctor = core_doctor.dependency_doctor().await;
            let _ = tx_doctor
                .send(AppEvent::AsyncOutcome(Box::new(
                    AsyncOutcome::DependencyDoctor(doctor),
                )))
                .await;
        })
        .abort_handle(),
    );

    let tx_health = tx.clone();
    let core_health = app.core.clone();
    background_tasks.push(
        tokio::spawn(async move {
            loop {
                let health = core_health.provider_health().await;
                if tx_health
                    .send(AppEvent::AsyncOutcome(Box::new(
                        AsyncOutcome::OllamaStatus(health),
                    )))
                    .await
                    .is_err()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        })
        .abort_handle(),
    );

    let tx_internet = tx.clone();
    background_tasks.push(
        tokio::spawn(async move {
            loop {
                let online = check_internet().await;
                if tx_internet
                    .send(AppEvent::AsyncOutcome(Box::new(
                        AsyncOutcome::InternetStatus(online),
                    )))
                    .await
                    .is_err()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        })
        .abort_handle(),
    );

    let result: Result<()> = async {
        while app.running {
            tokio::select! {
                Some(event) = rx.recv() => {
                    app.process_event(event, &tx).await?;

                    while let Ok(next) = rx.try_recv() {
                        app.process_event(next, &tx).await?;
                    }
                }
                () = tokio::time::sleep(Duration::from_millis(16)), if app.busy => {
                    if app.last_spinner_tick.elapsed() >= Duration::from_millis(100) {
                        app.tick_spinner();
                        app.last_spinner_tick = Instant::now();
                    }
                }
            }

            terminal.draw(|frame| draw(frame, &app))?;
        }
        Ok(())
    }
    .await;

    for handle in background_tasks {
        handle.abort();
    }

    shutdown_managed_ollama(&app.managed_ollama_pid);

    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Autopilot,
    Scout,
}

impl ExecutionMode {
    pub fn label(self, lang: &str) -> &'static str {
        match lang.to_lowercase().trim() {
            "spanish" | "español" | "espanol" => match self {
                Self::Autopilot => "Autopiloto",
                Self::Scout => "Scout",
            },
            _ => match self {
                Self::Autopilot => "Autopilot",
                Self::Scout => "Scout",
            },
        }
    }
}

pub struct TuiApp {
    pub core: AppCore,
    pub history: Vec<ChatEntry>,
    pub history_scroll: usize,
    pub input: String,
    pub suggestions: Vec<Suggestion>,
    pub selected_suggestion: usize,
    pub modal: Option<Modal>,
    pub running: bool,
    pub busy: bool,
    pub busy_message: String,
    pub spinner_frame: u8,
    pub last_spinner_tick: Instant,
    pub ollama_health: Option<OllamaHealth>,
    pub internet_online: Option<bool>,
    pub execution_mode: ExecutionMode,
    pub scout_pending: bool,
    pub pending_modal: Option<Modal>,
    pub in_flight_abort: Option<tokio::task::AbortHandle>,
    pub last_context_usage: Option<LlmContextUsage>,
    pub streaming_response_index: Option<usize>,
    pub dependency_doctor: Option<DependencyDoctor>,
    dependency_retry: Option<DependencyRetry>,
    blocked_intent: Option<Intent>,
    pub onboarding_active: bool,
    pub onboarding_remote_deferred: bool,
    pub onboarding_language_confirmed: bool,
    managed_ollama_pid: Arc<Mutex<Option<u32>>>,
}

#[derive(Debug, Clone, Copy)]
struct DependencyRetry {
    kind: DependencyKind,
    blocking: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct DependencyModalAction {
    pub(crate) label: String,
    pub(crate) action: DependencyModalActionKind,
}

#[derive(Debug, Clone)]
pub(crate) enum DependencyModalActionKind {
    RunRecovery(DependencyAction),
    CopyCommand(String),
    RetryOnly,
    Close,
    ExitCli,
}

struct StreamingContext {
    tx_token: mpsc::UnboundedSender<String>,
    forwarder: tokio::task::JoinHandle<()>,
}

impl StreamingContext {
    fn new(tx: &mpsc::Sender<AppEvent>) -> Self {
        let (tx_token, mut rx_token) = mpsc::unbounded_channel::<String>();
        let tx_events = tx.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(token) = rx_token.recv().await {
                let _ = tx_events
                    .send(AppEvent::AsyncOutcome(Box::new(
                        AsyncOutcome::StreamingChunk(token),
                    )))
                    .await;
            }
        });
        Self {
            tx_token,
            forwarder,
        }
    }
}

impl TuiApp {
    pub(crate) fn new(core: AppCore) -> Self {
        Self {
            core,
            history: Vec::new(),
            history_scroll: 0,
            input: String::new(),
            suggestions: Vec::new(),
            selected_suggestion: 0,
            modal: None,
            running: true,
            busy: false,
            busy_message: String::new(),
            spinner_frame: 0,
            last_spinner_tick: Instant::now(),
            ollama_health: None,
            internet_online: None,
            execution_mode: ExecutionMode::Autopilot,
            scout_pending: false,
            pending_modal: None,
            in_flight_abort: None,
            last_context_usage: None,
            streaming_response_index: None,
            dependency_doctor: None,
            dependency_retry: None,
            blocked_intent: None,
            onboarding_active: false,
            onboarding_remote_deferred: false,
            onboarding_language_confirmed: false,
            managed_ollama_pid: Arc::new(Mutex::new(None)),
        }
    }

    async fn process_event(&mut self, event: AppEvent, tx: &mpsc::Sender<AppEvent>) -> Result<()> {
        match event {
            AppEvent::Key(key) => {
                self.handle_key(key, tx.clone()).await?;
            }
            AppEvent::Mouse(mouse) => {
                self.handle_mouse(mouse);
            }
            AppEvent::Tick => {}
            AppEvent::AsyncOutcome(outcome) => match *outcome {
                AsyncOutcome::StreamingChunk(chunk) => {
                    if let Some(index) = self.streaming_response_index
                        && let Some(entry) = self.history.get_mut(index)
                        && entry.role == ChatRole::Assistant
                    {
                        if entry.message
                            == translate("Working on it...", &self.core.config.language)
                        {
                            entry.message.clear();
                        }
                        entry.message.push_str(&strip_emoji(&chunk));
                    }
                }
                AsyncOutcome::StreamEnd => {
                    self.busy = false;
                    self.in_flight_abort = None;
                    self.streaming_response_index = None;
                    self.trim_history();
                    if self.execution_mode == ExecutionMode::Scout {
                        self.modal = Some(Modal::ScoutDecision { selected: 0 });
                        self.pending_modal = None;
                        self.scout_pending = false;
                    }
                }
                AsyncOutcome::DependencyCommandOutput(line) => {
                    self.append_dependency_command_log(line);
                }
                AsyncOutcome::DependencyCommandFinished {
                    report,
                    issue,
                    blocking,
                    resume_intent,
                    succeeded,
                } => {
                    self.busy = false;
                    self.in_flight_abort = None;
                    self.streaming_response_index = None;
                    self.finish_dependency_command(
                        report,
                        issue,
                        blocking,
                        resume_intent,
                        succeeded,
                        tx.clone(),
                    );
                }
                other => {
                    if outcome_completes_busy(&other) {
                        self.busy = false;
                        self.in_flight_abort = None;
                        self.streaming_response_index = None;
                    }
                    self.apply_outcome(other);
                }
            },
        }
        Ok(())
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if matches!(self.modal, Some(Modal::CommitPlanReview { .. })) {
                    let max_scroll = self.commit_plan_max_scroll();
                    if let Some(Modal::CommitPlanReview { scroll, .. }) = self.modal.as_mut() {
                        *scroll = scroll.saturating_sub(3).min(max_scroll);
                    }
                } else if let Some(Modal::PrDraft { scroll, .. }) = self.modal.as_mut() {
                    *scroll = scroll.saturating_sub(3);
                } else if let Some(Modal::CommitReview { scroll, .. }) = self.modal.as_mut() {
                    *scroll = scroll.saturating_sub(3);
                } else {
                    self.scroll_history_up(3);
                }
            }
            MouseEventKind::ScrollDown => {
                if matches!(self.modal, Some(Modal::CommitPlanReview { .. })) {
                    self.scroll_commit_plan(3);
                } else if let Some(Modal::PrDraft { scroll, .. }) = self.modal.as_mut() {
                    *scroll = scroll.saturating_add(3);
                } else if let Some(Modal::CommitReview { scroll, .. }) = self.modal.as_mut() {
                    *scroll = scroll.saturating_add(3);
                } else {
                    self.scroll_history_down(3);
                }
            }
            _ => {}
        }
    }

    fn tick_spinner(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % (SPINNER.len() as u8);
    }

    fn ollama_running(&self) -> bool {
        self.ollama_health
            .as_ref()
            .is_some_and(|health| health.running)
    }

    fn cycle_execution_mode(&mut self) {
        self.clear_scout_pending();
        let lang = self.core.config.language.clone();
        if let Some(issue) = self.ollama_issue() {
            let msg = match lang.to_lowercase().trim() {
                "spanish" | "español" | "espanol" => {
                    "Ollama está desconectado. Modos Autopilot y Scout no están disponibles."
                }
                _ => "Ollama is offline. Autopilot and Scout modes are not available.",
            };
            self.push_system(msg);
            self.open_dependency_issue(issue, false);
            return;
        }

        self.execution_mode = match self.execution_mode {
            ExecutionMode::Autopilot => ExecutionMode::Scout,
            ExecutionMode::Scout => ExecutionMode::Autopilot,
        };
        let mode_lbl = match self.execution_mode {
            ExecutionMode::Autopilot => "Autopilot",
            ExecutionMode::Scout => "Scout",
        };
        let msg = match lang.to_lowercase().trim() {
            "spanish" | "español" | "espanol" => {
                format!("Modo de ejecución cambiado a: {}", mode_lbl)
            }
            _ => format!("Execution mode changed to: {}", mode_lbl),
        };
        self.push_assistant(msg);
    }

    async fn handle_key(&mut self, key: KeyEvent, tx: mpsc::Sender<AppEvent>) -> Result<()> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.running = false;
            return Ok(());
        }
        if self.busy {
            if !self.onboarding_active
                && key.code == KeyCode::Esc
                && let Some(handle) = self.in_flight_abort.take()
            {
                handle.abort();
                self.busy = false;
                self.busy_message.clear();
                self.blocked_intent = None;
                if let Some(Modal::CommandExecution { logs, .. }) = self.modal.as_mut() {
                    logs.push(translate(
                        "Recovery action cancelled. Press Enter to return.",
                        &self.core.config.language,
                    ));
                }
                let lang = self.core.config.language.clone();
                let msg = match lang.to_lowercase().trim() {
                    "spanish" | "español" | "espanol" => "Operación cancelada.",
                    _ => "Operation cancelled.",
                };
                self.push_system(msg);
            }
            return Ok(());
        }
        if self.scout_pending && self.modal.is_none() {
            match key.code {
                KeyCode::Enter if self.input.is_empty() => {
                    self.modal = self.pending_modal.take();
                    self.scout_pending = false;
                    return Ok(());
                }
                KeyCode::Esc => {
                    self.clear_scout_pending();
                    self.input.clear();
                    return Ok(());
                }
                _ => {
                    self.clear_scout_pending();
                }
            }
        }
        if self.onboarding_active && self.modal.is_none() {
            self.open_onboarding_wizard();
        }
        if self.modal.is_some() {
            return self.handle_modal_key(key, tx).await;
        }

        match key.code {
            KeyCode::F(2) => {
                if self.should_block_for_onboarding() {
                    self.open_onboarding_wizard();
                } else {
                    self.open_settings();
                }
            }
            KeyCode::BackTab | KeyCode::F(3) => self.cycle_execution_mode(),
            KeyCode::Esc => {
                self.input.clear();
                self.suggestions.clear();
            }
            KeyCode::Char(ch) => {
                self.input.push(ch);
                self.refresh_suggestions();
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.refresh_suggestions();
            }
            KeyCode::PageUp => self.scroll_history_up(8),
            KeyCode::PageDown => self.scroll_history_down(8),
            KeyCode::Home => self.scroll_history_up(1_000),
            KeyCode::End => self.reset_history_scroll(),
            KeyCode::Up => {
                if !self.suggestions.is_empty() {
                    self.selected_suggestion = if self.selected_suggestion == 0 {
                        self.suggestions.len() - 1
                    } else {
                        self.selected_suggestion - 1
                    };
                } else {
                    self.scroll_history_up(1);
                }
            }
            KeyCode::Down => {
                if !self.suggestions.is_empty() {
                    self.selected_suggestion =
                        (self.selected_suggestion + 1) % self.suggestions.len();
                } else {
                    self.scroll_history_down(1);
                }
            }
            KeyCode::Tab => {
                if let Some(suggestion) = self.suggestions.get(self.selected_suggestion) {
                    self.input = suggestion.command.to_string();
                    self.suggestions.clear();
                    self.selected_suggestion = 0;
                }
            }
            KeyCode::Enter => self.submit_input(tx).await?,
            _ => {}
        }
        Ok(())
    }

    async fn submit_input(&mut self, tx: mpsc::Sender<AppEvent>) -> Result<()> {
        if self.busy {
            return Ok(());
        }

        let input = self.input.trim().to_string();
        if input.is_empty() {
            self.suggestions.clear();
            self.selected_suggestion = 0;
            return Ok(());
        }

        let selected = if !self.suggestions.is_empty() {
            let idx = self.selected_suggestion.min(self.suggestions.len() - 1);
            Some(self.suggestions[idx].clone())
        } else {
            None
        };

        let display = if let Some(ref s) = selected {
            s.command.to_string()
        } else {
            input.clone()
        };
        self.push_user(display);
        self.input.clear();
        self.suggestions.clear();
        self.selected_suggestion = 0;

        let intent = if let Some(suggestion) = selected {
            suggestion.intent
        } else if input.starts_with('/')
            && let Some(suggestion) = slash_suggestions(&input).first()
        {
            suggestion.intent.clone()
        } else {
            match IntentParser::parse(&input) {
                IntentDecision::Certain(intent) => intent,
                IntentDecision::Unknown => {
                    self.push_assistant("Use slash commands, for example /commit or /help.");
                    return Ok(());
                }
            }
        };

        self.execute_intent(intent, tx);
        Ok(())
    }

    fn execute_intent(&mut self, intent: Intent, tx: mpsc::Sender<AppEvent>) {
        let lang = self.core.config.language.clone();

        if matches!(intent, Intent::Resolve) {
            self.resolve_next_issue();
            return;
        }

        if !matches!(intent, Intent::Reset | Intent::Resolve)
            && let Some(issue) = self.git_issue()
        {
            self.blocked_intent = Some(intent.clone());
            self.open_dependency_issue(issue, true);
            return;
        }

        if intent_requires_ai_provider(&intent)
            && let Some(issue) = self.provider_issue()
        {
            self.blocked_intent = Some(intent.clone());
            self.open_dependency_issue(issue, false);
            return;
        }

        if self.core.config.provider == LlmProviderKind::Ollama
            && intent_requires_ollama(&intent)
            && let Some(issue) = self.ollama_issue()
        {
            self.blocked_intent = Some(intent.clone());
            self.open_dependency_issue(issue, false);
            return;
        }

        match intent {
            Intent::Help => self.push_assistant(help_text(&lang)),
            Intent::Exit => self.running = false,
            Intent::Config => {
                if self.should_block_for_onboarding() {
                    self.open_onboarding_wizard();
                } else {
                    self.open_settings();
                }
            }
            Intent::Provider => self.open_provider_picker(),
            Intent::Model => {
                self.start_model_picker_flow(tx);
            }
            Intent::Lang(Some(language)) => {
                let _ = self.core.set_language(language.clone());
                self.push_system(format!("Language changed to {}", language));
            }
            Intent::Lang(None) => self.open_language_picker(),
            Intent::Switch => self.start_switch_branch_flow(tx),
            Intent::Log => self.start_commit_log_flow(tx),
            Intent::Explain | Intent::Review | Intent::Status => {
                self.busy = true;
                self.busy_message = intent_busy_message(&intent, &lang);
                self.push_streaming_assistant(translate("Working on it...", &lang));
                let mut core = self.core.clone();
                let streaming_intent = intent.clone();
                let tx_clone = tx.clone();
                let handle = tokio::spawn(async move {
                    let stream_ctx = StreamingContext::new(&tx_clone);
                    let result = match streaming_intent {
                        Intent::Explain => core.explain_stream(stream_ctx.tx_token).await,
                        Intent::Review => core.review_stream(stream_ctx.tx_token).await,
                        Intent::Status => core.status_summary_stream(stream_ctx.tx_token).await,
                        _ => unreachable!("non-streaming intent routed to streaming handler"),
                    };
                    let outcome = match result {
                        Ok(_) => AsyncOutcome::StreamEnd,
                        Err(e) => AsyncOutcome::Error(e),
                    };
                    let _ = tx_clone
                        .send(AppEvent::AsyncOutcome(Box::new(outcome)))
                        .await;
                    let _ = stream_ctx.forwarder.await;
                });
                self.in_flight_abort = Some(handle.abort_handle());
            }
            Intent::Staged | Intent::Diff | Intent::DeprecatedDryRun => {
                self.busy = true;
                self.busy_message = intent_busy_message(&intent, &lang);
                let mut core = self.core.clone();
                let intent_clone = intent.clone();
                let handle = tokio::spawn(async move {
                    let outcome = match core.execute_simple(&intent_clone).await {
                        Ok(message) => AsyncOutcome::Message(message),
                        Err(error) => AsyncOutcome::Error(error),
                    };
                    let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
                });
                self.in_flight_abort = Some(handle.abort_handle());
            }
            Intent::Commit => {
                let branch = self.core.status().branch;
                if is_protected_branch(&branch) {
                    self.blocked_intent = Some(Intent::Commit);
                    self.busy = true;
                    self.busy_message = intent_busy_message(&Intent::Switch, &lang);
                    let core = self.core.clone();
                    self.spawn_outcome_task(tx, async move {
                        match core.switch_branches_async().await {
                            Ok(branches) => AsyncOutcome::SwitchBranchesReady(branches),
                            Err(_) => AsyncOutcome::SwitchBranchesReady(SwitchBranches::default()),
                        }
                    });
                } else {
                    self.start_commit_plan_flow(tx);
                }
            }
            Intent::Push => {
                if self.internet_online == Some(false) {
                    let msg = match lang.to_lowercase().trim() {
                        "spanish" | "español" | "espanol" => {
                            "No tienes conexión a internet. Esta acción requiere conexión a internet."
                        }
                        _ => "You are offline. This action requires internet connectivity.",
                    };
                    self.push_error(AppError::Custom(msg.to_string()));
                    return;
                }
                self.modal = Some(Modal::Confirm {
                    title: translate("Push branch", &lang),
                    message: translate("Push the current branch?", &lang),
                    selected: 1,
                    kind: ConfirmKind::PushFirst,
                });
            }
            Intent::Pr => {
                if self.internet_online == Some(false) {
                    let msg = match lang.to_lowercase().trim() {
                        "spanish" | "español" | "espanol" => {
                            "No tienes conexión a internet. Esta acción requiere conexión a internet."
                        }
                        _ => "You are offline. This action requires internet connectivity.",
                    };
                    self.push_error(AppError::Custom(msg.to_string()));
                    return;
                }
                if let Some(issue) = self.github_issue() {
                    self.blocked_intent = Some(intent.clone());
                    self.open_dependency_issue(issue, false);
                    return;
                }
                self.busy = true;
                self.busy_message = intent_busy_message(&intent, &lang);
                let core = self.core.clone();
                let handle = tokio::spawn(async move {
                    let outcome = match core.remote_branches_async().await {
                        Ok(branches) => AsyncOutcome::RemoteBranches(branches),
                        Err(error) => AsyncOutcome::Error(error),
                    };
                    let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
                });
                self.in_flight_abort = Some(handle.abort_handle());
            }
            Intent::Setup => {
                if self.should_block_for_onboarding() {
                    self.open_onboarding_wizard();
                } else {
                    self.modal = Some(Modal::Setup { selected: 0 });
                }
            }
            Intent::Reset => {
                self.modal = Some(Modal::Confirm {
                    title: translate("Reset configuration", &lang),
                    message: reset_confirmation_message(&lang),
                    selected: 1,
                    kind: ConfirmKind::ResetConfiguration,
                });
            }
            Intent::Resolve => {}
            Intent::Theme => {
                let items = crate::domain::settings::ThemeChoice::all()
                    .iter()
                    .map(|choice| PickerItem {
                        label: choice.label().to_string(),
                        value: PickerValue::Theme(choice.label().to_string()),
                    })
                    .collect::<Vec<_>>();
                self.modal = Some(Modal::Picker {
                    title: translate("Select UI theme", &lang),
                    items,
                    selected: 0,
                });
            }
        }
    }

    fn start_commit_plan_flow(&mut self, tx: mpsc::Sender<AppEvent>) {
        let lang = self.core.config.language.clone();
        self.busy = true;
        self.busy_message = intent_busy_message(&Intent::Commit, &lang);
        let core = self.core.clone();
        self.spawn_outcome_task(tx, draft_commit_plan_outcome(core));
    }

    fn start_switch_branch_flow(&mut self, tx: mpsc::Sender<AppEvent>) {
        let lang = self.core.config.language.clone();
        self.busy = true;
        self.busy_message = intent_busy_message(&Intent::Switch, &lang);
        let core = self.core.clone();
        self.spawn_outcome_task(tx, switch_branches_outcome(core));
    }

    fn start_commit_log_flow(&mut self, tx: mpsc::Sender<AppEvent>) {
        let lang = self.core.config.language.clone();
        self.busy = true;
        self.busy_message = intent_busy_message(&Intent::Log, &lang);
        let core = self.core.clone();
        self.spawn_outcome_task(tx, commit_log_outcome(core));
    }

    pub fn apply_outcome(&mut self, outcome: AsyncOutcome) {
        let lang = self.core.config.language.clone();
        match outcome {
            AsyncOutcome::Message(msg) => self.push_assistant(msg),
            AsyncOutcome::Error(err) => {
                if let Some((issue, blocking)) = self.dependency_issue_from_error(&err) {
                    self.open_dependency_issue(issue, blocking);
                } else {
                    self.push_error(err);
                }
            }
            AsyncOutcome::DependencyDoctor(report) => {
                let retry_focus = self.dependency_retry.take();
                if self.core.config.provider == LlmProviderKind::Ollama {
                    self.ollama_health = Some(ollama_health_from_report(&report));
                }
                self.dependency_doctor = Some(report.clone());

                if let Some(retry) = retry_focus {
                    let issue = report.status(retry.kind);
                    if !issue.is_ready() {
                        if self.onboarding_active || self.current_onboarding_step().is_some() {
                            self.maybe_open_onboarding();
                        } else {
                            self.open_dependency_issue(
                                issue.clone(),
                                retry.blocking || issue.is_blocking(),
                            );
                        }
                    } else if report.git.is_blocking() {
                        self.maybe_open_onboarding();
                    }
                } else {
                    self.maybe_open_onboarding();
                }
            }
            AsyncOutcome::SwitchBranchesReady(branches) => {
                if self.blocked_intent == Some(Intent::Commit) {
                    let selected = if branches.total_count() == 0 { 1 } else { 0 };
                    self.modal = Some(Modal::ProtectedBranchCommit {
                        branch: self.core.status().branch,
                        branches,
                        selected,
                        new_branch: String::new(),
                        editing_new_branch: selected == 1,
                    });
                } else {
                    self.modal = Some(Modal::BranchSwitch {
                        branches,
                        selected: 0,
                    });
                }
            }
            AsyncOutcome::ModelPicker(models) => {
                self.busy = false;
                self.busy_message.clear();
                self.in_flight_abort = None;
                if models.is_empty() {
                    self.open_model_input_with_reason("The provider returned an empty model list. Enter the exact model name manually or change provider.");
                } else {
                    let provider_label = self.core.provider_label();
                    let title = match self.core.config.language.to_lowercase().trim() {
                        "spanish" | "español" | "espanol" => {
                            format!("Seleccioná modelo de {}", provider_label)
                        }
                        _ => format!("Select {} model", provider_label),
                    };
                    self.modal = Some(Modal::Picker {
                        title,
                        items: models
                            .into_iter()
                            .map(|model| PickerItem {
                                label: model.clone(),
                                value: PickerValue::Model(model),
                            })
                            .collect(),
                        selected: 0,
                    });
                }
            }
            AsyncOutcome::ModelFetchError(reason) => {
                self.busy = false;
                self.busy_message.clear();
                self.in_flight_abort = None;
                let lower_reason = reason.to_lowercase();
                if lower_reason.contains("api key")
                    || lower_reason.contains("unauthorized")
                    || lower_reason.contains("authentication")
                    || lower_reason.contains("401")
                    || lower_reason.contains("403")
                {
                    self.push_error(AppError::Custom(reason));
                    self.open_api_key_input();
                } else {
                    self.open_model_input_with_reason(&reason);
                }
            }
            AsyncOutcome::CommitPlanReady(plan, usage) => {
                self.last_context_usage = Some(usage);
                self.pending_modal = None;
                self.scout_pending = false;
                self.modal = Some(Modal::CommitPlanReview {
                    plan,
                    selected: 0,
                    scroll: 0,
                });
            }
            AsyncOutcome::CommitLogReady(entries) => {
                if entries.is_empty() {
                    self.push_system(translate("No commits found.", &lang));
                } else {
                    self.modal = Some(Modal::CommitLog {
                        entries,
                        selected: 0,
                        action: 0,
                        scroll: 0,
                    });
                }
            }
            AsyncOutcome::CommitLogReset(output) => {
                let mut message = translate(
                    "Soft reset completed. Changes were kept for recommit.",
                    &lang,
                );
                if !output.trim().is_empty() {
                    message.push_str("\n\n");
                    message.push_str(output.trim());
                }
                self.push_system(message);
            }
            AsyncOutcome::PrDraft(base, draft) => {
                self.pending_modal = None;
                self.scout_pending = false;
                self.modal = Some(Modal::PrDraft {
                    base,
                    draft,
                    selected: 0,
                    scroll: 0,
                });
            }
            AsyncOutcome::ExistingPrs(prs, base, _head) => {
                self.busy = false;
                self.modal = Some(Modal::ExistingPrs {
                    prs,
                    base,
                    selected: 0,
                });
            }
            AsyncOutcome::MergeCheckResult(false, base) => {
                self.busy = false;
                self.modal = Some(Modal::ConflictResolution { base, selected: 0 });
            }
            AsyncOutcome::MergeCheckResult(true, _) => {}
            AsyncOutcome::PushCompleted(output) => {
                self.push_system(push_completed_message(
                    self.core.status().branch.as_str(),
                    output.as_str(),
                    &lang,
                ));
            }
            AsyncOutcome::CommitCreated(message) => {
                self.push_assistant(format!("Commit created:\n{}", message.title()));
                if self.core.config.prompt_push_after_commit {
                    self.modal = Some(Modal::Confirm {
                        title: translate("Push branch", &lang),
                        message: translate("Push the current branch?", &lang),
                        selected: 1,
                        kind: ConfirmKind::PushFirst,
                    });
                }
            }
            AsyncOutcome::CommitPlanExecuted(output) => {
                let message = if output.trim().is_empty() {
                    translate("Commit plan executed.", &lang)
                } else {
                    output
                };
                self.push_assistant(message);
                if self.core.config.prompt_push_after_commit {
                    self.modal = Some(Modal::Confirm {
                        title: translate("Push branch", &lang),
                        message: translate("Push the current branch?", &lang),
                        selected: 1,
                        kind: ConfirmKind::PushFirst,
                    });
                }
            }
            AsyncOutcome::OllamaStatus(health) => {
                self.ollama_health = Some(health);
            }
            AsyncOutcome::InternetStatus(online) => {
                let was_none = self.internet_online.is_none();
                self.internet_online = Some(online);
                if was_none && !online {
                    let lang = self.core.config.language.to_lowercase();
                    let warning_msg = match lang.trim() {
                        "spanish" | "español" | "espanol" => {
                            "Conectividad a Internet: DESCONECTADO\n\
                             - Inhabilitado: /push, /pr, integración remota de GitHub en /setup.\n\
                             - Habilitado: /commit (local), /diff, /explain, /review, /staged, /theme y modo Scout."
                        }
                        _ => {
                            "Internet Connectivity: OFFLINE\n\
                             - Disabled: /push, /pr, remote GitHub integration in /setup.\n\
                             - Enabled: /commit (local), /diff, /explain, /review, /staged, /theme, and Scout mode."
                        }
                    };
                    self.push_system(warning_msg.to_string());
                }
            }
            AsyncOutcome::PreflightMessages(messages) => {
                for message in messages {
                    self.push_system(message);
                }
            }
            AsyncOutcome::StreamingChunk(_)
            | AsyncOutcome::StreamEnd
            | AsyncOutcome::DependencyCommandOutput(_)
            | AsyncOutcome::DependencyCommandFinished { .. } => {}
            AsyncOutcome::RemoteBranches(branches) => {
                let lang = self.core.config.language.clone();
                self.modal = Some(Modal::Picker {
                    title: translate("Select PR base branch", &lang),
                    items: branches
                        .into_iter()
                        .map(|branch| PickerItem {
                            label: branch.clone(),
                            value: PickerValue::PrBase(branch),
                        })
                        .collect(),
                    selected: 0,
                });
            }
        }
    }

    async fn handle_modal_key(&mut self, key: KeyEvent, tx: mpsc::Sender<AppEvent>) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                if let Some(Modal::OnboardingWizard { step, .. }) = self.modal.as_ref().cloned() {
                    self.go_back_in_onboarding(step);
                    return Ok(());
                } else if let Some(Modal::ProtectedBranchCommit {
                    editing_new_branch, ..
                }) = self.modal.as_mut()
                    && *editing_new_branch
                {
                    *editing_new_branch = false;
                    return Ok(());
                }
                if matches!(self.modal, Some(Modal::BranchSwitch { .. }))
                    && matches!(self.blocked_intent, Some(Intent::Commit))
                {
                    if let Some(Modal::BranchSwitch { branches, .. }) = self.modal.take() {
                        self.modal = Some(Modal::ProtectedBranchCommit {
                            branch: self.core.status().branch,
                            branches,
                            selected: 0,
                            new_branch: String::new(),
                            editing_new_branch: false,
                        });
                        return Ok(());
                    }
                } else if matches!(self.modal, Some(Modal::ScoutDecision { .. })) {
                    self.clear_scout_pending();
                    self.modal = None;
                    self.push_assistant(translate(
                        "Scout session closed.",
                        &self.core.config.language,
                    ));
                } else if self.onboarding_active {
                    self.resume_onboarding_if_needed();
                    return Ok(());
                } else if matches!(
                    self.modal,
                    Some(Modal::DependencyIssue { blocking: true, .. })
                ) {
                    self.running = false;
                } else if let Some(Modal::CommandExecution {
                    issue, blocking, ..
                }) = self.modal.take()
                {
                    self.open_dependency_issue(issue, blocking);
                } else if matches!(self.modal, Some(Modal::ProtectedBranchCommit { .. })) {
                    self.blocked_intent = None;
                    self.modal = None;
                } else {
                    self.modal = None;
                }
                return Ok(());
            }
            KeyCode::Up => {
                if let Some(Modal::PrDraft { scroll, .. }) = self.modal.as_mut() {
                    *scroll = scroll.saturating_sub(1);
                } else if let Some(Modal::CommitPlanReview { selected, .. }) = self.modal.as_mut() {
                    let max = 2;
                    *selected = if *selected == 0 { max } else { *selected - 1 };
                } else if let Some(Modal::CommitLog {
                    entries,
                    selected,
                    scroll,
                    ..
                }) = self.modal.as_mut()
                {
                    if !entries.is_empty() {
                        *selected = if *selected == 0 {
                            entries.len().saturating_sub(1)
                        } else {
                            selected.saturating_sub(1)
                        };
                        *scroll = (*selected).saturating_sub(8);
                    }
                } else if let Some(Modal::ProtectedBranchCommit {
                    selected,
                    editing_new_branch,
                    ..
                }) = self.modal.as_mut()
                {
                    *editing_new_branch = false;
                    *selected = if *selected == 0 { 2 } else { *selected - 1 };
                } else {
                    self.move_modal_selection(-1);
                }
            }
            KeyCode::Down => {
                if let Some(Modal::PrDraft { scroll, .. }) = self.modal.as_mut() {
                    *scroll = scroll.saturating_add(1);
                } else if let Some(Modal::CommitPlanReview { selected, .. }) = self.modal.as_mut() {
                    *selected = (*selected + 1) % 3;
                } else if let Some(Modal::CommitLog {
                    entries,
                    selected,
                    scroll,
                    ..
                }) = self.modal.as_mut()
                {
                    if !entries.is_empty() {
                        *selected = (*selected + 1) % entries.len();
                        *scroll = (*selected).saturating_sub(8);
                    }
                } else if let Some(Modal::ProtectedBranchCommit {
                    selected,
                    editing_new_branch,
                    ..
                }) = self.modal.as_mut()
                {
                    *editing_new_branch = false;
                    *selected = (*selected + 1) % 3;
                } else {
                    self.move_modal_selection(1);
                }
            }
            KeyCode::Left => {
                let prev_selected = |selected: &mut usize, max: usize| {
                    let max = max.saturating_sub(1);
                    *selected = if *selected == 0 { max } else { *selected - 1 };
                };
                if let Some(Modal::PrDraft { selected, .. }) = self.modal.as_mut() {
                    *selected = if *selected == 0 { 1 } else { 0 };
                } else if let Some(Modal::ExistingPrs { selected, .. }) = self.modal.as_mut() {
                    prev_selected(selected, 3);
                } else if let Some(Modal::ConflictResolution { selected, .. }) = self.modal.as_mut() {
                    prev_selected(selected, 3);
                } else if matches!(self.modal, Some(Modal::OnboardingWizard { .. })) {
                    // Onboarding actions are list selections; horizontal arrows do not mutate state.
                } else if let Some(Modal::DependencyIssue {
                    selected, actions, ..
                }) = self.modal.as_mut()
                {
                    let max = actions.len().saturating_sub(1);
                    *selected = if *selected == 0 { max } else { *selected - 1 };
                } else if let Some(Modal::BranchSwitch { .. }) = self.modal.as_mut() {
                    // No action.
                } else if let Some(Modal::CommitPlanReview { selected, .. }) = self.modal.as_mut() {
                    let max = 2;
                    *selected = if *selected == 0 { max } else { *selected - 1 };
                } else if let Some(Modal::CommitLog { action, .. }) = self.modal.as_mut() {
                    *action = if *action == 0 { 1 } else { 0 };
                } else if let Some(Modal::CommitReview { selected, .. }) = self.modal.as_mut() {
                    let max = 3;
                    *selected = if *selected == 0 { max } else { *selected - 1 };
                } else if let Some(Modal::ScoutDecision { .. }) = self.modal.as_ref() {
                    // No action
                } else {
                    self.adjust_settings(-1)?;
                }
            }
            KeyCode::Right => {
                let next_selected = |selected: &mut usize, max: usize| {
                    let max = max.saturating_sub(1);
                    *selected = if *selected == max { 0 } else { *selected + 1 };
                };
                if let Some(Modal::PrDraft { selected, .. }) = self.modal.as_mut() {
                    *selected = if *selected == 0 { 1 } else { 0 };
                } else if let Some(Modal::ExistingPrs { selected, .. }) = self.modal.as_mut() {
                    next_selected(selected, 3);
                } else if let Some(Modal::ConflictResolution { selected, .. }) = self.modal.as_mut() {
                    next_selected(selected, 3);
                } else if matches!(self.modal, Some(Modal::OnboardingWizard { .. })) {
                    // Onboarding actions are list selections; horizontal arrows do not mutate state.
                } else if let Some(Modal::DependencyIssue {
                    selected, actions, ..
                }) = self.modal.as_mut()
                {
                    let max = actions.len().saturating_sub(1);
                    *selected = if *selected == max { 0 } else { *selected + 1 };
                } else if let Some(Modal::BranchSwitch { .. }) = self.modal.as_mut() {
                    // No action.
                } else if let Some(Modal::CommitPlanReview { selected, .. }) = self.modal.as_mut() {
                    let max = 2;
                    *selected = if *selected == max { 0 } else { *selected + 1 };
                } else if let Some(Modal::CommitLog { action, .. }) = self.modal.as_mut() {
                    *action = (*action + 1) % 2;
                } else if let Some(Modal::CommitReview { selected, .. }) = self.modal.as_mut() {
                    let max = 3;
                    *selected = if *selected == max { 0 } else { *selected + 1 };
                } else if let Some(Modal::ScoutDecision { .. }) = self.modal.as_ref() {
                    // No action
                } else {
                    self.adjust_settings(1)?;
                }
            }
            KeyCode::Char(ch) => {
                if let Some(Modal::TextInput { value, .. }) = self.modal.as_mut() {
                    value.push(ch);
                } else if let Some(Modal::ProtectedBranchCommit {
                    selected,
                    new_branch,
                    editing_new_branch,
                    ..
                }) = self.modal.as_mut()
                {
                    *selected = 1;
                    *editing_new_branch = true;
                    new_branch.push(ch);
                }
            }
            KeyCode::Backspace => {
                if let Some(Modal::TextInput { value, .. }) = self.modal.as_mut() {
                    value.pop();
                } else if let Some(Modal::ProtectedBranchCommit {
                    new_branch,
                    editing_new_branch,
                    ..
                }) = self.modal.as_mut()
                    && *editing_new_branch
                {
                    new_branch.pop();
                }
            }
            KeyCode::PageUp => {
                if matches!(self.modal, Some(Modal::CommitPlanReview { .. })) {
                    let max_scroll = self.commit_plan_max_scroll();
                    if let Some(Modal::CommitPlanReview { scroll, .. }) = self.modal.as_mut() {
                        *scroll = scroll.saturating_sub(5).min(max_scroll);
                    }
                } else if let Some(Modal::CommitReview { scroll, .. }) = self.modal.as_mut() {
                    *scroll = scroll.saturating_sub(5);
                } else if let Some(Modal::PrDraft { scroll, .. }) = self.modal.as_mut() {
                    *scroll = scroll.saturating_sub(5);
                } else if let Some(Modal::CommitLog {
                    selected, scroll, ..
                }) = self.modal.as_mut()
                {
                    *selected = selected.saturating_sub(5);
                    *scroll = (*selected).saturating_sub(8);
                }
            }
            KeyCode::PageDown => {
                if matches!(self.modal, Some(Modal::CommitPlanReview { .. })) {
                    self.scroll_commit_plan(5);
                } else if let Some(Modal::CommitReview { scroll, .. }) = self.modal.as_mut() {
                    *scroll = scroll.saturating_add(5);
                } else if let Some(Modal::PrDraft { scroll, .. }) = self.modal.as_mut() {
                    *scroll = scroll.saturating_add(5);
                } else if let Some(Modal::CommitLog {
                    entries,
                    selected,
                    scroll,
                    ..
                }) = self.modal.as_mut()
                    && !entries.is_empty()
                {
                    *selected = (*selected + 5).min(entries.len().saturating_sub(1));
                    *scroll = (*selected).saturating_sub(8);
                }
            }
            KeyCode::Tab => {
                if let Some(Modal::OnboardingWizard { .. }) = self.modal.as_ref() {
                    self.move_modal_selection(1);
                } else if let Some(Modal::CommitPlanReview { selected, .. }) = self.modal.as_mut() {
                    *selected = (*selected + 1) % 3;
                } else if let Some(Modal::CommitLog { action, .. }) = self.modal.as_mut() {
                    *action = (*action + 1) % 2;
                } else if let Some(Modal::ProtectedBranchCommit {
                    selected,
                    editing_new_branch,
                    ..
                }) = self.modal.as_mut()
                {
                    if *selected == 1 {
                        *editing_new_branch = !*editing_new_branch;
                    } else {
                        *selected = 1;
                        *editing_new_branch = true;
                    }
                } else if let Some(Modal::DependencyIssue {
                    selected, actions, ..
                }) = self.modal.as_mut()
                    && !actions.is_empty()
                {
                    *selected = (*selected + 1) % actions.len();
                }
            }
            KeyCode::Enter => self.activate_modal(tx).await?,
            _ => {}
        }
        Ok(())
    }

    async fn activate_modal(&mut self, tx: mpsc::Sender<AppEvent>) -> Result<()> {
        let Some(modal) = self.modal.take() else {
            return Ok(());
        };

        match modal {
            Modal::OnboardingWizard { step, selected } => {
                self.activate_onboarding_step(step, selected, tx).await?;
            }
            Modal::Settings { selected } => {
                self.activate_setting(selected, tx).await?;
                if self.modal.is_none() && !self.busy {
                    self.modal = Some(Modal::Settings { selected });
                }
            }
            Modal::Picker {
                items, selected, ..
            } => {
                if let Some(item) = items.get(selected).cloned() {
                    self.activate_picker(item.value, tx).await?;
                }
            }
            Modal::Confirm { selected, kind, .. } => {
                if selected == 0 {
                    self.activate_confirm(kind, tx).await?;
                } else {
                    self.push_assistant("Cancelled.");
                }
            }
            Modal::TextInput { value, kind, .. } => {
                self.activate_text_input(value, kind, tx).await?
            }
            Modal::CommitReview {
                message,
                files,
                selected,
                ..
            } => {
                self.activate_commit_review(message, files, selected, tx)
                    .await?
            }
            Modal::CommitPlanReview { plan, selected, .. } => {
                self.activate_commit_plan(plan, selected, tx).await?
            }
            Modal::CommitLog {
                entries,
                selected,
                action,
                ..
            } => {
                self.activate_commit_log(entries, selected, action, tx)
                    .await?
            }
            Modal::BranchSwitch { branches, selected } => {
                self.activate_branch_switch(branches, selected, tx).await?
            }
            Modal::ProtectedBranchCommit {
                branches,
                selected,
                new_branch,
                editing_new_branch,
                ..
            } => {
                self.activate_protected_branch_commit(
                    branches,
                    selected,
                    new_branch,
                    editing_new_branch,
                    tx,
                )
                .await?
            }
            Modal::PrDraft {
                base,
                draft,
                selected,
                ..
            } => {
                if selected == 0 {
                    let lang = self.core.config.language.clone();
                    self.busy = true;
                    self.busy_message = translate("Creating pull request...", &lang);
                    let core = self.core.clone();
                    tokio::spawn(async move {
                        let outcome = match tokio::task::spawn_blocking(move || {
                            core.create_pr(&base, &draft)
                        })
                        .await
                        {
                            Ok(Ok(output)) => AsyncOutcome::Message(if output.is_empty() {
                                "Pull Request created.".to_string()
                            } else {
                                output
                            }),
                            Ok(Err(error)) => AsyncOutcome::Error(error),
                            Err(error) => AsyncOutcome::Error(AppError::Custom(format!(
                                "Background task failed: {}",
                                error
                            ))),
                        };
                        let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
                    });
                } else {
                    self.modal = None;
                }
            }
            Modal::ExistingPrs {
                prs,
                selected,
                base,
                ..
            } => {
                if prs.is_empty() {
                    self.modal = None;
                    return Ok(());
                }
                let pr = &prs[selected];
                let number = pr.number;
                let is_recreate = selected == 1;
                let is_cancel = selected == 2;

                if is_cancel {
                    self.modal = None;
                    self.push_assistant(translate("Cancelled.", &self.core.config.language));
                    return Ok(());
                }

                self.busy = true;
                self.busy_message = translate("Drafting PR updates...", &self.core.config.language);
                let mut core = self.core.clone();
                let base = base.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    match core.draft_pr(&base).await {
                        Ok(draft) => {
                            let result = if is_recreate {
                                core.close_pr(number).and_then(|_| core.create_pr(&base, &draft))
                            } else {
                                core.edit_pr(number, &draft)
                            };
                            let outcome = match result {
                                Ok(output) => AsyncOutcome::Message(if output.is_empty() {
                                    "PR updated successfully.".to_string()
                                } else {
                                    output
                                }),
                                Err(error) => AsyncOutcome::Error(error),
                            };
                            let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
                        }
                        Err(error) => {
                            let _ = tx.send(AppEvent::AsyncOutcome(Box::new(AsyncOutcome::Error(error)))).await;
                        }
                    }
                });
            }
            Modal::ConflictResolution { selected, base } => {
                let lang = self.core.config.language.clone();
                match selected {
                    0 => {
                        self.busy = true;
                        self.busy_message = translate("Resolving conflicts with AI...", &lang);
                        let mut core = self.core.clone();
                        let base = base.clone();
                        tokio::spawn(async move {
                            let outcome = match core.draft_pr(&base).await {
                                Ok(draft) => AsyncOutcome::PrDraft(base, draft),
                                Err(error) => AsyncOutcome::Error(error),
                            };
                            let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
                        });
                    }
                    1 => {
                        self.modal = None;
                        self.push_assistant(translate("Please resolve conflicts manually, then run /pr again.", &lang));
                    }
                    _ => { self.modal = None; }
                }
            }
            Modal::Setup { selected } => self.activate_setup(selected).await?,
            Modal::DependencyIssue {
                issue,
                actions,
                selected,
                blocking,
                ..
            } => {
                let Some(action) = actions.get(selected).cloned() else {
                    return Ok(());
                };
                match action.action {
                    DependencyModalActionKind::RunRecovery(recovery) => {
                        self.start_dependency_recovery(issue, recovery, blocking, tx);
                    }
                    DependencyModalActionKind::CopyCommand(command) => {
                        let notice = match self.copy_dependency_command(&command) {
                            Ok(message) => Some(message),
                            Err(message) => Some(message),
                        };
                        self.open_dependency_issue_with_notice(issue, blocking, notice);
                    }
                    DependencyModalActionKind::RetryOnly => {
                        self.retry_dependency_check(issue.kind, blocking, tx);
                    }
                    DependencyModalActionKind::Close => {
                        self.blocked_intent = None;
                        self.push_assistant(translate("Cancelled.", &self.core.config.language));
                    }
                    DependencyModalActionKind::ExitCli => {
                        self.blocked_intent = None;
                        self.running = false;
                    }
                }
            }
            Modal::CommandExecution {
                issue, blocking, ..
            } => {
                self.open_dependency_issue(issue, blocking);
            }
            Modal::ScoutDecision { .. } => {
                self.modal = Some(modal);
                self.activate_scout_decision(tx).await?;
            }
        }
        Ok(())
    }

    async fn activate_picker(
        &mut self,
        value: PickerValue,
        tx: mpsc::Sender<AppEvent>,
    ) -> Result<()> {
        match value {
            PickerValue::Provider(provider_name) => {
                let provider = LlmProviderKind::from_label(&provider_name).ok_or_else(|| {
                    AppError::InvalidLlmResponse("Invalid provider selection".to_string())
                })?;
                self.core.set_provider(provider)?;
                self.push_assistant(format!("Provider set to {}.", provider.label()));
                self.resume_onboarding_if_needed();
            }
            PickerValue::Model(model) => {
                self.core.set_model(model.clone())?;
                self.push_assistant(format!("Model set to {}.", model));
                self.resume_onboarding_if_needed();
            }
            PickerValue::Language(language) => {
                self.core.set_language(language.clone())?;
                self.push_system(format!("Language changed to {}", language));
                if self.onboarding_active {
                    self.onboarding_language_confirmed = true;
                    self.resume_onboarding_if_needed();
                }
            }
            PickerValue::PrBase(base) => {
                self.busy = true;
                let lang = self.core.config.language.clone();
                self.busy_message = translate("Checking for existing PRs...", &lang);
                let mut core = self.core.clone();
                let base = base.clone();
                tokio::spawn(async move {
                    let head = core.current_branch().unwrap_or_default();
                    match core.list_open_prs(&head, &base) {
                        Ok(prs) if !prs.is_empty() => {
                            let outcome = AsyncOutcome::ExistingPrs(prs, base, head);
                            let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
                        }
                        Ok(_) => {
                            let can_merge = core.can_merge(&base);
                            if can_merge {
                                let outcome = match core.draft_pr(&base).await {
                                    Ok(draft) => AsyncOutcome::PrDraft(base, draft),
                                    Err(error) => AsyncOutcome::Error(error),
                                };
                                let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
                            } else {
                                let outcome = AsyncOutcome::MergeCheckResult(false, base);
                                let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
                            }
                        }
                        Err(error) => {
                            let _ = tx.send(AppEvent::AsyncOutcome(Box::new(AsyncOutcome::Error(error)))).await;
                        }
                    }
                });
            }
            PickerValue::Visibility { repo, private } => {
                let lang = self.core.config.language.clone();
                self.busy = true;
                self.busy_message = translate("Creating GitHub repository...", &lang);
                let core = self.core.clone();
                tokio::spawn(async move {
                    let repo_name = repo.clone();
                    let outcome = match tokio::task::spawn_blocking(move || {
                        core.create_github_repo(&repo_name, private)
                    })
                    .await
                    {
                        Ok(Ok(_)) => {
                            AsyncOutcome::Message(format!("GitHub repository `{}` created.", repo))
                        }
                        Ok(Err(error)) => AsyncOutcome::Error(error),
                        Err(error) => AsyncOutcome::Error(AppError::Custom(format!(
                            "Background task failed: {}",
                            error
                        ))),
                    };
                    let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
                });
            }
            PickerValue::Theme(theme_name) => {
                if let Some(theme) = crate::domain::settings::ThemeChoice::from_str(&theme_name) {
                    self.core.config.theme = theme;
                    let _ = self.core.save_config();
                    self.push_assistant(format!("Theme changed to {}.", theme.label()));
                } else {
                    self.push_error(AppError::InvalidLlmResponse(
                        "Invalid theme selection".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    async fn activate_confirm(
        &mut self,
        kind: ConfirmKind,
        tx: mpsc::Sender<AppEvent>,
    ) -> Result<()> {
        let lang = self.core.config.language.clone();
        match kind {
            ConfirmKind::PushFirst => {
                self.busy = true;
                self.busy_message = translate("Pushing branch...", &lang);
                let core = self.core.clone();
                tokio::spawn(async move {
                    let outcome = match core.push() {
                        Ok(output) => AsyncOutcome::PushCompleted(output),
                        Err(error) => AsyncOutcome::Error(error),
                    };
                    let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
                });
            }
            ConfirmKind::ResetConfiguration => match self.core.reset_safe() {
                Ok(removed_origin) => {
                    self.last_context_usage = None;
                    self.ollama_health = None;
                    self.dependency_doctor = None;
                    self.internet_online = None;
                    self.pending_modal = None;
                    self.blocked_intent = None;
                    self.scout_pending = false;
                    self.onboarding_active = false;
                    self.onboarding_remote_deferred = false;
                    self.onboarding_language_confirmed = false;
                    self.streaming_response_index = None;
                    self.busy = false;
                    self.busy_message.clear();
                    self.in_flight_abort = None;
                    let msg = if removed_origin {
                        "Configuration reset. API keys were removed and origin was disconnected. Local Git history and files were not deleted."
                    } else {
                        "Configuration reset. API keys were removed. Local Git history and files were not deleted."
                    };
                    self.push_system(translate(msg, &self.core.config.language));
                    self.maybe_open_onboarding();
                }
                Err(error) => self.push_error(error),
            },
        }
        Ok(())
    }

    async fn activate_commit_plan(
        &mut self,
        plan: CommitPlan,
        selected: usize,
        tx: mpsc::Sender<AppEvent>,
    ) -> Result<()> {
        let lang = self.core.config.language.clone();

        if selected == 0 {
            self.busy = true;
            self.busy_message = translate("Executing commit plan...", &lang);
            let core = self.core.clone();
            tokio::spawn(async move {
                let outcome = match core.execute_commit_plan(&plan) {
                    Ok(output) => AsyncOutcome::CommitPlanExecuted(output),
                    Err(error) => AsyncOutcome::Error(error),
                };
                let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
            });
        } else if selected == 1 {
            self.busy = true;
            self.busy_message = translate("Generating a structured commit plan...", &lang);
            let core = self.core.clone();
            self.spawn_outcome_task(tx, draft_commit_plan_outcome(core));
        } else {
            self.push_assistant(translate("Cancelled.", &lang));
        }

        Ok(())
    }

    async fn activate_commit_log(
        &mut self,
        entries: Vec<CommitLogEntry>,
        selected: usize,
        action: usize,
        tx: mpsc::Sender<AppEvent>,
    ) -> Result<()> {
        let lang = self.core.config.language.clone();
        if action != 0 {
            self.push_assistant(translate("Cancelled.", &lang));
            return Ok(());
        }

        let Some(entry) = entries.get(selected).cloned() else {
            self.push_error(AppError::Custom(
                "Selected commit is unavailable.".to_string(),
            ));
            return Ok(());
        };

        self.busy = true;
        self.busy_message = translate("Rewriting history safely...", &lang);
        self.push_system(format!(
            "Soft reset to `{}` selected. No files will be deleted; changes stay staged for recommit.",
            entry.short_hash
        ));
        let core = self.core.clone();
        self.spawn_outcome_task(tx, commit_log_reset_outcome(core, entry.hash));
        Ok(())
    }

    async fn activate_branch_switch(
        &mut self,
        branches: SwitchBranches,
        selected: usize,
        tx: mpsc::Sender<AppEvent>,
    ) -> Result<()> {
        let Some(branch) = branches.get(selected).cloned() else {
            return Ok(());
        };

        if branch.is_current {
            self.push_assistant(format!("Already on branch `{}`.", branch.name));
            return Ok(());
        }

        let lang = self.core.config.language.clone();
        self.busy = true;
        self.busy_message = translate("Switching branch...", &lang);
        let mut core = self.core.clone();
        let continue_commit = self.blocked_intent.take() == Some(Intent::Commit);
        self.spawn_outcome_task(tx, async move {
            let branch_name = branch.name.clone();
            let core_switch = core.clone();
            match tokio::task::spawn_blocking(move || core_switch.switch_branch(&branch)).await {
                Ok(Ok(output)) => {
                    if continue_commit {
                        match core.draft_commit_plan().await {
                            Ok((plan, usage)) => AsyncOutcome::CommitPlanReady(plan, usage),
                            Err(error) => AsyncOutcome::Error(error),
                        }
                    } else {
                        AsyncOutcome::Message(if output.trim().is_empty() {
                            format!("Switched to branch `{}`.", branch_name)
                        } else {
                            output
                        })
                    }
                }
                Ok(Err(error)) => AsyncOutcome::Error(error),
                Err(error) => AsyncOutcome::Error(AppError::Custom(format!(
                    "Background task failed: {}",
                    error
                ))),
            }
        });
        Ok(())
    }

    async fn activate_protected_branch_commit(
        &mut self,
        branches: SwitchBranches,
        selected: usize,
        new_branch: String,
        editing_new_branch: bool,
        tx: mpsc::Sender<AppEvent>,
    ) -> Result<()> {
        let lang = self.core.config.language.clone();
        match selected {
            0 => {
                if branches.total_count() == 0 {
                    self.modal = Some(Modal::ProtectedBranchCommit {
                        branch: self.core.status().branch,
                        branches,
                        selected: 1,
                        new_branch,
                        editing_new_branch: true,
                    });
                } else {
                    self.blocked_intent = Some(Intent::Commit);
                    self.modal = Some(Modal::BranchSwitch {
                        branches,
                        selected: 0,
                    });
                }
            }
            1 => {
                if new_branch.trim().is_empty() || !editing_new_branch {
                    self.modal = Some(Modal::ProtectedBranchCommit {
                        branch: self.core.status().branch,
                        branches,
                        selected: 1,
                        new_branch,
                        editing_new_branch: true,
                    });
                    return Ok(());
                }
                self.busy = true;
                self.busy_message = translate("Creating branch...", &lang);
                let mut core = self.core.clone();
                self.spawn_outcome_task(tx, async move {
                    let branch = new_branch.trim().to_string();
                    let core_switch = core.clone();
                    match tokio::task::spawn_blocking(move || {
                        core_switch.create_and_switch_branch(&branch)
                    })
                    .await
                    {
                        Ok(Ok(_)) => match core.draft_commit_plan().await {
                            Ok((plan, usage)) => AsyncOutcome::CommitPlanReady(plan, usage),
                            Err(error) => AsyncOutcome::Error(error),
                        },
                        Ok(Err(error)) => AsyncOutcome::Error(error),
                        Err(error) => AsyncOutcome::Error(AppError::Custom(format!(
                            "Background task failed: {}",
                            error
                        ))),
                    }
                });
            }
            _ => self.start_commit_plan_flow(tx),
        }
        Ok(())
    }

    async fn activate_text_input(
        &mut self,
        value: String,
        kind: TextInputKind,
        tx: mpsc::Sender<AppEvent>,
    ) -> Result<()> {
        let lang = self.core.config.language.clone();
        match kind {
            TextInputKind::OriginUrl => match self.core.add_origin(value.trim()) {
                Ok(_) => {
                    self.push_assistant("Origin remote added.");
                    self.resume_onboarding_if_needed();
                }
                Err(error) => {
                    if let Some((issue, blocking)) = self.dependency_issue_from_error(&error) {
                        self.open_dependency_issue(issue, blocking);
                    } else {
                        self.push_error(error);
                    }
                }
            },
            TextInputKind::RepoName => {
                let repo = value.trim().to_string();
                if repo.is_empty() {
                    self.push_error(AppError::EmptyRepositoryName);
                } else {
                    self.modal = Some(Modal::Picker {
                        title: translate("Repository visibility", &lang),
                        items: vec![
                            PickerItem {
                                label: translate("Private", &lang),
                                value: PickerValue::Visibility {
                                    repo: repo.clone(),
                                    private: true,
                                },
                            },
                            PickerItem {
                                label: translate("Public", &lang),
                                value: PickerValue::Visibility {
                                    repo,
                                    private: false,
                                },
                            },
                        ],
                        selected: 0,
                    });
                }
            }
            TextInputKind::CommitSubject(mut message) => {
                if !value.trim().is_empty() {
                    message.subject = value.trim().to_string();
                }
                self.modal = Some(Modal::CommitReview {
                    message,
                    files: Vec::new(),
                    selected: 0,
                    scroll: 0,
                });
            }
            TextInputKind::ScoutQuestion => {
                let lang = self.core.config.language.clone();
                self.busy = true;
                self.busy_message = translate("Asking Scout question...", &lang);
                self.push_streaming_assistant(translate("Working on it...", &lang));
                let value = value.clone();
                let mut core = self.core.clone();
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    let stream_ctx = StreamingContext::new(&tx_clone);
                    let outcome = match core
                        .scout_question_stream(&value, stream_ctx.tx_token)
                        .await
                    {
                        Ok(_) => AsyncOutcome::StreamEnd,
                        Err(e) => AsyncOutcome::Error(e),
                    };
                    let _ = tx_clone
                        .send(AppEvent::AsyncOutcome(Box::new(outcome)))
                        .await;
                    let _ = stream_ctx.forwarder.await;
                });
            }
            TextInputKind::BaseUrl => {
                self.core.set_base_url(value)?;
                self.push_assistant("Base URL updated.");
                self.resume_onboarding_if_needed();
            }
            TextInputKind::ModelName => {
                self.core.set_model(value.trim().to_string())?;
                self.push_assistant("Model updated.");
                self.resume_onboarding_if_needed();
            }
            TextInputKind::ApiKey => {
                self.core.set_api_key(&value)?;
                self.push_assistant("API key updated.");
                if self
                    .core
                    .config
                    .model
                    .as_deref()
                    .is_none_or(|model| model.trim().is_empty())
                {
                    self.start_model_picker_flow(tx);
                } else {
                    self.resume_onboarding_if_needed();
                }
            }
        }
        Ok(())
    }

    fn clear_scout_pending(&mut self) {
        self.scout_pending = false;
        self.pending_modal = None;
    }

    async fn activate_scout_decision(&mut self, tx: mpsc::Sender<AppEvent>) -> Result<()> {
        let Some(Modal::ScoutDecision { selected }) = self.modal.take() else {
            return Ok(());
        };
        let lang = self.core.config.language.clone();

        match selected {
            0 => {
                self.busy = true;
                self.busy_message = translate("Explaining changes...", &lang);
                self.push_streaming_assistant(translate("Working on it...", &lang));
                let mut core = self.core.clone();
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    let stream_ctx = StreamingContext::new(&tx_clone);
                    let outcome = match core.explain_stream(stream_ctx.tx_token).await {
                        Ok(_) => AsyncOutcome::StreamEnd,
                        Err(e) => AsyncOutcome::Error(e),
                    };
                    let _ = tx_clone
                        .send(AppEvent::AsyncOutcome(Box::new(outcome)))
                        .await;
                    let _ = stream_ctx.forwarder.await;
                });
            }
            1 => {
                self.busy = true;
                self.busy_message = translate("Reviewing changes...", &lang);
                self.push_streaming_assistant(translate("Working on it...", &lang));
                let mut core = self.core.clone();
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    let stream_ctx = StreamingContext::new(&tx_clone);
                    let outcome = match core.review_stream(stream_ctx.tx_token).await {
                        Ok(_) => AsyncOutcome::StreamEnd,
                        Err(e) => AsyncOutcome::Error(e),
                    };
                    let _ = tx_clone
                        .send(AppEvent::AsyncOutcome(Box::new(outcome)))
                        .await;
                    let _ = stream_ctx.forwarder.await;
                });
            }
            2 => {
                self.modal = Some(Modal::TextInput {
                    title: translate("Ask a custom question about changes", &lang),
                    value: String::new(),
                    kind: TextInputKind::ScoutQuestion,
                });
            }
            _ => {
                self.push_assistant(translate("Scout session closed.", &lang));
            }
        }
        Ok(())
    }

    async fn activate_commit_review(
        &mut self,
        message: CommitMessage,
        _files: Vec<FileEntry>,
        selected: usize,
        tx: mpsc::Sender<AppEvent>,
    ) -> Result<()> {
        let lang = self.core.config.language.clone();

        match selected {
            0 => {
                let core = self.core.clone();
                let msg = message.clone();
                tokio::spawn(async move {
                    let outcome = match core.create_commit(&msg) {
                        Ok(_) => AsyncOutcome::CommitCreated(msg),
                        Err(error) => AsyncOutcome::Error(error),
                    };
                    let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
                });
            }
            1 => {
                let subject = message.subject.clone();
                self.modal = Some(Modal::TextInput {
                    title: translate("Edit commit subject", &lang),
                    value: subject,
                    kind: TextInputKind::CommitSubject(message),
                });
            }
            2 => {
                self.busy = true;
                let lang_str = lang.clone();
                self.busy_message = translate("Generating commit message...", &lang_str);
                let mut core = self.core.clone();
                tokio::spawn(async move {
                    let outcome = match core.draft_commit_plan().await {
                        Ok((plan, usage)) => AsyncOutcome::CommitPlanReady(plan, usage),
                        Err(error) => AsyncOutcome::Error(error),
                    };
                    let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
                });
            }
            _ => {
                self.push_assistant(translate("Cancelled.", &lang));
            }
        }
        Ok(())
    }

    async fn activate_setup(&mut self, selected: usize) -> Result<()> {
        let lang = &self.core.config.language;
        match selected {
            0 => match self.core.init_repo() {
                Ok(_) => self.push_assistant("Git repository initialized."),
                Err(error) => {
                    if let Some((issue, blocking)) = self.dependency_issue_from_error(&error) {
                        self.open_dependency_issue(issue, blocking);
                    } else {
                        self.push_error(error);
                    }
                }
            },
            1 => {
                if !self.core.status().is_repo {
                    let _ = self.core.init_repo();
                }
                self.modal = Some(Modal::TextInput {
                    title: translate("Origin URL", lang),
                    value: String::new(),
                    kind: TextInputKind::OriginUrl,
                });
            }
            2 => {
                if let Some(issue) = self.github_issue() {
                    self.open_dependency_issue(issue, false);
                    return Ok(());
                }
                let default_name = self.core.status().repo;
                self.modal = Some(Modal::TextInput {
                    title: translate("GitHub repository name", lang),
                    value: default_name,
                    kind: TextInputKind::RepoName,
                });
            }
            _ => self.push_assistant("Setup cancelled."),
        }
        Ok(())
    }

    async fn activate_setting(
        &mut self,
        selected: usize,
        tx: mpsc::Sender<AppEvent>,
    ) -> Result<()> {
        match selected {
            0 => self.open_language_picker(),
            1 => self.open_provider_picker(),
            2 => self.start_model_picker_flow(tx),
            3 => {
                self.modal = Some(Modal::TextInput {
                    title: "Base URL".to_string(),
                    value: self.core.config.base_url.clone().unwrap_or_default(),
                    kind: TextInputKind::BaseUrl,
                });
            }
            4 => {
                self.modal = Some(Modal::TextInput {
                    title: "API key".to_string(),
                    value: String::new(),
                    kind: TextInputKind::ApiKey,
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn adjust_settings(&mut self, direction: i32) -> Result<()> {
        let Some(Modal::Settings { selected }) = self.modal.as_ref() else {
            return Ok(());
        };
        match selected {
            0 => {
                self.core.config.language =
                    cycle_language(&self.core.config.language, direction).to_string();
            }
            5 => self.core.config.staged_only = !self.core.config.staged_only,
            6 => self.core.config.auto_setup_repo = !self.core.config.auto_setup_repo,
            7 => {
                self.core.config.prompt_push_after_commit =
                    !self.core.config.prompt_push_after_commit
            }
            8 => {
                self.core.config.theme = if direction >= 0 {
                    self.core.config.theme.next()
                } else {
                    self.core.config.theme.previous()
                };
            }
            9 => {
                self.core.config.history_limit = if direction >= 0 {
                    self.core.config.history_limit.next()
                } else {
                    self.core.config.history_limit.previous()
                };
            }
            _ => {}
        }
        self.core.save_config()?;
        self.trim_history();
        Ok(())
    }

    fn move_modal_selection(&mut self, delta: i32) {
        let Some(modal) = self.modal.as_mut() else {
            return;
        };
        let (selected, len) = match modal {
            Modal::OnboardingWizard { selected, step } => (selected, onboarding_action_count(step)),
            Modal::Settings { selected } => (selected, SETTINGS_ROWS),
            Modal::Picker {
                selected, items, ..
            } => (selected, items.len()),
            Modal::Confirm { selected, .. } => (selected, 2),
            Modal::CommitReview { selected, .. } => (selected, 4),
            Modal::CommitPlanReview { selected, .. } => (selected, 3),
            Modal::CommitLog {
                selected, entries, ..
            } => (selected, entries.len()),
            Modal::BranchSwitch {
                selected, branches, ..
            } => (selected, branches.total_count()),
            Modal::ProtectedBranchCommit { selected, .. } => (selected, 3),
            Modal::PrDraft { selected, .. } => (selected, 2),
            Modal::ExistingPrs { selected, .. } => (selected, 3),
            Modal::ConflictResolution { selected, .. } => (selected, 3),
            Modal::Setup { selected } => (selected, 4),
            Modal::TextInput { .. } => return,
            Modal::DependencyIssue {
                selected, actions, ..
            } => (selected, actions.len()),
            Modal::CommandExecution { .. } => return,
            Modal::ScoutDecision { selected } => (selected, 4),
        };
        if len == 0 {
            return;
        }
        if delta < 0 {
            *selected = if *selected == 0 {
                len - 1
            } else {
                *selected - 1
            };
        } else {
            *selected = (*selected + 1) % len;
        }
    }

    fn open_settings(&mut self) {
        self.modal = Some(Modal::Settings {
            selected: self.initial_settings_row(),
        });
    }

    fn should_block_for_onboarding(&self) -> bool {
        self.current_onboarding_step().is_some()
    }

    fn resume_onboarding_if_needed(&mut self) {
        if !self.onboarding_active {
            return;
        }
        if self.should_block_for_onboarding() {
            self.open_onboarding_wizard();
        } else {
            self.onboarding_active = false;
            self.modal = None;
        }
    }

    fn go_back_in_onboarding(&mut self, step: OnboardingStep) {
        if !self.onboarding_active {
            return;
        }
        let selected = if self.core.config.language.eq_ignore_ascii_case("spanish") {
            1
        } else {
            0
        };
        self.modal = Some(Modal::OnboardingWizard {
            step: match step {
                OnboardingStep::LanguageSelection => OnboardingStep::LanguageSelection,
                _ => OnboardingStep::LanguageSelection,
            },
            selected,
        });
    }

    fn maybe_open_onboarding(&mut self) {
        if !self.should_block_for_onboarding() {
            self.onboarding_active = false;
            return;
        }
        self.onboarding_active = true;
        if self.modal.is_none() {
            self.open_onboarding_wizard();
        }
    }

    fn open_onboarding_wizard(&mut self) {
        let Some(step) = self.current_onboarding_step() else {
            self.onboarding_active = false;
            self.modal = None;
            return;
        };
        self.onboarding_active = true;
        self.modal = Some(Modal::OnboardingWizard { step, selected: 0 });
    }

    fn current_onboarding_step(&self) -> Option<OnboardingStep> {
        let status = self.core.status();
        let provider_selected = self.core.config.provider.is_selected();
        let api_key_missing = self.core.config.provider.uses_api_key()
            && !self.core.api_key_configured().unwrap_or(false);
        let model_missing = self
            .core
            .config
            .model
            .as_deref()
            .is_none_or(|value| value.trim().is_empty());
        let dependency_setup_missing = self.dependency_doctor.as_ref().is_some_and(|doctor| {
            !doctor.git.is_ready()
                || (self.core.config.provider == LlmProviderKind::Ollama
                    && !doctor.ollama.is_ready())
                || (provider_selected && !doctor.llm_provider.is_ready())
        });
        let has_pending_setup = dependency_setup_missing
            || !status.is_repo
            || (!status.has_origin && !self.onboarding_remote_deferred)
            || !provider_selected
            || api_key_missing
            || model_missing;
        if has_pending_setup && !self.onboarding_language_confirmed {
            return Some(OnboardingStep::LanguageSelection);
        }
        if let Some(doctor) = self.dependency_doctor.as_ref()
            && !doctor.git.is_ready()
        {
            return Some(OnboardingStep::GitDependency(doctor.git.clone()));
        }
        if !status.is_repo {
            return Some(OnboardingStep::RepoSetupChoice);
        }
        if !status.has_origin && !self.onboarding_remote_deferred {
            return Some(OnboardingStep::RemoteSetupChoice);
        }
        if !self.core.config.provider.is_selected() {
            return Some(OnboardingStep::ProviderSelection);
        }
        if let Some(doctor) = self.dependency_doctor.as_ref()
            && self.core.config.provider == LlmProviderKind::Ollama
            && !doctor.ollama.is_ready()
        {
            return Some(OnboardingStep::ProviderDependency(doctor.ollama.clone()));
        }
        if self.core.config.provider.uses_api_key()
            && !self.core.api_key_configured().unwrap_or(false)
        {
            return Some(OnboardingStep::ApiKeyConfiguration);
        }
        if self
            .core
            .config
            .model
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Some(OnboardingStep::ModelConfiguration);
        }
        if let Some(doctor) = self.dependency_doctor.as_ref()
            && !doctor.llm_provider.is_ready()
        {
            return Some(OnboardingStep::ProviderDependency(
                doctor.llm_provider.clone(),
            ));
        }
        None
    }

    async fn activate_onboarding_step(
        &mut self,
        step: OnboardingStep,
        selected: usize,
        tx: mpsc::Sender<AppEvent>,
    ) -> Result<()> {
        match step {
            OnboardingStep::GitDependency(issue) | OnboardingStep::ProviderDependency(issue) => {
                let actions = onboarding_dependency_actions(&issue, &self.core.config.language);
                let Some(action) = actions.get(selected).cloned() else {
                    return Ok(());
                };
                match action.action {
                    DependencyModalActionKind::RunRecovery(recovery) => {
                        self.start_dependency_recovery(issue, recovery, false, tx);
                    }
                    DependencyModalActionKind::CopyCommand(command) => {
                        let notice = self.copy_dependency_command(&command).ok();
                        self.open_onboarding_wizard();
                        if let Some(message) = notice {
                            self.push_system(message);
                        }
                    }
                    DependencyModalActionKind::RetryOnly => {
                        self.retry_dependency_check(issue.kind, false, tx);
                    }
                    DependencyModalActionKind::Close | DependencyModalActionKind::ExitCli => {}
                }
                Ok(())
            }
            OnboardingStep::LanguageSelection => {
                let selected_language = if selected == 1 { "Spanish" } else { "English" };
                self.core.config.language = selected_language.to_string();
                if let Err(error) = self.core.save_config() {
                    self.push_system(format!("Could not save language preference: {}", error));
                }
                self.push_system(format!("Language changed to {}", selected_language));
                self.onboarding_language_confirmed = true;
                self.resume_onboarding_if_needed();
                Ok(())
            }
            OnboardingStep::RepoSetupChoice => match selected {
                0 => {
                    self.core.init_repo()?;
                    self.resume_onboarding_if_needed();
                    Ok(())
                }
                1 => {
                    self.core.init_repo()?;
                    self.modal = Some(Modal::TextInput {
                        title: translate("Origin URL", &self.core.config.language),
                        value: String::new(),
                        kind: TextInputKind::OriginUrl,
                    });
                    Ok(())
                }
                _ => Ok(()),
            },
            OnboardingStep::RemoteSetupChoice => match selected {
                0 => {
                    self.modal = Some(Modal::TextInput {
                        title: translate("Origin URL", &self.core.config.language),
                        value: String::new(),
                        kind: TextInputKind::OriginUrl,
                    });
                    Ok(())
                }
                1 => {
                    self.onboarding_remote_deferred = true;
                    self.resume_onboarding_if_needed();
                    Ok(())
                }
                _ => Ok(()),
            },
            OnboardingStep::ProviderSelection => {
                self.open_provider_picker();
                Ok(())
            }
            OnboardingStep::ModelConfiguration => {
                match selected {
                    0 => self.start_model_picker_flow(tx),
                    1 => self.open_model_input_with_reason(
                        "Enter the exact model name expected by your provider.",
                    ),
                    2 => self.open_provider_picker(),
                    _ => {}
                }
                Ok(())
            }
            OnboardingStep::ApiKeyConfiguration => {
                if selected == 1 {
                    self.open_provider_picker();
                } else {
                    self.modal = Some(Modal::TextInput {
                        title: if self.core.config.language.eq_ignore_ascii_case("spanish") {
                            "Clave API".to_string()
                        } else {
                            "API key".to_string()
                        },
                        value: String::new(),
                        kind: TextInputKind::ApiKey,
                    });
                }
                Ok(())
            }
        }
    }

    fn initial_settings_row(&self) -> usize {
        if !self.core.config.provider.is_selected() {
            1
        } else if self.core.config.provider.uses_api_key()
            && !self.core.api_key_configured().unwrap_or(false)
        {
            4
        } else if self
            .core
            .config
            .model
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            2
        } else {
            0
        }
    }

    fn scroll_commit_plan(&mut self, delta: usize) {
        let max_scroll = self.commit_plan_max_scroll();
        if let Some(Modal::CommitPlanReview { scroll, .. }) = self.modal.as_mut() {
            *scroll = scroll.saturating_add(delta).min(max_scroll);
        }
    }

    fn commit_plan_max_scroll(&self) -> usize {
        let Some(Modal::CommitPlanReview { plan, .. }) = self.modal.as_ref() else {
            return 0;
        };
        let Ok((width, height)) = size() else {
            return commit_plan_text_lines(plan).saturating_sub(12);
        };
        let modal_width = if width < 70 {
            width.saturating_mul(98) / 100
        } else if width < 100 {
            width.saturating_mul(90) / 100
        } else if width < 140 {
            width.saturating_mul(80) / 100
        } else {
            width.saturating_mul(68) / 100
        };
        let modal_height = if height < 25 {
            height.saturating_mul(95) / 100
        } else if height < 40 {
            height.saturating_mul(85) / 100
        } else {
            height.saturating_mul(60) / 100
        };
        let content_width = modal_width.saturating_sub(6).max(1) as usize;
        let visible_lines = modal_height.saturating_sub(6).max(1) as usize;
        commit_plan_wrapped_lines(plan, content_width).saturating_sub(visible_lines)
    }

    fn start_model_picker_flow(&mut self, tx: mpsc::Sender<AppEvent>) {
        if !self.core.config.provider.supports_model_listing() {
            self.open_model_input_with_reason(
                "This provider does not support model listing. Enter the exact model name manually.",
            );
            return;
        }
        self.busy = true;
        self.busy_message = match self.core.config.language.to_lowercase().trim() {
            "spanish" | "español" | "espanol" => "Cargando modelos del proveedor...".to_string(),
            _ => "Loading provider models...".to_string(),
        };
        let core = self.core.clone();
        let handle = tokio::spawn(async move {
            let outcome = match core.models().await {
                Ok(models) => AsyncOutcome::ModelPicker(models),
                Err(error) => AsyncOutcome::ModelFetchError(format!(
                    "Could not list provider models: {}",
                    error
                )),
            };
            let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
        });
        self.in_flight_abort = Some(handle.abort_handle());
    }

    fn open_api_key_input(&mut self) {
        self.modal = Some(Modal::TextInput {
            title: "API key".to_string(),
            value: String::new(),
            kind: TextInputKind::ApiKey,
        });
    }

    fn open_model_input_with_reason(&mut self, reason: &str) {
        if !reason.trim().is_empty() {
            self.push_system(reason.to_string());
        }
        self.modal = Some(Modal::TextInput {
            title: format!("{} model", self.core.provider_label()),
            value: self.core.config.model.clone().unwrap_or_default(),
            kind: TextInputKind::ModelName,
        });
    }

    fn open_language_picker(&mut self) {
        let lang = self.core.config.language.clone();
        self.modal = Some(Modal::Picker {
            title: translate("Select language", &lang).to_string(),
            items: ["English", "Spanish"]
                .into_iter()
                .map(|language| PickerItem {
                    label: language.to_string(),
                    value: PickerValue::Language(language.to_string()),
                })
                .collect(),
            selected: 0,
        });
    }

    fn open_provider_picker(&mut self) {
        let lang = self.core.config.language.clone();
        self.modal = Some(Modal::Picker {
            title: translate("Select provider", &lang),
            items: LlmProviderKind::all()
                .iter()
                .map(|provider| PickerItem {
                    label: provider.label().to_string(),
                    value: PickerValue::Provider(provider.label().to_string()),
                })
                .collect(),
            selected: 0,
        });
    }

    fn refresh_suggestions(&mut self) {
        if self.input.starts_with('/') {
            self.suggestions = slash_suggestions(&self.input);
            self.selected_suggestion = 0;
        } else {
            self.suggestions.clear();
            self.selected_suggestion = 0;
        }
    }

    pub fn push_user(&mut self, message: impl Into<String>) {
        self.history.push(ChatEntry {
            role: ChatRole::User,
            message: message.into(),
        });
        self.reset_history_scroll();
        self.trim_history();
    }

    pub fn push_assistant(&mut self, message: impl Into<String>) {
        let msg = message.into();
        let translated = strip_emoji(&translate(&msg, &self.core.config.language));
        self.history.push(ChatEntry {
            role: ChatRole::Assistant,
            message: translated,
        });
        self.reset_history_scroll();
        self.trim_history();
    }

    pub fn push_streaming_assistant(&mut self, message: impl Into<String>) {
        let msg = message.into();
        let translated = strip_emoji(&translate(&msg, &self.core.config.language));
        self.history.push(ChatEntry {
            role: ChatRole::Assistant,
            message: translated,
        });
        self.streaming_response_index = Some(self.history.len().saturating_sub(1));
        self.reset_history_scroll();
    }

    pub fn push_system(&mut self, message: impl Into<String>) {
        let msg = message.into();
        let translated = strip_emoji(&translate(&msg, &self.core.config.language));
        self.history.push(ChatEntry {
            role: ChatRole::System,
            message: translated,
        });
        self.reset_history_scroll();
        self.trim_history();
    }

    pub fn push_error(&mut self, error: AppError) {
        self.history.push(ChatEntry {
            role: ChatRole::Error,
            message: format!("Error: {}", display_error_message(&error)),
        });
        self.reset_history_scroll();
        self.trim_history();
    }

    fn scroll_history_up(&mut self, amount: usize) {
        if self.history.is_empty() {
            self.history_scroll = 0;
        } else {
            self.history_scroll = self.history_scroll.saturating_add(amount);
        }
    }

    fn scroll_history_down(&mut self, amount: usize) {
        self.history_scroll = self.history_scroll.saturating_sub(amount);
    }

    fn reset_history_scroll(&mut self) {
        self.history_scroll = 0;
    }

    fn trim_history(&mut self) {
        let limit = self.core.config.history_limit.value();
        if self.history.len() > limit {
            self.history.drain(0..self.history.len() - limit);
        }
    }

    fn spawn_outcome_task<F>(&mut self, tx: mpsc::Sender<AppEvent>, future: F)
    where
        F: Future<Output = AsyncOutcome> + Send + 'static,
    {
        let operation = tokio::spawn(future);
        let abort_handle = operation.abort_handle();
        tokio::spawn(async move {
            let outcome = match operation.await {
                Ok(outcome) => outcome,
                Err(error) if error.is_cancelled() => return,
                Err(error) => AsyncOutcome::Error(AppError::Custom(format!(
                    "Operation task failed before returning a result: {error}"
                ))),
            };
            let _ = tx.send(AppEvent::AsyncOutcome(Box::new(outcome))).await;
        });
        self.in_flight_abort = Some(abort_handle);
    }

    fn open_dependency_issue(&mut self, issue: DependencyStatus, blocking: bool) {
        self.open_dependency_issue_with_notice(issue, blocking, None);
    }

    fn open_dependency_issue_with_notice(
        &mut self,
        issue: DependencyStatus,
        blocking: bool,
        notice: Option<String>,
    ) {
        let actions = dependency_modal_actions(&issue, blocking, &self.core.config.language);
        self.modal = Some(Modal::DependencyIssue {
            issue,
            actions,
            selected: 0,
            blocking,
            notice,
        });
    }

    fn resolve_next_issue(&mut self) {
        if self.should_block_for_onboarding() {
            self.open_onboarding_wizard();
            return;
        }
        if let Some(issue) = self.git_issue() {
            self.open_dependency_issue(issue, true);
            return;
        }
        if let Some(issue) = self.provider_issue() {
            self.open_dependency_issue(issue, false);
            return;
        }
        if self.core.config.provider == LlmProviderKind::Ollama
            && let Some(issue) = self.ollama_issue()
        {
            self.open_dependency_issue(issue, false);
            return;
        }
        if let Some(issue) = self.github_issue() {
            self.open_dependency_issue(issue, false);
            return;
        }
        self.push_system(translate("No pending issues.", &self.core.config.language));
    }

    fn append_dependency_command_log(&mut self, line: String) {
        if let Some(Modal::CommandExecution { logs, .. }) = self.modal.as_mut() {
            logs.push(line);
            if logs.len() > 200 {
                let overflow = logs.len() - 200;
                logs.drain(0..overflow);
            }
        }
    }

    fn copy_dependency_command(&mut self, command: &str) -> std::result::Result<String, String> {
        let outcome = Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(command.to_string()))
            .map(|_| ());

        match outcome {
            Ok(()) => Ok(translate(
                "Command copied to clipboard.",
                &self.core.config.language,
            )),
            Err(error) => Err(format!(
                "{} {}",
                translate(
                    "Failed to copy command to clipboard:",
                    &self.core.config.language
                ),
                error
            )),
        }
    }

    fn start_dependency_recovery(
        &mut self,
        issue: DependencyStatus,
        action: DependencyAction,
        blocking: bool,
        tx: mpsc::Sender<AppEvent>,
    ) {
        let title = localized_dependency_action_title(&action, &self.core.config.language);
        let command = action
            .command
            .as_ref()
            .map(|spec| spec.display.clone())
            .or_else(|| action.manual_command.clone())
            .unwrap_or_else(|| issue.kind.label().to_string());

        if action.runnable_in_cli {
            self.busy = true;
            self.busy_message =
                localized_dependency_busy_message(&action, &self.core.config.language);
            self.modal = Some(Modal::CommandExecution {
                title,
                command: command.clone(),
                logs: vec![format!("$ {command}")],
                issue: issue.clone(),
                blocking,
            });
            let resume_intent = self.blocked_intent.clone();
            let core = self.core.clone();
            self.spawn_outcome_task(
                tx.clone(),
                run_dependency_action_outcome(
                    core,
                    tx,
                    issue,
                    action,
                    blocking,
                    resume_intent,
                    self.managed_ollama_pid.clone(),
                ),
            );
        } else {
            self.modal = Some(Modal::CommandExecution {
                title,
                command: command.clone(),
                logs: manual_dependency_logs(&issue, &action, &self.core.config.language),
                issue,
                blocking,
            });
        }
    }

    fn finish_dependency_command(
        &mut self,
        report: DependencyDoctor,
        issue: DependencyStatus,
        blocking: bool,
        resume_intent: Option<Intent>,
        succeeded: bool,
        tx: mpsc::Sender<AppEvent>,
    ) {
        self.ollama_health = Some(ollama_health_from_report(&report));
        self.dependency_doctor = Some(report);

        if issue.is_ready() {
            self.modal = None;
            if self.onboarding_active {
                self.resume_onboarding_if_needed();
                return;
            }
            let intent_to_resume = resume_intent.or_else(|| self.blocked_intent.take());
            self.blocked_intent = None;
            if let Some(intent) = intent_to_resume {
                self.push_system(translate(
                    "Dependency recovered. Retrying previous command.",
                    &self.core.config.language,
                ));
                self.execute_intent(intent, tx);
            }
            return;
        }

        self.blocked_intent = resume_intent;
        if succeeded {
            self.append_dependency_command_log(translate(
                "Dependency is still unavailable. Press Enter to review the suggested next step.",
                &self.core.config.language,
            ));
        }
        if self.onboarding_active {
            self.open_onboarding_wizard();
        } else if !matches!(self.modal, Some(Modal::CommandExecution { .. })) {
            self.open_dependency_issue(issue, blocking);
        } else if let Some(Modal::CommandExecution {
            issue: modal_issue, ..
        }) = self.modal.as_mut()
        {
            *modal_issue = issue;
        }
    }

    fn retry_dependency_check(
        &mut self,
        kind: DependencyKind,
        blocking: bool,
        tx: mpsc::Sender<AppEvent>,
    ) {
        self.busy = true;
        self.busy_message = translate("Checking dependencies...", &self.core.config.language);
        self.dependency_retry = Some(DependencyRetry { kind, blocking });
        let core = self.core.clone();
        self.spawn_outcome_task(tx, async move {
            AsyncOutcome::DependencyDoctor(core.dependency_doctor().await)
        });
    }

    fn git_issue(&self) -> Option<DependencyStatus> {
        self.dependency_doctor
            .as_ref()
            .map(|doctor| doctor.git.clone())
            .filter(|status| !status.is_ready())
    }

    fn github_issue(&self) -> Option<DependencyStatus> {
        self.dependency_doctor
            .as_ref()
            .map(|doctor| doctor.gh.clone())
            .filter(|status| !status.is_ready())
    }

    fn provider_issue(&self) -> Option<DependencyStatus> {
        self.dependency_doctor
            .as_ref()
            .map(|doctor| doctor.llm_provider.clone())
            .filter(|status| !status.is_ready())
    }

    fn ollama_issue(&self) -> Option<DependencyStatus> {
        if self.core.config.provider != LlmProviderKind::Ollama {
            return None;
        }
        if let Some(doctor) = &self.dependency_doctor {
            let status = doctor.ollama.clone();
            return match status.state {
                DependencyState::Ready => None,
                DependencyState::Missing | DependencyState::NotRunning => {
                    if self.ollama_running() {
                        None
                    } else {
                        Some(status)
                    }
                }
                DependencyState::NotConfigured => Some(status),
            };
        }

        match self.ollama_health.as_ref() {
            Some(health) if health.running => None,
            Some(health) if health.installed => Some(DependencyStatus::ollama_not_running(
                &PlatformInfo::detect(),
                health.version.clone(),
                health
                    .runtime_message
                    .clone()
                    .unwrap_or_else(|| "Ollama is not responding.".to_string()),
            )),
            Some(health) => Some(DependencyStatus::missing(
                DependencyKind::Ollama,
                &PlatformInfo::detect(),
                health.install_message.clone(),
            )),
            None => None,
        }
    }

    fn dependency_issue_from_error(&self, error: &AppError) -> Option<(DependencyStatus, bool)> {
        let platform = self
            .dependency_doctor
            .as_ref()
            .map(|doctor| doctor.platform.clone())
            .unwrap_or_else(PlatformInfo::detect);

        match error {
            AppError::ProviderNotSelected => Some((
                DependencyStatus::llm_provider_not_configured(
                    &platform,
                    error.to_string(),
                    provider_fallback_url(self.core.config.provider),
                ),
                false,
            )),
            AppError::GitMissing => Some((
                DependencyStatus::missing(DependencyKind::Git, &platform, Some(error.to_string())),
                true,
            )),
            AppError::GhMissing => Some((
                DependencyStatus::missing(
                    DependencyKind::GitHubCli,
                    &platform,
                    Some(error.to_string()),
                ),
                false,
            )),
            AppError::GhAuthMissing => {
                let version = self
                    .dependency_doctor
                    .as_ref()
                    .and_then(|doctor| doctor.gh.version.clone());
                Some((DependencyStatus::gh_auth_missing(&platform, version), false))
            }
            AppError::NoOllamaModels => {
                let version = self
                    .dependency_doctor
                    .as_ref()
                    .and_then(|doctor| doctor.ollama.version.clone());
                Some((
                    DependencyStatus::ollama_no_models(&platform, version),
                    false,
                ))
            }
            AppError::OllamaUnavailable { .. }
            | AppError::OllamaHttp(_)
            | AppError::OllamaDecode(_) => Some((
                DependencyStatus::ollama_not_running(
                    &platform,
                    self.dependency_doctor
                        .as_ref()
                        .and_then(|doctor| doctor.ollama.version.clone()),
                    error.to_string(),
                ),
                false,
            )),
            AppError::MissingApiKey { .. }
            | AppError::MissingBaseUrl { .. }
            | AppError::MissingModel { .. } => Some((
                DependencyStatus::llm_provider_not_configured(
                    &platform,
                    error.to_string(),
                    provider_fallback_url(self.core.config.provider),
                ),
                false,
            )),
            AppError::ProviderHttp {
                status: 401 | 403 | 404,
                ..
            } => Some((
                DependencyStatus::llm_provider_not_configured(
                    &platform,
                    error.to_string(),
                    provider_fallback_url(self.core.config.provider),
                ),
                false,
            )),
            AppError::ProviderUnavailable { .. }
            | AppError::ProviderHttp { .. }
            | AppError::ProviderDecode { .. } => Some((
                DependencyStatus::llm_provider_not_running(
                    &platform,
                    error.to_string(),
                    provider_fallback_url(self.core.config.provider),
                ),
                false,
            )),
            _ => None,
        }
    }
}

fn outcome_completes_busy(outcome: &AsyncOutcome) -> bool {
    matches!(
        outcome,
        AsyncOutcome::DependencyDoctor(_)
            | AsyncOutcome::Message(_)
            | AsyncOutcome::Error(_)
            | AsyncOutcome::CommitPlanReady(_, _)
            | AsyncOutcome::CommitLogReady(_)
            | AsyncOutcome::CommitLogReset(_)
            | AsyncOutcome::SwitchBranchesReady(_)
            | AsyncOutcome::PrDraft(_, _)
            | AsyncOutcome::ModelPicker(_)
            | AsyncOutcome::PushCompleted(_)
            | AsyncOutcome::CommitCreated(_)
            | AsyncOutcome::CommitPlanExecuted(_)
            | AsyncOutcome::DependencyCommandFinished { .. }
            | AsyncOutcome::RemoteBranches(_)
    )
}

async fn commit_log_outcome(core: AppCore) -> AsyncOutcome {
    match tokio::task::spawn_blocking(move || core.commit_log()).await {
        Ok(Ok(entries)) => AsyncOutcome::CommitLogReady(entries),
        Ok(Err(error)) => AsyncOutcome::Error(error),
        Err(error) => AsyncOutcome::Error(AppError::Custom(format!(
            "Background task failed: {}",
            error
        ))),
    }
}

async fn commit_log_reset_outcome(core: AppCore, hash: String) -> AsyncOutcome {
    match tokio::task::spawn_blocking(move || core.reset_soft_to_commit(&hash)).await {
        Ok(Ok(output)) => AsyncOutcome::CommitLogReset(output),
        Ok(Err(error)) => AsyncOutcome::Error(error),
        Err(error) => AsyncOutcome::Error(AppError::Custom(format!(
            "Background task failed: {}",
            error
        ))),
    }
}

async fn draft_commit_plan_outcome(mut core: AppCore) -> AsyncOutcome {
    match tokio::time::timeout(
        Duration::from_secs(COMMIT_PLAN_TIMEOUT_SECS),
        core.draft_commit_plan(),
    )
    .await
    {
        Ok(Ok((plan, usage))) => AsyncOutcome::CommitPlanReady(plan, usage),
        Ok(Err(error)) => AsyncOutcome::Error(error),
        Err(_) => AsyncOutcome::Error(AppError::Custom(
            "commit plan generation timed out. Try a smaller diff, another model, or /staged."
                .to_string(),
        )),
    }
}

fn display_error_message(error: &AppError) -> String {
    match error {
        AppError::InvalidJson { source, value } => {
            let preview = value
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(180)
                .collect::<String>();
            format!(
                "LLM returned malformed JSON: {}. Try Regenerate, /staged, or another model. Preview: {}",
                source, preview
            )
        }
        _ => error.to_string(),
    }
}

fn push_completed_message(branch: &str, output: &str, language: &str) -> String {
    let is_spanish = matches!(
        language.to_lowercase().trim(),
        "spanish" | "español" | "espanol"
    );
    let branch = branch.trim();
    let mut message = if branch.is_empty() || branch == "-" {
        if is_spanish {
            "Push completado.".to_string()
        } else {
            "Push completed.".to_string()
        }
    } else if is_spanish {
        format!("Push completado para la rama `{branch}`.")
    } else {
        format!("Push completed for branch `{branch}`.")
    };

    let output = output.trim();
    if !output.is_empty() {
        if is_spanish {
            message.push_str("\n\nSalida de Git:\n");
        } else {
            message.push_str("\n\nGit output:\n");
        }
        message.push_str(output);
    }

    message
}

pub fn translate(text: &str, language: &str) -> String {
    let lang = language.to_lowercase();
    let lang_str = lang.trim();
    let result: &str = match lang_str {
        "spanish" | "español" | "espanol" => match text {
            "Cancelled." => "Cancelado.",
            "PR cancelled." => "PR cancelado.",
            "Push completed." => "Push completado.",
            "Origin remote added." => "Servidor remoto origin añadido.",
            "Pull Request created." => "Pull Request creado.",
            "Execute commit" => "Ejecutar commit",
            "Execute this commit? (git add + git commit)" => {
                "¿Ejecutar este commit? (git add + git commit)"
            }
            "Push branch" => "Hacer push de la rama",
            "Push the current branch?" => "¿Hacer push de la rama actual?",
            "Reset configuration" => "Resetear configuración",
            "Select PR base branch" => "Selecciona la rama base para el PR",
            "Select language" => "Seleccionar idioma",
            "Select provider" => "Seleccionar proveedor",
            "Switch branch" => "Cambiar de rama",
            "Fetching branches..." => "Obteniendo ramas...",
            "Loading commit history..." => "Cargando historial de commits...",
            "Rewriting history safely..." => "Reescribiendo historial de forma segura...",
            "Switching branch..." => "Cambiando de rama...",
            "Protected branch" => "Rama protegida",
            "Choose intended action" => "Selecciona la acción deseada",
            "Use slash commands, for example /commit or /help." => {
                "Usa comandos slash, por ejemplo /commit o /help."
            }
            "Final push confirmation" => "Confirmación final de push",
            "Push now? This updates the remote branch." => {
                "¿Hacer push ahora? Esto actualizará la rama remota."
            }
            "Repository visibility" => "Visibilidad del repositorio",
            "Private" => "Privado",
            "Public" => "Público",
            "Edit commit subject" => "Editar asunto del commit",
            "Origin URL" => "URL de origin",
            "GitHub repository name" => "Nombre del repositorio GitHub",
            "Explaining changes..." => "Explicando cambios...",
            "Reviewing changes..." => "Revisando cambios...",
            "Asking Scout question..." => "Preguntando a Scout...",
            "Ask a custom question about changes" => {
                "Haz una pregunta personalizada sobre los cambios"
            }
            "Scout session closed." => "Sesión de explorador Scout cerrada.",
            "Configuration reset. API keys were removed and origin was disconnected. Local Git history and files were not deleted." => {
                "Configuración reseteada. Se borraron las API keys y se desconectó origin. El historial Git local y los archivos no fueron borrados."
            }
            "Configuration reset. API keys were removed. Local Git history and files were not deleted." => {
                "Configuración reseteada. Se borraron las API keys. El historial Git local y los archivos no fueron borrados."
            }
            "Select commit type" => "Selecciona el tipo de commit",
            "Select UI theme" => "Selecciona el tema de color para la TUI",
            "Enter commit scope (optional)" => "Ingresa el alcance (scope) del commit (opcional)",
            "Enter commit subject" => "Ingresa el asunto (subject) del commit",
            "Enter commit body" => "Ingresa la descripción larga (body) del commit",
            "Commit plan executed." => "Plan de commits ejecutado.",
            "No commits found." => "No se encontraron commits.",
            "Soft reset completed. Changes were kept for recommit." => {
                "Reset soft completado. Los cambios quedaron disponibles para volver a commitear."
            }
            "Retry" => "Reintentar",
            "Close" => "Cerrar",
            "Exit CLI" => "Salir del CLI",
            "Command copied to clipboard." => "Comando copiado al portapapeles.",
            "No pending issues." => "No hay problemas pendientes.",
            "Failed to copy command to clipboard:" => {
                "No se pudo copiar el comando al portapapeles:"
            }
            "Working on it..." => "Trabajando en eso...",
            "Executing commit plan..." => "Ejecutando plan de commits...",
            "Generating commit plan..." => "Generando plan de commits...",
            "Generating a structured commit plan..." => {
                "Generando un plan de commits estructurado..."
            }
            "Creating pull request..." => "Creando pull request...",
            "Creating branch..." => "Creando rama...",
            "Creating GitHub repository..." => "Creando repositorio GitHub...",
            "Recovery action cancelled. Press Enter to return." => {
                "Acción de recuperación cancelada. Presiona Enter para volver."
            }
            "Dependency recovered. Retrying previous command." => {
                "Dependencia recuperada. Reintentando el comando anterior."
            }
            "Dependency is still unavailable. Press Enter to review the suggested next step." => {
                "La dependencia todavía no está disponible. Presiona Enter para revisar el siguiente paso sugerido."
            }
            _ => text,
        },
        _ => text,
    };

    if result != text {
        result.to_string()
    } else {
        match lang_str {
            "spanish" | "español" | "espanol" => {
                if text.starts_with("Model set to ") {
                    let model = text.strip_prefix("Model set to ").unwrap();
                    format!("Modelo configurado en {}.", model)
                } else if text.starts_with("Language changed to ") {
                    let language = text.strip_prefix("Language changed to ").unwrap();
                    format!("Idioma cambiado a {}", language)
                } else if text.starts_with("GitHub repository `") && text.ends_with("` created.") {
                    let repo = text
                        .strip_prefix("GitHub repository `")
                        .unwrap()
                        .strip_suffix("` created.")
                        .unwrap();
                    format!("Repositorio GitHub `{}` creado.", repo)
                } else if text.starts_with("Commit created:\n") {
                    let title = text.strip_prefix("Commit created:\n").unwrap();
                    format!("Commit creado:\n{}", title)
                } else if text.starts_with("Already on branch `") && text.ends_with("`.") {
                    let branch = text
                        .strip_prefix("Already on branch `")
                        .unwrap()
                        .strip_suffix("`.")
                        .unwrap();
                    format!("Ya estás en la rama `{}`.", branch)
                } else if text.starts_with("Switched to branch `") && text.ends_with("`.") {
                    let branch = text
                        .strip_prefix("Switched to branch `")
                        .unwrap()
                        .strip_suffix("`.")
                        .unwrap();
                    format!("Cambiado a la rama `{}`.", branch)
                } else if text.starts_with("You are on protected branch `")
                    && text.ends_with("`. Committing directly here is unusual. Continue?")
                {
                    let branch = text
                        .strip_prefix("You are on protected branch `")
                        .unwrap()
                        .strip_suffix("`. Committing directly here is unusual. Continue?")
                        .unwrap();
                    format!(
                        "Estás en la rama protegida `{}`. Hacer commits directamente aquí no suele ser lo normal. ¿Continuar?",
                        branch
                    )
                } else {
                    text.to_string()
                }
            }
            _ => text.to_string(),
        }
    }
}

pub const SETTINGS_ROWS: usize = 10;

#[derive(Debug, Clone)]
pub struct ChatEntry {
    pub role: ChatRole,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
    System,
    Error,
}

#[derive(Debug, Clone)]
pub enum Modal {
    OnboardingWizard {
        step: OnboardingStep,
        selected: usize,
    },
    Settings {
        selected: usize,
    },
    Picker {
        title: String,
        items: Vec<PickerItem>,
        selected: usize,
    },
    Confirm {
        title: String,
        message: String,
        selected: usize,
        kind: ConfirmKind,
    },
    TextInput {
        title: String,
        value: String,
        kind: TextInputKind,
    },
    CommitReview {
        message: CommitMessage,
        files: Vec<FileEntry>,
        selected: usize,
        scroll: usize,
    },
    CommitPlanReview {
        plan: CommitPlan,
        selected: usize,
        scroll: usize,
    },
    CommitLog {
        entries: Vec<CommitLogEntry>,
        selected: usize,
        action: usize,
        scroll: usize,
    },
    BranchSwitch {
        branches: SwitchBranches,
        selected: usize,
    },
    ProtectedBranchCommit {
        branch: String,
        branches: SwitchBranches,
        selected: usize,
        new_branch: String,
        editing_new_branch: bool,
    },
    PrDraft {
        base: String,
        draft: PullRequestDraft,
        selected: usize,
        scroll: usize,
    },
    ExistingPrs {
        prs: Vec<PrInfo>,
        base: String,
        selected: usize,
    },
    ConflictResolution {
        base: String,
        selected: usize,
    },
    Setup {
        selected: usize,
    },
    DependencyIssue {
        issue: DependencyStatus,
        actions: Vec<DependencyModalAction>,
        selected: usize,
        blocking: bool,
        notice: Option<String>,
    },
    CommandExecution {
        title: String,
        command: String,
        logs: Vec<String>,
        issue: DependencyStatus,
        blocking: bool,
    },
    ScoutDecision {
        selected: usize,
    },
}

#[derive(Debug, Clone)]
pub struct PickerItem {
    pub label: String,
    pub value: PickerValue,
}

#[derive(Debug, Clone)]
pub enum PickerValue {
    Provider(String),
    Model(String),
    Language(String),
    PrBase(String),
    Visibility { repo: String, private: bool },
    Theme(String),
}

#[derive(Debug, Clone)]
pub enum ConfirmKind {
    PushFirst,
    ResetConfiguration,
}

#[derive(Debug, Clone)]
pub enum TextInputKind {
    OriginUrl,
    RepoName,
    CommitSubject(CommitMessage),
    ScoutQuestion,
    BaseUrl,
    ModelName,
    ApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingStep {
    LanguageSelection,
    GitDependency(DependencyStatus),
    RepoSetupChoice,
    RemoteSetupChoice,
    ProviderSelection,
    ProviderDependency(DependencyStatus),
    ModelConfiguration,
    ApiKeyConfiguration,
}

fn cycle_language(current: &str, direction: i32) -> &'static str {
    let languages = ["English", "Spanish"];
    let index = languages
        .iter()
        .position(|language| language.eq_ignore_ascii_case(current))
        .unwrap_or(0);
    let next = if direction >= 0 {
        (index + 1) % languages.len()
    } else if index == 0 {
        languages.len() - 1
    } else {
        index - 1
    };
    languages[next]
}

fn intent_busy_message(intent: &Intent, language: &str) -> String {
    let lang = language.to_lowercase();
    let lang_str = lang.trim();
    match lang_str {
        "spanish" | "español" | "espanol" => match intent {
            Intent::Commit => "Generando un plan de commits estructurado...".to_string(),
            Intent::Switch => "Obteniendo ramas...".to_string(),
            Intent::Log => "Cargando historial de commits...".to_string(),
            Intent::Explain => "Explicando cambios...".to_string(),
            Intent::Review => "Revisando cambios...".to_string(),
            Intent::Status => "Analizando estado de cambios...".to_string(),
            Intent::DeprecatedDryRun => "Mostrando aviso de comando eliminado...".to_string(),
            Intent::Push => "Haciendo push de la rama...".to_string(),
            Intent::Pr => "Obteniendo ramas remotas...".to_string(),
            Intent::Setup => "Configurando repositorio...".to_string(),
            Intent::Diff => "Cargando diff...".to_string(),
            Intent::Provider => "Cargando proveedores...".to_string(),
            Intent::Model => "Obteniendo modelos...".to_string(),
            _ => "Pensando...".to_string(),
        },
        _ => match intent {
            Intent::Commit => "Generating a structured commit plan...".to_string(),
            Intent::Switch => "Fetching branches...".to_string(),
            Intent::Log => "Loading commit history...".to_string(),
            Intent::Explain => "Explaining changes...".to_string(),
            Intent::Review => "Reviewing changes...".to_string(),
            Intent::Status => "Analyzing change status...".to_string(),
            Intent::DeprecatedDryRun => "Showing removed command notice...".to_string(),
            Intent::Push => "Pushing branch...".to_string(),
            Intent::Pr => "Fetching remote branches...".to_string(),
            Intent::Setup => "Setting up repository...".to_string(),
            Intent::Diff => "Loading diff...".to_string(),
            Intent::Provider => "Loading providers...".to_string(),
            Intent::Model => "Fetching models...".to_string(),
            _ => "Thinking...".to_string(),
        },
    }
}

pub(crate) fn reset_confirmation_message(language: &str) -> String {
    match language.to_lowercase().trim() {
        "spanish" | "español" | "espanol" => [
            "Esto reinicia Remix Autopilot para volver al setup inicial.",
            "",
            "Se borra: proveedor IA, modelo, base URL, preferencias globales, API keys guardadas y remote origin del repo actual.",
            "",
            "No se borra: .git, commits, ramas, archivos locales ni historial remoto.",
        ]
        .join("\n"),
        _ => [
            "This resets Remix Autopilot back to first-run setup.",
            "",
            "Deleted: AI provider, model, base URL, global preferences, saved API keys, and origin remote for the current repo.",
            "",
            "Not deleted: .git, commits, branches, local files, or remote history.",
        ]
        .join("\n"),
    }
}

fn onboarding_action_count(step: &OnboardingStep) -> usize {
    match step {
        OnboardingStep::LanguageSelection => 2,
        OnboardingStep::GitDependency(issue) | OnboardingStep::ProviderDependency(issue) => {
            onboarding_dependency_actions(issue, "English").len().max(1)
        }
        OnboardingStep::RepoSetupChoice => 2,
        OnboardingStep::RemoteSetupChoice => 2,
        OnboardingStep::ProviderSelection => 1,
        OnboardingStep::ModelConfiguration => 3,
        OnboardingStep::ApiKeyConfiguration => 2,
    }
}

fn intent_requires_ai_provider(intent: &Intent) -> bool {
    matches!(
        intent,
        Intent::Commit
            | Intent::Explain
            | Intent::Review
            | Intent::Status
            | Intent::Pr
            | Intent::Model
    )
}

fn intent_requires_ollama(intent: &Intent) -> bool {
    matches!(
        intent,
        Intent::Commit
            | Intent::Explain
            | Intent::Review
            | Intent::Status
            | Intent::Pr
            | Intent::Model
    )
}

fn is_protected_branch(branch: &str) -> bool {
    matches!(branch.trim(), "main" | "master")
}

fn ollama_health_from_report(report: &DependencyDoctor) -> OllamaHealth {
    match report.ollama.state {
        DependencyState::Ready | DependencyState::NotConfigured => {
            OllamaHealth::ready(report.ollama.version.clone().unwrap_or_default())
        }
        DependencyState::Missing => OllamaHealth::not_installed(
            report
                .ollama
                .detail
                .clone()
                .unwrap_or_else(|| "Ollama is not installed.".to_string()),
        ),
        DependencyState::NotRunning => OllamaHealth::not_running(
            report.ollama.version.clone(),
            report
                .ollama
                .detail
                .clone()
                .unwrap_or_else(|| "Ollama is not responding.".to_string()),
        ),
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

pub(crate) fn onboarding_dependency_actions(
    issue: &DependencyStatus,
    language: &str,
) -> Vec<DependencyModalAction> {
    dependency_modal_actions(issue, false, language)
        .into_iter()
        .filter(|action| {
            !matches!(
                action.action,
                DependencyModalActionKind::Close | DependencyModalActionKind::ExitCli
            )
        })
        .collect()
}

fn dependency_modal_actions(
    issue: &DependencyStatus,
    blocking: bool,
    language: &str,
) -> Vec<DependencyModalAction> {
    let mut actions = Vec::new();

    if let Some(recovery) = issue.recovery_action() {
        let manual_copy = if !recovery.runnable_in_cli {
            recovery
                .manual_command
                .clone()
                .or_else(|| recovery.command.as_ref().map(|spec| spec.display.clone()))
        } else {
            None
        };
        actions.push(DependencyModalAction {
            label: localized_dependency_action_button(&recovery, language),
            action: DependencyModalActionKind::RunRecovery(recovery),
        });
        if let Some(command) = manual_copy {
            actions.push(DependencyModalAction {
                label: localized_copy_command_button(language),
                action: DependencyModalActionKind::CopyCommand(command),
            });
        }
    }

    actions.push(DependencyModalAction {
        label: translate("Retry", language),
        action: DependencyModalActionKind::RetryOnly,
    });

    actions.push(DependencyModalAction {
        label: if blocking {
            translate("Exit CLI", language)
        } else {
            translate("Close", language)
        },
        action: if blocking {
            DependencyModalActionKind::ExitCli
        } else {
            DependencyModalActionKind::Close
        },
    });

    actions
}

fn localized_copy_command_button(language: &str) -> String {
    if is_spanish(language) {
        "Copiar comando".to_string()
    } else {
        "Copy command".to_string()
    }
}

fn localized_dependency_action_button(action: &DependencyAction, language: &str) -> String {
    let is_spanish = is_spanish(language);
    match action.kind {
        DependencyActionKind::Install if action.runnable_in_cli => {
            if is_spanish {
                "Instalar ahora".to_string()
            } else {
                "Run install now".to_string()
            }
        }
        DependencyActionKind::Install => {
            if is_spanish {
                "Ver comando de instalación".to_string()
            } else {
                "Show install command".to_string()
            }
        }
        DependencyActionKind::StartService => {
            if is_spanish {
                "Iniciar Ollama ahora".to_string()
            } else {
                "Start Ollama now".to_string()
            }
        }
        DependencyActionKind::PullModel => {
            if is_spanish {
                "Descargar modelo ahora".to_string()
            } else {
                "Pull model now".to_string()
            }
        }
        DependencyActionKind::ManualAuth => {
            if is_spanish {
                "Ver comando de autenticación".to_string()
            } else {
                "Show auth command".to_string()
            }
        }
    }
}

fn localized_dependency_action_title(action: &DependencyAction, language: &str) -> String {
    let is_spanish = is_spanish(language);
    match action.kind {
        DependencyActionKind::Install => {
            if is_spanish {
                "Instalación de dependencia".to_string()
            } else {
                "Dependency installation".to_string()
            }
        }
        DependencyActionKind::StartService => {
            if is_spanish {
                "Inicio de servicio".to_string()
            } else {
                "Service startup".to_string()
            }
        }
        DependencyActionKind::PullModel => {
            if is_spanish {
                "Descarga de modelo".to_string()
            } else {
                "Model download".to_string()
            }
        }
        DependencyActionKind::ManualAuth => {
            if is_spanish {
                "Paso manual requerido".to_string()
            } else {
                "Manual step required".to_string()
            }
        }
    }
}

fn localized_dependency_busy_message(action: &DependencyAction, language: &str) -> String {
    let is_spanish = is_spanish(language);
    match action.kind {
        DependencyActionKind::Install => {
            if is_spanish {
                "Instalando dependencia...".to_string()
            } else {
                "Installing dependency...".to_string()
            }
        }
        DependencyActionKind::StartService => {
            if is_spanish {
                "Iniciando servicio...".to_string()
            } else {
                "Starting service...".to_string()
            }
        }
        DependencyActionKind::PullModel => {
            if is_spanish {
                "Descargando modelo...".to_string()
            } else {
                "Pulling model...".to_string()
            }
        }
        DependencyActionKind::ManualAuth => {
            if is_spanish {
                "Esperando autenticación manual...".to_string()
            } else {
                "Waiting for manual authentication...".to_string()
            }
        }
    }
}

fn manual_dependency_logs(
    issue: &DependencyStatus,
    action: &DependencyAction,
    language: &str,
) -> Vec<String> {
    let mut logs = Vec::new();
    if let Some(command) = action
        .manual_command
        .clone()
        .or_else(|| action.command.as_ref().map(|spec| spec.display.clone()))
    {
        logs.push(format!("$ {command}"));
    }

    if is_spanish(language) {
        logs.push(
            "Este paso necesita otra terminal o interacción manual. Ejecutalo afuera del TUI y después volvé con Reintentar."
                .to_string(),
        );
        if action.requires_elevation {
            logs.push(
                "La instalación dentro del CLI está deshabilitada porque esta sesión no está elevada."
                    .to_string(),
            );
        }
        if let Some(url) = issue.fallback_url {
            logs.push(format!("URL alternativa: {url}"));
        }
    } else {
        logs.push(
            "This step needs another terminal or manual interaction. Run it outside the TUI, then come back and use Retry."
                .to_string(),
        );
        if action.requires_elevation {
            logs.push(
                "In-CLI installation is disabled because this session is not elevated.".to_string(),
            );
        }
        if let Some(url) = issue.fallback_url {
            logs.push(format!("Fallback URL: {url}"));
        }
    }

    logs
}

async fn run_dependency_action_outcome(
    core: AppCore,
    tx: mpsc::Sender<AppEvent>,
    issue: DependencyStatus,
    action: DependencyAction,
    blocking: bool,
    resume_intent: Option<Intent>,
    managed_ollama_pid: Arc<Mutex<Option<u32>>>,
) -> AsyncOutcome {
    let execution = match action.kind {
        DependencyActionKind::Install | DependencyActionKind::PullModel => {
            run_inline_dependency_command(&tx, &action).await
        }
        DependencyActionKind::StartService => {
            start_dependency_service(&core, &tx, &action, managed_ollama_pid).await
        }
        DependencyActionKind::ManualAuth => Ok(()),
    };

    if let Err(error) = &execution {
        let _ = tx
            .send(AppEvent::AsyncOutcome(Box::new(
                AsyncOutcome::DependencyCommandOutput(format!("Error: {error}")),
            )))
            .await;
    }

    let report = core.dependency_doctor().await;
    let refreshed_issue = report.status(issue.kind).clone();

    let final_line = if refreshed_issue.is_ready() {
        "Dependency is ready."
    } else {
        "Dependency is still unavailable."
    };
    let _ = tx
        .send(AppEvent::AsyncOutcome(Box::new(
            AsyncOutcome::DependencyCommandOutput(final_line.to_string()),
        )))
        .await;

    AsyncOutcome::DependencyCommandFinished {
        report,
        issue: refreshed_issue,
        blocking,
        resume_intent,
        succeeded: execution.is_ok(),
    }
}

async fn run_inline_dependency_command(
    tx: &mpsc::Sender<AppEvent>,
    action: &DependencyAction,
) -> Result<()> {
    let Some(command) = action.command.as_ref() else {
        return Err(AppError::Custom(
            "No runnable command is available for this dependency action.".to_string(),
        ));
    };

    let mut child = TokioCommand::new(&command.program);
    child
        .args(&command.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);

    let mut child = child.spawn().map_err(|error| {
        AppError::Custom(format!("Failed to start `{}`: {}", command.display, error))
    })?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let stdout_task = tokio::spawn(stream_command_output(tx.clone(), stdout));
    let stderr_task = tokio::spawn(stream_command_output(tx.clone(), stderr));

    let status = child.wait().await.map_err(|error| {
        AppError::Custom(format!(
            "Failed while waiting for `{}`: {}",
            command.display, error
        ))
    })?;

    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if status.success() {
        Ok(())
    } else {
        Err(AppError::Custom(format!(
            "`{}` exited with status {}",
            command.display, status
        )))
    }
}

async fn stream_command_output(
    tx: mpsc::Sender<AppEvent>,
    stream: Option<impl tokio::io::AsyncRead + Unpin>,
) {
    let Some(stream) = stream else {
        return;
    };
    let mut reader = BufReader::new(stream).lines();
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                let _ = tx
                    .send(AppEvent::AsyncOutcome(Box::new(
                        AsyncOutcome::DependencyCommandOutput(line),
                    )))
                    .await;
            }
            Ok(None) => break,
            Err(error) => {
                let _ = tx
                    .send(AppEvent::AsyncOutcome(Box::new(
                        AsyncOutcome::DependencyCommandOutput(format!("Stream error: {error}")),
                    )))
                    .await;
                break;
            }
        }
    }
}

async fn start_dependency_service(
    core: &AppCore,
    tx: &mpsc::Sender<AppEvent>,
    action: &DependencyAction,
    managed_ollama_pid: Arc<Mutex<Option<u32>>>,
) -> Result<()> {
    let Some(command) = action.command.as_ref() else {
        return Err(AppError::Custom(
            "No service command is available for this dependency action.".to_string(),
        ));
    };

    let _ = tx
        .send(AppEvent::AsyncOutcome(Box::new(
            AsyncOutcome::DependencyCommandOutput(format!("Starting `{}`...", command.display)),
        )))
        .await;

    let child = TokioCommand::new(&command.program)
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            AppError::Custom(format!("Failed to start `{}`: {}", command.display, error))
        })?;

    if let Some(pid) = child.id()
        && let Ok(mut slot) = managed_ollama_pid.lock()
    {
        *slot = Some(pid);
    }

    for attempt in 1..=15 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let report = core.dependency_doctor().await;
        if report.ollama.is_ready() {
            let _ = tx
                .send(AppEvent::AsyncOutcome(Box::new(
                    AsyncOutcome::DependencyCommandOutput("Ollama is responding.".to_string()),
                )))
                .await;
            return Ok(());
        }
        if attempt < 15 {
            let _ = tx
                .send(AppEvent::AsyncOutcome(Box::new(
                    AsyncOutcome::DependencyCommandOutput(
                        "Waiting for Ollama to become ready...".to_string(),
                    ),
                )))
                .await;
        }
    }

    Err(AppError::Custom(
        "Ollama did not become ready after starting the service.".to_string(),
    ))
}

fn is_spanish(language: &str) -> bool {
    matches!(
        language.to_lowercase().trim(),
        "spanish" | "español" | "espanol"
    )
}

fn shutdown_managed_ollama(pid_slot: &Arc<Mutex<Option<u32>>>) {
    let pid = match pid_slot.lock() {
        Ok(mut slot) => slot.take(),
        Err(_) => None,
    };
    if let Some(pid) = pid {
        terminate_process(pid);
    }
}

fn terminate_process(pid: u32) {
    if pid == 0 {
        return;
    }

    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

async fn switch_branches_outcome(core: AppCore) -> AsyncOutcome {
    match core.switch_branches_async().await {
        Ok(branches) => AsyncOutcome::SwitchBranchesReady(branches),
        Err(error) => AsyncOutcome::Error(error),
    }
}

fn commit_plan_text_lines(plan: &CommitPlan) -> usize {
    commit_plan_wrapped_lines(plan, usize::MAX / 2)
}

fn commit_plan_wrapped_lines(plan: &CommitPlan, width: usize) -> usize {
    let width = width.max(1);
    let mut lines = 2usize;
    for group in &plan.groups {
        lines += wrapped_line_count(&group.commit.title(), width);
        if !group.commit.body.trim().is_empty() {
            lines += wrapped_line_count(group.commit.body.trim(), width);
        }
        if !group.rationale.trim().is_empty() {
            lines += wrapped_line_count(group.rationale.trim(), width);
        }
        lines += 1;
        for file in &group.files {
            lines += wrapped_line_count(
                &format!("{} ({}) - {}", file.path, file.status, file.description),
                width,
            );
        }
        lines += 1;
    }
    lines
}

fn wrapped_line_count(line: &str, width: usize) -> usize {
    let len = line.chars().count();
    len.div_ceil(width).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Config;
    use crate::domain::commit::CommitGroup;
    use crate::infrastructure::dependencies::PlatformOs;
    use crate::infrastructure::{
        BranchOption, BranchSource, DependencyStatus, PackageManager, PlatformInfo, SwitchBranches,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};
    use reqwest::Client;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::tempdir;

    fn make_app() -> TuiApp {
        make_app_in(PathBuf::from("."))
    }

    fn make_app_in(cwd: PathBuf) -> TuiApp {
        let config = Config::default();
        let core = AppCore::new(cwd, config, Client::new());
        TuiApp::new(core)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn git_in(path: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(path)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    #[tokio::test]
    async fn history_keyboard_scrolls_when_no_modal_or_suggestions() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.push_assistant("line 1\nline 2\nline 3");

        app.handle_key(key(KeyCode::PageUp), tx.clone())
            .await
            .unwrap();
        assert_eq!(app.history_scroll, 8);

        app.handle_key(key(KeyCode::PageDown), tx.clone())
            .await
            .unwrap();
        assert_eq!(app.history_scroll, 0);

        app.handle_key(key(KeyCode::Up), tx.clone()).await.unwrap();
        assert_eq!(app.history_scroll, 1);

        app.handle_key(key(KeyCode::Down), tx.clone())
            .await
            .unwrap();
        assert_eq!(app.history_scroll, 0);

        app.handle_key(key(KeyCode::Home), tx.clone())
            .await
            .unwrap();
        assert_eq!(app.history_scroll, 1_000);

        app.handle_key(key(KeyCode::End), tx).await.unwrap();
        assert_eq!(app.history_scroll, 0);
    }

    #[tokio::test]
    async fn arrow_keys_keep_navigating_slash_suggestions() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.input = "/".to_string();
        app.refresh_suggestions();

        app.handle_key(key(KeyCode::Down), tx.clone())
            .await
            .unwrap();

        assert_eq!(app.history_scroll, 0);
        assert_eq!(app.selected_suggestion, 1);

        app.handle_key(key(KeyCode::Up), tx).await.unwrap();

        assert_eq!(app.history_scroll, 0);
        assert_eq!(app.selected_suggestion, 0);
    }

    fn mock_switch_branches() -> SwitchBranches {
        SwitchBranches {
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
        }
    }

    fn commit_plan_with_groups(count: usize) -> CommitPlan {
        let groups = (0..count)
            .map(|index| CommitGroup {
                commit: CommitMessage {
                    commit_type: "fix".to_string(),
                    scope: "tui".to_string(),
                    subject: format!("update modal {}", index + 1),
                    body: String::new(),
                },
                files: vec![FileEntry {
                    id: format!("src/file{}.rs", index + 1),
                    path: format!("src/file{}.rs", index + 1),
                    status: "modified".to_string(),
                    description: "test file".to_string(),
                    patch: None,
                }],
                rationale: "test group".to_string(),
            })
            .collect();

        CommitPlan {
            summary: "test plan".to_string(),
            groups,
        }
    }

    fn platform() -> PlatformInfo {
        PlatformInfo {
            os: PlatformOs::Windows,
            package_manager: Some(PackageManager::Winget),
            is_elevated: true,
        }
    }

    #[tokio::test]
    async fn ollama_health_update_does_not_unlock_busy_input() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.busy = true;
        app.busy_message = "Generating commit plan...".to_string();

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::OllamaStatus(OllamaHealth::ready(
                "0.9.0".to_string(),
            )))),
            &tx,
        )
        .await
        .unwrap();
        app.handle_key(key(KeyCode::Char('/')), tx).await.unwrap();

        assert!(app.busy);
        assert!(app.input.is_empty());
        assert!(app.history.is_empty());
    }

    #[tokio::test]
    async fn internet_status_update_does_not_unlock_busy_input() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.busy = true;

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::InternetStatus(true))),
            &tx,
        )
        .await
        .unwrap();
        app.handle_key(key(KeyCode::Char('/')), tx).await.unwrap();

        assert!(app.busy);
        assert!(app.input.is_empty());
    }

    #[tokio::test]
    async fn operation_error_unlocks_input_after_busy() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.busy = true;

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::Error(AppError::NoChanges))),
            &tx,
        )
        .await
        .unwrap();
        app.handle_key(key(KeyCode::Char('/')), tx).await.unwrap();

        assert!(!app.busy);
        assert_eq!(app.input, "/");
    }

    #[tokio::test]
    async fn commit_plan_ready_unlocks_input_and_opens_review_modal() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.busy = true;

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::CommitPlanReady(
                commit_plan_with_groups(2),
                LlmContextUsage {
                    estimated_tokens: 120,
                    limit: 1000,
                    truncated: false,
                },
            ))),
            &tx,
        )
        .await
        .unwrap();

        assert!(!app.busy);
        assert!(matches!(app.modal, Some(Modal::CommitPlanReview { .. })));
        assert_eq!(
            app.last_context_usage,
            Some(LlmContextUsage {
                estimated_tokens: 120,
                limit: 1000,
                truncated: false,
            })
        );
    }

    #[tokio::test]
    async fn scout_commit_plan_ready_opens_review_modal_without_pseudo_options() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.execution_mode = ExecutionMode::Scout;
        app.busy = true;

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::CommitPlanReady(
                commit_plan_with_groups(2),
                LlmContextUsage {
                    estimated_tokens: 120,
                    limit: 1000,
                    truncated: false,
                },
            ))),
            &tx,
        )
        .await
        .unwrap();

        assert!(!app.busy);
        assert!(matches!(app.modal, Some(Modal::CommitPlanReview { .. })));
        assert!(!app.scout_pending);
        assert!(app.pending_modal.is_none());
        assert!(!app.history.iter().any(|entry| {
            entry.message.contains("Scout Options") || entry.message.contains("Opciones de Scout")
        }));
    }

    #[tokio::test]
    async fn scout_stream_end_opens_real_decision_modal() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.execution_mode = ExecutionMode::Scout;
        app.busy = true;

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::StreamEnd)),
            &tx,
        )
        .await
        .unwrap();

        assert!(!app.busy);
        assert!(matches!(app.modal, Some(Modal::ScoutDecision { .. })));
        assert!(!app.scout_pending);
        assert!(app.pending_modal.is_none());
        assert!(!app.history.iter().any(|entry| {
            entry.message.contains("Scout Options") || entry.message.contains("Opciones de Scout")
        }));
    }

    #[tokio::test]
    async fn escape_from_scout_decision_closes_real_modal() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.execution_mode = ExecutionMode::Scout;
        app.modal = Some(Modal::ScoutDecision { selected: 0 });
        app.pending_modal = Some(Modal::ScoutDecision { selected: 0 });
        app.scout_pending = true;

        app.handle_key(key(KeyCode::Esc), tx).await.unwrap();

        assert!(app.modal.is_none());
        assert!(!app.scout_pending);
        assert!(app.pending_modal.is_none());
        assert!(
            app.history
                .iter()
                .any(|entry| entry.message.contains("Scout session closed"))
        );
    }

    #[tokio::test]
    async fn operation_task_panic_unlocks_input_with_visible_error() {
        let mut app = make_app();
        let (tx, mut rx) = mpsc::channel(4);
        app.busy = true;
        app.spawn_outcome_task(tx.clone(), async {
            panic!("simulated commit task panic");
        });

        let event = rx.recv().await.unwrap();
        app.process_event(event, &tx).await.unwrap();

        assert!(!app.busy);
        assert!(app.in_flight_abort.is_none());
        assert!(app.history.iter().any(|entry| {
            entry.role == ChatRole::Error
                && entry
                    .message
                    .contains("Operation task failed before returning a result")
        }));
    }

    #[tokio::test]
    #[ignore = "manual smoke test against live Ollama and the current repository"]
    async fn live_commit_command_reaches_visible_terminal_state() {
        let mut app = make_app();
        let (tx, mut rx) = mpsc::channel(4);
        app.ollama_health = Some(OllamaHealth::ready("test".to_string()));

        app.execute_intent(Intent::Commit, tx.clone());

        let event =
            tokio::time::timeout(Duration::from_secs(COMMIT_PLAN_TIMEOUT_SECS + 5), rx.recv())
                .await
                .expect("timed out waiting for /commit outcome")
                .expect("channel closed before /commit outcome");

        app.process_event(event, &tx).await.unwrap();

        assert!(!app.busy);
        let reached_visible_state = matches!(app.modal, Some(Modal::CommitPlanReview { .. }))
            || app.history.iter().any(|entry| {
                matches!(
                    entry.role,
                    ChatRole::Error | ChatRole::Assistant | ChatRole::System
                ) && !entry.message.trim().is_empty()
            });
        assert!(
            reached_visible_state,
            "/commit finished without opening a modal or writing a visible message"
        );
    }

    #[tokio::test]
    async fn commit_on_main_fetches_branches_without_blocking_modal_thread() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        git_in(
            dir.path(),
            &["remote", "add", "origin", "https://example.com/repo.git"],
        );
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);
        app.ollama_health = Some(OllamaHealth::ready("0.9.0".to_string()));

        app.execute_intent(Intent::Commit, tx);

        assert!(app.busy);
        assert_eq!(app.busy_message, "Fetching branches...");
        assert!(app.modal.is_none());
        assert_eq!(app.blocked_intent, Some(Intent::Commit));
    }

    #[tokio::test]
    async fn protected_branch_switch_branches_outcome_opens_protected_modal() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);
        app.blocked_intent = Some(Intent::Commit);

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::SwitchBranchesReady(
                mock_switch_branches(),
            ))),
            &tx,
        )
        .await
        .unwrap();

        assert!(!app.busy);
        let Some(Modal::ProtectedBranchCommit {
            branch,
            selected,
            editing_new_branch,
            ..
        }) = app.modal
        else {
            panic!("expected protected-branch modal");
        };
        assert_eq!(branch, "main");
        assert_eq!(selected, 0);
        assert!(!editing_new_branch);
    }

    #[tokio::test]
    async fn protected_branch_empty_branch_list_focuses_create_branch() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);
        app.blocked_intent = Some(Intent::Commit);

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::SwitchBranchesReady(
                SwitchBranches::default(),
            ))),
            &tx,
        )
        .await
        .unwrap();

        let Some(Modal::ProtectedBranchCommit {
            selected,
            editing_new_branch,
            ..
        }) = app.modal
        else {
            panic!("expected protected-branch modal");
        };
        assert_eq!(selected, 1);
        assert!(editing_new_branch);
    }

    #[tokio::test]
    async fn protected_branch_escape_from_create_input_returns_to_actions() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.modal = Some(Modal::ProtectedBranchCommit {
            branch: "main".to_string(),
            branches: mock_switch_branches(),
            selected: 1,
            new_branch: "feature/refactor".to_string(),
            editing_new_branch: true,
        });

        app.handle_key(key(KeyCode::Esc), tx).await.unwrap();

        let Some(Modal::ProtectedBranchCommit {
            selected,
            new_branch,
            editing_new_branch,
            ..
        }) = app.modal
        else {
            panic!("expected protected-branch modal");
        };
        assert_eq!(selected, 1);
        assert_eq!(new_branch, "feature/refactor");
        assert!(!editing_new_branch);
        assert!(app.running);
    }

    #[tokio::test]
    async fn branch_switch_escape_from_protected_commit_returns_to_protected_modal() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        let branches = mock_switch_branches();
        let branch_count = branches.total_count();
        app.blocked_intent = Some(Intent::Commit);
        app.modal = Some(Modal::BranchSwitch {
            branches,
            selected: 1,
        });

        app.handle_key(key(KeyCode::Esc), tx).await.unwrap();

        let Some(Modal::ProtectedBranchCommit {
            selected,
            branches,
            editing_new_branch,
            ..
        }) = app.modal
        else {
            panic!("expected protected-branch modal");
        };
        assert_eq!(selected, 0);
        assert_eq!(branches.total_count(), branch_count);
        assert!(!editing_new_branch);
        assert_eq!(app.blocked_intent, Some(Intent::Commit));
    }

    #[tokio::test]
    async fn protected_branch_continue_starts_commit_plan_flow() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);
        app.ollama_health = Some(OllamaHealth::ready("0.9.0".to_string()));

        app.modal = Some(Modal::ProtectedBranchCommit {
            branch: "main".to_string(),
            branches: SwitchBranches::default(),
            selected: 2,
            new_branch: String::new(),
            editing_new_branch: false,
        });

        app.activate_modal(tx).await.unwrap();

        assert!(app.busy);
        assert_eq!(app.busy_message, "Generating a structured commit plan...");
    }

    #[tokio::test]
    async fn switch_intent_is_available_even_when_ollama_is_offline() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);
        app.ollama_health = Some(OllamaHealth::not_running(
            Some("0.9.0".to_string()),
            "down".to_string(),
        ));

        app.execute_intent(Intent::Switch, tx);

        assert!(app.busy);
        assert!(!matches!(app.modal, Some(Modal::DependencyIssue { .. })));
    }

    #[tokio::test]
    async fn switch_branches_outcome_opens_branch_switch_modal() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::SwitchBranchesReady(
                mock_switch_branches(),
            ))),
            &tx,
        )
        .await
        .unwrap();

        let Some(Modal::BranchSwitch { selected, .. }) = app.modal else {
            panic!("expected branch-switch modal");
        };
        assert_eq!(selected, 0);
    }

    #[test]
    fn dependency_issue_prefers_recovery_action_before_retry() {
        let issue = DependencyStatus::ollama_not_running(
            &platform(),
            Some("0.9.0".to_string()),
            "down".to_string(),
        );

        let actions = dependency_modal_actions(&issue, false, "English");

        assert_eq!(actions.len(), 3);
        assert_eq!(actions[0].label, "Start Ollama now");
        assert_eq!(actions[1].label, "Retry");
        assert_eq!(actions[2].label, "Close");
    }

    #[test]
    fn dependency_issue_manual_actions_include_copy_command() {
        let issue = DependencyStatus::gh_auth_missing(&platform(), Some("2.60".to_string()));

        let actions = dependency_modal_actions(&issue, false, "English");

        assert_eq!(actions.len(), 4);
        assert_eq!(actions[0].label, "Show auth command");
        assert_eq!(actions[1].label, "Copy command");
        assert_eq!(actions[2].label, "Retry");
        assert_eq!(actions[3].label, "Close");
    }

    #[tokio::test]
    async fn dependency_recovery_action_opens_command_execution_modal() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        let issue = DependencyStatus::ollama_not_running(
            &platform(),
            Some("0.9.0".to_string()),
            "down".to_string(),
        );
        app.modal = Some(Modal::DependencyIssue {
            issue: issue.clone(),
            actions: dependency_modal_actions(&issue, false, "English"),
            selected: 0,
            blocking: false,
            notice: None,
        });

        app.activate_modal(tx).await.unwrap();

        assert!(app.busy);
        let Some(Modal::CommandExecution { command, .. }) = app.modal else {
            panic!("expected command execution modal");
        };
        assert_eq!(command, "ollama serve");
    }

    #[tokio::test]
    async fn command_execution_enter_returns_to_dependency_issue() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        let issue = DependencyStatus::ollama_not_running(
            &platform(),
            Some("0.9.0".to_string()),
            "down".to_string(),
        );
        app.modal = Some(Modal::CommandExecution {
            title: "Service startup".to_string(),
            command: "ollama serve".to_string(),
            logs: vec!["$ ollama serve".to_string()],
            issue: issue.clone(),
            blocking: false,
        });

        app.activate_modal(tx).await.unwrap();

        let Some(Modal::DependencyIssue { issue: current, .. }) = app.modal else {
            panic!("expected dependency issue modal");
        };
        assert_eq!(current.kind, issue.kind);
        assert_eq!(current.state, issue.state);
    }

    #[tokio::test]
    async fn dependency_command_success_resumes_blocked_intent() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "feature/test"]);
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);
        app.blocked_intent = Some(Intent::Switch);
        app.modal = Some(Modal::CommandExecution {
            title: "Service startup".to_string(),
            command: "ollama serve".to_string(),
            logs: vec!["$ ollama serve".to_string()],
            issue: DependencyStatus::ollama_not_running(
                &platform(),
                Some("0.9.0".to_string()),
                "down".to_string(),
            ),
            blocking: false,
        });

        let ready_report = DependencyDoctor {
            platform: platform(),
            git: DependencyStatus::ready(
                DependencyKind::Git,
                &platform(),
                Some("2.47".to_string()),
            ),
            llm_provider: DependencyStatus::ready(
                DependencyKind::LlmProvider,
                &platform(),
                Some("ok".to_string()),
            ),
            ollama: DependencyStatus::ready(
                DependencyKind::Ollama,
                &platform(),
                Some("0.9.0".to_string()),
            ),
            gh: DependencyStatus::ready(
                DependencyKind::GitHubCli,
                &platform(),
                Some("2.60".to_string()),
            ),
        };

        app.finish_dependency_command(
            ready_report,
            DependencyStatus::ready(
                DependencyKind::Ollama,
                &platform(),
                Some("0.9.0".to_string()),
            ),
            false,
            Some(Intent::Switch),
            true,
            tx,
        );

        assert!(app.busy);
        assert_eq!(app.busy_message, "Fetching branches...");
        assert!(app.blocked_intent.is_none());
    }

    #[tokio::test]
    async fn degraded_dependency_doctor_does_not_push_startup_history_messages() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        let doctor = DependencyDoctor {
            platform: platform(),
            git: DependencyStatus::ready(
                DependencyKind::Git,
                &platform(),
                Some("2.47".to_string()),
            ),
            llm_provider: DependencyStatus::llm_provider_not_running(
                &platform(),
                "down".to_string(),
                Some("https://ollama.com/download"),
            ),
            ollama: DependencyStatus::ollama_not_running(
                &platform(),
                Some("0.9.0".to_string()),
                "down".to_string(),
            ),
            gh: DependencyStatus::gh_auth_missing(&platform(), Some("2.60".to_string())),
        };

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::DependencyDoctor(doctor))),
            &tx,
        )
        .await
        .unwrap();

        assert!(app.history.is_empty());
    }

    #[tokio::test]
    async fn resolve_intent_opens_pending_github_auth_issue() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.onboarding_language_confirmed = true;
        app.onboarding_remote_deferred = true;
        app.core.config.provider = LlmProviderKind::Ollama;
        app.core.config.model = Some("qwen3.5:latest".to_string());
        app.dependency_doctor = Some(DependencyDoctor {
            platform: platform(),
            git: DependencyStatus::ready(
                DependencyKind::Git,
                &platform(),
                Some("2.47".to_string()),
            ),
            llm_provider: DependencyStatus::ready(
                DependencyKind::LlmProvider,
                &platform(),
                Some("ok".to_string()),
            ),
            ollama: DependencyStatus::ready(
                DependencyKind::Ollama,
                &platform(),
                Some("0.9.0".to_string()),
            ),
            gh: DependencyStatus::gh_auth_missing(&platform(), Some("2.60".to_string())),
        });
        app.input = "/resolve".to_string();

        app.handle_key(key(KeyCode::Enter), tx).await.unwrap();

        let Some(Modal::DependencyIssue { issue, .. }) = app.modal else {
            panic!("expected dependency issue modal");
        };
        assert_eq!(issue.kind, DependencyKind::GitHubCli);
        assert_eq!(issue.state, DependencyState::NotConfigured);
    }

    #[tokio::test]
    async fn resolve_intent_reports_when_no_pending_issues() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.onboarding_language_confirmed = true;
        app.onboarding_remote_deferred = true;
        app.core.config.provider = LlmProviderKind::Ollama;
        app.core.config.model = Some("qwen3.5:latest".to_string());
        app.dependency_doctor = Some(DependencyDoctor {
            platform: platform(),
            git: DependencyStatus::ready(
                DependencyKind::Git,
                &platform(),
                Some("2.47".to_string()),
            ),
            llm_provider: DependencyStatus::ready(
                DependencyKind::LlmProvider,
                &platform(),
                Some("ok".to_string()),
            ),
            ollama: DependencyStatus::ready(
                DependencyKind::Ollama,
                &platform(),
                Some("0.9.0".to_string()),
            ),
            gh: DependencyStatus::ready(
                DependencyKind::GitHubCli,
                &platform(),
                Some("2.60".to_string()),
            ),
        });
        app.input = "/resolve".to_string();

        app.handle_key(key(KeyCode::Enter), tx).await.unwrap();

        assert!(app.modal.is_none());
        assert!(
            app.history.iter().any(
                |entry| entry.role == ChatRole::System && entry.message == "No pending issues."
            )
        );
    }

    #[tokio::test]
    async fn scout_pr_draft_opens_real_pr_modal_without_pseudo_options() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.execution_mode = ExecutionMode::Scout;
        app.busy = true;

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::PrDraft(
                "main".to_string(),
                PullRequestDraft {
                    title: "Update TUI".to_string(),
                    body: "Body".to_string(),
                },
            ))),
            &tx,
        )
        .await
        .unwrap();

        assert!(!app.busy);
        assert!(matches!(app.modal, Some(Modal::PrDraft { .. })));
        assert!(!app.scout_pending);
        assert!(app.pending_modal.is_none());
        assert!(!app.history.iter().any(|entry| {
            entry.message.contains("Scout Options") || entry.message.contains("Opciones de Scout")
        }));
    }

    #[test]
    fn shutdown_managed_ollama_clears_tracked_pid_slot() {
        let pid_slot = Arc::new(Mutex::new(Some(0)));

        shutdown_managed_ollama(&pid_slot);

        assert!(pid_slot.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn branch_switch_selecting_current_branch_pushes_noop_message() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.modal = Some(Modal::BranchSwitch {
            branches: mock_switch_branches(),
            selected: 2,
        });

        app.activate_modal(tx).await.unwrap();

        assert!(app.history.iter().any(|entry| {
            entry.role == ChatRole::Assistant
                && entry.message.contains("Already on branch `feature/api`.")
        }));
    }

    #[test]
    fn push_completed_message_explicitly_names_branch_and_git_output() {
        let message = push_completed_message(
            "feature/ui",
            "To github.com:owner/repo.git\n   abc..def  feature/ui -> feature/ui",
            "English",
        );

        assert!(
            message.contains("Push completed for branch `feature/ui`."),
            "{message}"
        );
        assert!(message.contains("Git output:"), "{message}");
        assert!(message.contains("feature/ui -> feature/ui"), "{message}");
    }

    #[test]
    fn push_completed_message_is_localized_in_spanish() {
        let message = push_completed_message("main", "", "Spanish");

        assert_eq!(message, "Push completado para la rama `main`.");
    }

    #[tokio::test]
    async fn push_completed_outcome_adds_explicit_system_message() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "feature/ui"]);
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::PushCompleted(
                "Everything up-to-date".to_string(),
            ))),
            &tx,
        )
        .await
        .unwrap();

        assert!(app.history.iter().any(|entry| {
            entry.role == ChatRole::System
                && entry
                    .message
                    .contains("Push completed for branch `feature/ui`.")
                && entry.message.contains("Git output:")
                && entry.message.contains("Everything up-to-date")
        }));
    }

    #[tokio::test]
    async fn commit_plan_modal_arrows_cycle_actions_without_scrolling() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.modal = Some(Modal::CommitPlanReview {
            plan: commit_plan_with_groups(9),
            selected: 0,
            scroll: 0,
        });

        app.handle_key(key(KeyCode::Down), tx.clone())
            .await
            .unwrap();
        app.handle_key(key(KeyCode::Down), tx).await.unwrap();

        let Some(Modal::CommitPlanReview {
            selected, scroll, ..
        }) = app.modal
        else {
            panic!("expected commit plan modal");
        };
        assert_eq!(selected, 2);
        assert_eq!(scroll, 0);
    }

    #[tokio::test]
    async fn commit_plan_modal_tab_cycles_actions() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.modal = Some(Modal::CommitPlanReview {
            plan: commit_plan_with_groups(9),
            selected: 0,
            scroll: 3,
        });

        app.handle_key(key(KeyCode::Tab), tx.clone()).await.unwrap();
        app.handle_key(key(KeyCode::Tab), tx).await.unwrap();

        let Some(Modal::CommitPlanReview {
            selected, scroll, ..
        }) = app.modal
        else {
            panic!("expected commit plan modal");
        };
        assert_eq!(selected, 2);
        assert_eq!(scroll, 3);
    }

    #[tokio::test]
    async fn commit_plan_modal_mouse_wheel_scroll_is_clamped() {
        let mut app = make_app();
        app.modal = Some(Modal::CommitPlanReview {
            plan: commit_plan_with_groups(6),
            selected: 0,
            scroll: 0,
        });

        for _ in 0..200 {
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            });
        }
        let max_scroll = app.commit_plan_max_scroll();

        let Some(Modal::CommitPlanReview {
            selected, scroll, ..
        }) = app.modal
        else {
            panic!("expected commit plan modal");
        };
        assert_eq!(selected, 0);
        assert_eq!(scroll, max_scroll);

        app.handle_mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let Some(Modal::CommitPlanReview { scroll, .. }) = app.modal else {
            panic!("expected commit plan modal");
        };
        assert!(scroll < max_scroll);
    }

    #[tokio::test]
    async fn config_intent_opens_interactive_settings_modal() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        git_in(
            dir.path(),
            &["remote", "add", "origin", "https://example.com/repo.git"],
        );
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);
        app.core.config.provider = LlmProviderKind::Ollama;
        app.core.config.model = Some("qwen3.5:latest".to_string());
        app.input = "/config".to_string();

        app.handle_key(key(KeyCode::Enter), tx).await.unwrap();

        let Some(Modal::Settings { selected }) = app.modal else {
            panic!("expected settings modal");
        };
        assert_eq!(selected, 0);
    }

    #[tokio::test]
    async fn reset_intent_opens_safe_reset_confirmation() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.input = "/reset".to_string();

        app.handle_key(key(KeyCode::Enter), tx).await.unwrap();

        let Some(Modal::Confirm {
            title,
            message,
            selected,
            kind,
        }) = app.modal
        else {
            panic!("expected reset confirmation");
        };
        assert_eq!(title, "Reset configuration");
        assert!(message.contains("Not deleted: .git"));
        assert_eq!(selected, 1);
        assert!(matches!(kind, ConfirmKind::ResetConfiguration));
    }

    #[tokio::test]
    async fn settings_navigation_wraps_from_first_to_last() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.modal = Some(Modal::Settings { selected: 0 });

        app.handle_key(key(KeyCode::Up), tx).await.unwrap();

        let Some(Modal::Settings { selected }) = app.modal else {
            panic!("expected settings modal");
        };
        assert_eq!(selected, SETTINGS_ROWS - 1);
    }

    #[tokio::test]
    async fn provider_warning_opens_setup_modal_automatically() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);
        let doctor = DependencyDoctor {
            platform: platform(),
            git: DependencyStatus::ready(
                DependencyKind::Git,
                &platform(),
                Some("2.47".to_string()),
            ),
            llm_provider: DependencyStatus::llm_provider_not_configured(
                &platform(),
                "missing provider".to_string(),
                None,
            ),
            ollama: DependencyStatus::missing(DependencyKind::Ollama, &platform(), None),
            gh: DependencyStatus::ready(
                DependencyKind::GitHubCli,
                &platform(),
                Some("2.60".to_string()),
            ),
        };

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::DependencyDoctor(doctor))),
            &tx,
        )
        .await
        .unwrap();

        let Some(Modal::OnboardingWizard { .. }) = app.modal else {
            panic!("expected onboarding wizard");
        };
    }

    #[tokio::test]
    async fn onboarding_wizard_ignores_escape() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.onboarding_active = true;
        app.modal = Some(Modal::OnboardingWizard {
            step: OnboardingStep::ProviderSelection,
            selected: 0,
        });

        app.handle_key(key(KeyCode::Esc), tx).await.unwrap();

        assert!(matches!(app.modal, Some(Modal::OnboardingWizard { .. })));
    }

    #[tokio::test]
    async fn onboarding_starts_with_language_when_setup_is_pending() {
        let dir = tempdir().unwrap();
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);
        let platform = platform();
        let doctor = DependencyDoctor {
            platform: platform.clone(),
            git: DependencyStatus::ready(DependencyKind::Git, &platform, Some("2.47".to_string())),
            llm_provider: DependencyStatus::llm_provider_not_configured(
                &platform,
                "missing provider".to_string(),
                None,
            ),
            ollama: DependencyStatus::missing(DependencyKind::Ollama, &platform, None),
            gh: DependencyStatus::ready(
                DependencyKind::GitHubCli,
                &platform,
                Some("2.60".to_string()),
            ),
        };

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::DependencyDoctor(doctor))),
            &tx,
        )
        .await
        .unwrap();

        let Some(Modal::OnboardingWizard { step, selected }) = app.modal else {
            panic!("expected onboarding wizard");
        };
        assert_eq!(step, OnboardingStep::LanguageSelection);
        assert_eq!(selected, 0);
    }

    #[test]
    fn onboarding_opens_before_dependency_doctor_when_local_setup_is_missing() {
        let dir = tempdir().unwrap();
        let mut app = make_app_in(dir.path().to_path_buf());

        app.maybe_open_onboarding();

        let Some(Modal::OnboardingWizard { step, selected }) = app.modal else {
            panic!("expected onboarding wizard");
        };
        assert_eq!(step, OnboardingStep::LanguageSelection);
        assert_eq!(selected, 0);
        assert!(app.dependency_doctor.is_none());
    }

    #[tokio::test]
    async fn onboarding_switches_to_git_dependency_when_doctor_reports_missing_git() {
        let dir = tempdir().unwrap();
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);
        let platform = platform();
        app.onboarding_active = true;
        app.onboarding_language_confirmed = true;
        app.modal = None;

        let doctor = DependencyDoctor {
            platform: platform.clone(),
            git: DependencyStatus::missing(
                DependencyKind::Git,
                &platform,
                Some("missing".to_string()),
            ),
            llm_provider: DependencyStatus::llm_provider_not_configured(
                &platform,
                "missing provider".to_string(),
                None,
            ),
            ollama: DependencyStatus::missing(DependencyKind::Ollama, &platform, None),
            gh: DependencyStatus::ready(
                DependencyKind::GitHubCli,
                &platform,
                Some("2.60".to_string()),
            ),
        };

        app.process_event(
            AppEvent::AsyncOutcome(Box::new(AsyncOutcome::DependencyDoctor(doctor))),
            &tx,
        )
        .await
        .unwrap();

        assert!(matches!(
            app.modal,
            Some(Modal::OnboardingWizard {
                step: OnboardingStep::GitDependency(_),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn onboarding_language_selection_saves_spanish_and_advances() {
        let dir = tempdir().unwrap();
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);
        let platform = platform();
        app.dependency_doctor = Some(DependencyDoctor {
            platform: platform.clone(),
            git: DependencyStatus::ready(DependencyKind::Git, &platform, Some("2.47".to_string())),
            llm_provider: DependencyStatus::llm_provider_not_configured(
                &platform,
                "missing provider".to_string(),
                None,
            ),
            ollama: DependencyStatus::missing(DependencyKind::Ollama, &platform, None),
            gh: DependencyStatus::ready(
                DependencyKind::GitHubCli,
                &platform,
                Some("2.60".to_string()),
            ),
        });
        app.onboarding_active = true;
        app.modal = Some(Modal::OnboardingWizard {
            step: OnboardingStep::LanguageSelection,
            selected: 1,
        });

        app.handle_key(key(KeyCode::Enter), tx).await.unwrap();

        assert_eq!(app.core.config.language, "Spanish");
        assert!(app.onboarding_language_confirmed);
        assert!(app.history.iter().any(|entry| {
            entry.role == ChatRole::System && entry.message == "Idioma cambiado a Spanish"
        }));
        assert!(matches!(
            app.modal,
            Some(Modal::OnboardingWizard {
                step: OnboardingStep::RepoSetupChoice,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn onboarding_navigation_wraps_and_tab_cycles_actions() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.onboarding_active = true;
        app.modal = Some(Modal::OnboardingWizard {
            step: OnboardingStep::LanguageSelection,
            selected: 0,
        });

        app.handle_key(key(KeyCode::Up), tx.clone()).await.unwrap();
        let Some(Modal::OnboardingWizard { selected, .. }) = app.modal else {
            panic!("expected onboarding wizard");
        };
        assert_eq!(selected, 1);

        app.handle_key(key(KeyCode::Tab), tx).await.unwrap();
        let Some(Modal::OnboardingWizard { selected, .. }) = app.modal else {
            panic!("expected onboarding wizard");
        };
        assert_eq!(selected, 0);
    }

    #[tokio::test]
    async fn onboarding_language_selection_uses_vertical_selection_only() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.onboarding_active = true;
        app.modal = Some(Modal::OnboardingWizard {
            step: OnboardingStep::LanguageSelection,
            selected: 0,
        });

        app.handle_key(key(KeyCode::Right), tx.clone())
            .await
            .unwrap();

        assert_eq!(app.core.config.language, "English");
        assert!(matches!(
            app.modal,
            Some(Modal::OnboardingWizard { selected: 0, .. })
        ));

        app.handle_key(key(KeyCode::Down), tx.clone())
            .await
            .unwrap();

        assert!(matches!(
            app.modal,
            Some(Modal::OnboardingWizard { selected: 1, .. })
        ));

        app.handle_key(key(KeyCode::Enter), tx).await.unwrap();

        assert_eq!(app.core.config.language, "Spanish");
    }

    #[tokio::test]
    async fn onboarding_language_selection_matches_spanish_labels() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.core.config.language = "Spanish".to_string();
        app.onboarding_active = true;
        app.modal = Some(Modal::OnboardingWizard {
            step: OnboardingStep::LanguageSelection,
            selected: 0,
        });

        app.handle_key(key(KeyCode::Enter), tx.clone())
            .await
            .unwrap();

        assert_eq!(app.core.config.language, "English");

        app.modal = Some(Modal::OnboardingWizard {
            step: OnboardingStep::LanguageSelection,
            selected: 1,
        });
        app.handle_key(key(KeyCode::Enter), tx).await.unwrap();

        assert_eq!(app.core.config.language, "Spanish");
    }

    #[tokio::test]
    async fn onboarding_escape_returns_to_language_step_without_closing() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.core.config.language = "Spanish".to_string();
        app.onboarding_active = true;
        app.onboarding_language_confirmed = true;
        app.modal = Some(Modal::OnboardingWizard {
            step: OnboardingStep::ProviderSelection,
            selected: 0,
        });

        app.handle_key(key(KeyCode::Esc), tx).await.unwrap();

        let Some(Modal::OnboardingWizard { step, selected }) = app.modal else {
            panic!("expected onboarding wizard");
        };
        assert_eq!(step, OnboardingStep::LanguageSelection);
        assert_eq!(selected, 1);
    }

    #[tokio::test]
    async fn onboarding_escape_from_child_modal_returns_to_wizard() {
        let dir = tempdir().unwrap();
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);
        let platform = platform();
        app.dependency_doctor = Some(DependencyDoctor {
            platform: platform.clone(),
            git: DependencyStatus::ready(DependencyKind::Git, &platform, Some("2.47".to_string())),
            llm_provider: DependencyStatus::llm_provider_not_configured(
                &platform,
                "missing provider".to_string(),
                None,
            ),
            ollama: DependencyStatus::missing(DependencyKind::Ollama, &platform, None),
            gh: DependencyStatus::ready(
                DependencyKind::GitHubCli,
                &platform,
                Some("2.60".to_string()),
            ),
        });
        app.onboarding_active = true;
        app.onboarding_language_confirmed = true;
        app.modal = Some(Modal::TextInput {
            title: "Origin URL".to_string(),
            value: "https://example.com/repo.git".to_string(),
            kind: TextInputKind::OriginUrl,
        });

        app.handle_key(key(KeyCode::Esc), tx).await.unwrap();

        assert!(matches!(
            app.modal,
            Some(Modal::OnboardingWizard {
                step: OnboardingStep::RepoSetupChoice,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn onboarding_origin_url_advances_after_adding_remote_with_cached_status() {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-b", "main"]);
        let mut app = make_app_in(dir.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(4);
        app.onboarding_active = true;
        app.onboarding_language_confirmed = true;
        app.modal = Some(Modal::TextInput {
            title: "Origin URL".to_string(),
            value: "https://example.com/repo.git".to_string(),
            kind: TextInputKind::OriginUrl,
        });

        let cached_status = app.core.status();
        assert!(cached_status.is_repo);
        assert!(!cached_status.has_origin);

        app.handle_key(key(KeyCode::Enter), tx).await.unwrap();

        assert!(app.core.status().has_origin);
        assert!(!matches!(
            app.modal,
            Some(Modal::OnboardingWizard {
                step: OnboardingStep::RemoteSetupChoice,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn model_configuration_starts_async_model_loading() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.core.config.provider = LlmProviderKind::OpenAi;
        app.onboarding_active = true;
        app.modal = Some(Modal::OnboardingWizard {
            step: OnboardingStep::ModelConfiguration,
            selected: 0,
        });

        app.handle_key(key(KeyCode::Enter), tx).await.unwrap();

        assert!(app.busy);
        assert_eq!(app.busy_message, "Loading provider models...");
    }

    #[tokio::test]
    async fn model_configuration_can_open_manual_model_input() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel(4);
        app.core.config.provider = LlmProviderKind::OpenAi;
        app.onboarding_active = true;
        app.modal = Some(Modal::OnboardingWizard {
            step: OnboardingStep::ModelConfiguration,
            selected: 1,
        });

        app.handle_key(key(KeyCode::Enter), tx).await.unwrap();

        assert!(matches!(
            app.modal,
            Some(Modal::TextInput {
                kind: TextInputKind::ModelName,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn model_picker_outcome_opens_model_picker() {
        let mut app = make_app();
        app.core.config.provider = LlmProviderKind::OpenAi;

        app.apply_outcome(AsyncOutcome::ModelPicker(vec![
            "gpt-4.1".to_string(),
            "gpt-4.1-mini".to_string(),
        ]));

        let Some(Modal::Picker {
            items, selected, ..
        }) = app.modal
        else {
            panic!("expected model picker");
        };
        assert_eq!(selected, 0);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "gpt-4.1");
    }

    #[tokio::test]
    async fn empty_model_picker_outcome_opens_manual_input() {
        let mut app = make_app();
        app.core.config.provider = LlmProviderKind::OpenAi;

        app.apply_outcome(AsyncOutcome::ModelPicker(Vec::new()));

        assert!(matches!(
            app.modal,
            Some(Modal::TextInput {
                kind: TextInputKind::ModelName,
                ..
            })
        ));
        assert!(app.history.iter().any(|m| m.role == ChatRole::System));
    }

    #[test]
    fn display_error_message_summarizes_invalid_json() {
        let source = serde_json::from_str::<serde_json::Value>("{ broken").unwrap_err();
        let error = AppError::InvalidJson {
            source,
            value: "{\"groups\":[{\"very\":\"long\"}]}".repeat(100),
        };

        let message = display_error_message(&error);

        assert!(message.contains("LLM returned malformed JSON"));
        assert!(message.contains("Try Regenerate, /staged, or another model"));
        assert!(message.chars().count() < 360);
    }
}
