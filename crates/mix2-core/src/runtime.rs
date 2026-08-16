use crate::agents::agent::{Agent, AgentRequest, AgentResult, AgentSession, AuthStatus};
use crate::agents::claude::ClaudeAgent;
use crate::agents::codex::CodexAgent;
use crate::agents::{AgentEvent, AgentKind, AgentRole};
use crate::collaboration::consult::{ActiveTurn, ConsultServer, ConsultUpdate};
use crate::collaboration::ConsultBudget;
use crate::config::Config;
use crate::ipc::{AgentInfo, Command, Event, Speaker, PROTOCOL_VERSION};
use crate::session::Mix2Session;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Messages funnelled from the in-flight lead task into the main loop.
enum LeadMsg {
    Event(Uuid, AgentEvent),
    Done(Uuid, Result<AgentResult>),
}

struct TurnState {
    ui_id: String,
    uuid: Uuid,
    cancel: CancellationToken,
    successful_consults: u32,
    started: Instant,
    cancelled: bool,
}

fn update_turn_id(update: &ConsultUpdate) -> Uuid {
    match update {
        ConsultUpdate::Started { turn_id, .. }
        | ConsultUpdate::AgentEvent { turn_id, .. }
        | ConsultUpdate::Completed { turn_id, .. }
        | ConsultUpdate::Failed { turn_id, .. }
        | ConsultUpdate::DisagreementRecorded { turn_id, .. } => *turn_id,
    }
}

pub struct RuntimeOptions {
    pub lead: Option<String>,
    pub cwd: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub debug: bool,
}

fn build_agent(kind: AgentKind, config: &Config) -> Arc<dyn Agent> {
    // Test/dev injection: MIX2_CLAUDE_CMD / MIX2_CODEX_CMD point the
    // adapters at fake provider fixtures without touching user config.
    let command = match kind {
        AgentKind::Claude => {
            std::env::var("MIX2_CLAUDE_CMD").unwrap_or_else(|_| config.claude_command.clone())
        }
        AgentKind::Codex => {
            std::env::var("MIX2_CODEX_CMD").unwrap_or_else(|_| config.codex_command.clone())
        }
    };
    match kind {
        AgentKind::Claude => Arc::new(ClaudeAgent::new(command)),
        AgentKind::Codex => Arc::new(CodexAgent::new(command)),
    }
}

fn emit(event: &Event) {
    let mut stdout = std::io::stdout().lock();
    if let Ok(json) = serde_json::to_string(event) {
        let _ = writeln!(stdout, "{json}");
        let _ = stdout.flush();
    }
}

/// Directory holding per-session runtime state (consult socket, consult
/// file-IPC mailbox). Never holds credentials. Removed on shutdown unless
/// debug mode asked to keep it.
fn runtime_dir_for(session_id: Uuid) -> PathBuf {
    // Unix-socket paths are limited to ~104 bytes on macOS, so avoid the
    // long per-user `/var/folders/...` temp dir and use `/tmp` directly.
    #[cfg(unix)]
    let base = PathBuf::from("/tmp");
    #[cfg(not(unix))]
    let base = std::env::temp_dir();
    base.join("mix2").join(session_id.to_string())
}

/// Heuristic: does the working directory look like a software project?
/// When it doesn't, the team adapts to general brainstorming (business
/// ideas, viability, strategy) instead of forcing a code lens.
fn detect_project(cwd: &std::path::Path) -> bool {
    if cwd.join(".git").exists() {
        return true;
    }
    const MANIFESTS: &[&str] = &[
        "package.json",
        "Cargo.toml",
        "pyproject.toml",
        "go.mod",
        "pom.xml",
        "build.gradle",
        "CMakeLists.txt",
        "Makefile",
        "Gemfile",
        "composer.json",
        "mix.exs",
    ];
    MANIFESTS.iter().any(|m| cwd.join(m).exists())
}

fn helper_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_owned()))
}

pub struct Runtime {
    config: Config,
    session: Mix2Session,
    project: bool,
    lead_model: Option<String>,
    lead_agent: Arc<dyn Agent>,
    lead_info: AgentInfo,
    teammate_info: AgentInfo,
    consult_server: ConsultServer,
    runtime_dir: PathBuf,
    lead_msgs: mpsc::Sender<LeadMsg>,
    debug: bool,
}

