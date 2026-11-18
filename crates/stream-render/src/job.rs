//! One render, from the claim to the finish: a local server, a browser on the render page, a
//! heartbeat, then the remux, the checks and the upload. Whatever happens, the browser is killed,
//! the server stopped and the render's directory deleted before the next claim.

use std::path::{Path, PathBuf};

use stream_common::api::{
    ApiClient, ApiError, RenderClaim, RenderFinishInput, UploadedPart, file_ms,
};
use stream_common::redact::register_secret;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::browser::{Browser, BrowserSession};
use crate::config::Settings;
use crate::heartbeat::{Heartbeat, HeartbeatTiming};
use crate::media;
use crate::server::{LocalServer, ServerConfig};
use crate::sink::{FinishCode, SinkEvent};
use crate::upload::{self, UploadLimits};

/// Why a render did not produce a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The render failed; the site is told with this code.
    Failed(FinishCode),
    /// The site no longer holds the render (cancelled, or the lease ran out).
    LeaseLost,
    /// The worker is shutting down: the lease is left to run out, and the site queues the render again.
    Shutdown,
    /// The page stopped answering while the browser stayed up (its renderer was killed, for example
    /// by the memory limit): nothing is reported, the lease runs out and the site queues the render
    /// again, or fails it at its last attempt. Waiting for the deadline would hold the worker for hours.
    Stalled,
}

/// How many status reads in a row may fail or time out before the page is given up on: a minute at the
/// usual interval, far longer than any pause of a live page.
const MAX_STATUS_FAILURES: u32 = 12;

/// A finished render, uploaded and waiting for the finish call.
#[derive(Debug, PartialEq, Eq)]
pub struct Rendered {
    pub size_bytes: i64,
    pub rendered_ms: i64,
    pub parts: Vec<UploadedPart>,
}

/// Ids name directories and appear in addresses: only plain characters.
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The lengths of countdown card the site offers (ms); anything else is drawn as no card.
const PREROLLS_MS: [u64; 5] = [0, 60_000, 120_000, 300_000, 600_000];
/// The most characters of the lesson's title and of the teacher's name the card shows (the site
/// bounds them too; the worker does not trust it).
const CARD_TITLE_CHARS: usize = 80;
const CARD_TEACHER_CHARS: usize = 60;

/// The countdown card at the front of the video: how long, and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Card {
    preroll_ms: u64,
    title: String,
    teacher: String,
}

impl Card {
    fn from_claim(claim: &RenderClaim) -> Self {
        let preroll_ms = if PREROLLS_MS.contains(&claim.preroll_ms) {
            claim.preroll_ms
        } else {
            0
        };
        Self {
            preroll_ms,
            title: plain_text(&claim.card_title, CARD_TITLE_CHARS),
            teacher: plain_text(&claim.card_teacher, CARD_TEACHER_CHARS),
        }
    }

    /// The whole file's length: the card, then the lesson.
    fn file_ms(&self, duration_ms: i64) -> i64 {
        file_ms(duration_ms, self.preroll_ms)
    }
}

/// `text` without control characters, trimmed, at most `max` characters.
fn plain_text(text: &str, max: usize) -> String {
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    clean
        .trim()
        .chars()
        .take(max)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// The render page's address: the inputs are in the fragment, which no server ever sees. A card
/// adds three more (`preroll`, `card_title`, `card_teacher`); without one the address is the
/// one a render with no card has always had.
fn page_url(
    port: u16,
    token: &str,
    recording: &str,
    request: &str,
    locale: &str,
    card: &Card,
) -> String {
    let mut url = format!(
        "http://127.0.0.1:{port}/render?lang={}#api=/api/v4&token={}&recording={}&request={}&sink=/__sink",
        page_lang(locale),
        encode_component(token),
        encode_component(recording),
        encode_component(request),
    );
    if card.preroll_ms > 0 {
        url.push_str(&format!(
            "&preroll={}&card_title={}&card_teacher={}",
            card.preroll_ms,
            encode_component(&card.title),
            encode_component(&card.teacher),
        ));
    }
    url
}

/// The page's language: `vi` or `en`, never the server's text.
fn page_lang(locale: &str) -> &'static str {
    if locale == "vi" { "vi" } else { "en" }
}

