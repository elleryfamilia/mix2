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
    /// None omits `lead` from initialize (the built-in default, eligible
    /// for coordinator fallback); Some is an explicit user choice.
    lead: Option<&'static str>,
    claude_cmd: Option<String>,
    codex_cmd: Option<String>,
    /// Defaults to a nonexistent path so discovery never probes a real
    /// cursor-agent install on the dev machine.
    cursor_cmd: Option<String>,
    /// Same hermetic default for the real opencode install.
    opencode_cmd: Option<String>,
    /// Same hermetic default for the real copilot install.
    copilot_cmd: Option<String>,
    max_consults: Option<u32>,
    /// Raw TOML appended to the generated config file (slot tables, legacy
    /// sections) for the config-schema tests.
    config_extra: String,
    /// Sent on initialize: forces the selection handshake.
    pick_team: bool,
    env: Vec<(String, String)>,
}

impl Default for CoreOptions {
    fn default() -> Self {
        Self {
            lead: Some("claude"),
            claude_cmd: Some(fixtures_dir().join("fake-claude").display().to_string()),
            codex_cmd: Some(fixtures_dir().join("fake-codex").display().to_string()),
            cursor_cmd: Some("/nonexistent/cursor-agent-binary".into()),
            opencode_cmd: Some("/nonexistent/opencode-binary".into()),
            copilot_cmd: Some("/nonexistent/copilot-binary".into()),
            max_consults: None,
            config_extra: String::new(),
            pick_team: false,
            env: Vec::new(),
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
        config.push_str(&options.config_extra);
        if let Some(max) = options.max_consults {
            config.push_str(&format!(
                "\n[collaboration]\nmax_consults_per_turn = {max}\n"
            ));
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
            .env_remove("MIX2_CODEX_CMD")
            .env_remove("MIX2_CURSOR_CMD")
            .env_remove("MIX2_OPENCODE_CMD")
            .env_remove("MIX2_COPILOT_CMD");
        if let Some(claude) = &options.claude_cmd {
            cmd.env("MIX2_CLAUDE_CMD", claude);
        }
        if let Some(codex) = &options.codex_cmd {
            cmd.env("MIX2_CODEX_CMD", codex);
        }
        if let Some(cursor) = &options.cursor_cmd {
            cmd.env("MIX2_CURSOR_CMD", cursor);
        }
        if let Some(opencode) = &options.opencode_cmd {
            cmd.env("MIX2_OPENCODE_CMD", opencode);
        }
        if let Some(copilot) = &options.copilot_cmd {
            cmd.env("MIX2_COPILOT_CMD", copilot);
        }
        for (key, value) in &options.env {
            cmd.env(key, value);
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
        let mut init = serde_json::json!({
            "type": "initialize", "protocol": 3,
            "cwd": std::env::current_dir().unwrap().display().to_string(),
        });
        if let Some(lead) = options.lead {
            init["lead"] = serde_json::json!(lead);
        }
        if options.pick_team {
            init["pick_team"] = serde_json::json!(true);
        }
        core.send(&init);
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
    assert_eq!(ready["one"]["slot"], "one");
    assert_eq!(ready["one"]["harness"], "claude");
    assert_eq!(ready["two"]["harness"], "codex");
    assert_eq!(ready["two"]["available"], true);
    assert_eq!(ready["lead_slot"], "one");

    core.submit("t1", "hi");
    let events = core.events_until("turn.completed", LONG);
    assert!(find(&events, "message.user").is_some());
    assert!(find(&events, "agent.started").is_some());
    assert_eq!(count(&events, "consult.started"), 0);
    let final_msg = find(&events, "message.final").unwrap();
    assert_eq!(final_msg["speaker"], "one");
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
    assert_eq!(started["slot"], "two");
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
        lead: Some("codex"),
        ..CoreOptions::default()
    });
    let startup = core.events_until("ready", LONG);
    let ready = find(&startup, "ready").unwrap();
    // Reversed lead: the harness-to-slot mapping is stable (slot one is
    // still Claude); only the lead slot moves.
    assert_eq!(ready["lead_slot"], "two");
    assert_eq!(ready["one"]["harness"], "claude");
    assert_eq!(ready["two"]["harness"], "codex");

    core.submit("t1", "SCENARIO:consult review this");
    let events = core.events_until("turn.completed", LONG);
    let completed = find(&events, "consult.completed").unwrap();
    assert_eq!(completed["slot"], "one");
    let teammate_text = completed["text"].as_str().unwrap();
    assert!(teammate_text.contains("fake-claude reply"));
    assert!(
        teammate_text.contains("[scratchpad:ro]"),
        "teammates must not get scratchpad write permission: {teammate_text}"
    );
    let final_msg = find(&events, "message.final").unwrap();
    assert_eq!(final_msg["speaker"], "team");
    assert_eq!(final_msg["lead_slot"], "two");
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
fn abandoned_consultation_never_bleeds_into_the_next_turn() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);

    // Turn 1 starts a consultation (slow teammate) and finishes without
    // ever collecting it: solo attribution, and the orphan dies with the
    // turn instead of crediting the next one.
    core.submit(
        "t1",
        "SCENARIO:consult_abandon CONSULT_PROMPT:SCENARIO:slow fire and forget",
    );
    let events = core.events_until("turn.completed", LONG);
    assert_eq!(count(&events, "consult.started"), 1);
    assert_eq!(count(&events, "consult.completed"), 0);
    let final_msg = find(&events, "message.final").unwrap();
    assert_eq!(final_msg["speaker"], "one");
    assert!(final_msg["text"]
        .as_str()
        .unwrap()
        .contains("[consult_started:ok]"));

    core.submit("t2", "hi again");
    let events = core.events_until("turn.completed", LONG);
    assert_eq!(count(&events, "consult.started"), 0);
    assert_eq!(count(&events, "consult.completed"), 0);
    assert_eq!(count(&events, "consult.failed"), 0);
    let final_msg = find(&events, "message.final").unwrap();
    assert_eq!(final_msg["speaker"], "one");
    assert_eq!(final_msg["consultations"], 0);
}

#[test]
fn consult_requests_without_the_turn_token_are_refused() {
    use std::io::{BufRead as _, Write as _};

    let mut core = Core::start(CoreOptions::default());
    let startup = core.events_until("ready", LONG);
    let session_id = find(&startup, "ready").unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Hold a turn open so the server has an active turn to protect.
    core.submit("t1", "SCENARIO:slow long think");
    core.events_until("agent.started", LONG);

    let sock = std::path::PathBuf::from("/tmp/mix2")
        .join(&session_id)
        .join("consult.sock");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !sock.exists() {
        assert!(Instant::now() < deadline, "socket never appeared");
        std::thread::sleep(Duration::from_millis(100));
    }

    // A forged request claiming lead role and depth 0 — but without the
    // capability token only the coordinator's environment carries.
    let mut stream = std::os::unix::net::UnixStream::connect(&sock).expect("connect");
    stream
        .write_all(b"{\"v\":1,\"prompt\":\"evil\",\"role\":\"lead\",\"depth\":0}\n")
        .expect("write");
    let mut line = String::new();
    std::io::BufReader::new(stream)
        .read_line(&mut line)
        .expect("read");
    let response: serde_json::Value = serde_json::from_str(&line).expect("json");
    assert_eq!(response["ok"], false);
    assert!(
        response["error"]
            .as_str()
            .unwrap()
            .contains("not authorized"),
        "got: {line}"
    );

    core.send(&serde_json::json!({"type": "cancel", "turn_id": "t1"}));
    core.events_until("turn.cancelled", LONG);
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
    assert_eq!(final_msg["speaker"], "one");
    assert!(final_msg["text"]
        .as_str()
        .unwrap()
        .contains("[consult1:err:"));
}

#[test]
fn missing_teammate_is_fatal_with_install_hint() {
    // mix2 is the two-agent team — no solo mode. A missing teammate blocks
    // startup just as hard as a missing lead.
    let core = Core::start(CoreOptions {
        codex_cmd: Some("/nonexistent/codex-binary".into()),
        ..CoreOptions::default()
    });
    let events = core.events_until("fatal", LONG);
    let message = find(&events, "fatal").unwrap()["message"].as_str().unwrap();
    assert!(message.contains("both agents"), "got: {message}");
    assert!(message.contains("Claude — ready"), "got: {message}");
    assert!(message.contains("Codex — not installed"), "got: {message}");
    assert!(
        message.contains("npm i -g @openai/codex"),
        "actionable fix: {message}"
    );
}

#[test]
fn unauthenticated_lead_is_fatal_with_instructions() {
    let core = Core::start(CoreOptions {
        env: vec![("FAKE_CLAUDE_LOGGED_OUT".into(), "1".into())],
        ..CoreOptions::default()
    });
    let events = core.events_until("fatal", LONG);
    let message = find(&events, "fatal").unwrap()["message"].as_str().unwrap();
    assert!(message.contains("Claude — not signed in"), "got: {message}");
    assert!(
        message.contains("sign in"),
        "should tell the user how: {message}"
    );
    assert!(message.contains("Codex — ready"), "got: {message}");
}

#[test]
fn unauthenticated_teammate_is_fatal_with_login_hint() {
    let core = Core::start(CoreOptions {
        env: vec![("FAKE_CODEX_LOGGED_OUT".into(), "1".into())],
        ..CoreOptions::default()
    });
    let events = core.events_until("fatal", LONG);
    let message = find(&events, "fatal").unwrap()["message"].as_str().unwrap();
    assert!(message.contains("Codex — not signed in"), "got: {message}");
    assert!(message.contains("codex login"), "actionable fix: {message}");
    assert!(message.contains("Claude — ready"), "got: {message}");
}

#[test]
fn set_model_applies_to_lead_and_teammate() {
    let mut core = Core::start(CoreOptions::default());
    let startup = core.events_until("ready", LONG);
    let ready = find(&startup, "ready").unwrap();
    assert!(ready["one"]["model"].is_null());

    // Legacy harness names still target the slot while they're unambiguous.
    core.send(&serde_json::json!({"type": "set_model", "slot": "claude", "model": "sonnet"}));
    let events = core.events_until("agent.model", LONG);
    let confirm = find(&events, "agent.model").unwrap();
    assert_eq!(confirm["slot"], "one");
    assert_eq!(confirm["model"], "sonnet");
    assert_eq!(confirm["source"], "selected");

    core.send(&serde_json::json!({"type": "set_model", "slot": "two", "model": "gpt-5-codex"}));
    core.events_until("agent.model", LONG);

    core.submit("t1", "SCENARIO:consult check models");
    let events = core.events_until("turn.completed", LONG);
    let final_text = find(&events, "message.final").unwrap()["text"]
        .as_str()
        .unwrap();
    assert!(
        final_text.contains("[model:sonnet]"),
        "lead model: {final_text}"
    );
    let consult = find(&events, "consult.completed").unwrap()["text"]
        .as_str()
        .unwrap();
    assert!(
        consult.contains("[model:gpt-5-codex]"),
        "teammate model: {consult}"
    );

    // Clearing returns to the provider default.
    core.send(&serde_json::json!({"type": "set_model", "slot": "one", "model": null}));
    let events = core.events_until("agent.model", LONG);
    assert!(find(&events, "agent.model").unwrap()["model"].is_null());
    core.submit("t2", "plain again");
    let events = core.events_until("turn.completed", LONG);
    assert!(find(&events, "message.final").unwrap()["text"]
        .as_str()
        .unwrap()
        .contains("[model:default]"));
}

#[test]
fn neither_agent_ready_is_fatal_with_both_fixes() {
    let core = Core::start(CoreOptions {
        lead: None,
        claude_cmd: Some("/nonexistent/claude-binary".into()),
        codex_cmd: Some("/nonexistent/codex-binary".into()),
        ..CoreOptions::default()
    });
    let events = core.events_until("fatal", LONG);
    let message = find(&events, "fatal").unwrap()["message"].as_str().unwrap();
    assert!(message.contains("both agents"), "got: {message}");
    assert!(message.contains("Claude — not installed"), "got: {message}");
    assert!(message.contains("Codex — not installed"), "got: {message}");
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

#[test]
fn disagreement_flows_to_message_final() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);

