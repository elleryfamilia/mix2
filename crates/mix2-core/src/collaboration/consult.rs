use crate::agents::agent::{Agent, AgentRequest};
use crate::agents::{AgentEvent, AgentRole, HarnessKind, Team};
use crate::collaboration::disagreement::{self, DisagreementRecord};
use crate::collaboration::limits::ConsultBudget;
use crate::collaboration::prompts;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Absolute ceiling on nesting, enforced in code (not prompts): the lead runs
/// at depth 0, a consulted teammate at depth 1, and nothing may consult from
/// depth >= 1.
pub const MAX_DEPTH: u32 = 1;

pub const SOCKET_NAME: &str = "consult.sock";
pub const FILE_DIR_NAME: &str = "consult";

pub const REFUSAL_TEAMMATE: &str =
    "Consultation unavailable: this agent is already running as a mix2 teammate. \
     Complete your independent analysis without delegating.";

pub fn budget_exhausted_message() -> String {
    "The teammate consultation budget for this turn has been exhausted.\n\n\
     Resolve the remaining question yourself or explain the unresolved \
     disagreement to the user."
        .to_owned()
}

/// Refusal when the lead tries to record a split before it has heard from the
/// teammate: without a completed consultation there is nothing to disagree
/// about, so the honest fallback is prose.
pub const REFUSAL_NO_COMPLETED_CONSULT: &str =
    "no completed consultation this turn — disclose the disagreement in prose instead.";

/// Refusal once the per-turn revision cap is spent.
pub const REFUSAL_REVISION_LIMIT: &str =
    "revision limit reached — the earlier record stands; note any change in prose.";

/// Reply to an accepted record. It tells the lead the split is already on
/// screen so the final answer does not repeat it at length.
pub const DISAGREEMENT_RECORDED: &str =
    "Recorded — the interface renders the split beside your answer. In your final answer, \
     cover the disagreement itself in at most one sentence (the team's call); the rest of \
     your answer is unaffected.";

/// How many times one turn's disagreement may be rewritten. Revisions 1..=3
/// are accepted; further distinct records are refused.
pub const MAX_DISAGREEMENT_REVISIONS: u32 = 3;

pub fn teammate_unavailable_message(teammate: HarnessKind, reason: &str) -> String {
    format!(
        "{} is unavailable, so a second opinion could not be obtained ({reason}). \
         Continue with your own analysis.",
        teammate.display_name()
    )
}

/// Request sent by `mix2-consult` over the socket or file transport.
///
/// `mode` selects between the blocking flow and the concurrent flow:
/// - `sync` (default): run the consultation and reply with the result.
/// - `start`: launch the consultation and reply immediately with a ticket,
///   so the caller can keep doing its own research in parallel.
/// - `wait`: block until the ticketed consultation finishes, then reply.
#[derive(Debug, Serialize, Deserialize)]
pub struct ConsultRequest {
    pub v: u32,
    #[serde(default)]
    pub prompt: String,
    /// MIX2_ROLE of the calling agent process.
    pub role: String,
    /// MIX2_DEPTH of the calling agent process.
    pub depth: u32,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub ticket: Option<String>,
    /// Per-turn capability token. Injected only into the coordinator's
    /// environment, so role/depth claims are not the authorization.
    #[serde(default)]
    pub token: Option<String>,
    /// Raw `disagree` payload, forwarded verbatim by the helper. Absent for
    /// every other mode; the grammar is parsed here, server-side.
    #[serde(default)]
    pub disagreement_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsultResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
}

/// Progress updates surfaced to the runtime while consultations run.
#[derive(Debug)]
pub enum ConsultUpdate {
    Started {
        turn_id: Uuid,
        index: u32,
        max: u32,
        /// The lead's written consultation prompt (shown in the team panel).
        prompt: String,
    },
    AgentEvent {
        turn_id: Uuid,
        event: AgentEvent,
    },
    Completed {
        turn_id: Uuid,
        index: u32,
        duration_ms: u64,
        text: String,
    },
    Failed {
        turn_id: Uuid,
        index: u32,
        message: String,
    },
    /// A disagreement was committed for this turn. Live-only: the settled UI
    /// reads the record `end_turn` hands to `finish_turn` instead.
    DisagreementRecorded {
        turn_id: Uuid,
        record: DisagreementRecord,
        revision: u32,
    },
}

