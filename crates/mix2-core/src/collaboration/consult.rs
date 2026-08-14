use crate::agents::agent::{Agent, AgentRequest};
use crate::agents::{AgentEvent, AgentKind, AgentRole};
use crate::collaboration::limits::ConsultBudget;
use crate::collaboration::prompts;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
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

pub fn teammate_unavailable_message(teammate: AgentKind, reason: &str) -> String {
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
}

pub struct ActiveTurn {
    pub turn_id: Uuid,
    pub budget: Arc<ConsultBudget>,
    pub cancel: CancellationToken,
}

pub struct ConsultServer {
    shared: Arc<Shared>,
}

struct Shared {
    teammate: Option<Arc<dyn Agent>>,
    teammate_kind: AgentKind,
    teammate_unavailable_reason: Option<String>,
    lead_kind: AgentKind,
    cwd: PathBuf,
    runtime_dir: PathBuf,
    session_id: Uuid,
    /// Directory containing `mix2-consult`, prepended to the teammate's
    /// PATH too — so a misbehaving teammate that tries to consult receives
    /// the explicit refusal instead of a confusing "command not found".
    helper_dir: Option<PathBuf>,
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
        teammate: Option<Arc<dyn Agent>>,
        teammate_kind: AgentKind,
        teammate_unavailable_reason: Option<String>,
        lead_kind: AgentKind,
        cwd: PathBuf,
        runtime_dir: PathBuf,
        session_id: Uuid,
        helper_dir: Option<PathBuf>,
        updates: mpsc::Sender<ConsultUpdate>,
    ) -> Result<Self> {
        tokio::fs::create_dir_all(runtime_dir.join(FILE_DIR_NAME))
            .await
            .context("failed to create consult runtime dir")?;

        let shared = Arc::new(Shared {
            teammate,
            teammate_kind,
            teammate_unavailable_reason,
            lead_kind,
            cwd,
            runtime_dir: runtime_dir.clone(),
            session_id,
            helper_dir,
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

    pub async fn end_turn(&self) {
        *self.shared.active.write().await = None;
        self.shared.pending.lock().await.clear();
    }
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
    if mode != "sync" && mode != "start" {
        return refuse(format!("Unknown consult mode '{mode}'."));
    }
    if request.prompt.trim().is_empty() {
        return refuse("Consultation failed: empty prompt.".to_owned());
    }

    let (turn_id, budget, cancel) = {
        let active = shared.active.read().await;
        match active.as_ref() {
            Some(turn) => (turn.turn_id, Arc::clone(&turn.budget), turn.cancel.clone()),
            None => return refuse("Consultation unavailable: no active mix2 turn.".to_owned()),
        }
    };

    let Some(teammate) = shared.teammate.clone() else {
        let reason = shared
            .teammate_unavailable_reason
            .clone()
            .unwrap_or_else(|| "not installed".to_owned());
        let msg = teammate_unavailable_message(shared.teammate_kind, &reason);
        let _ = shared
            .updates
            .send(ConsultUpdate::Failed {
                turn_id,
                index: budget.used() + 1,
                message: msg.clone(),
            })
            .await;
        return refuse(msg);
    };

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
                let message =
                    teammate_unavailable_message(task_shared.teammate_kind, &e.to_string());
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
        instructions: prompts::teammate_instructions(shared.lead_kind, shared.teammate_kind),
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
    // the value of the second opinion.
    let result =
        tokio::time::timeout(shared.consult_timeout, teammate.start(request, tx, cancel)).await;
    let _ = forward.await;

    match result {
        Ok(Ok(result)) => Ok(result.text),
        Ok(Err(e)) => Err(e),
        Err(_) => anyhow::bail!("consultation timed out"),
    }
}
