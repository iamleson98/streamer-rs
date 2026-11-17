//! Settings from the environment and from files. The token and the private key are files so they
//! never sit in the environment (where a process listing or a crash report could show them).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use stream_common::key::{KeyError, RunnerPrivateKey, Secret};
use stream_common::media::parse_hosts;

const DEFAULT_TOKEN_FILE: &str = "/secrets/runner-token";
const DEFAULT_KEY_FILE: &str = "/secrets/runner-key.pem";
const DEFAULT_WORKER_ID: &str = "stream-runner";
const DEFAULT_POLL_SECS: u64 = 5;

/// Why the runner will not start.
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
    #[error("the private key in {0} is not usable: {1}")]
    PrivateKey(PathBuf, KeyError),
}

/// Timings and tools. The defaults are the product's; tests shorten the timings.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Names this runner to the site; letters, digits, `-`, `_` and `.`, at most 64.
    pub worker_id: String,
    pub poll_interval: Duration,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub heartbeat_interval: Duration,
    /// No successful heartbeat for this long ends the stream: the site gives the request up then.
    pub lease_timeout: Duration,
    /// Resumes allowed within `resume_window` of the first failure.
    pub max_resumes: usize,
    pub resume_window: Duration,
    pub resume_delay: Duration,
    /// How long ffmpeg gets to end after an interrupt before it is killed.
    pub stop_grace: Duration,
    pub probe_timeout: Duration,
    pub probe_retry_delay: Duration,
    pub finish_retry_delay: Duration,
    pub request_timeout: Duration,
    /// ffmpeg is stopped when the position in the file has not moved for this long.
    pub stall_timeout: Duration,
    /// ffmpeg is stopped when one run has lasted the file's remaining length plus this.
    pub overrun_slack: Duration,
    /// How long the one report made while shutting down (the finish, or giving a stream back) may take.
    pub shutdown_report_timeout: Duration,
    /// The only hosts a file's address may name (https); no other address is opened.
    pub media_hosts: Vec<String>,
    /// Replaces the platform's ingest address (tests only; the build feature `test-ingest`).
    #[cfg(feature = "test-ingest")]
    pub test_ingest_url: Option<String>,
}

impl Settings {
    pub fn new(worker_id: impl Into<String>) -> Self {
        Self {
            worker_id: worker_id.into(),
            poll_interval: Duration::from_secs(DEFAULT_POLL_SECS),
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: PathBuf::from("ffprobe"),
            heartbeat_interval: Duration::from_secs(15),
            lease_timeout: Duration::from_secs(45),
            max_resumes: 2,
            resume_window: Duration::from_secs(180),
            resume_delay: Duration::from_secs(3),
            stop_grace: Duration::from_secs(10),
            probe_timeout: Duration::from_secs(45),
            probe_retry_delay: Duration::from_secs(3),
            finish_retry_delay: Duration::from_secs(3),
            request_timeout: Duration::from_secs(10),
            stall_timeout: Duration::from_secs(60),
            overrun_slack: Duration::from_secs(60),
            shutdown_report_timeout: Duration::from_secs(5),
            media_hosts: Vec::new(),
            #[cfg(feature = "test-ingest")]
            test_ingest_url: None,
        }
    }
}

/// Everything the runner starts with.
#[derive(Debug)]
pub struct Config {
    pub api_base_url: String,
    pub token: Secret,
    pub private_key: RunnerPrivateKey,
    pub settings: Settings,
}

impl Config {
    /// Reads the settings through `lookup` (the environment in production) and the two secret files.
    pub fn load(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |name: &'static str| lookup(name).filter(|v| !v.trim().is_empty());

        let api_base_url =
            get("STREAM_API_BASE_URL").ok_or(ConfigError::Missing("STREAM_API_BASE_URL"))?;
        let api_base_url = api_base_url.trim().trim_end_matches('/').to_owned();
        check_base_url(&api_base_url)?;

        let worker_id = get("STREAM_WORKER_ID").unwrap_or_else(|| DEFAULT_WORKER_ID.to_owned());
        if worker_id.is_empty()
            || worker_id.len() > 64
            || !worker_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(ConfigError::Invalid(
                "STREAM_WORKER_ID",
                "letters, digits, - _ . and at most 64 characters",
            ));
        }
        let mut settings = Settings::new(worker_id);

