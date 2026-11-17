//! `chromium-headless-shell`, driven through the Chrome DevTools Protocol over a WebSocket.
//!
//! Only four commands are needed (open the page, read `window.__render`, call
//! `window.__setRenderToken`, and the browser's own exit), so the protocol is spoken directly
//! instead of through a general automation crate: fewer dependencies and nothing to learn about
//! what a library does behind the worker's back. The page's address holds the render token in its
//! fragment, so it goes to the browser over the loopback WebSocket and never on a command line.
//!
//! The browser runs in its own process group, which is killed as a whole: its renderer and utility
//! processes die with it, and the worker's container needs no init beyond `docker run --init`
//! reaping them.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tokio_util::sync::CancellationToken;

use crate::browser::{Browser, BrowserError, BrowserSession, PageStatus};

/// One DevTools command gets this long.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// The flags of every launch. The page needs a window of the output's size, WebCodecs and no user
/// to click anything; nothing else (no sync, no updates, no crash upload, no first-run pages).
const FLAGS: [&str; 14] = [
    "--remote-debugging-port=0",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    "--disable-sync",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-default-apps",
    "--disable-breakpad",
    "--disable-gpu",
    "--hide-scrollbars",
    "--mute-audio",
    "--autoplay-policy=no-user-gesture-required",
    "--window-size=1280,720",
];

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Launches the browser at `binary`.
#[derive(Debug, Clone)]
pub struct Chrome {
    binary: PathBuf,
    no_sandbox: bool,
    start_timeout: Duration,
}

impl Chrome {
    pub fn new(binary: PathBuf, no_sandbox: bool, start_timeout: Duration) -> Self {
        Self {
            binary,
            no_sandbox,
            start_timeout,
        }
    }

    fn command(&self, profile_dir: &Path) -> Command {
        let mut command = Command::new(&self.binary);
        command.args(FLAGS);
        if self.no_sandbox {
            command.arg("--no-sandbox");
        }
        command
            .arg(format!("--user-data-dir={}", profile_dir.display()))
            .arg("about:blank")
            .env_clear()
            .env("HOME", profile_dir)
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        command
    }
}

impl Browser for Chrome {
    type Session = ChromeSession;

    async fn launch(
        &self,
        page_url: &str,
        profile_dir: &Path,
    ) -> Result<ChromeSession, BrowserError> {
        tokio::fs::create_dir_all(profile_dir)
            .await
            .map_err(|_| BrowserError::Start)?;
        let mut child = self
            .command(profile_dir)
            .spawn()
            .map_err(|_| BrowserError::Start)?;
        let stderr = child.stderr.take().ok_or(BrowserError::Start)?;
        let (endpoint_tx, endpoint_rx) = oneshot::channel();
        tokio::spawn(read_stderr(stderr, endpoint_tx));

        let (exit_tx, exit_rx) = watch::channel(false);
        let kill = CancellationToken::new();
        let supervisor = tokio::spawn(supervise(child, kill.clone(), exit_tx));

        let opened = timeout(self.start_timeout, async {
            let endpoint = endpoint_rx.await.map_err(|_| BrowserError::Gone)?;
            let (mut socket, _) = connect_async(endpoint.as_str())
                .await
                .map_err(|_| BrowserError::Start)?;
            let target = call(
                &mut socket,
                1,
                None,
                "Target.createTarget",
                json!({"url": page_url}),
            )
            .await?;
            let target_id = target["targetId"].as_str().ok_or(BrowserError::Protocol)?;
            let attached = call(
                &mut socket,
                2,
                None,
                "Target.attachToTarget",
                json!({"targetId": target_id, "flatten": true}),
            )
            .await?;
            let session_id = attached["sessionId"]
                .as_str()
                .ok_or(BrowserError::Protocol)?
                .to_owned();
            Ok::<_, BrowserError>((socket, session_id))
        })
        .await;
        match opened {
            Ok(Ok((socket, session_id))) => Ok(ChromeSession {
                socket,
                session_id,
                next_id: 3,
                exited: exit_rx,
                kill,
                supervisor,
            }),
            Ok(Err(error)) => {
                stop(&kill, supervisor).await;
                Err(error)
            }
            Err(_) => {
                stop(&kill, supervisor).await;
                Err(BrowserError::Timeout)
            }
        }
    }
}

