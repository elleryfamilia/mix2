//! `mix2-consult` — the command a mix2 lead agent runs to ask its
//! teammate for an independent opinion, or to record a disagreement.
//!
//! Four forms:
//!   mix2-consult                 read prompt from stdin, block, print reply
//!   mix2-consult start           read prompt from stdin, print a ticket
//!                                  immediately so the caller can keep
//!                                  working while the teammate researches
//!   mix2-consult wait <ticket>   block until that consultation finishes,
//!                                  print the teammate's reply
//!   mix2-consult disagree        read disagreement text from stdin, send it
//!                                  as-is to the runtime (no parsing here)
//!
//! Transport: first a Unix socket at `$MIX2_RUNTIME_DIR/consult.sock`
//! (reachable from Claude Code's Bash sandbox); if the sandbox blocks
//! sockets (Codex does), falls back to file-based request/response in
//! `$MIX2_RUNTIME_DIR/consult/`.
//!
//! Recursion prevention runs here *and* in the runtime: if this process is
//! already a teammate (MIX2_ROLE=teammate) or too deep (MIX2_DEPTH),
//! the call is refused before any provider is spawned.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_DEPTH: u32 = 1;
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(20 * 60);

fn main() -> ExitCode {
    match run() {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            // The message goes to stdout so the calling model always sees it
            // (some providers only surface stdout for failed commands), and
            // to stderr for humans and logs.
            println!("{message}");
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}

enum Mode {
    Sync,
    Start,
    Wait(String),
    Disagree,
}

fn run() -> Result<String, String> {
    let role = std::env::var("MIX2_ROLE").unwrap_or_default();
    let depth: u32 = std::env::var("MIX2_DEPTH")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0);

    if role == "teammate" {
        return Err(
            "Consultation unavailable: this agent is already running as a mix2 teammate. \
             Complete your independent analysis without delegating."
                .to_owned(),
        );
    }
    if depth >= MAX_DEPTH {
        return Err(format!(
            "Consultation unavailable: maximum collaboration depth ({MAX_DEPTH}) reached."
        ));
    }

    let runtime_dir = std::env::var("MIX2_RUNTIME_DIR").map_err(|_| {
        "Consultation unavailable: not running inside a mix2 session \
         (MIX2_RUNTIME_DIR is not set)."
            .to_owned()
    })?;
    let runtime_dir = PathBuf::from(runtime_dir);

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match args.first().map(String::as_str) {
        Some("start") => Mode::Start,
        Some("wait") => {
            let ticket = args
                .get(1)
                .cloned()
                .ok_or_else(|| "Usage: mix2-consult wait <ticket>".to_owned())?;
            Mode::Wait(ticket)
        }
        Some("disagree") => Mode::Disagree,
        _ => Mode::Sync,
    };

    let (prompt, disagreement_text) = match &mode {
        Mode::Wait(_) => (String::new(), None),
        Mode::Disagree => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|e| format!("Consultation failed: could not read stdin: {e}"))?;
            let text = text.trim().to_owned();
            if text.is_empty() {
                return Err("Nothing to record. Pipe the split on stdin, e.g.\n\
                     mix2-consult disagree <<'SPLIT'\n...\nSPLIT"
                    .to_owned());
            }
            (String::new(), Some(text))
        }
        Mode::Sync | Mode::Start => {
            let mut prompt = String::new();
            let extra: Vec<&String> = match mode {
                Mode::Start => args.iter().skip(1).collect(),
                _ => args.iter().collect(),
            };
            if !extra.is_empty() {
                prompt = extra
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
            } else {
                std::io::stdin()
                    .read_to_string(&mut prompt)
                    .map_err(|e| format!("Consultation failed: could not read stdin: {e}"))?;
            }
            let prompt = prompt.trim().to_owned();
            if prompt.is_empty() {
                return Err(
                    "Consultation failed: empty prompt. Pipe the consultation prompt on stdin, e.g.\n\
                     mix2-consult <<'CONSULT'\n...prompt...\nCONSULT"
                        .to_owned(),
                );
            }
            (prompt, None)
        }
    };

    let consult_token = std::env::var("MIX2_CONSULT_TOKEN").ok();
    let mut request_obj = serde_json::json!({
        "v": 1,
        "prompt": prompt,
        "role": if role.is_empty() { "lead" } else { &role },
        "depth": depth,
        "token": consult_token,
        "mode": match &mode { Mode::Sync => "sync", Mode::Start => "start", Mode::Wait(_) => "wait", Mode::Disagree => "disagree" },
        "ticket": match &mode { Mode::Wait(t) => Some(t.clone()), _ => None },
    });
    if let Some(text) = disagreement_text {
        request_obj["disagreement_text"] = serde_json::Value::String(text);
    }
    let request = request_obj.to_string();

    let response = match try_socket(&runtime_dir, &request) {
        Ok(response) => response,
        Err(_socket_err) => match &mode {
            // File transport: `wait` polls the done-file the runtime writes
            // when the ticketed consultation finishes; no request needed.
            Mode::Wait(ticket) => poll_done_file(&runtime_dir, ticket)?,
            _ => try_files(&runtime_dir, &request)?,
        },
    };

    let value: serde_json::Value = serde_json::from_str(&response)
        .map_err(|e| format!("Consultation failed: invalid runtime response: {e}"))?;
    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        if let Some(ticket) = value.get("ticket").and_then(serde_json::Value::as_str) {
            return Ok(format!(
                "ticket: {ticket}\nThe teammate is now working. Continue your own research, \
                 then run `mix2-consult wait {ticket}` to collect the response."
            ));
        }
        Ok(value
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned())
    } else {
        Err(value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Consultation failed for an unknown reason.")
            .to_owned())
    }
}