/// A render's directory, deleted when it goes out of scope as well as when `remove` is awaited.
#[derive(Debug)]
struct WorkDir {
    path: PathBuf,
    removed: bool,
}

impl WorkDir {
    async fn create(path: PathBuf) -> std::io::Result<Self> {
        let work = Self {
            path,
            removed: false,
        };
        tokio::fs::create_dir_all(&work.path).await?;
        Ok(work)
    }

    async fn remove(mut self) {
        self.removed = true;
        if let Err(error) = tokio::fs::remove_dir_all(&self.path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "cannot delete the render's directory");
        }
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        if !self.removed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Removes whatever a worker that was killed left in the work directory.
pub async fn clean_work_dir(dir: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let removed = match entry.file_type().await {
            Ok(t) if t.is_dir() => tokio::fs::remove_dir_all(&path).await,
            _ => tokio::fs::remove_file(&path).await,
        };
        if removed.is_err() {
            tracing::warn!("cannot delete what an earlier render left behind");
        }
    }
}

/// The worker: claims renders and runs them one at a time.
#[derive(Debug)]
pub struct Worker<B> {
    pub settings: Settings,
    pub api_base_url: String,
    pub api: ApiClient,
    pub storage: reqwest::Client,
    pub browser: B,
    pub shutdown: CancellationToken,
}

impl<B: Browser> Worker<B> {
    /// Runs one claimed render and tells the site how it ended.
    pub async fn render(&self, claim: RenderClaim) {
        let request = claim.request_id.clone();
        if !plain_id(&request) || !plain_id(&claim.recording_id) || claim.duration_ms <= 0 {
            tracing::error!("the site sent a render this worker cannot run");
            self.finish(&request, Err(Ended::Failed(FinishCode::RenderFailed)))
                .await;
            return;
        }
        let _token = register_secret(claim.render_token.expose());
        tracing::info!(duration_ms = claim.duration_ms, "render started");
        let heartbeat = Heartbeat::start(
            self.api.clone(),
            self.settings.worker_id.clone(),
            request.clone(),
            HeartbeatTiming {
                interval: self.settings.heartbeat_interval,
                lease_timeout: self.settings.lease_timeout,
            },
        );
        let result = match WorkDir::create(self.settings.work_dir.join(&request)).await {
            Ok(work) => {
                let result = self.produce(&claim, &work.path, &heartbeat).await;
                work.remove().await;
                result
            }
            Err(_) => Err(Ended::Failed(FinishCode::RenderFailed)),
        };
        let result = match (result, heartbeat.reason()) {
            (Err(_), Some(_)) => Err(Ended::LeaseLost),
            (other, _) => other,
        };
        heartbeat.finish().await;
        self.finish(&request, result).await;
    }