        let hosts = get("STREAM_MEDIA_HOST").ok_or(ConfigError::Missing("STREAM_MEDIA_HOST"))?;
        settings.media_hosts = parse_hosts(&hosts).ok_or(ConfigError::Invalid(
            "STREAM_MEDIA_HOST",
            "a comma-separated list of host names",
        ))?;
        if let Some(secs) = get("STREAM_STALL_TIMEOUT_SECS") {
            let secs: u64 = secs
                .trim()
                .parse()
                .map_err(|_| ConfigError::Invalid("STREAM_STALL_TIMEOUT_SECS", "a whole number"))?;
            if !(10..=3600).contains(&secs) {
                return Err(ConfigError::Invalid(
                    "STREAM_STALL_TIMEOUT_SECS",
                    "between 10 and 3600",
                ));
            }
            settings.stall_timeout = Duration::from_secs(secs);
        }
        #[cfg(feature = "test-ingest")]
        if let Some(url) = get("STREAM_TEST_INGEST_URL") {
            let url = url.trim().trim_end_matches('/').to_owned();
            let local = ["rtmp://127.0.0.1", "rtmp://localhost"].iter().any(|p| {
                url.strip_prefix(p).is_some_and(|rest| {
                    rest.is_empty() || rest.starts_with(':') || rest.starts_with('/')
                })
            });
            if !local {
                return Err(ConfigError::Invalid(
                    "STREAM_TEST_INGEST_URL",
                    "an rtmp:// address on this machine",
                ));
            }
            settings.test_ingest_url = Some(url);
        }

        if let Some(secs) = get("STREAM_POLL_INTERVAL_SECS") {
            let secs: u64 = secs
                .trim()
                .parse()
                .map_err(|_| ConfigError::Invalid("STREAM_POLL_INTERVAL_SECS", "a whole number"))?;
            if !(1..=300).contains(&secs) {
                return Err(ConfigError::Invalid(
                    "STREAM_POLL_INTERVAL_SECS",
                    "between 1 and 300",
                ));
            }
            settings.poll_interval = Duration::from_secs(secs);
        }
        if let Some(path) = get("STREAM_FFMPEG") {
            settings.ffmpeg = PathBuf::from(path);
        }
        if let Some(path) = get("STREAM_FFPROBE") {
            settings.ffprobe = PathBuf::from(path);
        }

        let token_file = PathBuf::from(
            get("STREAM_RUNNER_TOKEN_FILE").unwrap_or_else(|| DEFAULT_TOKEN_FILE.to_owned()),
        );
        let token = read_text(&token_file)?.trim().to_owned();
        if token.is_empty() {
            return Err(ConfigError::Empty(token_file));
        }

        let key_file = PathBuf::from(
            get("STREAM_RUNNER_KEY_FILE").unwrap_or_else(|| DEFAULT_KEY_FILE.to_owned()),
        );
        let private_key = RunnerPrivateKey::from_pem(&read_text(&key_file)?)
            .map_err(|e| ConfigError::PrivateKey(key_file, e))?;

        Ok(Self {
            api_base_url,
            token: Secret::new(token),
            private_key,
            settings,
        })
    }
}

fn read_text(path: &Path) -> Result<String, ConfigError> {
    fs::read_to_string(path).map_err(|_| ConfigError::Unreadable(path.to_owned()))
}

