//! `mix2-consult` — the command a mix2 lead agent runs to ask its
//! teammate for an independent opinion, or to record a disagreement.
//!
//! Five forms:
//!   mix2-consult                 read prompt from stdin, block, print reply
//!   mix2-consult start           read prompt from stdin, print a ticket
//!                                  immediately so the caller can keep
//!                                  working while the teammate researches
//!   mix2-consult wait <ticket> [--timeout <secs>]
//!                                wait for that consultation, print the
//!                                  teammate's reply. Bounded (default 90s):
//!                                  past the bound it prints "not ready yet"
//!                                  so the caller's own shell timeout never
//!                                  kills the wait mid-block — run it again.
//!   mix2-consult status <ticket> instant readiness check, never blocks
//!   mix2-consult disagree        read disagreement text from stdin, send it
//!                                  as-is to the runtime (no parsing here)
//!
//! Prompts are read from STDIN ONLY (heredoc). Positional words are never
//! silently promoted to a prompt: an unknown subcommand is an error, not a
//! consultation — `mix2-consult status <id>` once fired a real consultation
//! whose entire prompt was "status <uuid>".
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
/// Default bound on `wait`: comfortably inside the ~120s shell-tool timeout
/// most harnesses impose, so a wait always returns cleanly instead of being
/// killed (and backgrounded) by the caller's own harness.
const DEFAULT_WAIT_SECS: u64 = 90;
/// Ceiling on a caller-supplied `--timeout`, inside RESPONSE_TIMEOUT.
const MAX_WAIT_SECS: u64 = 600;

const USAGE: &str = "Usage:\n\
     \x20 mix2-consult <<'CONSULT' ... CONSULT          blocking consultation\n\
     \x20 mix2-consult start <<'CONSULT' ... CONSULT    returns a ticket immediately\n\
     \x20 mix2-consult wait <ticket> [--timeout <secs>] collect the response (bounded)\n\
     \x20 mix2-consult status <ticket>                  instant readiness check\n\
     \x20 mix2-consult disagree <<'SPLIT' ... SPLIT     record a disagreement\n\
     The consultation prompt is read from stdin (heredoc), never from arguments.";

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

#[derive(Debug, PartialEq)]
enum Mode {
    Sync,
    Start,
    Wait { ticket: String, timeout_secs: u64 },
    Status { ticket: String },
    Disagree,
}

