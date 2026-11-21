//! Settings from the environment and from a file. The token is a file so it never sits in the
//! environment (where a process listing or a crash report could show it).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use stream_common::key::Secret;

const DEFAULT_TOKEN_FILE: &str = "/secrets/render-token";
const DEFAULT_WORKER_ID: &str = "render-worker";
const DEFAULT_WEB_DIR: &str = "/srv/web";
const DEFAULT_WORK_DIR: &str = "/work";
const DEFAULT_BROWSER: &str = "/usr/local/bin/headless-shell";
const DEFAULT_POLL_SECS: u64 = 5;

/// Why the worker will not start.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} is not set")]
    Missing(&'static str),
    #[error("{0} is not usable: {1}")]
    Invalid(&'static str, &'static str),
    #[error("cannot read {0}")]
    Unreadable(PathBuf),
    #[error("{0} is empty")]
    Empty(PathBuf),
}

/// What makes the video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EngineKind {
    /// The site's render page in a headless browser: the path the worker has always had.
    #[default]
    Chromium,
    /// Skia and ffmpeg in this process. It does not draw the quiz and Quest stages yet: a lesson
    /// with a game fails with `render_failed` under it, so Chromium stays the default.
    Native,
}

/// Timings, limits and tools. The defaults are the product's; tests shorten the timings.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Names this worker to the site; letters, digits, `-`, `_` and `.`, at most 64.
    pub worker_id: String,
    pub poll_interval: Duration,
    /// The built web app (a static site) the render page is served from.
    pub web_dir: PathBuf,
    /// Where each render keeps its files; the worker owns everything in it.
    pub work_dir: PathBuf,
    pub browser: PathBuf,
    /// Run the browser without its own sandbox. Off: the container is then not the only boundary
    /// around the decoders. On only where the container cannot give the browser user namespaces.
    pub browser_no_sandbox: bool,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    /// Which engine renders.
    pub engine: EngineKind,
    /// The native engine's fonts: the board's faces and the fallback faces, in one directory.
    pub font_dir: Option<PathBuf>,
    /// The most the native engine downloads ahead of the frame it is drawing (bytes).
    pub prefetch_bytes: u64,
    /// How many pieces the native engine renders at once (`RENDER_CHUNKS`); `None` picks by the
    /// cores the worker may use and the length of the video.
    pub chunks: Option<usize>,
    pub heartbeat_interval: Duration,
    /// How often the page's progress is read, between the heartbeats that report it.
    pub status_interval: Duration,
    /// No successful heartbeat for this long ends the render: the site gives the request up then.
    pub lease_timeout: Duration,
    /// How long the browser gets to open the page and hand over its controls.
    pub browser_start_timeout: Duration,
    /// What is added to twice the recording's length to get the longest a render may take.
    pub deadline_grace: Duration,
    /// The longest the remux of the page's output may take.
    pub ffmpeg_timeout: Duration,
    pub probe_timeout: Duration,
    /// The most the page may write, the raw output of one render.
    pub max_raw_bytes: u64,
    pub request_timeout: Duration,
    /// How long one part's PUT to storage may take.
    pub part_timeout: Duration,
    /// Tries of each part, and of the finish call, before giving up.
    pub attempts: usize,
    pub retry_delay: Duration,
}

impl Settings {
    pub fn new(worker_id: impl Into<String>) -> Self {
        Self {
            worker_id: worker_id.into(),
            poll_interval: Duration::from_secs(DEFAULT_POLL_SECS),
            web_dir: PathBuf::from(DEFAULT_WEB_DIR),
            work_dir: PathBuf::from(DEFAULT_WORK_DIR),
            browser: PathBuf::from(DEFAULT_BROWSER),
            browser_no_sandbox: false,
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: PathBuf::from("ffprobe"),
            engine: EngineKind::Chromium,
            font_dir: None,
            prefetch_bytes: 1 << 30,
            chunks: None,
            heartbeat_interval: Duration::from_secs(15),
            status_interval: Duration::from_secs(5),
            lease_timeout: Duration::from_secs(45),
            browser_start_timeout: Duration::from_secs(60),
            deadline_grace: Duration::from_secs(10 * 60),
            ffmpeg_timeout: Duration::from_secs(30 * 60),
            probe_timeout: Duration::from_secs(60),
            max_raw_bytes: 6 << 30,
            request_timeout: Duration::from_secs(10),
            part_timeout: Duration::from_secs(10 * 60),
            attempts: 3,
            retry_delay: Duration::from_secs(3),
        }
    }

