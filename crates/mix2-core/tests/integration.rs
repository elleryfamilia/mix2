//! End-to-end tests of the mix2-core serve loop against the fake provider
//! fixtures in tests/fixtures. No real Claude/Codex quota is ever used.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .canonicalize()
        .expect("fixtures dir exists")
}

/// Harness around a `mix2-core serve` child speaking JSONL on stdio.
struct Core {
    child: Child,
    stdin: std::process::ChildStdin,
    events: mpsc::Receiver<serde_json::Value>,
}

struct CoreOptions {
    lead: &'static str,
    claude_cmd: Option<String>,
    codex_cmd: Option<String>,
    max_consults: Option<u32>,
}

impl Default for CoreOptions {
    fn default() -> Self {
        Self {
            lead: "claude",
            claude_cmd: Some(fixtures_dir().join("fake-claude").display().to_string()),
            codex_cmd: Some(fixtures_dir().join("fake-codex").display().to_string()),
            max_consults: None,
        }
    }
}

impl Core {
    fn start(options: CoreOptions) -> Self {
        // Ensure the consult helper is built and resolvable next to the core
        // binary (runtime prepends its own directory to the lead's PATH).
        let _ = env!("CARGO_BIN_EXE_mix2-consult");

        let config_dir = tempfile::tempdir().expect("tempdir");
        let config_path = config_dir.path().join("config.toml");
        let mut config = String::new();
        if let Some(max) = options.max_consults {
            config.push_str(&format!("[collaboration]\nmax_consults_per_turn = {max}\n"));
        }
        std::fs::write(&config_path, config).expect("write config");
        // Leak the tempdir so the config outlives the child.
        std::mem::forget(config_dir);

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mix2-core"));
        cmd.args(["serve", "--config", config_path.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("MIX2_CLAUDE_CMD")
            .env_remove("MIX2_CODEX_CMD");
        if let Some(claude) = &options.claude_cmd {
            cmd.env("MIX2_CLAUDE_CMD", claude);
        }
        if let Some(codex) = &options.codex_cmd {
            cmd.env("MIX2_CODEX_CMD", codex);
        }

        let mut child = cmd.spawn().expect("spawn mix2-core");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });

        let mut core = Self {
            child,
            stdin,
            events: rx,
        };
        core.send(&serde_json::json!({
            "type": "initialize", "protocol": 1, "lead": options.lead,
            "cwd": std::env::current_dir().unwrap().display().to_string(),
        }));
        core
    }

    fn send(&mut self, value: &serde_json::Value) {
        writeln!(self.stdin, "{value}").expect("write command");
        self.stdin.flush().expect("flush");
    }

    fn submit(&mut self, id: &str, text: &str) {
        self.send(&serde_json::json!({"type": "submit", "id": id, "text": text}));
    }

    /// Collect events until one matches `stop`, or panic on timeout.
    fn events_until(&self, stop: &str, timeout: Duration) -> Vec<serde_json::Value> {
        let deadline = Instant::now() + timeout;
        let mut out = Vec::new();
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| panic!("timeout waiting for `{stop}`; got: {:#?}", types(&out)));
            match self.events.recv_timeout(remaining) {
                Ok(event) => {
                    let is_stop = event["type"] == stop;
                    out.push(event);
                    if is_stop {
                        return out;
                    }
                }
                Err(_) => panic!("timeout waiting for `{stop}`; got: {:#?}", types(&out)),
            }
        }
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        let _ = self.send_shutdown();
        let _ = self.child.wait();
    }
}

impl Core {
    fn send_shutdown(&mut self) -> std::io::Result<()> {
        writeln!(self.stdin, r#"{{"type":"shutdown"}}"#)?;
        self.stdin.flush()
    }
}

fn types(events: &[serde_json::Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| e["type"].as_str().unwrap_or("?").to_owned())
        .collect()
}

fn find<'a>(events: &'a [serde_json::Value], ty: &str) -> Option<&'a serde_json::Value> {
    events.iter().find(|e| e["type"] == ty)
}

fn count(events: &[serde_json::Value], ty: &str) -> usize {
    events.iter().filter(|e| e["type"] == ty).count()
}

const LONG: Duration = Duration::from_secs(60);