/// Strict argument grammar. Unknown first words are refused with usage —
/// never reinterpreted as a prompt — and trailing junk is an error too.
fn parse_mode(args: &[String]) -> Result<Mode, String> {
    let reject_extra = |mode: Mode, extra: &[String]| {
        if extra.is_empty() {
            Ok(mode)
        } else {
            Err(format!(
                "Unexpected argument '{}'. The prompt is read from stdin.\n{USAGE}",
                extra[0]
            ))
        }
    };
    match args.first().map(String::as_str) {
        None => Ok(Mode::Sync),
        Some("start") => reject_extra(Mode::Start, &args[1..]),
        Some("disagree") => reject_extra(Mode::Disagree, &args[1..]),
        Some("status") => {
            let ticket = args
                .get(1)
                .cloned()
                .ok_or_else(|| format!("Usage: mix2-consult status <ticket>\n{USAGE}"))?;
            reject_extra(Mode::Status { ticket }, &args[2..])
        }
        Some("wait") => {
            let ticket = args.get(1).cloned().ok_or_else(|| {
                format!("Usage: mix2-consult wait <ticket> [--timeout <secs>]\n{USAGE}")
            })?;
            let mut timeout_secs = DEFAULT_WAIT_SECS;
            let mut rest = &args[2..];
            if let Some(flag) = rest.first() {
                let value = if let Some(v) = flag.strip_prefix("--timeout=") {
                    Some(v.to_owned())
                } else if flag == "--timeout" {
                    let v = rest.get(1).cloned().ok_or_else(|| {
                        format!("--timeout requires a value in seconds.\n{USAGE}")
                    })?;
                    rest = &rest[1..];
                    Some(v)
                } else {
                    None
                };
                if let Some(v) = value {
                    let secs: u64 = v.parse().map_err(|_| {
                        format!("--timeout must be a number of seconds, got '{v}'.\n{USAGE}")
                    })?;
                    timeout_secs = secs.clamp(1, MAX_WAIT_SECS);
                    rest = &rest[1..];
                }
            }
            reject_extra(
                Mode::Wait {
                    ticket,
                    timeout_secs,
                },
                rest,
            )
        }
        Some(other) => Err(format!(
            "Unknown subcommand '{other}' — refusing to guess (arguments are never treated \
             as a consultation prompt).\n{USAGE}"
        )),
    }
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
    let mode = parse_mode(&args)?;

    let (prompt, disagreement_text) = match &mode {
        Mode::Wait { .. } | Mode::Status { .. } => (String::new(), None),
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
            std::io::stdin()
                .read_to_string(&mut prompt)
                .map_err(|e| format!("Consultation failed: could not read stdin: {e}"))?;
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
        "mode": match &mode {
            Mode::Sync => "sync",
            Mode::Start => "start",
            Mode::Wait { .. } => "wait",
            Mode::Status { .. } => "status",
            Mode::Disagree => "disagree",
        },
        "ticket": match &mode {
            Mode::Wait { ticket, .. } | Mode::Status { ticket } => Some(ticket.clone()),
            _ => None,
        },
    });
    if let Mode::Wait { timeout_secs, .. } = &mode {
        request_obj["timeout_secs"] = serde_json::Value::from(*timeout_secs);
    }
    if let Some(text) = disagreement_text {
        request_obj["disagreement_text"] = serde_json::Value::String(text);
    }
    let request = request_obj.to_string();

    let response = match try_socket(&runtime_dir, &request) {
        Ok(response) => response,
        Err(_socket_err) => match &mode {
            // File transport: `wait` polls the done-file the runtime writes
            // when the ticketed consultation finishes; no request needed.
            // The poll honors the same bound as the socket path.
            Mode::Wait {
                ticket,
                timeout_secs,
            } => match poll_done_file(&runtime_dir, ticket, *timeout_secs)? {
                Some(body) => body,
                None => pending_json(ticket),
            },
            _ => try_files(&runtime_dir, &request)?,
        },
    };

    let value: serde_json::Value = serde_json::from_str(&response)
        .map_err(|e| format!("Consultation failed: invalid runtime response: {e}"))?;
    render_response(&mode, &value)
}

/// Synthetic `pending` response for the file transport, so both transports
/// flow through the same rendering.
fn pending_json(ticket: &str) -> String {
    serde_json::json!({ "ok": true, "ticket": ticket, "pending": true }).to_string()
}

fn render_response(mode: &Mode, value: &serde_json::Value) -> Result<String, String> {
    if value.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Consultation failed for an unknown reason.")
            .to_owned());
    }
    let pending = value.get("pending").and_then(serde_json::Value::as_bool) == Some(true);
    match mode {
        Mode::Wait {
            ticket,
            timeout_secs,
        } if pending => Ok(format!(
            "Not ready yet — the teammate is still working ({timeout_secs}s waited). Run \
             `mix2-consult wait {ticket}` again, in the foreground, to keep waiting. Collect \
             the response BEFORE you write your final answer: the ticket dies with your turn \
             and nothing can be delivered to the user afterwards."
        )),
        Mode::Status { ticket } => Ok(if pending {
            format!(
                "Still working. Run `mix2-consult wait {ticket}` to collect the response; \
                 collect it before you write your final answer — the ticket dies with your turn."
            )
        } else {
            format!("Ready. Run `mix2-consult wait {ticket}` to collect the response.")
        }),
        Mode::Start => {
            let ticket = value
                .get("ticket")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "Consultation failed: no ticket in response.".to_owned())?;
            Ok(format!(
                "ticket: {ticket}\nThe teammate is now working. Continue your own research, \
                 then run `mix2-consult wait {ticket}` to collect the response — each wait \
                 returns within ~{DEFAULT_WAIT_SECS}s and says so if the teammate is still \
                 working (just run it again). You MUST collect the response before writing \
                 your final answer: the ticket dies with your turn."
            ))
        }
        _ => Ok(value
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned()),
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