fn try_socket(runtime_dir: &Path, request: &str) -> Result<String, String> {
    let path = runtime_dir.join("consult.sock");
    let mut stream = UnixStream::connect(&path).map_err(|e| format!("socket connect: {e}"))?;
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.write_all(b"\n"))
        .map_err(|e| format!("socket write: {e}"))?;
    let _ = stream.set_read_timeout(Some(RESPONSE_TIMEOUT));
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| format!("socket read: {e}"))?;
    if line.trim().is_empty() {
        return Err("empty response".to_owned());
    }
    Ok(line)
}

fn try_files(runtime_dir: &Path, request: &str) -> Result<String, String> {
    let dir = runtime_dir.join("consult");
    let id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let tmp = dir.join(format!("req-{id}.json.tmp"));
    let req = dir.join(format!("req-{id}.json"));
    let res = dir.join(format!("res-{id}.json"));

    std::fs::write(&tmp, request).map_err(|e| {
        format!(
            "Consultation failed: the mix2 runtime is unreachable from this sandbox \
             (socket blocked and cannot write {}: {e}). Continue with your own analysis.",
            dir.display()
        )
    })?;
    std::fs::rename(&tmp, &req)
        .map_err(|e| format!("Consultation failed: could not submit request: {e}"))?;

    let started = Instant::now();
    loop {
        if let Ok(body) = std::fs::read_to_string(&res) {
            let _ = std::fs::remove_file(&res);
            return Ok(body);
        }
        if started.elapsed() > RESPONSE_TIMEOUT {
            let _ = std::fs::remove_file(&req);
            return Err("Consultation failed: timed out waiting for the teammate.".to_owned());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn poll_done_file(runtime_dir: &Path, ticket: &str) -> Result<String, String> {
    let path = runtime_dir
        .join("consult")
        .join(format!("done-{ticket}.json"));
    let started = Instant::now();
    loop {
        if let Ok(body) = std::fs::read_to_string(&path) {
            return Ok(body);
        }
        if started.elapsed() > RESPONSE_TIMEOUT {
            return Err("Consultation failed: timed out waiting for the teammate.".to_owned());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}