    core.submit("t1", "SCENARIO:disagree p99 question");
    let events = core.events_until("turn.completed", LONG);

    assert!(find(&events, "disagreement.recorded").is_some());
    let fin = find(&events, "message.final").unwrap();
    assert_eq!(fin["disagreement"]["stances"].as_array().unwrap().len(), 2);
    assert!(fin["text"].as_str().unwrap().contains("[disagree:0]"));
}

#[test]
fn disagree_without_consult_is_refused() {
    let mut core = Core::start(CoreOptions::default());
    core.events_until("ready", LONG);

    core.submit("t1", "SCENARIO:disagree_solo attempt");
    let events = core.events_until("turn.completed", LONG);

    assert!(find(&events, "disagreement.recorded").is_none());
    let fin = find(&events, "message.final").unwrap();
    assert!(fin.get("disagreement").is_none());
    assert!(fin["text"].as_str().unwrap().contains("[disagree:2:"));
}

#[test]
fn legacy_config_file_launches_identically() {
    // A pure legacy file — harness-keyed sections, harness-named lead, no
    // env overrides — resolves exactly as it always has: claude on slot
    // one, codex leading from slot two.
    let config = format!(
        "lead = \"codex\"\n[claude]\ncommand = \"{}\"\n[codex]\ncommand = \"{}\"\n",
        fixtures_dir().join("fake-claude").display(),
        fixtures_dir().join("fake-codex").display(),
    );
    let mut core = Core::start(CoreOptions {
        lead: None,
        claude_cmd: None,
        codex_cmd: None,
        config_extra: config,
        ..CoreOptions::default()
    });
    let startup = core.events_until("ready", LONG);
    let ready = find(&startup, "ready").unwrap();
    assert_eq!(ready["lead_slot"], "two");
    assert_eq!(ready["one"]["harness"], "claude");
    assert_eq!(ready["one"]["name"], "Claude");
    assert_eq!(ready["two"]["harness"], "codex");

    core.submit("t1", "hi");
    let events = core.events_until("turn.completed", LONG);
    assert_eq!(find(&events, "message.final").unwrap()["speaker"], "two");
}