#[test]
fn greeting_uses_lead_only() {
    let mut core = Core::start(CoreOptions::default());
    let startup = core.events_until("ready", LONG);
    let ready = find(&startup, "ready").unwrap();
    assert_eq!(ready["lead"]["kind"], "claude");
    assert_eq!(ready["teammate"]["kind"], "codex");
    assert_eq!(ready["teammate"]["available"], true);

    core.submit("t1", "hi");
    let events = core.events_until("turn.completed", LONG);
    assert!(find(&events, "message.user").is_some());
    assert!(find(&events, "agent.started").is_some());
    assert_eq!(count(&events, "consult.started"), 0);
    let final_msg = find(&events, "message.final").unwrap();
    assert_eq!(final_msg["speaker"], "claude");
    let text = final_msg["text"].as_str().unwrap();
    assert!(text.contains("[resumed:no]"), "unexpected text: {text}");
    assert!(
        text.contains("[role:lead]"),
        "lead instructions not injected: {text}"
    );
    assert!(
        text.contains("[scratchpad:rw]"),
        "lead should get scratchpad-scoped write permission: {text}"
    );
}

#[test]
fn claude_lead_consults_codex_and_answer_is_team() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);

    core.submit("t1", "SCENARIO:consult what do both of you think?");
    let events = core.events_until("turn.completed", LONG);

    let started = find(&events, "consult.started").unwrap();
    assert_eq!(started["agent"], "codex");
    assert_eq!(started["index"], 1);
    assert_eq!(started["max"], 2);

    let completed = find(&events, "consult.completed").unwrap();
    let teammate_text = completed["text"].as_str().unwrap();
    assert!(teammate_text.contains("fake-codex reply"));
    assert!(
        teammate_text.contains("[role:teammate]"),
        "teammate instructions not injected: {teammate_text}"
    );

    assert!(find(&events, "lead.synthesizing").is_some());

    let final_msg = find(&events, "message.final").unwrap();
    assert_eq!(final_msg["speaker"], "team");
    assert_eq!(final_msg["consultations"], 1);
    let text = final_msg["text"].as_str().unwrap();
    assert!(
        text.contains("[consult1:ok:"),
        "lead did not receive reply: {text}"
    );
}

#[test]
fn codex_lead_consults_claude() {
    let mut core = Core::start(CoreOptions {
        lead: "codex",
        ..CoreOptions::default()
    });
    let startup = core.events_until("ready", LONG);
    let ready = find(&startup, "ready").unwrap();
    assert_eq!(ready["lead"]["kind"], "codex");
    assert_eq!(ready["teammate"]["kind"], "claude");

    core.submit("t1", "SCENARIO:consult review this");
    let events = core.events_until("turn.completed", LONG);
    let completed = find(&events, "consult.completed").unwrap();
    assert_eq!(completed["agent"], "claude");
    let teammate_text = completed["text"].as_str().unwrap();
    assert!(teammate_text.contains("fake-claude reply"));
    assert!(
        teammate_text.contains("[scratchpad:ro]"),
        "teammates must not get scratchpad write permission: {teammate_text}"
    );
    let final_msg = find(&events, "message.final").unwrap();
    assert_eq!(final_msg["speaker"], "team");
    // Codex lead runs with the workspace-write sandbox for consult IPC.
    assert!(final_msg["text"]
        .as_str()
        .unwrap()
        .contains("[sandbox:\"workspace-write\"]"));
}

#[test]
fn concurrent_consultation_via_start_and_wait() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);

    core.submit("t1", "SCENARIO:consult_async evaluate concurrently");
    let events = core.events_until("turn.completed", LONG);

    // The consultation ran and completed while the lead kept working.
    assert_eq!(count(&events, "consult.started"), 1);
    assert_eq!(count(&events, "consult.completed"), 1);
    let final_msg = find(&events, "message.final").unwrap();
    assert_eq!(final_msg["speaker"], "team");
    let text = final_msg["text"].as_str().unwrap();
    assert!(
        text.contains("[own-research:done]"),
        "lead should have researched between start and wait: {text}"
    );
    assert!(
        text.contains("[consult1:ok:fake-codex reply"),
        "wait should return the teammate's response: {text}"
    );
}

#[test]
fn async_consultations_share_the_budget() {
    let mut core = Core::start(CoreOptions {
        max_consults: Some(1),
        ..CoreOptions::default()
    });
    core.events_until("ready", LONG);

    // consult_async uses the one slot via `start`; a second sync consult in
    // the same turn must be refused.
    core.submit("t1", "SCENARIO:consult_async SCENARIO:consult budget check");
    let events = core.events_until("turn.completed", LONG);
    assert_eq!(count(&events, "consult.completed"), 1);
    let text = find(&events, "message.final").unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        text.contains("budget"),
        "second consult should hit the budget message: {text}"
    );
}