/// The token goes in a header, so the address must be encrypted; plain HTTP is for a server on this
/// machine (tests, a local site).
fn check_base_url(url: &str) -> Result<(), ConfigError> {
    let local = ["http://127.0.0.1", "http://localhost", "http://[::1]"]
        .iter()
        .any(|p| {
            url.strip_prefix(p).is_some_and(|rest| {
                rest.is_empty() || rest.starts_with(':') || rest.starts_with('/')
            })
        });
    if url.starts_with("https://") || local {
        Ok(())
    } else {
        Err(ConfigError::Invalid(
            "STREAM_API_BASE_URL",
            "an https:// address",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aws_lc_rs_pem::pem_of_new_key;

    use super::*;

    /// Writes a token file and a key file into `dir` and returns the environment pointing at them.
    fn env(dir: &Path) -> HashMap<&'static str, String> {
        fs::write(dir.join("token"), "  tok-123\n").unwrap();
        fs::write(dir.join("key.pem"), pem_of_new_key()).unwrap();
        HashMap::from([
            ("STREAM_MEDIA_HOST", "store.example".to_owned()),
            (
                "STREAM_API_BASE_URL",
                "https://api.example.org/api/v4/".to_owned(),
            ),
            (
                "STREAM_RUNNER_TOKEN_FILE",
                dir.join("token").display().to_string(),
            ),
            (
                "STREAM_RUNNER_KEY_FILE",
                dir.join("key.pem").display().to_string(),
            ),
        ])
    }

    fn load(env: &HashMap<&'static str, String>) -> Result<Config, ConfigError> {
        Config::load(|k| env.get(k).cloned())
    }

    #[test]
    fn loads_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = load(&env(dir.path())).unwrap();
        assert_eq!(config.api_base_url, "https://api.example.org/api/v4");
        assert_eq!(config.token.expose(), "tok-123");
        assert_eq!(config.settings.poll_interval, Duration::from_secs(5));
        assert_eq!(config.settings.worker_id, "stream-runner");
        assert_eq!(config.settings.heartbeat_interval, Duration::from_secs(15));
        assert_eq!(config.settings.lease_timeout, Duration::from_secs(45));
        assert_eq!(config.settings.stall_timeout, Duration::from_secs(60));
        assert_eq!(config.settings.media_hosts, ["store.example"]);
    }

    #[test]
    fn will_not_start_without_the_media_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = env(dir.path());
        e.remove("STREAM_MEDIA_HOST");
        assert!(matches!(
            load(&e),
            Err(ConfigError::Missing("STREAM_MEDIA_HOST"))
        ));
        e.insert("STREAM_MEDIA_HOST", "  ".to_owned());
        assert!(matches!(
            load(&e),
            Err(ConfigError::Missing("STREAM_MEDIA_HOST"))
        ));
        for bad in [" , ", "https://store.example", "a b"] {
            e.insert("STREAM_MEDIA_HOST", bad.to_owned());
            assert!(
                matches!(load(&e), Err(ConfigError::Invalid("STREAM_MEDIA_HOST", _))),
                "{bad:?}"
            );
        }
        e.insert("STREAM_MEDIA_HOST", "a.example, B.example".to_owned());
        assert_eq!(
            load(&e).unwrap().settings.media_hosts,
            ["a.example", "b.example"]
        );
    }

    #[test]
    fn refuses_to_start_without_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = env(dir.path());
        e.insert(
            "STREAM_RUNNER_TOKEN_FILE",
            dir.path().join("absent").display().to_string(),
        );
        assert!(matches!(load(&e), Err(ConfigError::Unreadable(_))));

        let mut e = env(dir.path());
        e.insert(
            "STREAM_RUNNER_KEY_FILE",
            dir.path().join("absent").display().to_string(),
        );
        assert!(matches!(load(&e), Err(ConfigError::Unreadable(_))));

        let mut e = env(dir.path());
        fs::write(dir.path().join("token"), " \n").unwrap();
        assert!(matches!(load(&e), Err(ConfigError::Empty(_))));

        fs::write(dir.path().join("token"), "tok").unwrap();
        fs::write(dir.path().join("key.pem"), "not a key").unwrap();
        assert!(matches!(load(&e), Err(ConfigError::PrivateKey(..))));
        e.remove("STREAM_API_BASE_URL");
        assert!(matches!(
            load(&e),
            Err(ConfigError::Missing("STREAM_API_BASE_URL"))
        ));
    }

    #[test]
    fn checks_the_values() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bad) in [
            ("STREAM_API_BASE_URL", "http://api.example.org/api/v4"),
            ("STREAM_API_BASE_URL", "http://localhost.example.org"),
            ("STREAM_WORKER_ID", "has space"),
            ("STREAM_WORKER_ID", &"w".repeat(65)),
            ("STREAM_POLL_INTERVAL_SECS", "0"),
            ("STREAM_POLL_INTERVAL_SECS", "soon"),
            ("STREAM_STALL_TIMEOUT_SECS", "5"),
            ("STREAM_STALL_TIMEOUT_SECS", "never"),
        ] {
            let mut e = env(dir.path());
            e.insert(name, bad.to_owned());
            assert!(
                matches!(load(&e), Err(ConfigError::Invalid(..))),
                "{name}={bad}"
            );
        }
        let mut e = env(dir.path());
        e.insert("STREAM_API_BASE_URL", "http://127.0.0.1:8065/api/v4".into());
        e.insert("STREAM_POLL_INTERVAL_SECS", "9".into());
        e.insert("STREAM_WORKER_ID", "runner-1.example".into());
        e.insert("STREAM_STALL_TIMEOUT_SECS", "120".into());
        let config = load(&e).unwrap();
        assert_eq!(config.settings.poll_interval, Duration::from_secs(9));
        assert_eq!(config.settings.worker_id, "runner-1.example");
        assert_eq!(config.settings.stall_timeout, Duration::from_secs(120));
    }

    #[test]
    fn errors_never_hold_the_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let e = env(dir.path());
        fs::write(dir.path().join("key.pem"), "tok-123 secret text").unwrap();
        let message = load(&e).unwrap_err().to_string();
        assert!(!message.contains("secret text") && !message.contains("tok-123"));
    }

    #[cfg(feature = "test-ingest")]
    #[test]
    fn the_test_ingest_address_must_be_on_this_machine() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = env(dir.path());
        e.insert(
            "STREAM_TEST_INGEST_URL",
            "rtmp://127.0.0.1:1935/live/".into(),
        );
        assert_eq!(
            load(&e).unwrap().settings.test_ingest_url.as_deref(),
            Some("rtmp://127.0.0.1:1935/live")
        );
        e.insert(
            "STREAM_TEST_INGEST_URL",
            "rtmp://ingest.example/live".into(),
        );
        assert!(matches!(load(&e), Err(ConfigError::Invalid(..))));
    }

    /// Makes a PKCS#8 PEM of a new RSA key, as `openssl genpkey` would.
    mod aws_lc_rs_pem {
        use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
        use aws_lc_rs::rsa::{KeySize, PrivateDecryptingKey};
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD;

        pub fn pem_of_new_key() -> String {
            let key = PrivateDecryptingKey::generate(KeySize::Rsa2048).unwrap();
            let der: Pkcs8V1Der<'static> = key.as_der().unwrap();
            format!(
                "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
                STANDARD.encode(der.as_ref())
            )
        }
    }
}
