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
use std::sync::{Arc, Mutex};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

/// The children of one render.
#[derive(Debug, Clone, Default)]
pub struct Registry(Arc<Mutex<Vec<u32>>>);

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
        self.pids().push(child.id());
        Ok(Proc {
            child,
            registry: self.clone(),
            log: log.to_owned(),
        })
    }

    /// Kills every running child.
    pub fn kill_all(&self) {
        for pid in self.pids().iter() {
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
        let _ = self.child.kill();
        let _ = self.child.wait();
        let id = self.child.id();
        self.registry.pids().retain(|p| *p != id);
    }
}

/// The standard head of every ffmpeg command: quiet, no input from the terminal.
pub fn ffmpeg(binary: &Path) -> Command {
    let mut command = Command::new(binary);
    command.args(["-nostdin", "-v", "error", "-y"]);
    command
}