pub struct ActiveTurn {
    pub turn_id: Uuid,
    pub budget: Arc<ConsultBudget>,
    pub cancel: CancellationToken,
    /// Capability required in every consult request this turn.
    pub token: String,
    /// Consultations that returned a teammate answer this turn. Incremented
    /// inside the consult task before the result is delivered, so it gates
    /// disagreement recording without depending on the update channel
    /// draining.
    pub completed_consults: Arc<AtomicU32>,
    /// The turn's recorded split and its revision (first record is 1).
    /// A `std` mutex on purpose: the whole record transaction runs without an
    /// await, inside the `active` read guard.
    pub disagreement: Arc<StdMutex<Option<(DisagreementRecord, u32)>>>,
}

pub struct ConsultServer {
    shared: Arc<Shared>,
}

struct Shared {
    teammate: Arc<dyn Agent>,
    /// The resolved team shape: slot harnesses + lead slot.
    team: Team,
    cwd: PathBuf,
    runtime_dir: PathBuf,
    session_id: Uuid,
    /// Directory containing `mix2-consult`, prepended to the teammate's
    /// PATH too — so a misbehaving teammate that tries to consult receives
    /// the explicit refusal instead of a confusing "command not found".
    helper_dir: Option<PathBuf>,
    /// Whether the cwd looks like a software project (teammate context).
    project: bool,
    /// Model override for teammate invocations (user /model selection).
    teammate_model: RwLock<Option<String>>,
    updates: mpsc::Sender<ConsultUpdate>,
    /// In-flight `start`ed consultations by ticket. Each holds a watch
    /// channel that flips from None to the final response. Cleared per turn.
    pending:
        tokio::sync::Mutex<HashMap<String, tokio::sync::watch::Receiver<Option<ConsultResponse>>>>,
    active: RwLock<Option<ActiveTurn>>,
    consult_timeout: Duration,
}

impl ConsultServer {
    /// Create the server and start both transports:
    /// - a Unix socket at `<runtime_dir>/consult.sock` (works from Claude
    ///   Code's Bash sandbox),
    /// - a polled request directory at `<runtime_dir>/consult/` (works from
    ///   Codex's workspace-write sandbox, which blocks sockets).
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        teammate: Arc<dyn Agent>,
        team: Team,
        cwd: PathBuf,
        runtime_dir: PathBuf,
        session_id: Uuid,
        helper_dir: Option<PathBuf>,
        project: bool,
        teammate_model: Option<String>,
        updates: mpsc::Sender<ConsultUpdate>,
    ) -> Result<Self> {
        tokio::fs::create_dir_all(runtime_dir.join(FILE_DIR_NAME))
            .await
            .context("failed to create consult runtime dir")?;

        let shared = Arc::new(Shared {
            teammate,
            team,
            cwd,
            runtime_dir: runtime_dir.clone(),
            session_id,
            helper_dir,
            project,
            teammate_model: RwLock::new(teammate_model),
            updates,
            pending: tokio::sync::Mutex::new(HashMap::new()),
            active: RwLock::new(None),
            consult_timeout: Duration::from_secs(15 * 60),
        });

        let sock_path = runtime_dir.join(SOCKET_NAME);
        let _ = tokio::fs::remove_file(&sock_path).await;
        let listener = UnixListener::bind(&sock_path)
            .with_context(|| format!("failed to bind {}", sock_path.display()))?;

        let socket_shared = Arc::clone(&shared);
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let shared = Arc::clone(&socket_shared);
                        tokio::spawn(async move {
                            let (read, mut write) = stream.into_split();
                            let mut lines = BufReader::new(read).lines();
                            if let Ok(Some(line)) = lines.next_line().await {
                                let response = handle_line(&shared, &line).await;
                                let mut payload =
                                    serde_json::to_string(&response).unwrap_or_default();
                                payload.push('\n');
                                let _ = write.write_all(payload.as_bytes()).await;
                                let _ = write.shutdown().await;
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("consult socket accept error: {e}");
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }
            }
        });

        let file_shared = Arc::clone(&shared);
        tokio::spawn(async move {
            file_transport_loop(file_shared).await;
        });

        Ok(Self { shared })
    }

    pub async fn begin_turn(&self, turn: ActiveTurn) {
        *self.shared.active.write().await = Some(turn);
        self.shared.pending.lock().await.clear();
    }

    pub async fn set_teammate_model(&self, model: Option<String>) {
        *self.shared.teammate_model.write().await = model;
    }

    /// Settle the turn and hand back whatever disagreement it recorded.
    ///
    /// The ActiveTurn is taken and its record extracted under ONE write lock.
    /// Because every commit happens inside a read guard, each committed record
    /// strictly happens-before this take: the LATEST committed record is
    /// returned; an earlier record superseded by a revision is not. A request
    /// arriving after it finds no active turn and is refused.
    pub async fn end_turn(&self) -> Option<DisagreementRecord> {
        let record = self.shared.active.write().await.take().and_then(|turn| {
            let mut slot = lock_disagreement(&turn.disagreement);
            slot.take().map(|(record, _revision)| record)
        });
        self.shared.pending.lock().await.clear();
        // Sweep the mailbox: stale req/res/done files from this turn are
        // dead weight and could confuse a future reader.
        let dir = self.shared.runtime_dir.join(FILE_DIR_NAME);
        if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("req-") || name.starts_with("res-") || name.starts_with("done-")
                {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                }
            }
        }
        record
    }
}