/// Finds the DevTools address in the browser's stderr, then keeps reading so the pipe never fills.
/// The lines are not kept: they may hold page addresses.
async fn read_stderr(stderr: tokio::process::ChildStderr, endpoint: oneshot::Sender<String>) {
    let mut endpoint = Some(endpoint);
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(found) = line.split("DevTools listening on ").nth(1)
            && let Some(tx) = endpoint.take()
        {
            let _ = tx.send(found.trim().to_owned());
        }
    }
}

/// Owns the process: it ends when the process does, or kills the group when told to.
async fn supervise(mut child: Child, kill: CancellationToken, exited: watch::Sender<bool>) {
    let group = child.id().and_then(|p| i32::try_from(p).ok());
    tokio::select! {
        _ = child.wait() => {}
        _ = kill.cancelled() => {
            if let Some(pid) = group {
                // The group may be gone already; there is nothing to do about that.
                let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
            }
            let _ = child.kill().await;
        }
    }
    // The renderer and utility processes of a browser that crashed on its own.
    if let Some(pid) = group {
        let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
    }
    let _ = exited.send(true);
}

async fn stop(kill: &CancellationToken, supervisor: JoinHandle<()>) {
    kill.cancel();
    let _ = supervisor.await;
}

async fn call(
    socket: &mut Socket,
    id: u64,
    session: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value, BrowserError> {
    let mut message = json!({"id": id, "method": method, "params": params});
    if let Some(session) = session {
        message["sessionId"] = json!(session);
    }
    socket
        .send(Message::text(message.to_string()))
        .await
        .map_err(|_| BrowserError::Gone)?;
    let answer = async {
        loop {
            let frame = socket
                .next()
                .await
                .ok_or(BrowserError::Gone)?
                .map_err(|_| BrowserError::Gone)?;
            let Message::Text(text) = frame else {
                if matches!(frame, Message::Close(_)) {
                    return Err(BrowserError::Gone);
                }
                continue;
            };
            let Ok(reply) = serde_json::from_str::<Value>(text.as_str()) else {
                continue;
            };
            if reply["id"].as_u64() == Some(id) {
                return if reply.get("error").is_some() {
                    Err(BrowserError::Protocol)
                } else {
                    Ok(reply["result"].clone())
                };
            }
        }
    };
    timeout(CALL_TIMEOUT, answer)
        .await
        .map_err(|_| BrowserError::Timeout)?
}

/// A browser with the page open.
#[derive(Debug)]
pub struct ChromeSession {
    socket: Socket,
    session_id: String,
    next_id: u64,
    exited: watch::Receiver<bool>,
    kill: CancellationToken,
    supervisor: JoinHandle<()>,
}

impl ChromeSession {
    async fn evaluate(&mut self, expression: &str) -> Result<Value, BrowserError> {
        let id = self.next_id;
        self.next_id += 1;
        let result = call(
            &mut self.socket,
            id,
            Some(&self.session_id),
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true}),
        )
        .await?;
        if result.get("exceptionDetails").is_some() {
            return Err(BrowserError::Protocol);
        }
        Ok(result["result"]["value"].clone())
    }
}

impl BrowserSession for ChromeSession {
    async fn status(&mut self) -> Result<Option<PageStatus>, BrowserError> {
        let value = self
            .evaluate("JSON.stringify(window.__render ?? null)")
            .await?;
        let Some(text) = value.as_str() else {
            return Err(BrowserError::Protocol);
        };
        let parsed: Value = serde_json::from_str(text).map_err(|_| BrowserError::Protocol)?;
        if parsed.is_null() {
            return Ok(None);
        }
        let progress = parsed["progress"].as_f64().unwrap_or(0.0);
        Ok(Some(PageStatus {
            state: parsed["state"].as_str().unwrap_or("").to_owned(),
            progress: if progress.is_finite() {
                progress.clamp(0.0, 1.0)
            } else {
                0.0
            },
        }))
    }

    async fn set_token(&mut self, token: &str) -> Result<(), BrowserError> {
        // A JSON string is a JavaScript string literal: nothing in the token can leave it.
        let literal = serde_json::to_string(token).map_err(|_| BrowserError::Protocol)?;
        self.evaluate(&format!(
            "window.__setRenderToken ? (window.__setRenderToken({literal}), true) : false"
        ))
        .await
        .map(|_| ())
    }

    fn exited(&self) -> watch::Receiver<bool> {
        self.exited.clone()
    }