    async fn produce(
        &self,
        claim: &RenderClaim,
        dir: &Path,
        heartbeat: &Heartbeat,
    ) -> Result<Rendered, Ended> {
        let failed = Ended::Failed(FinishCode::RenderFailed);
        let (raw, finished) = (dir.join("raw.mp4"), dir.join("final.mp4"));
        let (events, mut ended) = mpsc::channel(4);
        let server = LocalServer::start(
            ServerConfig {
                web_dir: &self.settings.web_dir,
                api_base: &self.api_base_url,
                recording_id: &claim.recording_id,
                raw_path: &raw,
                max_raw_bytes: self.settings.max_raw_bytes,
            },
            events,
        )
        .await
        .map_err(|_| failed)?;
        let card = Card::from_claim(claim);
        let file_ms = card.file_ms(claim.duration_ms);
        let page = page_url(
            server.port(),
            claim.render_token.expose(),
            &claim.recording_id,
            &claim.request_id,
            &claim.locale,
            &card,
        );
        let page_result = self
            .run_page(file_ms, &page, dir, &mut ended, heartbeat)
            .await;
        server.stop().await;
        let bytes = page_result?;

        // From here on the page is done: the rest is files, and the lease is kept by the heartbeat.
        heartbeat.set_position(file_ms);
        let lost = heartbeat.lost();
        self.race(&lost, async {
            let raw_size = tokio::fs::metadata(&raw).await.map(|m| m.len());
            if raw_size.ok() != Some(bytes) {
                return Err(failed);
            }
            media::remux(
                &self.settings.ffmpeg,
                &raw,
                &finished,
                self.settings.ffmpeg_timeout,
            )
            .await
            .map_err(|error| {
                tracing::error!(%error, "remux failed");
                failed
            })?;
            let _ = tokio::fs::remove_file(&raw).await;
            let rendered_ms = media::probe(
                &self.settings.ffprobe,
                &finished,
                self.settings.probe_timeout,
            )
            .await
            .map_err(|error| {
                tracing::error!(%error, "the finished file was refused");
                failed
            })?;
            if !media::length_within_tolerance(rendered_ms, file_ms) {
                tracing::error!(
                    rendered_ms,
                    "the finished file's length is not the recording's"
                );
                return Err(failed);
            }
            let (size_bytes, parts) = upload::upload(
                &self.api,
                &self.storage,
                &self.settings.worker_id,
                &claim.request_id,
                &finished,
                UploadLimits {
                    attempts: self.settings.attempts,
                    retry_delay: self.settings.retry_delay,
                    part_timeout: self.settings.part_timeout,
                },
            )
            .await
            .map_err(|error| {
                tracing::error!(%error, "upload failed");
                failed
            })?;
            Ok(Rendered {
                size_bytes,
                rendered_ms,
                parts,
            })
        })
        .await
    }