/// Lock the record slot, tolerating a poisoned mutex: the data behind it is a
/// plain value, and losing a turn's disagreement to another thread's panic
/// would be a worse outcome than reading it.
fn lock_disagreement(
    slot: &StdMutex<Option<(DisagreementRecord, u32)>>,
) -> std::sync::MutexGuard<'_, Option<(DisagreementRecord, u32)>> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Watch `<runtime_dir>/consult/` for `req-*.json`, answer with
/// `res-<id>.json` (written atomically via rename).
async fn file_transport_loop(shared: Arc<Shared>) {
    let dir = shared.runtime_dir.join(FILE_DIR_NAME);
    let mut seen: HashMap<String, ()> = HashMap::new();
    loop {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name
                .strip_prefix("req-")
                .and_then(|n| n.strip_suffix(".json"))
            else {
                continue;
            };
            if seen.contains_key(id) {
                continue;
            }
            seen.insert(id.to_owned(), ());
            let shared = Arc::clone(&shared);
            let dir = dir.clone();
            let id = id.to_owned();
            let path = entry.path();
            tokio::spawn(async move {
                let line = tokio::fs::read_to_string(&path).await.unwrap_or_default();
                let response = handle_line(&shared, &line).await;
                let payload = serde_json::to_string(&response).unwrap_or_default();
                let tmp = dir.join(format!("res-{id}.json.tmp"));
                let fin = dir.join(format!("res-{id}.json"));
                if tokio::fs::write(&tmp, payload).await.is_ok() {
                    let _ = tokio::fs::rename(&tmp, &fin).await;
                }
                let _ = tokio::fs::remove_file(&path).await;
            });
        }
    }
}

async fn handle_line(shared: &Arc<Shared>, line: &str) -> ConsultResponse {
    let request: ConsultRequest = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return ConsultResponse {
                ok: false,
                text: None,
                error: Some(format!("invalid consult request: {e}")),
                ticket: None,
            }
        }
    };
    handle_request(shared, request).await
}

fn refuse(error: String) -> ConsultResponse {
    ConsultResponse {
        ok: false,
        text: None,
        error: Some(error),
        ticket: None,
    }
}

/// Await a pending consultation's watch channel until its result lands.
async fn await_result(
    mut rx: tokio::sync::watch::Receiver<Option<ConsultResponse>>,
) -> ConsultResponse {
    loop {
        if let Some(response) = rx.borrow().clone() {
            return response;
        }
        if rx.changed().await.is_err() {
            return refuse(
                "Consultation ended without a result (the turn may have been cancelled)."
                    .to_owned(),
            );
        }
    }
}

