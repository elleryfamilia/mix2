use anyhow::Result;
use clap::{Parser, Subcommand};
use mix2_core::runtime::{self, RuntimeOptions};
use std::path::PathBuf;

/// Internal runtime binary behind the `mix2` TUI. Users interact with
/// `mix2`; this process speaks JSONL on stdin/stdout.
#[derive(Parser)]
#[command(name = "mix2-core", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Serve the JSONL IPC protocol on stdin/stdout (default).
    Serve {
        /// Lead agent: claude or codex.
        #[arg(short, long)]
        lead: Option<String>,
        /// Project working directory.
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Config file path override (mainly for tests).
        #[arg(long)]
        config: Option<PathBuf>,
        /// Verbose logging to stderr.
        #[arg(long)]
        debug: bool,
    },
    /// Development helper: run one prompt end-to-end and print events.
    Dev {
        /// The prompt to send to the lead.
        prompt: String,
        #[arg(short, long)]
        lead: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        debug: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => {
            init_tracing(false);
            runtime::serve(RuntimeOptions {
                lead: None,
                cwd: None,
                config_path: None,
                debug: false,
            })
            .await
        }
        Some(Cmd::Serve {
            lead,
            cwd,
            config,
            debug,
        }) => {
            init_tracing(debug);
            runtime::serve(RuntimeOptions {
                lead,
                cwd,
                config_path: config,
                debug,
            })
            .await
        }
        Some(Cmd::Dev {
            prompt,
            lead,
            cwd,
            config,
            debug,
        }) => {
            init_tracing(debug);
            runtime::dev_run(
                RuntimeOptions {
                    lead,
                    cwd,
                    config_path: config,
                    debug,
                },
                prompt,
            )
            .await
        }
    }
}

fn init_tracing(debug: bool) {
    use tracing_subscriber::EnvFilter;
    let filter = if debug {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"))
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