/// Poll for the runtime's done-file. `Ok(None)` means the bound elapsed with
/// the consultation still in flight — a normal outcome, not an error.
fn poll_done_file(
    runtime_dir: &Path,
    ticket: &str,
    timeout_secs: u64,
) -> Result<Option<String>, String> {
    let path = runtime_dir
        .join("consult")
        .join(format!("done-{ticket}.json"));
    let bound = Duration::from_secs(timeout_secs).min(RESPONSE_TIMEOUT);
    let started = Instant::now();
    loop {
        if let Ok(body) = std::fs::read_to_string(&path) {
            return Ok(Some(body));
        }
        if started.elapsed() > bound {
            return Ok(None);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn bare_invocation_is_sync() {
        assert_eq!(parse_mode(&[]).unwrap(), Mode::Sync);
    }

    #[test]
    fn wait_defaults_to_a_bounded_timeout() {
        let mode = parse_mode(&strings(&["wait", "t-1"])).unwrap();
        assert_eq!(
            mode,
            Mode::Wait {
                ticket: "t-1".to_owned(),
                timeout_secs: DEFAULT_WAIT_SECS
            }
        );
    }

    #[test]
    fn wait_accepts_timeout_in_both_spellings_and_clamps() {
        for args in [
            strings(&["wait", "t-1", "--timeout", "300"]),
            strings(&["wait", "t-1", "--timeout=300"]),
        ] {
            let mode = parse_mode(&args).unwrap();
            assert_eq!(
                mode,
                Mode::Wait {
                    ticket: "t-1".to_owned(),
                    timeout_secs: 300
                }
            );
        }
        let clamped = parse_mode(&strings(&["wait", "t-1", "--timeout", "99999"])).unwrap();
        assert_eq!(
            clamped,
            Mode::Wait {
                ticket: "t-1".to_owned(),
                timeout_secs: MAX_WAIT_SECS
            }
        );
    }

    #[test]
    fn unknown_subcommands_are_refused_not_promoted_to_prompts() {
        // The historical failure: `status <uuid>` became a consultation whose
        // prompt was literally "status <uuid>". Now: status is real, and any
        // other word is a hard error.
        let err = parse_mode(&strings(&["stauts", "t-1"])).unwrap_err();
        assert!(err.contains("Unknown subcommand 'stauts'"), "{err}");
        assert!(err.contains("Usage"), "{err}");

        let status = parse_mode(&strings(&["status", "t-1"])).unwrap();
        assert_eq!(
            status,
            Mode::Status {
                ticket: "t-1".to_owned()
            }
        );
    }

    #[test]
    fn stray_arguments_are_errors_everywhere() {
        assert!(parse_mode(&strings(&["start", "extra"])).is_err());
        assert!(parse_mode(&strings(&["disagree", "extra"])).is_err());
        assert!(parse_mode(&strings(&["status"])).is_err());
        assert!(parse_mode(&strings(&["wait"])).is_err());
        assert!(parse_mode(&strings(&["wait", "t-1", "surprise"])).is_err());
        assert!(parse_mode(&strings(&["wait", "t-1", "--timeout", "x"])).is_err());
    }

    #[test]
    fn pending_wait_renders_a_retry_instruction_not_an_error() {
        let mode = Mode::Wait {
            ticket: "t-9".to_owned(),
            timeout_secs: 90,
        };
        let value: serde_json::Value = serde_json::from_str(&pending_json("t-9")).unwrap();
        let text = render_response(&mode, &value).unwrap();
        assert!(text.contains("Not ready yet"), "{text}");
        assert!(text.contains("mix2-consult wait t-9"), "{text}");
        assert!(text.contains("dies with your turn"), "{text}");
    }

    #[test]
    fn status_renders_ready_and_still_working() {
        let mode = Mode::Status {
            ticket: "t-9".to_owned(),
        };
        let pending: serde_json::Value = serde_json::from_str(&pending_json("t-9")).unwrap();
        assert!(render_response(&mode, &pending)
            .unwrap()
            .contains("Still working"));
        let ready = serde_json::json!({ "ok": true, "ticket": "t-9" });
        assert!(render_response(&mode, &ready).unwrap().contains("Ready"));
    }
}
