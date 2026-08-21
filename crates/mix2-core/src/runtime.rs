use crate::agents::agent::{Agent, AgentRequest, AgentResult, AgentSession, AuthState};
use crate::agents::runner::HarnessAgent;
use crate::agents::{discovery, registry};
use crate::agents::{AgentEvent, AgentRole, HarnessKind, SlotId, Team};
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

/// Resolve the command a harness actually runs with. Test/dev env
/// injection wins, slot-targeted first: MIX2_SLOT_ONE_CMD /
/// MIX2_SLOT_TWO_CMD beat the legacy harness-keyed overrides
/// (MIX2_CLAUDE_CMD / MIX2_CODEX_CMD), which beat configured commands —
/// all without touching user config. Discovery and slot construction share
/// this so probe cache keys always match the commands slots run with.
fn effective_command(slot: Option<SlotId>, harness: HarnessKind, configured: &str) -> String {
    let slot_env = slot.map(|s| match s {
        SlotId::One => "MIX2_SLOT_ONE_CMD",
        SlotId::Two => "MIX2_SLOT_TWO_CMD",
    });
    slot_env
        .and_then(|key| std::env::var(key).ok())
        .or_else(|| std::env::var(registry::descriptor(harness).command_env_override).ok())
        .unwrap_or_else(|| configured.to_owned())
}

fn build_agent(harness: HarnessKind, command: String) -> Arc<dyn Agent> {
    Arc::new(HarnessAgent::new(registry::descriptor(harness), command))
}

/// Every `(harness, command)` pair discovery should probe: each configured
/// slot at its effective command, plus every registered harness at its
/// fallback command so a picker can offer harnesses no slot currently runs.
fn discovery_candidates(config: &Config) -> Vec<(HarnessKind, String)> {
    let mut out: Vec<(HarnessKind, String)> = Vec::new();
    for slot in SlotId::ALL {
        let harness = config.team.harness(slot);
        out.push((
            harness,
            effective_command(Some(slot), harness, &config.slot(slot).command),
        ));
    }
    for harness in registry::ALL {
        out.push((
            harness,
            effective_command(None, harness, &config.fallback_command(harness)),
        ));
    }
    out.dedup();
    out
}