    /// Runs `work` unless the worker is told to shut down or the lease is lost first.
    async fn race<T>(
        &self,
        lost: &CancellationToken,
        work: impl Future<Output = Result<T, Ended>>,
    ) -> Result<T, Ended> {
        tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => Err(Ended::Shutdown),
            _ = lost.cancelled() => Err(Ended::LeaseLost),
            result = work => result,
        }
    }

    /// Opens the page and waits for its end: done or an error from the page, a crash, the deadline,
    /// a lost lease or a shutdown. The browser is closed before this returns.
    async fn run_page(
        &self,
        file_ms: i64,
        page: &str,
        dir: &Path,
        ended: &mut mpsc::Receiver<SinkEvent>,
        heartbeat: &Heartbeat,
    ) -> Result<u64, Ended> {
        let failed = Ended::Failed(FinishCode::RenderFailed);
        let mut session = self
            .browser
            .launch(page, &dir.join("profile"))
            .await
            .map_err(|error| {
                tracing::error!(%error, "the browser did not start");
                failed
            })?;
        let mut exited = session.exited();
        let mut tokens = heartbeat.tokens();
        let lost = heartbeat.lost();
        let deadline = tokio::time::sleep(self.settings.render_deadline(file_ms));
        tokio::pin!(deadline);
        let interval = self.settings.status_interval;
        let mut status = tokio::time::interval_at(Instant::now() + interval, interval);
        let mut last_progress = -1.0;
        let mut last_perf = std::collections::BTreeMap::new();
        let mut status_failures: u32 = 0;

        let outcome = loop {
            tokio::select! {
                biased;
                Some(event) = ended.recv() => match event {
                    SinkEvent::Done { bytes, .. } => break Ok(bytes),
                    SinkEvent::Failed(code) => {
                        tracing::error!(code = code.as_str(), "the render page reported a failure");
                        break Err(Ended::Failed(code));
                    }
                },
                _ = self.shutdown.cancelled() => break Err(Ended::Shutdown),
                _ = lost.cancelled() => break Err(Ended::LeaseLost),
                // The guard `wait_for` returns is not `Send`: it is dropped inside the block.
                _ = async { let _ = exited.wait_for(|gone| *gone).await; } => {
                    tracing::error!("the browser exited");
                    break Err(failed);
                }
                _ = &mut deadline => {
                    tracing::error!("the render ran past its deadline");
                    break Err(failed);
                }
                _ = status.tick() => {
                    // A page that never answers must not stop the loop from seeing the deadline and the stops.
                    let read = tokio::time::timeout(interval * 3, session.status()).await;
                    if matches!(read, Ok(Ok(_))) {
                        status_failures = 0;
                    } else {
                        status_failures += 1;
                        if status_failures >= MAX_STATUS_FAILURES {
                            tracing::error!("the render page stopped answering");
                            break Err(Ended::Stalled);
                        }
                    }
                    match read.unwrap_or(Ok(None)) {
                        Ok(Some(page_status)) => {
                            if page_status.progress != last_progress {
                                last_progress = page_status.progress;
                                tracing::info!(state = page_status.state, progress = page_status.progress, "rendering");
                            }
                            if !page_status.perf.is_empty() && page_status.perf != last_perf {
                                tracing::info!(perf = crate::browser::perf_line(&page_status.perf), "render timing");
                                last_perf = page_status.perf;
                            }
                            heartbeat.set_position((page_status.progress * file_ms as f64) as i64);
                        }
                        Ok(None) => {}
                        Err(error) => tracing::warn!(%error, "cannot read the page's progress"),
                    }
                }
                Ok(()) = tokens.changed() => {
                    let fresh = tokens.borrow_and_update().clone();
                    if let Some(token) = fresh
                        && let Err(error) = session.set_token(token.expose()).await
                    {
                        tracing::warn!(%error, "cannot hand the page a fresh token");
                    }
                }
            }
        };
        session.close().await;
        outcome
    }

    /// Tells the site how the render ended. A render the worker was told to leave (the site asked for a
    /// stop, or says it does not hold it, or was out of reach for the whole lease) is not reported: the
    /// site knows. One left for a shutdown is not either:
    /// the lease runs out and the site queues it again.
    async fn finish(&self, request: &str, result: Result<Rendered, Ended>) {
        let mut input = RenderFinishInput {
            worker_id: self.settings.worker_id.clone(),
            ..Default::default()
        };
        match result {
            Ok(rendered) => {
                tracing::info!(size_bytes = rendered.size_bytes, "render finished");
                input.size_bytes = Some(rendered.size_bytes);
                input.rendered_ms = Some(rendered.rendered_ms);
                input.parts = rendered.parts;
            }
            Err(Ended::Failed(code)) => {
                tracing::warn!(code = code.as_str(), "render failed");
                input.failure_code = Some(code.as_str().to_owned());
            }
            Err(Ended::LeaseLost) => {
                tracing::info!("render dropped: the site no longer holds it");
                return;
            }
            Err(Ended::Shutdown) => {
                tracing::info!("render dropped for shutdown; the site queues it again");
                return;
            }
            Err(Ended::Stalled) => {
                tracing::warn!(
                    "render dropped: the page stopped answering; the site queues it again"
                );
                return;
            }
        }
        for attempt in 1..=self.settings.attempts {
            match self.api.render_finish(request, &input).await {
                Ok(()) => return,
                Err(ApiError::Unavailable) if attempt < self.settings.attempts => {
                    tokio::select! {
                        _ = tokio::time::sleep(self.settings.retry_delay) => {}
                        _ = self.shutdown.cancelled() => return,
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "the site did not take the finish");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_card() -> Card {
        Card {
            preroll_ms: 0,
            title: String::new(),
            teacher: String::new(),
        }
    }

    fn claim_with(preroll_ms: u64, title: &str, teacher: &str) -> RenderClaim {
        serde_json::from_value(serde_json::json!({
            "request_id": "r1", "recording_id": "c1", "duration_ms": 60_000,
            "render_token": "t", "token_expires_at": 1, "lease_until": 1, "locale": "vi",
            "preroll_ms": preroll_ms, "card_title": title, "card_teacher": teacher,
        }))
        .unwrap()
    }

    #[test]
    fn the_page_address_keeps_its_inputs_in_the_fragment() {
        let url = page_url(4242, "a b+c/d", "rec1", "req-2", "vi", &no_card());
        assert_eq!(
            url,
            "http://127.0.0.1:4242/render?lang=vi#api=/api/v4&token=a%20b%2Bc%2Fd&recording=rec1&request=req-2&sink=/__sink"
        );
        assert!(!url.split('#').next().unwrap().contains("token"));
    }

    #[test]
    fn the_page_language_is_vi_or_en() {
        assert_eq!(page_lang("vi"), "vi");
        for other in ["en", "", "VI", "vi&x=1", "fr"] {
            assert_eq!(page_lang(other), "en", "{other:?}");
        }
        assert!(page_url(1, "t", "r", "q", "a&b", &no_card()).contains("/render?lang=en#"));
    }

    #[test]
    fn ids_that_name_directories_are_plain() {
        assert!(plain_id("abc-DEF_123"));
        for bad in ["", "a/b", "..", "a b", "a.b", &"x".repeat(65)] {
            assert!(!plain_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_card_adds_its_inputs_to_the_fragment_encoded() {
        let card = Card::from_claim(&claim_with(300_000, "Nhập môn & tiếng Việt #1", "Văn Lê"));
        let url = page_url(1, "t", "r", "q", "vi", &card);
        let (before, fragment) = url.split_once('#').unwrap();
        assert!(!before.contains("card") && !before.contains("preroll"));
        assert!(fragment.ends_with(
            "&preroll=300000&card_title=Nh%E1%BA%ADp%20m%C3%B4n%20%26%20ti%E1%BA%BFng%20Vi%E1%BB%87t%20%231&card_teacher=V%C4%83n%20L%C3%AA"
        ));
        // Nothing but the encoded text can break out of its field.
        assert_eq!(fragment.matches('&').count(), 7);
    }

    #[test]
    fn no_card_leaves_the_address_as_it_was() {
        let card = Card::from_claim(&claim_with(0, "Title", "Teacher"));
        assert_eq!(
            page_url(7, "t", "r", "q", "en", &card),
            page_url(7, "t", "r", "q", "en", &no_card())
        );
        assert!(!page_url(7, "t", "r", "q", "en", &card).contains("card_"));
    }

    #[test]
    fn only_the_offered_prerolls_draw_a_card() {
        for ok in [0, 60_000, 120_000, 300_000, 600_000] {
            assert_eq!(Card::from_claim(&claim_with(ok, "t", "n")).preroll_ms, ok);
        }
        for bad in [1, 59_999, 90_000, 600_001, 3_600_000, u64::MAX] {
            assert_eq!(
                Card::from_claim(&claim_with(bad, "t", "n")).preroll_ms,
                0,
                "{bad}"
            );
        }
        let card = Card::from_claim(&claim_with(120_000, "t", "n"));
        assert_eq!(card.file_ms(60_000), 180_000);
        assert_eq!(
            Card::from_claim(&claim_with(90_000, "t", "n")).file_ms(60_000),
            60_000
        );
    }

    #[test]
    fn the_card_text_is_bounded_and_has_no_control_characters() {
        let card = Card::from_claim(&claim_with(
            60_000,
            &format!("  Line\none\t\u{7}{} ", "ê".repeat(200)),
            &"a".repeat(100),
        ));
        assert_eq!(card.title.chars().count(), 80);
        assert!(card.title.starts_with("Lineone"));
        assert!(!card.title.chars().any(char::is_control));
        assert_eq!(card.teacher.chars().count(), 60);
        let url = page_url(1, "t", "r", "q", "vi", &card);
        assert!(!url.contains("%0A") && !url.contains("%09") && !url.contains("%07"));
    }
}