impl Runtime {
    /// Probe providers, start the consult server, and report readiness.
    /// Both agents are required: if either is missing or signed out, this
    /// fails (with a `fatal` event emitted by the caller) listing the exact
    /// fix for each agent.
    async fn initialize(
        config: Config,
        cwd: PathBuf,
        debug: bool,
        consult_updates: mpsc::Sender<ConsultUpdate>,
        lead_msgs: mpsc::Sender<LeadMsg>,
    ) -> Result<Self> {
        let cwd = cwd
            .canonicalize()
            .with_context(|| format!("project directory {} does not exist", cwd.display()))?;

        let lead_agent = build_agent(config.lead, &config);
        let teammate_agent = build_agent(config.teammate, &config);

        // Both probes gate readiness, so both get the generous timeout.
        let probe = |agent: Arc<dyn Agent>| async move {
            tokio::time::timeout(Duration::from_secs(20), agent.version()).await
        };
        let (lead_version, teammate_version) = tokio::join!(
            probe(Arc::clone(&lead_agent)),
            probe(Arc::clone(&teammate_agent))
        );

        let lead_installed = match lead_version {
            Ok(Ok(v)) => Some(v.raw),
            _ => None,
        };
        let teammate_installed = match teammate_version {
            Ok(Ok(v)) => Some(v.raw),
            _ => None,
        };

        // Sign-in probes are local and quota-free.
        let (lead_auth, teammate_auth) =
            tokio::join!(lead_agent.auth_status(), teammate_agent.auth_status());

        fn ready_for_duty(installed: &Option<String>, auth: AuthStatus) -> bool {
            installed.is_some() && auth != AuthStatus::Unauthenticated
        }

        // mix2 is the two-agent team — there is no solo mode. If either
        // agent is missing or signed out, refuse to start and say exactly
        // what fixes each one.
        if !ready_for_duty(&lead_installed, lead_auth)
            || !ready_for_duty(&teammate_installed, teammate_auth)
        {
            let status = |kind: AgentKind, installed: &Option<String>, auth: AuthStatus| {
                if installed.is_none() {
                    format!(
                        "{} — not installed: {}",
                        kind.display_name(),
                        install_hint(kind)
                    )
                } else if auth == AuthStatus::Unauthenticated {
                    format!(
                        "{} — not signed in: {}",
                        kind.display_name(),
                        login_hint(kind)
                    )
                } else {
                    format!("{} — ready", kind.display_name())
                }
            };
            let (first, second) = if config.lead == AgentKind::Claude {
                (config.lead, config.teammate)
            } else {
                (config.teammate, config.lead)
            };
            let for_kind = |kind: AgentKind| {
                if kind == config.lead {
                    (&lead_installed, lead_auth)
                } else {
                    (&teammate_installed, teammate_auth)
                }
            };
            let (fi, fa) = for_kind(first);
            let (si, sa) = for_kind(second);
            anyhow::bail!(
                "mix2 needs both agents installed and signed in.\n{}\n{}\nFix the above, then restart mix2.",
                status(first, fi, fa),
                status(second, si, sa),
            );
        }
        let lead_version = lead_installed.expect("lead ready implies installed");
        let teammate_version_str = teammate_installed.expect("teammate ready implies installed");

        let session = Mix2Session::new(config.lead, cwd);
        let project = detect_project(&session.cwd);
        let runtime_dir = runtime_dir_for(session.id);
        tokio::fs::create_dir_all(&runtime_dir).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700));
        }

        let model_for = |kind: AgentKind| match kind {
            AgentKind::Claude => config.claude_model.clone(),
            AgentKind::Codex => config.codex_model.clone(),
        };
        let lead_model = model_for(config.lead);
        let teammate_model = model_for(config.teammate);

        let consult_server = ConsultServer::start(
            Arc::clone(&teammate_agent),
            config.teammate,
            config.lead,
            session.cwd.clone(),
            runtime_dir.clone(),
            session.id,
            helper_dir(),
            project,
            teammate_model.clone(),
            consult_updates,
        )
        .await?;

        let lead_info = AgentInfo {
            kind: config.lead,
            name: config.lead.display_name().to_owned(),
            version: Some(lead_version),
            available: true,
            reason: None,
            authenticated: auth_flag(lead_auth),
            model: lead_model.clone(),
            models: lead_agent.known_models(),
        };
        let teammate_info = AgentInfo {
            kind: config.teammate,
            name: config.teammate.display_name().to_owned(),
            version: Some(teammate_version_str),
            available: true,
            reason: None,
            authenticated: auth_flag(teammate_auth),
            model: teammate_model.clone(),
            models: teammate_agent.known_models(),
        };

        Ok(Self {
            config,
            session,
            project,
            lead_model,
            lead_agent,
            lead_info,
            teammate_info,
            consult_server,
            runtime_dir,
            lead_msgs,
            debug,
        })
    }

    fn mix2_env(
        &self,
        turn_uuid: Uuid,
        role: AgentRole,
        consult_token: Option<&str>,
    ) -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert("MIX2_ROLE".to_owned(), role.to_string());
        env.insert(
            "MIX2_DEPTH".to_owned(),
            if role == AgentRole::Lead { "0" } else { "1" }.to_owned(),
        );
        env.insert("MIX2_SESSION_ID".to_owned(), self.session.id.to_string());
        env.insert("MIX2_TURN_ID".to_owned(), turn_uuid.to_string());
        env.insert(
            "MIX2_RUNTIME_DIR".to_owned(),
            self.runtime_dir.display().to_string(),
        );
        if let Some(token) = consult_token {
            env.insert("MIX2_CONSULT_TOKEN".to_owned(), token.to_owned());
        }
        env
    }

    async fn start_turn(&mut self, ui_id: String, text: String) -> TurnState {
        let turn_uuid = Uuid::new_v4();
        let cancel = CancellationToken::new();
        let budget = Arc::new(ConsultBudget::new(self.config.max_consults_per_turn));
        let consult_token = Uuid::new_v4().to_string();

        emit(&Event::MessageUser {
            turn_id: ui_id.clone(),
            text: text.clone(),
        });
        emit(&Event::TurnStarted {
            turn_id: ui_id.clone(),
        });

        let request = AgentRequest {
            prompt: text,
            cwd: self.session.cwd.clone(),
            role: AgentRole::Lead,
            turn_id: turn_uuid,
            model: self.lead_model.clone(),
            instructions: crate::collaboration::prompts::lead_instructions(
                self.config.lead,
                self.config.teammate,
                self.project,
            ),
            env: self.mix2_env(turn_uuid, AgentRole::Lead, Some(&consult_token)),
            path_prepend: helper_dir(),
            runtime_dir: Some(self.runtime_dir.clone()),
        };

        let lead = Arc::clone(&self.lead_agent);
        let resume_session = self
            .session
            .lead_provider_session_id
            .clone()
            .map(|id| AgentSession {
                agent: self.config.lead,
                id,
            });
        let msgs = self.lead_msgs.clone();
        let token = cancel.clone();
        let budget_for_server = Arc::clone(&budget);

        // Register with the consult server BEFORE the lead exists: a fast
        // lead must never find "no active turn".
        self.consult_server
            .begin_turn(ActiveTurn {
                turn_id: turn_uuid,
                budget: budget_for_server,
                cancel: cancel.clone(),
                token: consult_token,
                completed_consults: Arc::new(AtomicU32::new(0)),
                disagreement: Arc::new(Mutex::new(None)),
            })
            .await;

        tokio::spawn(async move {
            let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
            let forward_msgs = msgs.clone();
            let forward = tokio::spawn(async move {
                while let Some(ev) = rx.recv().await {
                    let _ = forward_msgs.send(LeadMsg::Event(turn_uuid, ev)).await;
                }
            });
            let result = match &resume_session {
                Some(session) => lead.resume(session, request, tx, token).await,
                None => lead.start(request, tx, token).await,
            };
            let _ = forward.await;
            let _ = msgs.send(LeadMsg::Done(turn_uuid, result)).await;
        });

        TurnState {
            ui_id,
            uuid: turn_uuid,
            cancel: cancel.clone(),
            successful_consults: 0,
            started: Instant::now(),
            cancelled: false,
        }
    }

    fn handle_lead_event(&mut self, ui_id: &str, event: AgentEvent) {
        let turn_id = ui_id.to_owned();
        let role = AgentRole::Lead;
        match event {
            AgentEvent::Started { agent } => emit(&Event::AgentStarted {
                turn_id,
                agent,
                role,
            }),
            AgentEvent::SessionStarted { session_id, .. } => {
                self.session.lead_provider_session_id = Some(session_id);
            }
            AgentEvent::TextDelta { agent, text } => emit(&Event::AgentTextDelta {
                turn_id,
                agent,
                role,
                text,
            }),
            AgentEvent::ToolStarted {
                agent,
                name,
                detail,
            } => emit(&Event::AgentToolStarted {
                turn_id,
                agent,
                role,
                name,
                detail,
            }),
            AgentEvent::ToolFinished { agent, name } => emit(&Event::AgentToolFinished {
                turn_id,
                agent,
                role,
                name,
            }),
            AgentEvent::ModelObserved { agent, model } => emit(&Event::AgentModel {
                agent,
                model: Some(model),
                source: "observed".to_owned(),
            }),
            AgentEvent::ParserWarning { agent, message } => {
                tracing::warn!("{agent} parser: {message}");
                if self.debug {
                    emit(&Event::Warning {
                        message: format!("{agent} parser: {message}"),
                    });
                }
            }
            // Message/Usage/Completed/Failed are folded into the final
            // result handling; emitting them here would duplicate output.
            AgentEvent::Message { .. }
            | AgentEvent::Usage { .. }
            | AgentEvent::Completed { .. }
            | AgentEvent::Failed { .. } => {}
        }
    }

    fn handle_consult_update(&mut self, turn: &mut TurnState, update: ConsultUpdate) {
        let teammate = self.config.teammate;
        match update {
            ConsultUpdate::Started {
                index, max, prompt, ..
            } => emit(&Event::ConsultStarted {
                turn_id: turn.ui_id.clone(),
                agent: teammate,
                index,
                max,
                prompt,
            }),
            ConsultUpdate::AgentEvent { event, .. } => {
                let turn_id = turn.ui_id.clone();
                let role = AgentRole::Teammate;
                match event {
                    AgentEvent::Started { agent } => emit(&Event::AgentStarted {
                        turn_id,
                        agent,
                        role,
                    }),
                    AgentEvent::TextDelta { agent, text } => emit(&Event::AgentTextDelta {
                        turn_id,
                        agent,
                        role,
                        text,
                    }),
                    AgentEvent::ToolStarted {
                        agent,
                        name,
                        detail,
                    } => emit(&Event::AgentToolStarted {
                        turn_id,
                        agent,
                        role,
                        name,
                        detail,
                    }),
                    AgentEvent::ToolFinished { agent, name } => emit(&Event::AgentToolFinished {
                        turn_id,
                        agent,
                        role,
                        name,
                    }),
                    _ => {}
                }
            }
            ConsultUpdate::Completed {
                index,
                duration_ms,
                text,
                ..
            } => {
                turn.successful_consults += 1;
                emit(&Event::ConsultCompleted {
                    turn_id: turn.ui_id.clone(),
                    agent: teammate,
                    index,
                    duration_ms,
                    text,
                });
                emit(&Event::LeadSynthesizing {
                    turn_id: turn.ui_id.clone(),
                    agent: self.config.lead,
                });
            }
            // TODO: emit `disagreement.recorded` for the live team-panel ledger.
            ConsultUpdate::DisagreementRecorded { .. } => {}
            ConsultUpdate::Failed { index, message, .. } => emit(&Event::ConsultFailed {
                turn_id: turn.ui_id.clone(),
                agent: teammate,
                index,
                message,
            }),
        }
    }

    async fn finish_turn(&mut self, turn: TurnState, result: Result<AgentResult>) {
        // Kill anything still attached to this turn — in particular a
        // `start`ed consultation the lead never waited for. Its result
        // belongs to no one, and it must not bleed into the next turn.
        turn.cancel.cancel();
        // TODO: attach the settled record to the `message.final` payload.
        let _ = self.consult_server.end_turn().await;
        let duration_ms = turn.started.elapsed().as_millis() as u64;
        match result {
            _ if turn.cancelled => {
                emit(&Event::TurnCancelled {
                    turn_id: turn.ui_id,
                });
            }
            Ok(result) => {
                if let Some(id) = result.session_id {
                    self.session.lead_provider_session_id = Some(id);
                }
                let speaker = if turn.successful_consults > 0 {
                    Speaker::Team
                } else {
                    Speaker::from(self.config.lead)
                };
                emit(&Event::MessageFinal {
                    turn_id: turn.ui_id.clone(),
                    speaker,
                    lead: self.config.lead,
                    text: result.text,
                    consultations: turn.successful_consults,
                    duration_ms,
                });
                emit(&Event::TurnCompleted {
                    turn_id: turn.ui_id,
                    duration_ms,
                    consultations: turn.successful_consults,
                });
            }
            Err(e) => {
                emit(&Event::TurnFailed {
                    turn_id: turn.ui_id,
                    message: format!("{e:#}"),
                });
            }
        }
    }

    /// Apply a /model selection: store the override for future invocations
    /// (lead resumes and teammate consults both honor it) and confirm.
    async fn set_model(&mut self, agent: &str, model: Option<String>) {
        let Ok(kind) = agent.parse::<AgentKind>() else {
            emit(&Event::Error {
                message: format!("unknown agent '{agent}' (expected claude or codex)"),
            });
            return;
        };
        let model = model.filter(|m| !m.trim().is_empty() && m != "default");
        if kind == self.config.lead {
            self.lead_model = model.clone();
        } else {
            self.consult_server.set_teammate_model(model.clone()).await;
        }
        emit(&Event::AgentModel {
            agent: kind,
            model,
            source: "selected".to_owned(),
        });
    }

    async fn cleanup(&self) {
        let _ = tokio::fs::remove_dir_all(&self.runtime_dir).await;
    }
}