#[test]
fn consultation_budget_is_enforced() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);

    core.submit("t1", "SCENARIO:consult_thrice hard problem");
    let events = core.events_until("turn.completed", LONG);

    assert_eq!(
        count(&events, "consult.completed"),
        2,
        "budget must cap at 2"
    );
    let final_msg = find(&events, "message.final").unwrap();
    let text = final_msg["text"].as_str().unwrap();
    assert!(text.contains("[consult1:ok:"));
    assert!(text.contains("[consult2:ok:"));
    assert!(
        text.contains("[consult3:err:") && text.contains("budget"),
        "third consult should be refused with the budget message: {text}"
    );
    assert_eq!(find(&events, "message.final").unwrap()["consultations"], 2);
}

#[test]
fn custom_budget_from_config() {
    let mut core = Core::start(CoreOptions {
        max_consults: Some(1),
        ..CoreOptions::default()
    });
    core.events_until("ready", LONG);
    core.submit("t1", "SCENARIO:consult_twice tricky");
    let events = core.events_until("turn.completed", LONG);
    assert_eq!(count(&events, "consult.completed"), 1);
    let started = find(&events, "consult.started").unwrap();
    assert_eq!(started["max"], 1);
}

#[test]
fn teammate_cannot_recurse() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);

    // The lead consults; the consultation prompt itself carries
    // SCENARIO:consult, so the fake teammate tries to call mix2-consult
    // from its teammate context. The runtime must refuse.
    core.submit("t1", "SCENARIO:consult CONSULT_PROMPT:SCENARIO:consult");
    let events = core.events_until("turn.completed", LONG);

    assert_eq!(count(&events, "consult.completed"), 1);
    let completed = find(&events, "consult.completed").unwrap();
    let teammate_text = completed["text"].as_str().unwrap();
    assert!(
        teammate_text.contains("[consult1:err:Consultation unavailable"),
        "teammate consult attempt must be refused: {teammate_text}"
    );
}

#[test]
fn failed_consultation_does_not_fail_the_lead() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);

    core.submit("t1", "SCENARIO:consult CONSULT_PROMPT:SCENARIO:fail");
    let events = core.events_until("turn.completed", LONG);

    assert_eq!(count(&events, "consult.completed"), 0);
    assert_eq!(count(&events, "consult.failed"), 1);
    let failed = find(&events, "consult.failed").unwrap();
    assert!(failed["message"].as_str().unwrap().contains("unavailable"));

    // The lead still answered; attribution stays solo because no
    // consultation actually succeeded.
    let final_msg = find(&events, "message.final").unwrap();
    assert_eq!(final_msg["speaker"], "claude");
    assert!(final_msg["text"]
        .as_str()
        .unwrap()
        .contains("[consult1:err:"));
}

#[test]
fn missing_teammate_degrades_gracefully() {
    let mut core = Core::start(CoreOptions {
        codex_cmd: Some("/nonexistent/codex-binary".into()),
        ..CoreOptions::default()
    });
    let startup = core.events_until("ready", LONG);
    let ready = find(&startup, "ready").unwrap();
    assert_eq!(ready["teammate"]["available"], false);
    assert!(ready["teammate"]["reason"].is_string());

    core.submit("t1", "SCENARIO:consult still try");
    let events = core.events_until("turn.completed", LONG);
    assert_eq!(count(&events, "consult.completed"), 0);
    let final_msg = find(&events, "message.final").unwrap();
    assert_eq!(final_msg["speaker"], "claude");
    let text = final_msg["text"].as_str().unwrap();
    assert!(
        text.contains("[consult1:err:Codex is unavailable"),
        "lead should get the unavailability message: {text}"
    );
}

#[test]
fn missing_lead_is_fatal() {
    let core = Core::start(CoreOptions {
        claude_cmd: Some("/nonexistent/claude-binary".into()),
        ..CoreOptions::default()
    });
    let events = core.events_until("fatal", LONG);
    let fatal = find(&events, "fatal").unwrap();
    assert!(fatal["message"].as_str().unwrap().contains("Claude"));
}