async fn handle_request(shared: &Arc<Shared>, request: ConsultRequest) -> ConsultResponse {
    if request.v != 1 {
        return refuse(format!(
            "unsupported consult protocol version {}",
            request.v
        ));
    }
    // Recursion prevention is enforced here in code, not only in prompts.
    if request.role == "teammate" {
        return refuse(REFUSAL_TEAMMATE.to_owned());
    }
    if request.depth >= MAX_DEPTH {
        return refuse(format!(
            "Consultation unavailable: maximum collaboration depth ({MAX_DEPTH}) reached."
        ));
    }

    let mode = request.mode.as_deref().unwrap_or("sync");

    if mode == "wait" {
        let authorized = {
            let active = shared.active.read().await;
            active
                .as_ref()
                .is_some_and(|turn| request.token.as_deref() == Some(turn.token.as_str()))
        };
        if !authorized {
            return refuse(
                "Consultation unavailable: this process is not authorized to consult.".to_owned(),
            );
        }
        let Some(ticket) = request.ticket else {
            return refuse("wait requires a consultation ticket.".to_owned());
        };
        let rx = shared.pending.lock().await.get(&ticket).cloned();
        let Some(rx) = rx else {
            return refuse(format!(
                "Unknown consultation ticket {ticket} (it may belong to an earlier turn)."
            ));
        };
        return await_result(rx).await;
    }
    if mode == "disagree" {
        return record_disagreement(shared, &request).await;
    }
    if mode != "sync" && mode != "start" {
        return refuse(format!("Unknown consult mode '{mode}'."));
    }
    if request.prompt.trim().is_empty() {
        return refuse("Consultation failed: empty prompt.".to_owned());
    }

    let (turn_id, budget, cancel, expected_token, completed_consults) = {
        let active = shared.active.read().await;
        match active.as_ref() {
            Some(turn) => (
                turn.turn_id,
                Arc::clone(&turn.budget),
                turn.cancel.clone(),
                turn.token.clone(),
                Arc::clone(&turn.completed_consults),
            ),
            None => return refuse("Consultation unavailable: no active mix2 turn.".to_owned()),
        }
    };
    // Authorization is the capability token, not the caller's role/depth
    // claims: only the coordinator's environment carries it.
    if request.token.as_deref() != Some(expected_token.as_str()) {
        return refuse(
            "Consultation unavailable: this process is not authorized to consult.".to_owned(),
        );
    }

    let teammate = Arc::clone(&shared.teammate);

    let Some(index) = budget.try_acquire() else {
        return refuse(budget_exhausted_message());
    };

    let _ = shared
        .updates
        .send(ConsultUpdate::Started {
            turn_id,
            index,
            max: budget.max(),
            prompt: request.prompt.clone(),
        })
        .await;

    // Run the consultation as a detached task feeding a watch channel, so
    // `start` can return immediately while the caller keeps researching.
    let ticket = Uuid::new_v4().to_string();
    let (result_tx, result_rx) = tokio::sync::watch::channel::<Option<ConsultResponse>>(None);
    shared
        .pending
        .lock()
        .await
        .insert(ticket.clone(), result_rx.clone());

    let task_shared = Arc::clone(shared);
    let prompt = request.prompt.clone();
    let task_ticket = ticket.clone();
    tokio::spawn(async move {
        let started = Instant::now();
        let result = run_consultation(&task_shared, &teammate, turn_id, &prompt, cancel).await;
        let duration_ms = started.elapsed().as_millis() as u64;
        let response = match result {
            Ok(text) => {
                // Open the disagreement gate before the result reaches the
                // lead by any route: update channel, done-file, or watch.
                completed_consults.fetch_add(1, Ordering::SeqCst);
                let _ = task_shared
                    .updates
                    .send(ConsultUpdate::Completed {
                        turn_id,
                        index,
                        duration_ms,
                        text: text.clone(),
                    })
                    .await;
                ConsultResponse {
                    ok: true,
                    text: Some(text),
                    error: None,
                    ticket: None,
                }
            }
            Err(e) => {
                let message = teammate_unavailable_message(
                    task_shared.team.teammate_harness(),
                    &e.to_string(),
                );
                let _ = task_shared
                    .updates
                    .send(ConsultUpdate::Failed {
                        turn_id,
                        index,
                        message: message.clone(),
                    })
                    .await;
                refuse(message)
            }
        };
        // File-transport waiters poll for done-<ticket>.json directly.
        let dir = task_shared.runtime_dir.join(FILE_DIR_NAME);
        let payload = serde_json::to_string(&response).unwrap_or_default();
        let tmp = dir.join(format!("done-{task_ticket}.json.tmp"));
        let fin = dir.join(format!("done-{task_ticket}.json"));
        if tokio::fs::write(&tmp, payload).await.is_ok() {
            let _ = tokio::fs::rename(&tmp, &fin).await;
        }
        let _ = result_tx.send(Some(response));
    });

    if mode == "start" {
        return ConsultResponse {
            ok: true,
            text: None,
            error: None,
            ticket: Some(ticket),
        };
    }
    await_result(result_rx).await
}