fn auth_flag(status: AuthStatus) -> Option<bool> {
    match status {
        AuthStatus::Authenticated => Some(true),
        AuthStatus::Unauthenticated => Some(false),
        AuthStatus::Unknown => None,
    }
}

fn install_hint(kind: AgentKind) -> String {
    match kind {
        AgentKind::Claude => {
            "install Claude Code from https://claude.com/claude-code, then run `claude` once to sign in"
                .to_owned()
        }
        AgentKind::Codex => "install Codex from https://developers.openai.com/codex/cli (`npm i -g @openai/codex`), then run `codex login`"
            .to_owned(),
    }
}

fn login_hint(kind: AgentKind) -> String {
    match kind {
        AgentKind::Claude => "run `claude` once (or `claude auth login`) to sign in".to_owned(),
        AgentKind::Codex => "run `codex login`".to_owned(),
    }
}

/// Serve the JSONL protocol over stdin/stdout until shutdown or EOF.
pub async fn serve(options: RuntimeOptions) -> Result<()> {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    // First command must be `initialize`.
    let first = loop {
        match lines.next_line().await? {
            Some(line) if line.trim().is_empty() => continue,
            Some(line) => break line,
            None => return Ok(()),
        }
    };
    let init: Command = match serde_json::from_str(&first) {
        Ok(cmd) => cmd,
        Err(e) => {
            emit(&Event::Fatal {
                message: format!("invalid initialize command: {e}"),
            });
            return Ok(());
        }
    };
    let (protocol, cmd_lead, cmd_cwd, debug) = match init {
        Command::Initialize {
            protocol,
            lead,
            cwd,
            debug,
        } => (protocol, lead, cwd, debug || options.debug),
        other => {
            emit(&Event::Fatal {
                message: format!("expected initialize, got {other:?}"),
            });
            return Ok(());
        }
    };
    if protocol != PROTOCOL_VERSION {
        emit(&Event::Fatal {
            message: format!(
                "protocol mismatch: UI speaks {protocol}, core speaks {PROTOCOL_VERSION}"
            ),
        });
        return Ok(());
    }

    let file = match crate::config::load_file(options.config_path.as_deref()) {
        Ok(f) => f,
        Err(e) => {
            emit(&Event::Fatal {
                message: format!("{e:#}"),
            });
            return Ok(());
        }
    };
    let lead_arg = cmd_lead.or(options.lead);
    let config = match Config::resolve(lead_arg.as_deref(), &file) {
        Ok(c) => c,
        Err(e) => {
            emit(&Event::Fatal {
                message: format!("{e:#}"),
            });
            return Ok(());
        }
    };
    let cwd = cmd_cwd
        .map(PathBuf::from)
        .or(options.cwd)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let (consult_tx, mut consult_rx) = mpsc::channel::<ConsultUpdate>(256);
    let (lead_tx, mut lead_rx) = mpsc::channel::<LeadMsg>(256);

    let mut runtime = match Runtime::initialize(config, cwd, debug, consult_tx, lead_tx).await {
        Ok(r) => r,
        Err(e) => {
            emit(&Event::Fatal {
                message: format!("{e:#}"),
            });
            return Ok(());
        }
    };

    emit(&Event::Ready {
        protocol: PROTOCOL_VERSION,
        session_id: runtime.session.id.to_string(),
        lead: Box::new(runtime.lead_info.clone()),
        teammate: Box::new(runtime.teammate_info.clone()),
        cwd: runtime.session.cwd.display().to_string(),
        project: runtime.project,
    });

    let mut active: Option<TurnState> = None;

    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else {
                    // UI went away; make sure children die with us.
                    if let Some(turn) = active.take() {
                        turn.cancel.cancel();
                    }
                    break;
                };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Command>(&line) {
                    Ok(Command::Submit { id, text }) => {
                        if active.is_some() {
                            emit(&Event::Error {
                                message: "a turn is already running".to_owned(),
                            });
                        } else {
                            active = Some(runtime.start_turn(id, text).await);
                        }
                    }
                    Ok(Command::Cancel { turn_id }) => {
                        match active.as_mut() {
                            Some(turn) if turn.ui_id == turn_id => {
                                turn.cancelled = true;
                                turn.cancel.cancel();
                            }
                            _ => emit(&Event::Warning {
                                message: format!("cancel for unknown turn {turn_id}"),
                            }),
                        }
                    }
                    Ok(Command::SetModel { agent, model }) => {
                        runtime.set_model(&agent, model).await;
                    }
                    Ok(Command::Shutdown) => {
                        if let Some(turn) = active.take() {
                            turn.cancel.cancel();
                        }
                        break;
                    }
                    Ok(Command::Initialize { .. }) => {
                        emit(&Event::Error {
                            message: "already initialized".to_owned(),
                        });
                    }
                    Err(e) => {
                        emit(&Event::Error {
                            message: format!("invalid command: {e}"),
                        });
                    }
                }
            }
            Some(update) = consult_rx.recv() => {
                // Scope strictly to the originating turn: a late update
                // from an earlier turn must never relabel itself onto the
                // current one.
                if let Some(turn) = active.as_mut() {
                    if update_turn_id(&update) == turn.uuid {
                        runtime.handle_consult_update(turn, update);
                    }
                }
            }
            Some(msg) = lead_rx.recv() => {
                match msg {
                    LeadMsg::Event(uuid, event) => {
                        let ui_id = active
                            .as_ref()
                            .filter(|t| t.uuid == uuid)
                            .map(|t| t.ui_id.clone());
                        if let Some(ui_id) = ui_id {
                            runtime.handle_lead_event(&ui_id, event);
                        }
                    }
                    LeadMsg::Done(uuid, result) => {
                        if active.as_ref().is_some_and(|t| t.uuid == uuid) {
                            let turn = active.take().expect("checked above");
                            runtime.finish_turn(turn, result).await;
                        }
                    }
                }
            }
        }
    }

    // Give in-flight kill_tree sequences a beat to finish before the
    // runtime tears down; kill_on_drop only reaps direct children.
    tokio::time::sleep(Duration::from_millis(400)).await;
    runtime.cleanup().await;
    Ok(())
}

