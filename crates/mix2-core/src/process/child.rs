use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

/// A spawned provider CLI. On Unix the child gets its own process group so
/// cancellation can kill the whole tree (provider CLIs spawn their own
/// children: shells, node, etc.). Windows support would swap the group kill
/// for Job Objects behind this same interface.
pub struct ChildProcess {
    child: Child,
    /// Process-group id (== child pid) on Unix.
    #[cfg(unix)]
    pgid: Option<i32>,
}

pub struct SpawnOptions<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub cwd: &'a Path,
    pub env: &'a HashMap<String, String>,
    /// Inherited environment variables to unset before spawning (credential
    /// vars stripped from a sandboxed lead). Removed before `env` is applied.
    pub env_remove: &'a [String],
    /// Written to the child's stdin, which is then closed.
    pub stdin: Option<&'a str>,
}

impl ChildProcess {
    pub fn spawn(opts: SpawnOptions<'_>) -> Result<Self> {
        let mut cmd = Command::new(opts.program);
        cmd.args(opts.args)
            .current_dir(opts.cwd)
            .stdin(if opts.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Strip inherited credential vars first, then apply our additions —
        // an explicit `env` entry always wins over a removal of the same key.
        for k in opts.env_remove {
            cmd.env_remove(k);
        }
        for (k, v) in opts.env {
            cmd.env(k, v);
        }
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn `{}`", opts.program))?;

        #[cfg(unix)]
        let pgid = child.id().map(|id| id as i32);

        if let Some(input) = opts.stdin {
            let mut stdin = child.stdin.take().context("child stdin unavailable")?;
            let data = input.as_bytes().to_vec();
            tokio::spawn(async move {
                let _ = stdin.write_all(&data).await;
                let _ = stdin.shutdown().await;
            });
        }

        Ok(Self {
            child,
            #[cfg(unix)]
            pgid,
        })
    }

    /// Line reader over the child's stdout.
    pub fn stdout_lines(
        &mut self,
    ) -> Result<tokio::io::Lines<BufReader<tokio::process::ChildStdout>>> {
        let stdout = self
            .child
            .stdout
            .take()
            .context("child stdout unavailable")?;
        Ok(BufReader::new(stdout).lines())
    }

    /// Collect stderr in the background; returns a handle resolving to its
    /// tail. Reads in chunks and keeps only the last few KB, so a chatty
    /// provider can never balloon memory.
    pub fn stderr_tail(&mut self) -> Result<tokio::task::JoinHandle<String>> {
        let stderr = self
            .child
            .stderr
            .take()
            .context("child stderr unavailable")?;
        Ok(tokio::spawn(async move {
            const TAIL: usize = 4000;
            let mut reader = BufReader::new(stderr);
            let mut tail: Vec<u8> = Vec::with_capacity(TAIL * 2);
            let mut chunk = [0u8; 4096];
            loop {
                match reader.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        tail.extend_from_slice(&chunk[..n]);
                        if tail.len() > TAIL {
                            tail.drain(..tail.len() - TAIL);
                        }
                    }
                }
            }
            // A trimmed buffer can start mid-character; lossy decoding turns
            // that into a replacement char, which is fine for an error tail.
            String::from_utf8_lossy(&tail).into_owned()
        }))
    }

    pub async fn wait(&mut self) -> Result<std::process::ExitStatus> {
        Ok(self.child.wait().await?)
    }

    /// Terminate the child and every descendant. SIGTERM to the group first,
    /// then SIGKILL shortly after for anything that ignored it.
    pub async fn kill_tree(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            unsafe {
                libc::killpg(pgid, libc::SIGTERM);
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
        let _ = self.child.kill().await;
    }
}