#[test]
fn follow_up_resumes_the_same_lead_session() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);

    core.submit("t1", "first question");
    let first = core.events_until("turn.completed", LONG);
    let text1 = find(&first, "message.final").unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(text1.contains("[resumed:no]"));
    let session1 = extract_marker(&text1, "session");

    core.submit("t2", "follow-up question");
    let second = core.events_until("turn.completed", LONG);
    let text2 = find(&second, "message.final").unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        text2.contains("[resumed:yes]"),
        "second turn must resume: {text2}"
    );
    let session2 = extract_marker(&text2, "session");
    assert_eq!(
        session1, session2,
        "resume must target the same provider session"
    );
}

#[test]
fn new_mix2_session_does_not_resume_old_provider_session() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);
    core.submit("t1", "hello");
    core.events_until("turn.completed", LONG);
    drop(core);

    let mut fresh = Core::start(CoreOptions::default());
    fresh.events_until("ready", LONG);
    fresh.submit("t1", "hello again");
    let events = fresh.events_until("turn.completed", LONG);
    let text = find(&events, "message.final").unwrap()["text"]
        .as_str()
        .unwrap();
    assert!(
        text.contains("[resumed:no]"),
        "fresh session must not resume: {text}"
    );
}

#[test]
fn cancellation_kills_the_provider_tree() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);

    let pidfile = std::env::temp_dir().join(format!("mix2-test-pids-{}", std::process::id()));
    let _ = std::fs::remove_file(&pidfile);
    core.submit(
        "t1",
        &format!(
            "SCENARIO:spawn_child SCENARIO:slow PIDFILE:{}",
            pidfile.display()
        ),
    );
    core.events_until("agent.started", LONG);

    // Wait for the fake provider to record its pid and its child's pid.
    let deadline = Instant::now() + Duration::from_secs(20);
    let pids: Vec<i32> = loop {
        if let Ok(body) = std::fs::read_to_string(&pidfile) {
            let pids: Vec<i32> = body.lines().filter_map(|l| l.trim().parse().ok()).collect();
            if pids.len() == 2 {
                break pids;
            }
        }
        assert!(
            Instant::now() < deadline,
            "fake provider never wrote pidfile"
        );
        std::thread::sleep(Duration::from_millis(100));
    };

    core.send(&serde_json::json!({"type": "cancel", "turn_id": "t1"}));
    let events = core.events_until("turn.cancelled", LONG);
    assert!(find(&events, "message.final").is_none());

    // Both the provider process and its spawned child must be gone.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let alive: Vec<i32> = pids
            .iter()
            .copied()
            .filter(|&pid| unsafe { libc::kill(pid, 0) } == 0)
            .collect();
        if alive.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "processes still alive after cancel: {alive:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_file(&pidfile);

    // The session stays usable after cancellation.
    core.submit("t2", "are you still there?");
    let events = core.events_until("turn.completed", LONG);
    assert!(find(&events, "message.final").is_some());
}

#[test]
fn provider_failure_fails_the_turn_not_the_session() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);

    core.submit("t1", "SCENARIO:fail");
    let events = core.events_until("turn.failed", LONG);
    let failed = find(&events, "turn.failed").unwrap();
    assert!(failed["message"]
        .as_str()
        .unwrap()
        .contains("simulated provider failure"));

    core.submit("t2", "recovered?");
    let events = core.events_until("turn.completed", LONG);
    assert!(find(&events, "message.final").is_some());
}

#[test]
fn malformed_provider_output_is_tolerated() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);
    core.submit("t1", "SCENARIO:malformed hello");
    let events = core.events_until("turn.completed", LONG);
    assert!(find(&events, "message.final").is_some());
}

#[test]
fn invalid_command_yields_error_event() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);
    core.send(&serde_json::json!({"type": "reboot_the_matrix"}));
    let events = core.events_until("error", LONG);
    assert!(find(&events, "error").unwrap()["message"]
        .as_str()
        .unwrap()
        .contains("invalid command"));
}

#[test]
fn protocol_mismatch_is_fatal() {
    // Bypass Core::start's initialize by driving the child manually.
    let mut child = Command::new(env!("CARGO_BIN_EXE_mix2-core"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"type":"initialize","protocol":99}}"#).unwrap();
    drop(stdin);
    let out = child.wait_with_output().expect("wait");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("protocol mismatch"), "got: {text}");
}

fn extract_marker(text: &str, key: &str) -> String {
    let tag = format!("[{key}:");
    let start = text
        .find(&tag)
        .unwrap_or_else(|| panic!("no {key} marker in {text}"))
        + tag.len();
    let end = text[start..].find(']').expect("closing bracket") + start;
    text[start..end].to_owned()
}