    /// The longest a render may take: twice the recording's length and ten minutes.
    pub fn render_deadline(&self, duration_ms: i64) -> Duration {
        Duration::from_millis(u64::try_from(duration_ms).unwrap_or(0))
            .saturating_mul(2)
            .saturating_add(self.deadline_grace)
    }
}

/// Everything the worker starts with.
#[derive(Debug)]
pub struct Config {
    pub api_base_url: String,
    pub token: Secret,
    pub settings: Settings,
}

impl Config {
    /// Reads the settings through `lookup` (the environment in production) and the token file.
    pub fn load(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |name: &'static str| lookup(name).filter(|v| !v.trim().is_empty());

        let api_base_url =
            get("RENDER_API_BASE_URL").ok_or(ConfigError::Missing("RENDER_API_BASE_URL"))?;
        let api_base_url = api_base_url.trim().trim_end_matches('/').to_owned();
        match reqwest::Url::parse(&api_base_url) {
            Ok(url) if matches!(url.scheme(), "https" | "http") && url.host_str().is_some() => {}
            _ => {
                return Err(ConfigError::Invalid(
                    "RENDER_API_BASE_URL",
                    "an http(s) address such as https://host/api/v4",
                ));
            }
        }

        let worker_id = get("RENDER_WORKER_ID").unwrap_or_else(|| DEFAULT_WORKER_ID.to_owned());
        if worker_id.len() > 64
            || !worker_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(ConfigError::Invalid(
                "RENDER_WORKER_ID",
                "letters, digits, - _ . and at most 64 characters",
            ));
        }
        let mut settings = Settings::new(worker_id);

        if let Some(secs) = get("RENDER_POLL_INTERVAL_SECS") {
            let secs: u64 = secs
                .trim()
                .parse()
                .map_err(|_| ConfigError::Invalid("RENDER_POLL_INTERVAL_SECS", "a whole number"))?;
            if !(1..=300).contains(&secs) {
                return Err(ConfigError::Invalid(
                    "RENDER_POLL_INTERVAL_SECS",
                    "between 1 and 300",
                ));
            }
            settings.poll_interval = Duration::from_secs(secs);
        }
        for (name, slot) in [
            ("RENDER_WEB_DIR", &mut settings.web_dir),
            ("RENDER_WORK_DIR", &mut settings.work_dir),
            ("RENDER_BROWSER", &mut settings.browser),
            ("RENDER_FFMPEG", &mut settings.ffmpeg),
            ("RENDER_FFPROBE", &mut settings.ffprobe),
        ] {
            if let Some(path) = get(name) {
                *slot = PathBuf::from(path.trim());
            }
        }
        if let Some(engine) = get("RENDER_ENGINE") {
            settings.engine = match engine.trim() {
                "chromium" => EngineKind::Chromium,
                "native" => EngineKind::Native,
                _ => return Err(ConfigError::Invalid("RENDER_ENGINE", "chromium or native")),
            };
        }
        settings.font_dir = get("RENDER_FONT_DIR").map(|d| PathBuf::from(d.trim()));
        if settings.engine == EngineKind::Native && settings.font_dir.is_none() {
            return Err(ConfigError::Missing("RENDER_FONT_DIR"));
        }
        if let Some(bytes) = get("RENDER_PREFETCH_BYTES") {
            settings.prefetch_bytes =
                bytes
                    .trim()
                    .parse()
                    .ok()
                    .filter(|b| *b >= 1 << 20)
                    .ok_or(ConfigError::Invalid(
                        "RENDER_PREFETCH_BYTES",
                        "a whole number of at least 1048576",
                    ))?;
        }
        if let Some(chunks) = get("RENDER_CHUNKS") {
            settings.chunks = Some(
                chunks
                    .trim()
                    .parse()
                    .ok()
                    .filter(|n| (1..=32).contains(n))
                    .ok_or(ConfigError::Invalid(
                        "RENDER_CHUNKS",
                        "a whole number from 1 to 32",
                    ))?,
            );
        }
        if settings.engine == EngineKind::Chromium && !settings.web_dir.join("200.html").is_file() {
            return Err(ConfigError::Invalid(
                "RENDER_WEB_DIR",
                "a built web app (200.html is missing)",
            ));
        }
        if let Some(flag) = get("RENDER_BROWSER_NO_SANDBOX") {
            settings.browser_no_sandbox = match flag.trim() {
                "1" | "true" => true,
                "0" | "false" => false,
                _ => {
                    return Err(ConfigError::Invalid("RENDER_BROWSER_NO_SANDBOX", "0 or 1"));
                }
            };
        }