/// Handle `mode: "disagree"`: validate, then commit the turn's record.
///
/// Everything that decides the outcome — the gate, the parse, and the revision
/// transaction — runs inside one `active` read guard with no await, so a
/// concurrent `end_turn` either sees the commit or takes the turn away before
/// it starts. The update send happens only after both guards are released.
async fn record_disagreement(shared: &Arc<Shared>, request: &ConsultRequest) -> ConsultResponse {
    let outcome = {
        let active = shared.active.read().await;
        let Some(turn) = active.as_ref() else {
            return refuse("Consultation unavailable: no active mix2 turn.".to_owned());
        };
        if request.token.as_deref() != Some(turn.token.as_str()) {
            return refuse(
                "Consultation unavailable: this process is not authorized to consult.".to_owned(),
            );
        }
        if turn.completed_consults.load(Ordering::SeqCst) == 0 {
            return refuse(REFUSAL_NO_COMPLETED_CONSULT.to_owned());
        }
        let record = match disagreement::parse(
            request.disagreement_text.as_deref().unwrap_or_default(),
            &shared.team,
        ) {
            Ok(record) => record,
            Err(e) => return refuse(disagreement::refusal(&e, &shared.team)),
        };

        let mut slot = lock_disagreement(&turn.disagreement);
        match slot.as_ref() {
            // Re-sending the standing record is a no-op, not a revision:
            // a retrying lead must not burn the cap on an identical payload.
            Some((existing, _)) if *existing == record => None,
            Some((_, revision)) if *revision >= MAX_DISAGREEMENT_REVISIONS => {
                return refuse(REFUSAL_REVISION_LIMIT.to_owned())
            }
            existing => {
                let revision = existing.map_or(1, |(_, revision)| revision + 1);
                *slot = Some((record.clone(), revision));
                Some((turn.turn_id, record, revision))
            }
        }
    };

    if let Some((turn_id, record, revision)) = outcome {
        let _ = shared
            .updates
            .send(ConsultUpdate::DisagreementRecorded {
                turn_id,
                record,
                revision,
            })
            .await;
    }
    ConsultResponse {
        ok: true,
        text: Some(DISAGREEMENT_RECORDED.to_owned()),
        error: None,
        ticket: None,
    }
}