#[test]
fn slot_config_selects_harnesses_and_lead() {
    let config = format!(
        "lead = \"two\"\n[slot.one]\nharness = \"claude\"\ncommand = \"{}\"\n[slot.two]\nharness = \"codex\"\ncommand = \"{}\"\n",
        fixtures_dir().join("fake-claude").display(),
        fixtures_dir().join("fake-codex").display(),
    );
    let mut core = Core::start(CoreOptions {
        lead: None,
        claude_cmd: None,
        codex_cmd: None,
        config_extra: config,
        ..CoreOptions::default()
    });
    let startup = core.events_until("ready", LONG);
    let ready = find(&startup, "ready").unwrap();
    assert_eq!(ready["lead_slot"], "two");
    assert_eq!(ready["one"]["harness"], "claude");
    assert_eq!(ready["two"]["harness"], "codex");

    core.submit("t1", "hi");
    let events = core.events_until("turn.completed", LONG);
    let text = find(&events, "message.final").unwrap()["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("fake-codex"), "codex must lead: {text}");
}

#[test]
fn same_harness_team_launches_with_slot_qualified_names() {
    let fake_codex = fixtures_dir().join("fake-codex").display().to_string();
    let config = format!(
        "lead = \"one\"\n[slot.one]\nharness = \"codex\"\ncommand = \"{fake_codex}\"\n[slot.two]\nharness = \"codex\"\ncommand = \"{fake_codex}\"\n",
    );
    let mut core = Core::start(CoreOptions {
        lead: None,
        claude_cmd: None,
        codex_cmd: None,
        config_extra: config,
        ..CoreOptions::default()
    });
    let startup = core.events_until("ready", LONG);
    let ready = find(&startup, "ready").unwrap();
    assert_eq!(ready["one"]["harness"], "codex");
    assert_eq!(ready["two"]["harness"], "codex");
    assert_eq!(ready["one"]["name"], "Codex (one)");
    assert_eq!(ready["two"]["name"], "Codex (two)");
    assert_eq!(ready["lead_slot"], "one");

    // A full consult round works with both slots on the same harness.
    core.submit("t1", "SCENARIO:consult compare notes");
    let events = core.events_until("turn.completed", LONG);
    let completed = find(&events, "consult.completed").unwrap();
    assert_eq!(completed["slot"], "two");
    assert!(completed["text"]
        .as_str()
        .unwrap()
        .contains("fake-codex reply"));
    assert_eq!(find(&events, "message.final").unwrap()["speaker"], "team");
}

#[test]
fn slot_env_override_beats_legacy_env_override() {
    // MIX2_SLOT_TWO_CMD wins over MIX2_CODEX_CMD; if it didn't, slot two
    // would point at a nonexistent binary and startup would be fatal.
    let core = Core::start(CoreOptions {
        codex_cmd: Some("/nonexistent/codex-binary".into()),
        env: vec![(
            "MIX2_SLOT_TWO_CMD".into(),
            fixtures_dir().join("fake-codex").display().to_string(),
        )],
        ..CoreOptions::default()
    });
    let startup = core.events_until("ready", LONG);
    assert_eq!(find(&startup, "ready").unwrap()["two"]["harness"], "codex");
}

#[test]
fn mixed_config_prefers_slot_values_and_warns() {
    // The legacy [codex] section points at a broken binary; the slot table
    // overrides it. Startup succeeding proves precedence, and the conflict
    // is surfaced as a warning right after ready.
    let config = format!(
        "[codex]\ncommand = \"/nonexistent/codex-binary\"\n[slot.two]\ncommand = \"{}\"\n[slot.one]\ncommand = \"{}\"\n",
        fixtures_dir().join("fake-codex").display(),
        fixtures_dir().join("fake-claude").display(),
    );
    let core = Core::start(CoreOptions {
        lead: None,
        claude_cmd: None,
        codex_cmd: None,
        config_extra: config,
        ..CoreOptions::default()
    });
    let events = core.events_until("warning", LONG);
    assert!(find(&events, "ready").is_some(), "warnings follow ready");
    let warning = find(&events, "warning").unwrap()["message"]
        .as_str()
        .unwrap();
    assert!(
        warning.contains("[slot.two] command overrides"),
        "got: {warning}"
    );
}

#[test]
fn discovery_report_precedes_ready() {
    let core = Core::start(CoreOptions::default());
    let startup = core.events_until("ready", LONG);
    let discovered = find(&startup, "harnesses.discovered").unwrap();
    assert_eq!(discovered["auto"], true);
    assert_eq!(discovered["proposal"]["one"], "claude");
    assert_eq!(discovered["proposal"]["two"], "codex");
    assert_eq!(discovered["proposal"]["lead_slot"], "one");
    let harnesses = discovered["harnesses"].as_array().unwrap();
    let claude = harnesses.iter().find(|h| h["harness"] == "claude").unwrap();
    assert_eq!(claude["available"], true);
    assert_eq!(claude["auth"], "authenticated");
    assert!(claude["version"].is_string());
    assert_eq!(claude["lead_eligible"], true);
    assert_eq!(claude["capabilities"]["instruction_injection"], "enforced");
    // The discovered report always precedes ready.
    let discovered_idx = startup
        .iter()
        .position(|e| e["type"] == "harnesses.discovered")
        .unwrap();
    let ready_idx = startup.iter().position(|e| e["type"] == "ready").unwrap();
    assert!(discovered_idx < ready_idx);
}

#[test]
fn pick_team_handshake_selects_a_same_harness_team() {
    let mut core = Core::start(CoreOptions {
        pick_team: true,
        ..CoreOptions::default()
    });
    let startup = core.events_until("harnesses.discovered", LONG);
    let discovered = find(&startup, "harnesses.discovered").unwrap();
    assert_eq!(discovered["auto"], false);
    assert!(
        find(&startup, "ready").is_none(),
        "core must wait for select_team"
    );

    core.send(&serde_json::json!({
        "type": "select_team", "one": "codex", "two": "codex", "lead_slot": "two",
    }));
    let events = core.events_until("ready", LONG);
    let ready = find(&events, "ready").unwrap();
    assert_eq!(ready["one"]["harness"], "codex");
    assert_eq!(ready["two"]["harness"], "codex");
    assert_eq!(ready["one"]["name"], "Codex (one)");
    assert_eq!(ready["lead_slot"], "two");
}

#[test]
fn invalid_selection_is_refused_and_recoverable() {
    let mut core = Core::start(CoreOptions {
        pick_team: true,
        ..CoreOptions::default()
    });
    core.events_until("harnesses.discovered", LONG);

    // Unknown harness: registry-owned error, selection stays open.
    core.send(&serde_json::json!({
        "type": "select_team", "one": "gemini", "two": "codex", "lead_slot": "one",
    }));
    let events = core.events_until("error", LONG);
    let message = find(&events, "error").unwrap()["message"].as_str().unwrap();
    assert!(
        message.contains("unknown harness 'gemini'"),
        "got: {message}"
    );

    // Commands other than select_team are refused while waiting.
    core.submit("t1", "hello?");
    let events = core.events_until("error", LONG);
    assert!(find(&events, "error").unwrap()["message"]
        .as_str()
        .unwrap()
        .contains("no team selected yet"));

    // A valid selection still goes through.
    core.send(&serde_json::json!({
        "type": "select_team", "one": "claude", "two": "codex", "lead_slot": "one",
    }));
    core.events_until("ready", LONG);
}

#[test]
fn selecting_an_unavailable_harness_is_refused_with_its_reason() {
    let mut core = Core::start(CoreOptions {
        codex_cmd: Some("/nonexistent/codex-binary".into()),
        pick_team: true,
        ..CoreOptions::default()
    });
    let startup = core.events_until("harnesses.discovered", LONG);
    let harnesses = find(&startup, "harnesses.discovered").unwrap()["harnesses"]
        .as_array()
        .unwrap()
        .clone();
    let codex = harnesses.iter().find(|h| h["harness"] == "codex").unwrap();
    assert_eq!(codex["available"], false);
    assert!(codex["reason"].as_str().unwrap().contains("not installed"));

    core.send(&serde_json::json!({
        "type": "select_team", "one": "claude", "two": "codex", "lead_slot": "one",
    }));
    let events = core.events_until("error", LONG);
    assert!(find(&events, "error").unwrap()["message"]
        .as_str()
        .unwrap()
        .contains("not installed"));

    // The still-working harness carries the session on both slots.
    core.send(&serde_json::json!({
        "type": "select_team", "one": "claude", "two": "claude", "lead_slot": "one",
    }));
    let events = core.events_until("ready", LONG);
    assert_eq!(
        find(&events, "ready").unwrap()["two"]["name"],
        "Claude (two)"
    );
}

#[test]
fn wedged_binary_times_out_in_discovery_without_stalling_startup() {
    let mut core = Core::start(CoreOptions {
        claude_cmd: Some(fixtures_dir().join("fake-hang").display().to_string()),
        pick_team: true,
        env: vec![("MIX2_DISCOVERY_TIMEOUT_MS".into(), "1500".into())],
        ..CoreOptions::default()
    });
    let startup = core.events_until("harnesses.discovered", LONG);
    let harnesses = find(&startup, "harnesses.discovered").unwrap()["harnesses"]
        .as_array()
        .unwrap()
        .clone();
    let claude = harnesses.iter().find(|h| h["harness"] == "claude").unwrap();
    assert_eq!(claude["available"], false);
    assert!(claude["reason"].as_str().unwrap().contains("timed out"));

    core.send(&serde_json::json!({
        "type": "select_team", "one": "codex", "two": "codex", "lead_slot": "one",
    }));
    core.events_until("ready", LONG);
}

#[test]
fn signed_out_harness_reports_unauthenticated_and_refuses_selection() {
    let mut core = Core::start(CoreOptions {
        pick_team: true,
        env: vec![("FAKE_CODEX_LOGGED_OUT".into(), "1".into())],
        ..CoreOptions::default()
    });
    let startup = core.events_until("harnesses.discovered", LONG);
    let harnesses = find(&startup, "harnesses.discovered").unwrap()["harnesses"]
        .as_array()
        .unwrap()
        .clone();
    let codex = harnesses.iter().find(|h| h["harness"] == "codex").unwrap();
    assert_eq!(codex["auth"], "unauthenticated");
    assert!(codex["reason"].as_str().unwrap().contains("not signed in"));

    core.send(&serde_json::json!({
        "type": "select_team", "one": "claude", "two": "codex", "lead_slot": "one",
    }));
    let events = core.events_until("error", LONG);
    assert!(find(&events, "error").unwrap()["message"]
        .as_str()
        .unwrap()
        .contains("not signed in"));

    core.send(&serde_json::json!({
        "type": "select_team", "one": "claude", "two": "claude", "lead_slot": "one",
    }));
    core.events_until("ready", LONG);
}

#[test]
fn cursor_teammate_full_consult() {
    // Claude leads (env-injected fixture); slot two runs the cursor fixture
    // via the canonical slot schema. Explicit slots auto-confirm.
    let mut core = Core::start(CoreOptions {
        lead: None,
        cursor_cmd: Some(
            fixtures_dir()
                .join("fake-cursor-agent")
                .display()
                .to_string(),
        ),
        config_extra: "[slot.two]\nharness = \"cursor\"\n".to_owned(),
        ..CoreOptions::default()
    });
    let startup = core.events_until("ready", LONG);
    let ready = find(&startup, "ready").unwrap();
    assert_eq!(ready["two"]["harness"], "cursor");
    assert_eq!(ready["two"]["name"], "Cursor");
    assert_eq!(ready["two"]["auth"], "authenticated");

    core.submit("t1", "SCENARIO:consult what does the team think?");
    let events = core.events_until("turn.completed", LONG);
    let completed = find(&events, "consult.completed").unwrap();
    assert_eq!(completed["slot"], "two");
    let text = completed["text"].as_str().unwrap();
    assert!(text.contains("fake-cursor reply"), "got: {text}");
    assert!(
        text.contains("[role:teammate]"),
        "in-band instructions must reach the teammate: {text}"
    );
    assert!(text.contains("[mode:plan]"), "read-only mode: {text}");
    assert!(text.contains("[trust:yes]"), "trust flag passed: {text}");
    assert_eq!(find(&events, "message.final").unwrap()["speaker"], "team");
}

#[test]
fn cursor_named_model_restriction_fails_the_consult_not_the_turn() {
    let mut core = Core::start(CoreOptions {
        lead: None,
        cursor_cmd: Some(
            fixtures_dir()
                .join("fake-cursor-agent")
                .display()
                .to_string(),
        ),
        config_extra: "[slot.two]\nharness = \"cursor\"\n".to_owned(),
        ..CoreOptions::default()
    });
    core.events_until("ready", LONG);

    // The teammate consultation hits the free-plan bare-text refusal.
    core.submit("t1", "SCENARIO:consult CONSULT_PROMPT:SCENARIO:restricted");
    let events = core.events_until("turn.completed", LONG);
    assert_eq!(count(&events, "consult.completed"), 0);
    let failed = find(&events, "consult.failed").unwrap();
    assert!(
        failed["message"]
            .as_str()
            .unwrap()
            .contains("Named models unavailable"),
        "bare-text refusal must surface: {}",
        failed["message"]
    );
    // The lead still answers solo.
    let final_msg = find(&events, "message.final").unwrap();
    assert_eq!(final_msg["speaker"], "one");
    assert!(final_msg["text"]
        .as_str()
        .unwrap()
        .contains("[consult1:err:"));
}

#[test]
fn cursor_is_discoverable_with_note_and_refused_as_lead() {
    let mut core = Core::start(CoreOptions {
        cursor_cmd: Some(
            fixtures_dir()
                .join("fake-cursor-agent")
                .display()
                .to_string(),
        ),
        pick_team: true,
        ..CoreOptions::default()
    });
    let startup = core.events_until("harnesses.discovered", LONG);
    let harnesses = find(&startup, "harnesses.discovered").unwrap()["harnesses"]
        .as_array()
        .unwrap()
        .clone();
    let cursor = harnesses.iter().find(|h| h["harness"] == "cursor").unwrap();
    assert_eq!(cursor["available"], true);
    assert_eq!(cursor["auth"], "authenticated");
    assert_eq!(cursor["lead_eligible"], false);
    assert_eq!(cursor["teammate_eligible"], true);
    assert!(
        cursor["note"].as_str().unwrap().contains("--trust"),
        "trust disclosure surfaces at selection: {}",
        cursor["note"]
    );

    core.send(&serde_json::json!({
        "type": "select_team", "one": "cursor", "two": "claude", "lead_slot": "one",
    }));
    let events = core.events_until("error", LONG);
    assert!(find(&events, "error").unwrap()["message"]
        .as_str()
        .unwrap()
        .contains("cannot lead yet"));

    core.send(&serde_json::json!({
        "type": "select_team", "one": "claude", "two": "cursor", "lead_slot": "one",
    }));
    let events = core.events_until("ready", LONG);
    assert_eq!(find(&events, "ready").unwrap()["two"]["harness"], "cursor");
}

#[test]
fn signed_out_cursor_is_reported_in_discovery() {
    let core = Core::start(CoreOptions {
        cursor_cmd: Some(
            fixtures_dir()
                .join("fake-cursor-agent")
                .display()
                .to_string(),
        ),
        env: vec![("FAKE_CURSOR_LOGGED_OUT".into(), "1".into())],
        ..CoreOptions::default()
    });
    let startup = core.events_until("ready", LONG);
    let harnesses = find(&startup, "harnesses.discovered").unwrap()["harnesses"]
        .as_array()
        .unwrap()
        .clone();
    let cursor = harnesses.iter().find(|h| h["harness"] == "cursor").unwrap();
    assert_eq!(cursor["auth"], "unauthenticated");
    assert!(cursor["reason"]
        .as_str()
        .unwrap()
        .contains("cursor-agent login"));
}

#[test]
fn opencode_teammate_full_consult_with_live_model_listing() {
    let mut core = Core::start(CoreOptions {
        lead: None,
        opencode_cmd: Some(fixtures_dir().join("fake-opencode").display().to_string()),
        config_extra: "[slot.two]\nharness = \"opencode\"\n".to_owned(),
        ..CoreOptions::default()
    });
    let startup = core.events_until("ready", LONG);
    let ready = find(&startup, "ready").unwrap();
    assert_eq!(ready["two"]["harness"], "opencode");
    assert_eq!(ready["two"]["name"], "OpenCode");
    // Credential inventory maps to configured, never authenticated.
    assert_eq!(ready["two"]["auth"], "configured");
    // The model list came from live enumeration, not a curated constant.
    let models: Vec<String> = ready["two"]["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap().to_owned())
        .collect();
    assert!(models.contains(&"fake/model-a".to_owned()), "{models:?}");
    assert!(models.contains(&"opencode/big-pickle".to_owned()));

    core.submit("t1", "SCENARIO:consult what does the team think?");
    let events = core.events_until("turn.completed", LONG);
    let completed = find(&events, "consult.completed").unwrap();
    assert_eq!(completed["slot"], "two");
    let text = completed["text"].as_str().unwrap();
    assert!(text.contains("fake-opencode reply"), "got: {text}");
    assert!(text.contains("[role:teammate]"), "got: {text}");
    assert!(text.contains("[agent:plan]"), "read-only agent: {text}");
    // The pinned tool part surfaced as teammate tool activity.
    let tool_started = events
        .iter()
        .any(|e| e["type"] == "agent.tool.started" && e["slot"] == "two" && e["name"] == "read");
    assert!(tool_started, "tool part must reach the UI as activity");
    assert_eq!(find(&events, "message.final").unwrap()["speaker"], "team");
}

#[test]
fn opencode_model_selection_reaches_the_invocation() {
    let mut core = Core::start(CoreOptions {
        lead: None,
        opencode_cmd: Some(fixtures_dir().join("fake-opencode").display().to_string()),
        config_extra: "[slot.two]\nharness = \"opencode\"\n".to_owned(),
        ..CoreOptions::default()
    });
    core.events_until("ready", LONG);

    core.send(&serde_json::json!({"type": "set_model", "slot": "two", "model": "fake/model-b"}));
    core.events_until("agent.model", LONG);

    core.submit("t1", "SCENARIO:consult check the model");
    let events = core.events_until("turn.completed", LONG);
    let text = find(&events, "consult.completed").unwrap()["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("[model:fake/model-b]"), "got: {text}");
}

#[test]
fn opencode_model_enumeration_failure_degrades_to_manual_entry() {
    let mut core = Core::start(CoreOptions {
        lead: None,
        opencode_cmd: Some(fixtures_dir().join("fake-opencode").display().to_string()),
        config_extra: "[slot.two]\nharness = \"opencode\"\n".to_owned(),
        env: vec![("FAKE_OPENCODE_MODELS_FAIL".into(), "1".into())],
        ..CoreOptions::default()
    });
    let startup = core.events_until("ready", LONG);
    let ready = find(&startup, "ready").unwrap();
    // Still available and usable; the picker just has no list to offer
    // (typed /model entry still works, proven by the turn completing).
    assert_eq!(ready["two"]["harness"], "opencode");
    assert!(ready["two"]["models"]
        .as_array()
        .is_none_or(|m| m.is_empty()));

    core.submit("t1", "hi");
    core.events_until("turn.completed", LONG);
}

#[test]
fn opencode_error_envelope_fails_the_consult_cleanly() {
    let mut core = Core::start(CoreOptions {
        lead: None,
        opencode_cmd: Some(fixtures_dir().join("fake-opencode").display().to_string()),
        config_extra: "[slot.two]\nharness = \"opencode\"\n".to_owned(),
        ..CoreOptions::default()
    });
    core.events_until("ready", LONG);

    core.submit("t1", "SCENARIO:consult CONSULT_PROMPT:SCENARIO:error");
    let events = core.events_until("turn.completed", LONG);
    let failed = find(&events, "consult.failed").unwrap();
    assert!(failed["message"]
        .as_str()
        .unwrap()
        .contains("provider exploded"));
    assert_eq!(find(&events, "message.final").unwrap()["speaker"], "one");
}

#[test]
fn copilot_teammate_full_consult() {
    let mut core = Core::start(CoreOptions {
        lead: None,
        copilot_cmd: Some(fixtures_dir().join("fake-copilot").display().to_string()),
        config_extra: "[slot.two]\nharness = \"copilot\"\n".to_owned(),
        ..CoreOptions::default()
    });
    let startup = core.events_until("ready", LONG);
    let ready = find(&startup, "ready").unwrap();
    assert_eq!(ready["two"]["harness"], "copilot");
    assert_eq!(ready["two"]["name"], "Copilot");
    // No quota-free probe exists; state is unsupported, and it must not
    // have blocked startup.
    assert_eq!(ready["two"]["auth"], "unsupported");

    core.submit("t1", "SCENARIO:consult what does the team think?");
    let events = core.events_until("turn.completed", LONG);
    let completed = find(&events, "consult.completed").unwrap();
    assert_eq!(completed["slot"], "two");
    let text = completed["text"].as_str().unwrap();
    assert!(text.contains("fake-copilot reply"), "got: {text}");
    assert!(text.contains("[role:teammate]"), "got: {text}");
    assert!(
        text.contains("[deny:write,shell]"),
        "mutation denials must be present: {text}"
    );
    assert_eq!(find(&events, "message.final").unwrap()["speaker"], "team");
}

#[test]
fn copilot_auth_failure_surfaces_cleanly_at_run_time() {
    // The plan's contract for an unsupported auth probe: never trial
    // prompts at startup; a run-time auth failure carries the stderr tail.
    let mut core = Core::start(CoreOptions {
        lead: None,
        copilot_cmd: Some(fixtures_dir().join("fake-copilot").display().to_string()),
        config_extra: "[slot.two]\nharness = \"copilot\"\n".to_owned(),
        ..CoreOptions::default()
    });
    core.events_until("ready", LONG);

    core.submit("t1", "SCENARIO:consult CONSULT_PROMPT:SCENARIO:auth_fail");
    let events = core.events_until("turn.completed", LONG);
    let failed = find(&events, "consult.failed").unwrap();
    let message = failed["message"].as_str().unwrap();
    assert!(
        message.contains("authentication required"),
        "stderr tail must surface: {message}"
    );
    assert_eq!(find(&events, "message.final").unwrap()["speaker"], "one");
}

#[test]
fn copilot_empty_output_completes_without_failing_the_session() {
    let mut core = Core::start(CoreOptions {
        lead: None,
        copilot_cmd: Some(fixtures_dir().join("fake-copilot").display().to_string()),
        config_extra: "[slot.two]\nharness = \"copilot\"\n".to_owned(),
        ..CoreOptions::default()
    });
    core.events_until("ready", LONG);

    // Exit 0 with no output: the consult completes (empty response), the
    // turn and session march on.
    core.submit("t1", "SCENARIO:consult CONSULT_PROMPT:SCENARIO:empty");
    let events = core.events_until("turn.completed", LONG);
    assert_eq!(count(&events, "consult.completed"), 1);

    core.submit("t2", "hi again");
    core.events_until("turn.completed", LONG);
}

#[test]
fn cancelling_a_turn_kills_a_slow_copilot_consult() {
    let mut core = Core::start(CoreOptions {
        lead: None,
        copilot_cmd: Some(fixtures_dir().join("fake-copilot").display().to_string()),
        config_extra: "[slot.two]\nharness = \"copilot\"\n".to_owned(),
        ..CoreOptions::default()
    });
    core.events_until("ready", LONG);

    core.submit(
        "t1",
        "SCENARIO:consult_abandon CONSULT_PROMPT:SCENARIO:slow fire and forget",
    );
    core.events_until("consult.started", LONG);
    core.send(&serde_json::json!({"type": "cancel", "turn_id": "t1"}));
    let events = core.events_until("turn.cancelled", LONG);
    assert!(find(&events, "message.final").is_none());

    // The session stays usable afterwards.
    core.submit("t2", "still there?");
    core.events_until("turn.completed", LONG);
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