        let token_file = PathBuf::from(
            get("RENDER_TOKEN_FILE").unwrap_or_else(|| DEFAULT_TOKEN_FILE.to_owned()),
        );
        let token = read_text(&token_file)?.trim().to_owned();
        if token.is_empty() {
            return Err(ConfigError::Empty(token_file));
        }
        Ok(Self {
            api_base_url,
            token: Secret::new(token),
            settings,
        })
    }
}

fn read_text(path: &Path) -> Result<String, ConfigError> {
    fs::read_to_string(path).map_err(|_| ConfigError::Unreadable(path.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("token"), "the-token\n").unwrap();
            fs::create_dir(dir.path().join("web")).unwrap();
            fs::write(dir.path().join("web/200.html"), "<html></html>").unwrap();
            Self { dir }
        }

        fn load(&self, extra: &[(&str, &str)]) -> Result<Config, ConfigError> {
            let mut env: HashMap<String, String> = HashMap::from([
                (
                    "RENDER_API_BASE_URL".into(),
                    "https://site.example/api/v4/".into(),
                ),
                (
                    "RENDER_TOKEN_FILE".into(),
                    self.dir.path().join("token").display().to_string(),
                ),
                (
                    "RENDER_WEB_DIR".into(),
                    self.dir.path().join("web").display().to_string(),
                ),
            ]);
            for (k, v) in extra {
                env.insert((*k).into(), (*v).into());
            }
            Config::load(|name| env.get(name).cloned())
        }
    }

    #[test]
    fn the_defaults_and_the_token_file() {
        let config = Fixture::new().load(&[]).unwrap();
        assert_eq!(config.api_base_url, "https://site.example/api/v4");
        assert_eq!(config.token.expose(), "the-token");
        assert_eq!(config.settings.worker_id, "render-worker");
        assert!(!config.settings.browser_no_sandbox);
        assert_eq!(config.settings.heartbeat_interval, Duration::from_secs(15));
    }

    #[test]
    fn what_is_wrong_is_named_without_its_value() {
        let f = Fixture::new();
        for extra in [
            vec![("RENDER_API_BASE_URL", "not a url")],
            vec![("RENDER_API_BASE_URL", "ftp://host/api")],
            vec![("RENDER_WORKER_ID", "bad id")],
            vec![("RENDER_POLL_INTERVAL_SECS", "0")],
            vec![("RENDER_BROWSER_NO_SANDBOX", "maybe")],
            vec![("RENDER_WEB_DIR", "/nonexistent")],
            vec![("RENDER_ENGINE", "both")],
            vec![("RENDER_ENGINE", "native")],
            vec![("RENDER_PREFETCH_BYTES", "10")],
            vec![("RENDER_CHUNKS", "0")],
            vec![("RENDER_CHUNKS", "many")],
            vec![("RENDER_TOKEN_FILE", "/nonexistent")],
        ] {
            assert!(f.load(&extra).is_err(), "{extra:?}");
        }
        fs::write(f.dir.path().join("token"), " \n").unwrap();
        assert!(matches!(f.load(&[]), Err(ConfigError::Empty(_))));
    }

    #[test]
    fn the_engine_is_chromium_unless_native_is_asked_for_with_its_fonts() {
        let f = Fixture::new();
        assert_eq!(f.load(&[]).unwrap().settings.engine, EngineKind::Chromium);
        let native = f
            .load(&[("RENDER_ENGINE", "native"), ("RENDER_FONT_DIR", "/fonts")])
            .unwrap();
        assert_eq!(native.settings.engine, EngineKind::Native);
        assert_eq!(native.settings.font_dir, Some(PathBuf::from("/fonts")));
    }

    #[test]
    fn the_deadline_is_twice_the_recording_and_ten_minutes() {
        let s = Settings::new("w");
        assert_eq!(s.render_deadline(60_000), Duration::from_secs(120 + 600));
        assert_eq!(s.render_deadline(-1), Duration::from_secs(600));
    }
}