/// The env value controlling discovery probe timeouts (tests shrink it to
/// keep timeout fixtures fast).
fn discovery_timeout() -> Duration {
    std::env::var("MIX2_DISCOVERY_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(discovery::DEFAULT_PROBE_TIMEOUT)
}

/// Effective lead eligibility: native capability scoping, or the OS sandbox
/// standing in for it when available. The single derived rule shared by the
/// discovery report, `validate_selection`, and the `Runtime::initialize`
/// backstop, so every path agrees.
fn lead_eligible_effective(harness: HarnessKind, sandbox_available: bool) -> bool {
    let descriptor = registry::descriptor(harness);
    descriptor.capabilities.lead_eligible() || (sandbox_available && descriptor.sandboxable_lead)
}

/// Role-eligibility gate for a settled team: the lead harness must be able
/// to coordinate (natively or via the sandbox) and the teammate to consult
/// read-only. Every path that settles a team runs it — the picker's
/// `validate_selection` and the `Runtime::initialize` backstop that the
/// auto-confirm and `dev_run` paths also traverse — so a config naming an
/// ineligible lead is refused everywhere. Returns an actionable refusal.
fn check_team_eligibility(team: Team, sandbox_available: bool) -> std::result::Result<(), String> {
    if !lead_eligible_effective(team.lead_harness(), sandbox_available) {
        // Distinguish "needs the sandbox, which isn't available here" from
        // "can never lead" so the message is actionable.
        let harness = team.lead_harness();
        let msg = if registry::descriptor(harness).sandboxable_lead {
            format!(
                "{} can only lead under the OS sandbox (enable `[sandbox] mode` on a supported platform) — pick it as the teammate instead",
                harness.display_name()
            )
        } else {
            format!(
                "{} cannot lead yet — pick it as the teammate instead",
                harness.display_name()
            )
        };
        return Err(msg);
    }
    let teammate_caps = registry::descriptor(team.teammate_harness()).capabilities;
    if !teammate_caps.teammate_eligible() {
        return Err(format!(
            "{} cannot serve as the teammate",
            team.teammate_harness().display_name()
        ));
    }
    Ok(())
}

/// Validate a `select_team` request against discovery results. Returns the
/// settled team or an actionable refusal for the picker to display.
fn validate_selection(
    config: &Config,
    report: &discovery::Discovery,
    one: &str,
    two: &str,
    lead_slot: &str,
    sandbox_available: bool,
) -> std::result::Result<Team, String> {
    let resolve = |name: &str| {
        registry::harness_named(name).ok_or_else(|| registry::unknown_harness_message(name))
    };
    let one = resolve(one)?;
    let two = resolve(two)?;
    let lead: SlotId = lead_slot
        .parse()
        .map_err(|_| format!("invalid lead_slot '{lead_slot}' (expected 'one' or 'two')"))?;
    let team = Team { one, two, lead };

    for slot in SlotId::ALL {
        let harness = team.harness(slot);
        let command = effective_command(
            Some(slot),
            harness,
            &config.selection_command(slot, harness),
        );
        let Some(probe) = report.probe(harness, &command) else {
            return Err(format!(
                "{} was not discovered at `{command}` — restart mix2 to re-probe",
                harness.display_name()
            ));
        };
        if probe.version.is_none() {
            return Err(probe
                .reason
                .clone()
                .unwrap_or_else(|| format!("{} is unavailable", harness.display_name())));
        }
        if probe.auth == AuthState::Unauthenticated {
            return Err(format!(
                "{} — not signed in: {}",
                harness.display_name(),
                registry::descriptor(harness).login_hint
            ));
        }
    }
    check_team_eligibility(team, sandbox_available)?;
    Ok(team)
}

/// Display name for a slot's participant. Distinct harnesses read as the
/// harness ("Claude"); a same-harness team qualifies each side by slot
/// ("Codex (one)") so the two participants never become indistinguishable.
fn slot_display_name(team: Team, slot: SlotId) -> String {
    let harness = team.harness(slot);
    if team.one == team.two {
        format!("{} ({slot})", harness.display_name())
    } else {
        harness.display_name().to_owned()
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
    one_info: AgentInfo,
    two_info: AgentInfo,
    consult_server: ConsultServer,
    runtime_dir: PathBuf,
    lead_msgs: mpsc::Sender<LeadMsg>,
    debug: bool,
    /// The OS sandbox engine to wrap a non-natively-scoped lead in, when one
    /// is available and `[sandbox] mode` allows it. `None` means every lead
    /// runs natively (teammate mechanisms untouched).
    sandbox_engine: Option<crate::sandbox::SandboxEngine>,
}

impl Runtime {
    /// Probe providers, start the consult server, and report readiness.
    /// Both agents are required: if either is missing or signed out, this
    /// fails (with a `fatal` event emitted by the caller) listing the exact
    /// fix for each agent.
    #[allow(clippy::too_many_arguments)]
    async fn initialize(
        config: Config,
        team: Team,
        cwd: PathBuf,
        debug: bool,
        consult_updates: mpsc::Sender<ConsultUpdate>,
        lead_msgs: mpsc::Sender<LeadMsg>,
        report: &discovery::Discovery,
        sandbox_engine: Option<crate::sandbox::SandboxEngine>,
    ) -> Result<Self> {
        // Role-eligibility backstop for every entry point. The interactive
        // picker refuses ineligible teams in `validate_selection`, but the
        // auto-confirm branch and `dev_run` settle the configured team
        // without it — so a config naming an ineligible lead (e.g.
        // `[slot.one] harness = "cursor", lead = "one"`) would otherwise
        // start unrefused. Fail here before any probing or spawning.
        if let Err(message) = check_team_eligibility(team, sandbox_engine.is_some()) {
            anyhow::bail!(message);
        }

        let cwd = cwd
            .canonicalize()
            .with_context(|| format!("project directory {} does not exist", cwd.display()))?;

        // Selected-slot probes come from the discovery cache (keyed by
        // harness + effective command); a cold key is probed on demand.
        let slot_probe = |slot: SlotId| {
            let harness = team.harness(slot);
            let command = effective_command(
                Some(slot),
                harness,
                &config.selection_command(slot, harness),
            );
            async move {
                let probe = match report.probe(harness, &command) {
                    Some(cached) => cached.clone(),
                    None => discovery::probe_one(harness, &command, discovery_timeout()).await,
                };
                (command, probe)
            }
        };
        let ((one_command, one_probe), (two_command, two_probe)) =
            tokio::join!(slot_probe(SlotId::One), slot_probe(SlotId::Two));

        let one_agent = build_agent(team.one, one_command);
        let two_agent = build_agent(team.two, two_command);
        // Model lists in parallel with everything else that gates ready;
        // live enumeration is bounded and quota-free, and failure falls
        // back to the curated list inside the agent.
        let (one_models, two_models) = tokio::join!(one_agent.models(), two_agent.models());

        fn ready_for_duty(probe: &discovery::Probe) -> bool {
            probe.version.is_some() && probe.auth != AuthState::Unauthenticated
        }

        // mix2 is the two-agent team — there is no solo mode. If either
        // agent is missing or signed out, refuse to start and say exactly
        // what fixes each one.
        if !ready_for_duty(&one_probe) || !ready_for_duty(&two_probe) {
            let status = |harness: HarnessKind, probe: &discovery::Probe| {
                let descriptor = registry::descriptor(harness);
                if probe.version.is_none() {
                    format!(
                        "{} — not installed: {}",
                        harness.display_name(),
                        descriptor.install_hint
                    )
                } else if probe.auth == AuthState::Unauthenticated {
                    format!(
                        "{} — not signed in: {}",
                        harness.display_name(),
                        descriptor.login_hint
                    )
                } else {
                    format!("{} — ready", harness.display_name())
                }
            };
            anyhow::bail!(
                "mix2 needs both agents installed and signed in.\n{}\n{}\nFix the above, then restart mix2.",
                status(team.one, &one_probe),
                status(team.two, &two_probe),
            );
        }
        let one_version = one_probe.version.clone().expect("ready implies installed");
        let two_version = two_probe.version.clone().expect("ready implies installed");

        let session = Mix2Session::new(team, cwd);
        let project = detect_project(&session.cwd);
        let runtime_dir = runtime_dir_for(session.id);
        tokio::fs::create_dir_all(&runtime_dir).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700));
        }

        let lead_model = config.selection_model(team.lead, team.lead_harness());
        let teammate_model = config.selection_model(team.teammate(), team.teammate_harness());

        let (lead_agent, teammate_agent) = match team.lead {
            SlotId::One => (Arc::clone(&one_agent), Arc::clone(&two_agent)),
            SlotId::Two => (Arc::clone(&two_agent), Arc::clone(&one_agent)),
        };

        let consult_server = ConsultServer::start(
            teammate_agent,
            team,
            session.cwd.clone(),
            runtime_dir.clone(),
            session.id,
            helper_dir(),
            project,
            teammate_model,
            consult_updates,
        )
        .await?;

        let one_info = AgentInfo {
            slot: SlotId::One,
            harness: team.one,
            name: slot_display_name(team, SlotId::One),
            version: Some(one_version),
            available: true,
            reason: None,
            auth: one_probe.auth,
            model: config.selection_model(SlotId::One, team.one),
            models: one_models,
        };
        let two_info = AgentInfo {
            slot: SlotId::Two,
            harness: team.two,
            name: slot_display_name(team, SlotId::Two),
            version: Some(two_version),
            available: true,
            reason: None,
            auth: two_probe.auth,
            model: config.selection_model(SlotId::Two, team.two),
            models: two_models,
        };

        Ok(Self {
            config,
            session,
            project,
            lead_model,
            lead_agent,
            one_info,
            two_info,
            consult_server,
            runtime_dir,
            lead_msgs,
            debug,
            sandbox_engine,
        })
    }

    fn team(&self) -> Team {
        self.session.team
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

        let mut env = self.mix2_env(turn_uuid, AgentRole::Lead, Some(&consult_token));

        // Wrap the lead in the OS sandbox when its harness lacks native
        // write scoping and an engine is available. A natively-scoped lead
        // (claude/codex) is never wrapped — its own mechanism stands, and
        // Seatbelt does not nest.
        let lead_harness = self.team().lead_harness();
        let need_sandbox = self.sandbox_engine.filter(|_| {
            !registry::descriptor(lead_harness)
                .capabilities
                .lead_eligible()
        });
        // Produce the spec or an actionable failure. A harness that is
        // lead-eligible *only* because of the sandbox must NEVER fall back
        // to an unconfined run when assembly fails — that would silently
        // break the scope the picker disclosed. Fail the turn instead.
        let sandbox_result: std::result::Result<Option<crate::sandbox::SandboxSpec>, String> =
            match need_sandbox {
                Some(engine) => {
                    let descriptor = registry::descriptor(lead_harness);
                    let others: Vec<&str> = registry::ALL
                        .into_iter()
                        .filter(|h| *h != lead_harness)
                        .flat_map(|h| registry::descriptor(h).credential_files.iter().copied())
                        .collect();
                    match crate::sandbox::build_lead_spec(crate::sandbox::LeadSpecInputs {
                        engine,
                        cwd: &self.session.cwd,
                        runtime_dir: &self.runtime_dir,
                        state_dirs: descriptor.state_dirs,
                        other_credential_files: &others,
                        own_credential_files: descriptor.credential_files,
                        env_keep: descriptor.env_keep_sandboxed,
                    }) {
                        Ok((spec, lead_tmp)) => {
                            // Point the child's TMPDIR at the writable scratch
                            // so scratch writes land inside the grant, not the
                            // denied shared temp.
                            env.insert("TMPDIR".into(), lead_tmp.display().to_string());
                            Ok(Some(spec))
                        }
                        Err(e) => Err(format!(
                            "sandbox: could not confine the {} lead ({e}) — refusing to run it \
                             unsandboxed. Pick it as the teammate, or set `[sandbox] mode = off`.",
                            lead_harness.display_name()
                        )),
                    }
                }
                None => Ok(None),
            };

        let sandbox = match sandbox_result {
            Ok(spec) => spec,
            Err(message) => {
                // Fail the turn through the normal completion path: no
                // `begin_turn`, no lead spawn. `end_turn` (in `finish_turn`)
                // safely no-ops without an active turn.
                let msgs = self.lead_msgs.clone();
                tokio::spawn(async move {
                    let _ = msgs
                        .send(LeadMsg::Done(turn_uuid, Err(anyhow::anyhow!(message))))
                        .await;
                });
                return TurnState {
                    ui_id,
                    uuid: turn_uuid,
                    cancel,
                    successful_consults: 0,
                    started: Instant::now(),
                    cancelled: false,
                };
            }
        };

        let request = AgentRequest {
            prompt: text,
            cwd: self.session.cwd.clone(),
            role: AgentRole::Lead,
            turn_id: turn_uuid,
            model: self.lead_model.clone(),
            instructions: crate::collaboration::prompts::lead_instructions(
                self.team(),
                self.project,
            ),
            env,
            path_prepend: helper_dir(),
            runtime_dir: Some(self.runtime_dir.clone()),
            sandbox,
        };

        let lead = Arc::clone(&self.lead_agent);
        let resume_session = self
            .session
            .lead_provider_session_id
            .clone()
            .map(|id| AgentSession {
                harness: self.team().lead_harness(),
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
        // Adapter events are identity-free; the lead channel they arrived on
        // is what stamps the slot.
        let slot = self.team().lead;
        let role = AgentRole::Lead;
        match event {
            AgentEvent::Started => emit(&Event::AgentStarted {
                turn_id,
                slot,
                role,
            }),
            AgentEvent::SessionStarted { session_id } => {
                self.session.lead_provider_session_id = Some(session_id);
            }
            AgentEvent::TextDelta { text } => emit(&Event::AgentTextDelta {
                turn_id,
                slot,
                role,
                text,
            }),
            AgentEvent::ToolStarted { name, detail } => emit(&Event::AgentToolStarted {
                turn_id,
                slot,
                role,
                name,
                detail,
            }),
            AgentEvent::ToolFinished { name } => emit(&Event::AgentToolFinished {
                turn_id,
                slot,
                role,
                name,
            }),
            AgentEvent::ModelObserved { model } => emit(&Event::AgentModel {
                slot,
                model: Some(model),
                source: "observed".to_owned(),
            }),
            AgentEvent::ParserWarning { message } => {
                let harness = self.team().lead_harness();
                tracing::warn!("{harness} parser: {message}");
                if self.debug {
                    emit(&Event::Warning {
                        message: format!("{harness} parser: {message}"),
                    });
                }
            }
            // Message/Usage/Completed/Failed are folded into the final
            // result handling; emitting them here would duplicate output.
            AgentEvent::Message { .. }
            | AgentEvent::Usage { .. }
            | AgentEvent::Completed
            | AgentEvent::Failed { .. } => {}
        }
    }

    fn handle_consult_update(&mut self, turn: &mut TurnState, update: ConsultUpdate) {
        // Adapter events are identity-free; the consult channel they arrived
        // on is what stamps the teammate slot.
        let teammate = self.team().teammate();
        match update {
            ConsultUpdate::Started {
                index, max, prompt, ..
            } => emit(&Event::ConsultStarted {
                turn_id: turn.ui_id.clone(),
                slot: teammate,
                index,
                max,
                prompt,
            }),
            ConsultUpdate::AgentEvent { event, .. } => {
                let turn_id = turn.ui_id.clone();
                let role = AgentRole::Teammate;
                match event {
                    AgentEvent::Started => emit(&Event::AgentStarted {
                        turn_id,
                        slot: teammate,
                        role,
                    }),
                    AgentEvent::TextDelta { text } => emit(&Event::AgentTextDelta {
                        turn_id,
                        slot: teammate,
                        role,
                        text,
                    }),
                    AgentEvent::ToolStarted { name, detail } => emit(&Event::AgentToolStarted {
                        turn_id,
                        slot: teammate,
                        role,
                        name,
                        detail,
                    }),
                    AgentEvent::ToolFinished { name } => emit(&Event::AgentToolFinished {
                        turn_id,
                        slot: teammate,
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
                    slot: teammate,
                    index,
                    duration_ms,
                    text,
                });
                emit(&Event::LeadSynthesizing {
                    turn_id: turn.ui_id.clone(),
                    slot: self.team().lead,
                });
            }
            ConsultUpdate::DisagreementRecorded {
                record, revision, ..
            } => emit(&Event::DisagreementRecorded {
                turn_id: turn.ui_id.clone(),
                stances: record.stances,
                resolution: record.resolution,
                revision,
            }),
            ConsultUpdate::Failed { index, message, .. } => emit(&Event::ConsultFailed {
                turn_id: turn.ui_id.clone(),
                slot: teammate,
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
        let disagreement = self.consult_server.end_turn().await;
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
                    Speaker::from(self.team().lead)
                };
                emit(&Event::MessageFinal {
                    turn_id: turn.ui_id.clone(),
                    speaker,
                    lead_slot: self.team().lead,
                    text: result.text,
                    consultations: turn.successful_consults,
                    duration_ms,
                    disagreement,
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
    /// Targets a slot — `one`/`two` canonically, or a harness name while it
    /// names exactly one slot.
    async fn set_model(&mut self, slot: &str, model: Option<String>) {
        let Some(slot_id) = self.team().slot_named(slot) else {
            emit(&Event::Error {
                message: format!(
                    "unknown slot '{slot}' (expected one, two, or an unambiguous harness name)"
                ),
            });
            return;
        };
        let model = model.filter(|m| !m.trim().is_empty() && m != "default");
        if slot_id == self.team().lead {
            self.lead_model = model.clone();
        } else {
            self.consult_server.set_teammate_model(model.clone()).await;
        }
        emit(&Event::AgentModel {
            slot: slot_id,
            model,
            source: "selected".to_owned(),
        });
    }

    async fn cleanup(&self) {
        let _ = tokio::fs::remove_dir_all(&self.runtime_dir).await;
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
    let (protocol, cmd_lead, cmd_cwd, debug, interactive, pick_team) = match init {
        Command::Initialize {
            protocol,
            lead,
            cwd,
            debug,
            interactive,
            pick_team,
        } => (
            protocol,
            lead,
            cwd,
            debug || options.debug,
            interactive,
            pick_team,
        ),
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

    // Resolve the sandbox engine once: it decides which harnesses are
    // lead-eligible this run and wraps the lead at turn time. `off` (or no
    // engine on this platform) leaves everything native — today's behavior.
    let sandbox_engine = match config.sandbox_mode {
        crate::config::SandboxMode::Auto => crate::sandbox::SandboxSpec::detect_engine(),
        crate::config::SandboxMode::Off => None,
    };
    let sandbox_available = sandbox_engine.is_some();

    // Discovery: probe every candidate once, report, then either
    // auto-confirm the configured proposal or wait for a selection.
    let report = discovery::discover(
        discovery_candidates(&config),
        discovery_timeout(),
        sandbox_available,
    )
    .await;
    let auto = !pick_team && (config.explicit_slots || !interactive);
    let proposal = config.team;
    emit(&Event::HarnessesDiscovered {
        harnesses: report.harnesses.clone(),
        proposal: crate::ipc::TeamProposal {
            one: proposal.one,
            two: proposal.two,
            lead_slot: proposal.lead,
        },
        auto,
    });

    let team = if auto {
        proposal
    } else {
        // Awaiting selection: only select_team (or shutdown) makes
        // progress; an invalid selection is refused with an actionable
        // reason and the core keeps waiting, so a picker can retry.
        loop {
            let Some(line) = lines.next_line().await? else {
                return Ok(());
            };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Command>(&line) {
                Ok(Command::SelectTeam {
                    one,
                    two,
                    lead_slot,
                }) => match validate_selection(
                    &config,
                    &report,
                    &one,
                    &two,
                    &lead_slot,
                    sandbox_available,
                ) {
                    Ok(team) => break team,
                    Err(message) => emit(&Event::Error { message }),
                },
                Ok(Command::Shutdown) => return Ok(()),
                Ok(Command::Initialize { .. }) => emit(&Event::Error {
                    message: "already initialized".to_owned(),
                }),
                Ok(_) => emit(&Event::Error {
                    message: "no team selected yet — send select_team first".to_owned(),
                }),
                Err(e) => emit(&Event::Error {
                    message: format!("invalid command: {e}"),
                }),
            }
        }
    };

    let (consult_tx, mut consult_rx) = mpsc::channel::<ConsultUpdate>(256);
    let (lead_tx, mut lead_rx) = mpsc::channel::<LeadMsg>(256);

    let mut runtime = match Runtime::initialize(
        config,
        team,
        cwd,
        debug,
        consult_tx,
        lead_tx,
        &report,
        sandbox_engine,
    )
    .await
    {
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
        one: Box::new(runtime.one_info.clone()),
        two: Box::new(runtime.two_info.clone()),
        lead_slot: runtime.team().lead,
        cwd: runtime.session.cwd.display().to_string(),
        project: runtime.project,
    });
    for message in &runtime.config.warnings {
        emit(&Event::Warning {
            message: message.clone(),
        });
    }

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
                    Ok(Command::SetModel { slot, model }) => {
                        runtime.set_model(&slot, model).await;
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
                    Ok(Command::SelectTeam { .. }) => {
                        emit(&Event::Error {
                            message: "the team is already selected for this session".to_owned(),
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

    let sandbox_engine = match config.sandbox_mode {
        crate::config::SandboxMode::Auto => crate::sandbox::SandboxSpec::detect_engine(),
        crate::config::SandboxMode::Off => None,
    };

    // Scripted path: discovery still runs (informational) and the
    // configured proposal is always auto-confirmed.
    let report = discovery::discover(
        discovery_candidates(&config),
        discovery_timeout(),
        sandbox_engine.is_some(),
    )
    .await;
    let team = config.team;
    emit(&Event::HarnessesDiscovered {
        harnesses: report.harnesses.clone(),
        proposal: crate::ipc::TeamProposal {
            one: team.one,
            two: team.two,
            lead_slot: team.lead,
        },
        auto: true,
    });
    let mut runtime = Runtime::initialize(
        config,
        team,
        cwd,
        options.debug,
        consult_tx,
        lead_tx,
        &report,
        sandbox_engine,
    )
    .await?;

    emit(&Event::Ready {
        protocol: PROTOCOL_VERSION,
        session_id: runtime.session.id.to_string(),
        one: Box::new(runtime.one_info.clone()),
        two: Box::new(runtime.two_info.clone()),
        lead_slot: runtime.team().lead,
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
