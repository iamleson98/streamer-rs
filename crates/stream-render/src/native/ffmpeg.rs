//! ffmpeg and ffprobe as child processes. Nothing here is `unsafe` and nothing is linked: a
//! decoder that crashes kills a child, not the worker. Children are registered so that a stop (the
//! lease lost, a shutdown, the deadline) can kill all of them at once and unblock whatever is
//! reading from or writing to one.
//!
//! The arguments only ever hold local paths: the render token and the storage addresses never get
//! this far. A child's stderr goes to a file of the render's directory and is read back only to
//! name why it failed.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

/// The children of one render.
#[derive(Debug, Clone, Default)]
pub struct Registry(Arc<Mutex<Vec<u32>>>, Arc<AtomicBool>);

impl Registry {
    fn pids(&self) -> std::sync::MutexGuard<'_, Vec<u32>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts `command` with piped stdin and stdout, its stderr in `log`.
    pub fn spawn(&self, command: &mut Command, log: &Path) -> io::Result<Proc> {
        let stderr = File::create(log)?;
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr))
            .spawn()?;
        let mut child = child;
        {
            let mut pids = self.pids();
            // A render that was stopped starts nothing more.
            if self.1.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other("stopped"));
            }
            pids.push(child.id());
        }
        Ok(Proc {
            child,
            registry: self.clone(),
            log: log.to_owned(),
        })
    }

    /// Kills every running child.
    pub fn kill_all(&self) {
        let pids = self.pids();
        self.1.store(true, Ordering::SeqCst);
        for pid in pids.iter() {
            let _ = kill(Pid::from_raw(*pid as i32), Signal::SIGKILL);
        }
    }
}

/// A running child; killed and waited for when dropped.
#[derive(Debug)]
pub struct Proc {
    pub child: Child,
    registry: Registry,
    log: PathBuf,
}

impl Proc {
    /// The last lines the child wrote to stderr.
    pub fn stderr_tail(&self) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(5)..].join(" | ")
    }

    /// Waits for the child to exit; whether it succeeded.
    pub fn wait_ok(&mut self) -> bool {
        self.child.wait().map(|s| s.success()).unwrap_or(false)
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        // Kill, forget the pid, and only then reap it: a pid is never left in the list once it can
        // belong to another process.
        let _ = self.child.kill();
        let id = self.child.id();
        self.registry.pids().retain(|p| *p != id);
        let _ = self.child.wait();
    }
}

/// The standard head of every ffmpeg command: quiet, no input from the terminal.
pub fn ffmpeg(binary: &Path) -> Command {
    let mut command = Command::new(binary);
    command.args(["-nostdin", "-v", "error", "-y"]);
    command
}