    async fn close(self) {
        let _ = self.socket;
        stop(&self.kill, self.supervisor).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::TcpListener;

    /// A fake DevTools endpoint: answers the commands the session sends, evaluating nothing.
    async fn fake_devtools(evaluate_value: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(Message::Text(text))) = ws.next().await {
                let request: Value = serde_json::from_str(text.as_str()).unwrap();
                let result = match request["method"].as_str().unwrap() {
                    "Target.createTarget" => json!({"targetId": "T1"}),
                    "Target.attachToTarget" => json!({"sessionId": "S1"}),
                    _ => json!({"result": {"type": "string", "value": evaluate_value}}),
                };
                // An event for nobody comes first: the session must skip it.
                let _ = ws
                    .send(Message::text(
                        json!({"method": "Target.targetCreated"}).to_string(),
                    ))
                    .await;
                let reply = json!({"id": request["id"], "result": result});
                let _ = ws.send(Message::text(reply.to_string())).await;
            }
        });
        format!("ws://{addr}/devtools/browser/x")
    }

    /// A script that stands in for the browser: prints the DevTools line, then sleeps.
    fn fake_browser(dir: &Path, endpoint: &str) -> PathBuf {
        let path = dir.join("headless-shell");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$@\" > \"{dir}/argv\"\necho 'DevTools listening on {endpoint}' >&2\nexec sleep 60\n",
                dir = dir.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[tokio::test]
    async fn it_opens_the_page_reads_its_status_and_hands_over_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = fake_devtools(r#"{"state":"rendering","progress":0.25}"#).await;
        let chrome = Chrome::new(
            fake_browser(dir.path(), &endpoint),
            false,
            Duration::from_secs(10),
        );
        let mut session = chrome
            .launch(
                "http://127.0.0.1:1/render#token=SECRETTOKEN",
                &dir.path().join("profile"),
            )
            .await
            .unwrap();
        let status = session.status().await.unwrap().unwrap();
        assert_eq!(status.state, "rendering");
        assert!((status.progress - 0.25).abs() < 1e-9);
        session.set_token("fresh\"; alert(1); \"").await.unwrap();

        let argv = std::fs::read_to_string(dir.path().join("argv")).unwrap();
        assert!(
            !argv.contains("SECRETTOKEN"),
            "the token is never an argument"
        );
        assert!(!argv.contains("--no-sandbox"));
        assert!(argv.contains("--remote-debugging-port=0"));

        let exited = session.exited();
        assert!(!*exited.borrow());
        session.close().await;
        assert!(*exited.borrow(), "close waits until the browser is gone");
    }

    #[tokio::test]
    async fn the_sandbox_is_off_only_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = fake_devtools("null").await;
        let chrome = Chrome::new(
            fake_browser(dir.path(), &endpoint),
            true,
            Duration::from_secs(10),
        );
        let mut session = chrome
            .launch("http://127.0.0.1:1/render", &dir.path().join("profile"))
            .await
            .unwrap();
        assert!(
            std::fs::read_to_string(dir.path().join("argv"))
                .unwrap()
                .contains("--no-sandbox")
        );
        assert_eq!(
            session.status().await.unwrap(),
            None,
            "no page controls yet"
        );
        session.close().await;
    }

    #[tokio::test]
    async fn a_browser_that_dies_is_seen_as_exited() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = fake_devtools("null").await;
        let path = dir.path().join("headless-shell");
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho 'DevTools listening on {endpoint}' >&2\nsleep 1\nexit 7\n"),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let chrome = Chrome::new(path, false, Duration::from_secs(10));
        let session = chrome
            .launch("http://127.0.0.1:1/render", &dir.path().join("profile"))
            .await
            .unwrap();
        let mut exited = session.exited();
        timeout(Duration::from_secs(10), exited.wait_for(|gone| *gone))
            .await
            .unwrap()
            .unwrap();
        session.close().await;
    }

    #[tokio::test]
    async fn a_browser_that_never_says_where_it_listens_fails_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("headless-shell");
        std::fs::write(&path, "#!/bin/sh\nexec sleep 60\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let chrome = Chrome::new(path, false, Duration::from_millis(500));
        let error = chrome
            .launch("http://127.0.0.1:1/render", &dir.path().join("profile"))
            .await
            .unwrap_err();
        assert_eq!(error, BrowserError::Timeout);
        let missing = Chrome::new(dir.path().join("nope"), false, Duration::from_secs(1));
        assert_eq!(
            missing
                .launch("http://127.0.0.1:1/render", &dir.path().join("p2"))
                .await
                .unwrap_err(),
            BrowserError::Start
        );
    }
}
