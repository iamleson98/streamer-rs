//! Runs an engine on a synthetic recording served by the fake site, the way the worker would: a
//! claim, a heartbeat that brings fresh tokens, a work directory. Only the render is run (not the
//! upload or the finish), and its cost is measured.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fake_site::{Recording, Scenario, Server, build, serve};
use stream_common::api::{ApiClient, RenderClaim};
use stream_common::key::Secret;
use stream_render::card::Card;
use stream_render::config::Settings;
use stream_render::engine::{RenderEngine, RenderJob};
use stream_render::heartbeat::{Heartbeat, HeartbeatTiming};
use tokio_util::sync::CancellationToken;

use crate::cost::{Cost, Meter};

/// A request id as the site makes them: 26 lowercase letters and digits.
const REQUEST_ID: &str = "compare10000000000000000a";

/// A synthetic recording on a running fake site.
#[derive(Debug)]
pub struct Rig {
    pub recording: Recording,
    site: Server,
    pub preroll_ms: u64,
    pub locale: &'static str,
}

/// What one engine did.
#[derive(Debug)]
pub struct Run {
    /// The finished file, or why there is none.
    pub result: Result<PathBuf, String>,
    pub cost: Cost,
}

impl Rig {
    /// Builds `scenario` (`secs` long) with ffmpeg in `dir` and serves it.
    pub async fn start(
        scenario: Scenario,
        ffmpeg: &Path,
        dir: &Path,
        secs: u64,
        preroll_ms: u64,
    ) -> std::io::Result<Self> {
        let recording = build(scenario, ffmpeg, dir, secs)?;
        let site = serve(recording.clone()).await?;
        Ok(Self {
            recording,
            site,
            preroll_ms,
            locale: "vi",
        })
    }

    /// The whole file's length in ms: the card, then the lesson.
    pub fn file_ms(&self) -> i64 {
        self.recording.duration_ms as i64 + self.preroll_ms as i64
    }

    /// Renders with `engine` into `work`, measuring it.
    pub async fn run<E: RenderEngine>(&self, engine: &E, settings: &Settings, work: &Path) -> Run {
        let claim: RenderClaim = serde_json::from_value(serde_json::json!({
            "request_id": REQUEST_ID, "recording_id": "recording0000000000001",
            "duration_ms": self.recording.duration_ms, "render_token": "compare-token",
            "token_expires_at": 1, "lease_until": 1, "locale": self.locale,
            "preroll_ms": self.preroll_ms, "card_title": "Nhập môn đại số", "card_teacher": "Cô Lan",
        }))
        .expect("a claim");
        let card = Card::from_claim(&claim);
        let api = ApiClient::new(
            &self.site.api,
            Secret::new("worker-token"),
            Duration::from_secs(5),
        )
        .expect("a client");
        let heartbeat = Heartbeat::start(
            api,
            "compare".into(),
            REQUEST_ID.into(),
            HeartbeatTiming {
                interval: Duration::from_secs(1),
                lease_timeout: Duration::from_secs(60),
            },
        );
        let _ = std::fs::remove_dir_all(work);
        std::fs::create_dir_all(work).expect("a work directory");
        let shutdown = CancellationToken::new();
        let meter = Meter::start();
        let result = engine
            .render(RenderJob {
                claim: &claim,
                card: &card,
                file_ms: self.file_ms(),
                dir: work,
                settings,
                api_base_url: &self.site.api,
                heartbeat: &heartbeat,
                shutdown: &shutdown,
            })
            .await;
        let cost = meter.finish();
        heartbeat.finish().await;
        Run {
            result: result.map_err(|e| format!("{e:?}")),
            cost,
        }
    }
}
