//! The Chromium engine: a local server, a headless browser on the site's render page, and the
//! remux of what the page streams out. This is the render path the worker has always had.

use std::path::PathBuf;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::browser::{Browser, BrowserSession};
use crate::card::Card;
use crate::engine::{RenderEngine, RenderJob, race};
use crate::job::Ended;
use crate::media;
use crate::server::{LocalServer, ServerConfig};
use crate::sink::{FinishCode, SinkEvent};

/// How many status reads in a row may fail or time out before the page is given up on: a minute at the
/// usual interval, far longer than any pause of a live page.
const MAX_STATUS_FAILURES: u32 = 12;

/// Renders on the site's render page in a headless browser.
#[derive(Debug)]
pub struct ChromiumEngine<B> {
    pub browser: B,
}

impl<B: Browser> ChromiumEngine<B> {
    pub fn new(browser: B) -> Self {
        Self { browser }
    }
}

impl<B: Browser> RenderEngine for ChromiumEngine<B> {
    async fn render(&self, job: RenderJob<'_>) -> Result<PathBuf, Ended> {
        let failed = Ended::Failed(FinishCode::RenderFailed);
        let claim = job.claim;
        let (raw, finished) = (job.dir.join("raw.mp4"), job.dir.join("final.mp4"));
        let (events, mut ended) = mpsc::channel(4);
        let server = LocalServer::start(
            ServerConfig {
                web_dir: &job.settings.web_dir,
                api_base: job.api_base_url,
                recording_id: &claim.recording_id,
                raw_path: &raw,
                max_raw_bytes: job.settings.max_raw_bytes,
            },
            events,
        )
        .await
        .map_err(|_| failed)?;
        let page = page_url(
            server.port(),
            claim.render_token.expose(),
            &claim.recording_id,
            &claim.request_id,
            &claim.locale,
            job.card,
        );
        let page_result = self.run_page(&job, &page, &mut ended).await;
        server.stop().await;
        let bytes = page_result?;

        // From here on the page is done: the rest is files, and the lease is kept by the heartbeat.
        job.heartbeat.set_position(job.file_ms);
        let lost = job.heartbeat.lost();
        race(job.shutdown, &lost, async {
            let raw_size = tokio::fs::metadata(&raw).await.map(|m| m.len());
            if raw_size.ok() != Some(bytes) {
                return Err(failed);
            }
            media::remux(
                &job.settings.ffmpeg,
                &raw,
                &finished,
                job.settings.ffmpeg_timeout,
            )
            .await
            .map_err(|error| {
                tracing::error!(%error, "remux failed");
                failed
            })?;
            let _ = tokio::fs::remove_file(&raw).await;
            Ok(finished)
        })
        .await
    }
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

impl<B: Browser> ChromiumEngine<B> {
    /// Opens the page and waits for its end: done or an error from the page, a crash, the deadline,
    /// a lost lease or a shutdown. The browser is closed before this returns.
    async fn run_page(
        &self,
        job: &RenderJob<'_>,
        page: &str,
        ended: &mut mpsc::Receiver<SinkEvent>,
    ) -> Result<u64, Ended> {
        let (file_ms, dir, heartbeat) = (job.file_ms, job.dir, job.heartbeat);
        let (settings, shutdown) = (job.settings, job.shutdown);
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
        let deadline = tokio::time::sleep(settings.render_deadline(file_ms));
        tokio::pin!(deadline);
        let interval = settings.status_interval;
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
                _ = shutdown.cancelled() => break Err(Ended::Shutdown),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::claim_with;

    fn no_card() -> Card {
        Card {
            preroll_ms: 0,
            title: String::new(),
            teacher: String::new(),
        }
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
