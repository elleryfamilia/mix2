//! `cladex-consult` — the command a Cladex lead agent runs to ask its
//! teammate for an independent opinion. Reads the consultation prompt from
//! stdin, routes it to the Cladex runtime, and prints the teammate's final
//! response to stdout.
//!
//! Transport: first a Unix socket at `$CLADEX_RUNTIME_DIR/consult.sock`
//! (reachable from Claude Code's Bash sandbox); if the sandbox blocks
//! sockets (Codex does), falls back to file-based request/response in
//! `$CLADEX_RUNTIME_DIR/consult/`.
//!
//! Recursion prevention runs here *and* in the runtime: if this process is
//! already a teammate (CLADEX_ROLE=teammate) or too deep (CLADEX_DEPTH),
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

fn run() -> Result<String, String> {
    let role = std::env::var("CLADEX_ROLE").unwrap_or_default();
    let depth: u32 = std::env::var("CLADEX_DEPTH")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0);

    if role == "teammate" {
        return Err(
            "Consultation unavailable: this agent is already running as a Cladex teammate. \
             Complete your independent analysis without delegating."
                .to_owned(),
        );
    }
    if depth >= MAX_DEPTH {
        return Err(format!(
            "Consultation unavailable: maximum collaboration depth ({MAX_DEPTH}) reached."
        ));
    }

    let runtime_dir = std::env::var("CLADEX_RUNTIME_DIR").map_err(|_| {
        "Consultation unavailable: not running inside a Cladex session \
         (CLADEX_RUNTIME_DIR is not set)."
            .to_owned()
    })?;
    let runtime_dir = PathBuf::from(runtime_dir);

    let mut prompt = String::new();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        prompt = args.join(" ");
    } else {
        std::io::stdin()
            .read_to_string(&mut prompt)
            .map_err(|e| format!("Consultation failed: could not read stdin: {e}"))?;
    }
    let prompt = prompt.trim().to_owned();
    if prompt.is_empty() {
        return Err(
            "Consultation failed: empty prompt. Pipe the consultation prompt on stdin, e.g.\n\
             cladex-consult <<'CONSULT'\n...prompt...\nCONSULT"
                .to_owned(),
        );
    }

    let request = serde_json::json!({
        "v": 1,
        "prompt": prompt,
        "role": if role.is_empty() { "lead" } else { &role },
        "depth": depth,
    })
    .to_string();

    let response = match try_socket(&runtime_dir, &request) {
        Ok(response) => response,
        Err(_socket_err) => try_files(&runtime_dir, &request)?,
    };

    let value: serde_json::Value = serde_json::from_str(&response)
        .map_err(|e| format!("Consultation failed: invalid runtime response: {e}"))?;
    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
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
            "Consultation failed: the Cladex runtime is unreachable from this sandbox \
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