/// Development command: run one prompt end-to-end against the configured
/// lead and print the normalized event stream (Phase 1 demonstration:
/// prompt -> provider -> normalized events -> final response).
pub async fn dev_run(options: RuntimeOptions, prompt: String) -> Result<()> {
    let file = crate::config::load_file(options.config_path.as_deref())?;
    let config = Config::resolve(options.lead.as_deref(), &file)?;
    let cwd = options
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let (consult_tx, mut consult_rx) = mpsc::channel::<ConsultUpdate>(256);
    let (lead_tx, mut lead_rx) = mpsc::channel::<LeadMsg>(256);

    let mut runtime = Runtime::initialize(config, cwd, options.debug, consult_tx, lead_tx).await?;

    emit(&Event::Ready {
        protocol: PROTOCOL_VERSION,
        session_id: runtime.session.id.to_string(),
        lead: Box::new(runtime.lead_info.clone()),
        teammate: Box::new(runtime.teammate_info.clone()),
        cwd: runtime.session.cwd.display().to_string(),
        project: runtime.project,
    });

    let mut turn = Some(runtime.start_turn("dev-1".to_owned(), prompt).await);
    while let Some(state) = turn.as_mut() {
        tokio::select! {
            Some(update) = consult_rx.recv() => {
                if update_turn_id(&update) == state.uuid {
                    runtime.handle_consult_update(state, update);
                }
            }
            Some(msg) = lead_rx.recv() => {
                match msg {
                    LeadMsg::Event(_, event) => {
                        let ui_id = state.ui_id.clone();
                        runtime.handle_lead_event(&ui_id, event);
                    }
                    LeadMsg::Done(_, result) => {
                        let state = turn.take().expect("turn active");
                        runtime.finish_turn(state, result).await;
                    }
                }
            }
        }
    }
    runtime.cleanup().await;
    Ok(())
}