async fn run_consultation(
    shared: &Arc<Shared>,
    teammate: &Arc<dyn Agent>,
    turn_id: Uuid,
    prompt: &str,
    cancel: CancellationToken,
) -> Result<String> {
    let mut env = HashMap::new();
    env.insert("MIX2_ROLE".to_owned(), "teammate".to_owned());
    env.insert("MIX2_DEPTH".to_owned(), "1".to_owned());
    env.insert("MIX2_SESSION_ID".to_owned(), shared.session_id.to_string());
    env.insert("MIX2_TURN_ID".to_owned(), turn_id.to_string());
    env.insert(
        "MIX2_RUNTIME_DIR".to_owned(),
        shared.runtime_dir.display().to_string(),
    );

    let request = AgentRequest {
        prompt: prompt.to_owned(),
        cwd: shared.cwd.clone(),
        role: AgentRole::Teammate,
        turn_id,
        model: shared.teammate_model.read().await.clone(),
        instructions: prompts::teammate_instructions(shared.team, shared.project),
        env,
        path_prepend: shared.helper_dir.clone(),
        runtime_dir: None,
    };

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let updates = shared.updates.clone();
    let forward = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = updates
                .send(ConsultUpdate::AgentEvent { turn_id, event })
                .await;
        }
    });

    // Consultations are fresh sessions on purpose: independence preserves
    // the value of the second opinion. On timeout, cancel a child token
    // and await the adapter so its process TREE is killed — dropping the
    // future would only reap the direct child and could leak descendants.
    let consult_cancel = cancel.child_token();
    let start = teammate.start(request, tx, consult_cancel.clone());
    tokio::pin!(start);
    let result = match tokio::time::timeout(shared.consult_timeout, &mut start).await {
        Ok(result) => result,
        Err(_) => {
            consult_cancel.cancel();
            let _ = start.await;
            let _ = forward.await;
            anyhow::bail!("consultation timed out");
        }
    };
    let _ = forward.await;

    match result {
        Ok(result) => Ok(result.text),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::agent::{AgentResult, AgentSession, AgentVersion};
    use crate::agents::SlotId;
    use crate::collaboration::disagreement::{Outcome, DISAGREE_EXAMPLE};
    use async_trait::async_trait;
    use std::sync::atomic::AtomicU32;
    use std::sync::Mutex as StdMutex;

    const TOKEN: &str = "turn-token";

    const SPLIT_A: &str = "claude: cache the compiled schema in-process | chosen\n\
                           codex: move validation off the hot path | deferred\n\
                           team: ship the cache now; file the rework as a follow-up";
    const SPLIT_B: &str = "claude: cache the compiled schema in-process | chosen\n\
                           codex: precompute the schema at build time | dropped\n\
                           team: ship the cache now";
    const SPLIT_C: &str = "claude: keep the schema in a global | chosen\n\
                           codex: move validation off the hot path | deferred\n\
                           team: global for now";
    const SPLIT_D: &str = "claude: parse the schema lazily | chosen\n\
                           codex: move validation off the hot path | dropped\n\
                           team: lazy parse";

    /// Instant, quota-free teammate: consultations complete as soon as they
    /// start, so the completion gate is deterministic in tests.
    struct StubTeammate;

    #[async_trait]
    impl Agent for StubTeammate {
        fn harness(&self) -> HarnessKind {
            HarnessKind::Codex
        }

        async fn version(&self) -> Result<AgentVersion> {
            Ok(AgentVersion {
                raw: "stub".to_owned(),
            })
        }

        async fn start(
            &self,
            _request: AgentRequest,
            _events: mpsc::Sender<AgentEvent>,
            _cancel: CancellationToken,
        ) -> Result<AgentResult> {
            Ok(AgentResult {
                text: "teammate opinion".to_owned(),
                session_id: None,
            })
        }

        async fn resume(
            &self,
            _session: &AgentSession,
            request: AgentRequest,
            events: mpsc::Sender<AgentEvent>,
            cancel: CancellationToken,
        ) -> Result<AgentResult> {
            self.start(request, events, cancel).await
        }
    }

    struct Harness {
        server: ConsultServer,
        shared: Arc<Shared>,
        updates: mpsc::Receiver<ConsultUpdate>,
        _dir: tempfile::TempDir,
    }

    impl Harness {
        async fn new() -> Self {
            let dir = tempfile::TempDir::new().unwrap();
            let runtime_dir = dir.path().to_path_buf();
            tokio::fs::create_dir_all(runtime_dir.join(FILE_DIR_NAME))
                .await
                .unwrap();
            let (tx, updates) = mpsc::channel(64);
            let shared = Arc::new(Shared {
                teammate: Arc::new(StubTeammate),
                team: Team {
                    one: HarnessKind::Claude,
                    two: HarnessKind::Codex,
                    lead: SlotId::One,
                },
                cwd: runtime_dir.clone(),
                runtime_dir,
                session_id: Uuid::new_v4(),
                helper_dir: None,
                project: false,
                teammate_model: RwLock::new(None),
                updates: tx,
                pending: tokio::sync::Mutex::new(HashMap::new()),
                active: RwLock::new(None),
                consult_timeout: Duration::from_secs(30),
            });
            Self {
                server: ConsultServer {
                    shared: Arc::clone(&shared),
                },
                shared,
                updates,
                _dir: dir,
            }
        }

        /// Begin a turn whose completion gate is already open (`completed`
        /// consultations recorded) without running a consultation.
        async fn begin_turn(&self, completed: u32) -> Uuid {
            let turn_id = Uuid::new_v4();
            self.server
                .begin_turn(ActiveTurn {
                    turn_id,
                    budget: Arc::new(ConsultBudget::new(2)),
                    cancel: CancellationToken::new(),
                    token: TOKEN.to_owned(),
                    completed_consults: Arc::new(AtomicU32::new(completed)),
                    disagreement: Arc::new(StdMutex::new(None)),
                })
                .await;
            turn_id
        }

        async fn disagree(&self, text: &str) -> ConsultResponse {
            handle_request(&self.shared, disagree_request(text, Some(TOKEN))).await
        }

        /// The turn's stored `(record, revision)`, read straight from the
        /// ActiveTurn so tests assert on committed state, not on responses.
        async fn stored(&self) -> Option<(DisagreementRecord, u32)> {
            let active = self.shared.active.read().await;
            let turn = active.as_ref()?;
            let stored = turn.disagreement.lock().unwrap().clone();
            stored
        }

        fn drain(&mut self) -> Vec<ConsultUpdate> {
            let mut out = Vec::new();
            while let Ok(update) = self.updates.try_recv() {
                out.push(update);
            }
            out
        }
    }

    fn disagree_request(text: &str, token: Option<&str>) -> ConsultRequest {
        ConsultRequest {
            v: 1,
            prompt: String::new(),
            role: "lead".to_owned(),
            depth: 0,
            mode: Some("disagree".to_owned()),
            ticket: None,
            token: token.map(str::to_owned),
            disagreement_text: Some(text.to_owned()),
        }
    }

    fn consult_request() -> ConsultRequest {
        ConsultRequest {
            v: 1,
            prompt: "what do you think?".to_owned(),
            role: "lead".to_owned(),
            depth: 0,
            mode: Some("sync".to_owned()),
            ticket: None,
            token: Some(TOKEN.to_owned()),
            disagreement_text: None,
        }
    }

    #[tokio::test]
    async fn disagree_refused_without_completed_consult() {
        let mut h = Harness::new().await;
        h.begin_turn(0).await;

        let response = h.disagree(SPLIT_A).await;

        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some(
                "no completed consultation this turn — disclose the disagreement in prose instead."
            )
        );
        assert!(h.stored().await.is_none());
        assert!(h.drain().is_empty(), "no update for a refused record");
    }

    #[tokio::test]
    async fn disagree_records_after_completed_consult() {
        let mut h = Harness::new().await;
        let turn_id = h.begin_turn(0).await;

        // Drive a real consultation: the gate must open from inside the
        // spawned consult task, not from a test-only counter poke.
        let consulted = handle_request(&h.shared, consult_request()).await;
        assert!(consulted.ok, "stub consultation should succeed");

        let response = h.disagree(SPLIT_A).await;

        assert!(response.ok, "error: {:?}", response.error);
        assert_eq!(
            response.text.as_deref(),
            Some(
                "Recorded — the interface renders the split beside your answer. In your final \
                 answer, cover the disagreement itself in at most one sentence (the team's call); \
                 the rest of your answer is unaffected."
            )
        );

        let (record, revision) = h.stored().await.expect("record committed");
        assert_eq!(revision, 1);
        assert_eq!(record.stances.len(), 2);
        assert_eq!(record.stances[0].slot, SlotId::One);
        assert_eq!(record.stances[1].outcome, Outcome::Deferred);

        let recorded: Vec<_> = h
            .drain()
            .into_iter()
            .filter_map(|u| match u {
                ConsultUpdate::DisagreementRecorded {
                    turn_id,
                    record,
                    revision,
                } => Some((turn_id, record, revision)),
                _ => None,
            })
            .collect();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, turn_id);
        assert_eq!(recorded[0].1, record);
        assert_eq!(recorded[0].2, 1);
    }

    #[tokio::test]
    async fn disagree_identical_rerecord_is_idempotent() {
        let mut h = Harness::new().await;
        h.begin_turn(1).await;

        assert!(h.disagree(SPLIT_A).await.ok);
        let first = h.drain();
        assert_eq!(first.len(), 1);

        // Same content, different whitespace: the parsed record is equal, so
        // the re-record is a no-op rather than a revision.
        let response = h
            .disagree(
                "  claude: cache the compiled schema in-process   |  chosen \n\
                       \n\
                       codex: move validation off the hot path | deferred\n\
                       team: ship the cache now; file the rework as a follow-up",
            )
            .await;

        assert!(response.ok, "error: {:?}", response.error);
        assert_eq!(h.stored().await.unwrap().1, 1, "revision unchanged");
        assert!(h.drain().is_empty(), "idempotent re-record emits no update");
    }

    #[tokio::test]
    async fn disagree_distinct_rerecord_bumps_revision_and_caps_at_3() {
        let mut h = Harness::new().await;
        h.begin_turn(1).await;

        for (text, expected) in [(SPLIT_A, 1), (SPLIT_B, 2), (SPLIT_C, 3)] {
            let response = h.disagree(text).await;
            assert!(response.ok, "error: {:?}", response.error);
            assert_eq!(h.stored().await.unwrap().1, expected);
        }
        assert_eq!(h.drain().len(), 3, "one update per accepted revision");

        let response = h.disagree(SPLIT_D).await;
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("revision limit reached — the earlier record stands; note any change in prose.")
        );

        let (record, revision) = h.stored().await.unwrap();
        assert_eq!(revision, 3, "the third record still stands");
        assert_eq!(record.stances[0].position, "keep the schema in a global");
        assert!(h.drain().is_empty());

        // The cap does not break idempotency: re-sending the standing record
        // is still an accepted no-op.
        assert!(h.disagree(SPLIT_C).await.ok);
    }

    #[tokio::test]
    async fn disagree_requires_token_and_valid_agents() {
        let mut h = Harness::new().await;

        // No active turn at all.
        let response = h.disagree(SPLIT_A).await;
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("Consultation unavailable: no active mix2 turn.")
        );

        h.begin_turn(1).await;

        for token in [None, Some("wrong-token")] {
            let response = handle_request(&h.shared, disagree_request(SPLIT_A, token)).await;
            assert!(!response.ok);
            assert_eq!(
                response.error.as_deref(),
                Some("Consultation unavailable: this process is not authorized to consult.")
            );
        }

        // A teammate process may not record, whatever it claims.
        let mut from_teammate = disagree_request(SPLIT_A, Some(TOKEN));
        from_teammate.role = "teammate".to_owned();
        assert_eq!(
            handle_request(&h.shared, from_teammate)
                .await
                .error
                .as_deref(),
            Some(REFUSAL_TEAMMATE)
        );

        // Agent names must be this session's lead and teammate.
        let response = h
            .disagree(
                "claude: a | chosen\n\
                 gemini: b | deferred\n\
                 team: c",
            )
            .await;
        assert!(!response.ok);
        assert!(response
            .error
            .as_deref()
            .unwrap()
            .contains("each agent needs exactly one line"));

        assert!(h.stored().await.is_none());
        assert!(h.drain().is_empty());
    }

    #[tokio::test]
    async fn end_turn_returns_the_record_and_then_refuses_new_ones() {
        let mut h = Harness::new().await;
        h.begin_turn(1).await;
        assert!(h.disagree(SPLIT_A).await.ok);
        let _ = h.drain();

        let record = h.server.end_turn().await.expect("record handed to caller");
        assert_eq!(record.stances.len(), 2);
        assert_eq!(
            record.resolution,
            "ship the cache now; file the rework as a follow-up"
        );

        // After the take there is no turn, so a late record is refused.
        let response = h.disagree(SPLIT_B).await;
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("Consultation unavailable: no active mix2 turn.")
        );

        assert!(
            h.server.end_turn().await.is_none(),
            "records are taken once"
        );

        // A turn with no disagreement settles to None.
        h.begin_turn(1).await;
        assert!(h.server.end_turn().await.is_none());
    }

    #[tokio::test]
    async fn disagree_parse_error_refusal_embeds_example() {
        let mut h = Harness::new().await;
        h.begin_turn(1).await;

        let response = h
            .disagree(
                "claude: a | maybe\n\
                 codex: b | deferred\n\
                 team: c",
            )
            .await;

        assert!(!response.ok);
        let error = response.error.unwrap();
        assert!(error.contains("maybe"), "names the offending tail: {error}");
        assert!(
            error.contains(DISAGREE_EXAMPLE),
            "embeds the worked example"
        );
        assert!(error.contains("If this fails twice"), "bounded retry");
        assert!(h.stored().await.is_none());
        assert!(h.drain().is_empty());
    }

    #[tokio::test]
    async fn empty_disagreement_text_is_refused_not_panicked() {
        let mut h = Harness::new().await;
        h.begin_turn(1).await;

        let mut request = disagree_request("", Some(TOKEN));
        request.disagreement_text = None;
        let response = handle_request(&h.shared, request).await;

        assert!(!response.ok);
        assert!(response.error.unwrap().contains(DISAGREE_EXAMPLE));
        assert!(h.drain().is_empty());
    }

    #[tokio::test]
    async fn completed_consult_counter_survives_a_failed_consultation() {
        // Only the Ok branch opens the gate: a request for a mode the server
        // does not run must not count as a completed consultation.
        let h = Harness::new().await;
        h.begin_turn(0).await;

        let mut request = consult_request();
        request.mode = Some("bogus".to_owned());
        assert!(!handle_request(&h.shared, request).await.ok);

        let response = h.disagree(SPLIT_A).await;
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some(
                "no completed consultation this turn — disclose the disagreement in prose instead."
            )
        );
    }
}
