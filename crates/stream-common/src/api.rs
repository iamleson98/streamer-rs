//! The site's stream worker routes under `{api base}/lms/stream_worker/`: the request and response
//! bodies as the API describes them (`stream/*` for the runner, `render/*` for the render worker),
//! and a client that speaks them with the worker's bearer token.

use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::key::Secret;

/// `POST stream/claim`: names the worker; it holds the requests it claimed.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimInput {
    pub worker_id: String,
}

/// The answer to a claim: a stream to run.
#[derive(Debug, Clone, Deserialize)]
pub struct StreamClaim {
    pub request_id: String,
    pub platform: String,
    /// The label the key was sealed with.
    pub destination_id: String,
    /// The sealed stream key, base64.
    pub key_sealed: String,
    /// A presigned GET of the rendered file.
    pub mp4_url: Secret,
    pub duration_ms: i64,
    /// Length of the countdown card at the front of the file (ms); an older site does not send it.
    #[serde(default)]
    pub preroll_ms: u64,
    pub lease_until: i64,
}

impl StreamClaim {
    /// The whole rendered file: the countdown card, then the lesson.
    pub fn file_ms(&self) -> i64 {
        file_ms(self.duration_ms, self.preroll_ms)
    }
}

/// The length of a rendered file: the lesson's `duration_ms` and the card before it.
pub fn file_ms(duration_ms: i64, preroll_ms: u64) -> i64 {
    duration_ms.saturating_add(i64::try_from(preroll_ms).unwrap_or(i64::MAX))
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Secret::new)
    }
}

/// `POST stream/{id}/heartbeat`: how far the stream is, in video milliseconds.
#[derive(Debug, Serialize, Deserialize)]
pub struct HeartbeatInput {
    pub worker_id: String,
    pub position_ms: i64,
}

/// The answer to a heartbeat. `stop` is true when the teacher asked the stream to stop, or the
/// runner no longer holds the request.
#[derive(Debug, Deserialize)]
pub struct Heartbeat {
    #[serde(default)]
    pub lease_until: i64,
    #[serde(default)]
    pub stop: bool,
}

/// `POST stream/{id}/finish`: no failure code means the whole file was streamed.
#[derive(Debug, Serialize, Deserialize)]
pub struct FinishInput {
    pub worker_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    pub streamed_ms: i64,
}

/// The answer to a render claim: a recording to render, and the short-lived token that reads it.
#[derive(Debug, Clone, Deserialize)]
pub struct RenderClaim {
    pub request_id: String,
    pub recording_id: String,
    pub duration_ms: i64,
    pub render_token: Secret,
    /// When `render_token` stops working (epoch ms).
    pub token_expires_at: i64,
    pub lease_until: i64,
    /// The teacher's language, `vi` or `en`; an older site does not send it.
    #[serde(default)]
    pub locale: String,
    /// Length of the countdown card drawn before the lesson (ms); an older site does not send it.
    #[serde(default)]
    pub preroll_ms: u64,
    /// The lesson's title, shown on the card; an older site does not send it.
    #[serde(default)]
    pub card_title: String,
    /// The teacher's name, shown on the card; an older site does not send it.
    #[serde(default)]
    pub card_teacher: String,
}

impl RenderClaim {
    /// The whole rendered file: the countdown card, then the lesson.
    pub fn file_ms(&self) -> i64 {
        file_ms(self.duration_ms, self.preroll_ms)
    }
}

/// The answer to a render heartbeat: like a stream's, plus a fresh render token while the worker
/// holds the request.
#[derive(Debug, Deserialize)]
pub struct RenderHeartbeat {
    #[serde(default)]
    pub lease_until: i64,
    #[serde(default)]
    pub stop: bool,
    #[serde(default)]
    pub render_token: Option<Secret>,
    #[serde(default)]
    pub render_token_expires_at: i64,
}

/// `POST render/{id}/uploads`: the finished MP4's size.
#[derive(Debug, Serialize, Deserialize)]
pub struct UploadInput {
    pub worker_id: String,
    pub size_bytes: i64,
}

/// One part of the MP4's multipart upload: a presigned PUT of exactly the part's size.
#[derive(Debug, Clone, Deserialize)]
pub struct UploadPart {
    pub number: u32,
    pub url: Secret,
}

/// The presigned multipart upload: every part but the last is exactly `part_bytes` long.
#[derive(Debug, Deserialize)]
pub struct UploadPlan {
    pub part_bytes: i64,
    pub parts: Vec<UploadPart>,
}

/// A part that was uploaded, with the ETag storage answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadedPart {
    pub number: u32,
    pub etag: String,
}

/// `POST render/{id}/finish`: a failure code alone, or the finished file's size, length and parts.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RenderFinishInput {
    pub worker_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_ms: Option<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub parts: Vec<UploadedPart>,
}

/// Why a call did not give an answer. None of the messages carries a body, an address or a token.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The server or the network failed (a 5xx such as 503, a timeout, a dropped connection): ask again.
    #[error("the site is not answering")]
    Unavailable,
    /// The worker's token was refused.
    #[error("the site refused the worker token")]
    Unauthorized,
    /// The request is unknown to the site.
    #[error("the site does not know the request")]
    NotFound,
    /// The site refused the call because the worker does not hold the request (any more), or the
    /// request is already past that step (409).
    #[error("the site says the worker does not hold the request")]
    Conflict,
    /// The site refused the call for another reason.
    #[error("the site answered with status {0}")]
    Rejected(u16),
    /// The site's answer was not what the API describes.
    #[error("the site's answer could not be read")]
    Malformed,
    /// The client could not be built.
    #[error("the API address is not usable")]
    Config,
    /// The id the site gave is not a 26-character id; it is never put in an address.
    #[error("the site sent a request id that is not an id")]
    BadId,
}

/// Whether `id` is what the site's ids are: 26 lowercase letters and digits.
pub fn is_site_id(id: &str) -> bool {
    id.len() == 26
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn check_id(id: &str) -> Result<(), ApiError> {
    if is_site_id(id) {
        Ok(())
    } else {
        Err(ApiError::BadId)
    }
}

/// The stream worker routes, with the runner's token.
#[derive(Debug, Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base: String,
    token: Secret,
}

impl ApiClient {
    /// `base_url` is the API root (for example `https://host/api/v4`). Redirects are not followed: the
    /// token goes only where it was pointed.
    pub fn new(base_url: &str, token: Secret, timeout: Duration) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ApiError::Config)?;
        let base = format!("{}/lms/stream_worker", base_url.trim_end_matches('/'));
        reqwest::Url::parse(&base).map_err(|_| ApiError::Config)?;
        Ok(Self { http, base, token })
    }

    /// Takes the next stream due, `None` when nothing waits (204).
    pub async fn claim(&self, worker_id: &str) -> Result<Option<StreamClaim>, ApiError> {
        let body = ClaimInput {
            worker_id: worker_id.to_owned(),
        };
        let claim: Option<StreamClaim> = self.claim_at("stream", &body).await?;
        if let Some(claim) = &claim {
            check_id(&claim.request_id)?;
        }
        Ok(claim)
    }

    /// Takes the next render due, `None` when nothing waits (204).
    pub async fn render_claim(&self, worker_id: &str) -> Result<Option<RenderClaim>, ApiError> {
        let body = ClaimInput {
            worker_id: worker_id.to_owned(),
        };
        let claim: Option<RenderClaim> = self.claim_at("render", &body).await?;
        if let Some(claim) = &claim {
            check_id(&claim.request_id)?;
        }
        Ok(claim)
    }

    async fn claim_at<T: serde::de::DeserializeOwned>(
        &self,
        kind: &str,
        body: &ClaimInput,
    ) -> Result<Option<T>, ApiError> {
        let response = self
            .post(&format!("{}/{kind}/claim", self.base), body)
            .await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        response
            .json()
            .await
            .map(Some)
            .map_err(|_| ApiError::Malformed)
    }

    pub async fn heartbeat(
        &self,
        request_id: &str,
        input: &HeartbeatInput,
    ) -> Result<Heartbeat, ApiError> {
        let response = self
            .post(&self.url("stream", request_id, "heartbeat")?, input)
            .await?;
        response.json().await.map_err(|_| ApiError::Malformed)
    }

    pub async fn finish(&self, request_id: &str, input: &FinishInput) -> Result<(), ApiError> {
        self.post(&self.url("stream", request_id, "finish")?, input)
            .await
            .map(|_| ())
    }

    /// Gives a claimed stream back (204): for a runner that is shutting down before it went live.
    /// `Conflict` when the runner does not hold it, or it was streamed already.
    pub async fn release(&self, request_id: &str, worker_id: &str) -> Result<(), ApiError> {
        let body = ClaimInput {
            worker_id: worker_id.to_owned(),
        };
        self.post(&self.url("stream", request_id, "release")?, &body)
            .await
            .map(|_| ())
    }

    pub async fn render_heartbeat(
        &self,
        request_id: &str,
        input: &HeartbeatInput,
    ) -> Result<RenderHeartbeat, ApiError> {
        let response = self
            .post(&self.url("render", request_id, "heartbeat")?, input)
            .await?;
        response.json().await.map_err(|_| ApiError::Malformed)
    }

    /// Asks where the finished MP4 of `size_bytes` goes: the presigned part URLs.
    pub async fn render_uploads(
        &self,
        request_id: &str,
        input: &UploadInput,
    ) -> Result<UploadPlan, ApiError> {
        let response = self
            .post(&self.url("render", request_id, "uploads")?, input)
            .await?;
        response.json().await.map_err(|_| ApiError::Malformed)
    }

    pub async fn render_finish(
        &self,
        request_id: &str,
        input: &RenderFinishInput,
    ) -> Result<(), ApiError> {
        self.post(&self.url("render", request_id, "finish")?, input)
            .await
            .map(|_| ())
    }

    /// The address of `action` on the request `request_id`; refuses an id that is not a site id.
    fn url(&self, kind: &str, request_id: &str, action: &str) -> Result<String, ApiError> {
        check_id(request_id)?;
        Ok(format!("{}/{kind}/{request_id}/{action}", self.base))
    }

    async fn post<B: Serialize>(&self, url: &str, body: &B) -> Result<reqwest::Response, ApiError> {
        let response = self
            .http
            .post(url)
            .bearer_auth(self.token.expose())
            .json(body)
            .send()
            .await
            .map_err(|_| ApiError::Unavailable)?;
        match response.status() {
            s if s.is_success() => Ok(response),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(ApiError::Unauthorized),
            StatusCode::NOT_FOUND => Err(ApiError::NotFound),
            StatusCode::CONFLICT => Err(ApiError::Conflict),
            s if s.is_server_error()
                || s == StatusCode::TOO_MANY_REQUESTS
                || s == StatusCode::REQUEST_TIMEOUT =>
            {
                Err(ApiError::Unavailable)
            }
            s => Err(ApiError::Rejected(s.as_u16())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_claim_answer_reads_as_the_api_describes_it() {
        let claim: StreamClaim = serde_json::from_str(
            r#"{"request_id":"r1","platform":"YOUTUBE","destination_id":"d1","key_sealed":"AAAA",
                "mp4_url":"https://store.example/f.mp4?sig=1","duration_ms":60000,"lease_until":1700000000000,"extra":1}"#,
        )
        .unwrap();
        assert_eq!(claim.platform, "YOUTUBE");
        assert_eq!((claim.preroll_ms, claim.file_ms()), (0, 60_000));
        assert_eq!(claim.mp4_url.expose(), "https://store.example/f.mp4?sig=1");
        assert!(!format!("{claim:?}").contains("sig=1"));
    }

    #[test]
    fn a_stream_claim_with_a_countdown_card_is_that_much_longer() {
        let claim: StreamClaim = serde_json::from_str(
            r#"{"request_id":"r1","platform":"YOUTUBE","destination_id":"d1","key_sealed":"AAAA",
                "mp4_url":"https://store.example/f.mp4","duration_ms":60000,"preroll_ms":300000,"lease_until":1}"#,
        )
        .unwrap();
        assert_eq!((claim.preroll_ms, claim.file_ms()), (300_000, 360_000));
    }

    #[test]
    fn the_heartbeat_answer_defaults() {
        let h: Heartbeat = serde_json::from_str(r#"{"lease_until":5}"#).unwrap();
        assert!(!h.stop);
        let h: Heartbeat =
            serde_json::from_str(r#"{"lease_until":5,"stop":true,"render_token":"x"}"#).unwrap();
        assert!(h.stop);
    }

    #[test]
    fn finish_leaves_out_an_absent_code() {
        let ok = serde_json::to_string(&FinishInput {
            worker_id: "w".into(),
            failure_code: None,
            streamed_ms: 7,
        })
        .unwrap();
        assert_eq!(ok, r#"{"worker_id":"w","streamed_ms":7}"#);
        let failed = serde_json::to_string(&FinishInput {
            worker_id: "w".into(),
            failure_code: Some("key_rejected".into()),
            streamed_ms: 0,
        })
        .unwrap();
        assert_eq!(
            failed,
            r#"{"worker_id":"w","failure_code":"key_rejected","streamed_ms":0}"#
        );
    }

    #[test]
    fn the_render_answers_read_as_the_api_describes_them() {
        let claim: RenderClaim = serde_json::from_str(
            r#"{"request_id":"r1","recording_id":"c1","duration_ms":60000,"render_token":"tok-secret",
                "token_expires_at":5,"lease_until":6}"#,
        )
        .unwrap();
        assert_eq!(claim.render_token.expose(), "tok-secret");
        assert!(!format!("{claim:?}").contains("tok-secret"));
        // An older site sends no card: no preroll, nothing on it.
        assert_eq!(
            (
                claim.preroll_ms,
                claim.card_title.as_str(),
                claim.card_teacher.as_str()
            ),
            (0, "", "")
        );
        assert_eq!(claim.file_ms(), 60_000);
        let carded: RenderClaim = serde_json::from_str(
            r#"{"request_id":"r1","recording_id":"c1","duration_ms":60000,"render_token":"t",
                "token_expires_at":5,"lease_until":6,"locale":"vi","preroll_ms":120000,
                "card_title":"Nhập môn","card_teacher":"Văn Lê"}"#,
        )
        .unwrap();
        assert_eq!(carded.preroll_ms, 120_000);
        assert_eq!(carded.file_ms(), 180_000);
        assert_eq!(
            (carded.card_title.as_str(), carded.card_teacher.as_str()),
            ("Nhập môn", "Văn Lê")
        );
        let h: RenderHeartbeat = serde_json::from_str(
            r#"{"lease_until":5,"stop":false,"render_token":"fresh","render_token_expires_at":9}"#,
        )
        .unwrap();
        assert_eq!(h.render_token.unwrap().expose(), "fresh");
        let h: RenderHeartbeat = serde_json::from_str(r#"{"lease_until":5,"stop":true}"#).unwrap();
        assert!(h.stop && h.render_token.is_none());
        let plan: UploadPlan = serde_json::from_str(
            r#"{"part_bytes":67108864,"parts":[{"number":1,"url":"https://s.example/p1?sig=x"}]}"#,
        )
        .unwrap();
        assert_eq!(plan.parts[0].number, 1);
        assert!(!format!("{plan:?}").contains("sig=x"));
    }

    #[test]
    fn a_render_finish_carries_a_code_or_the_file() {
        let failed = serde_json::to_string(&RenderFinishInput {
            worker_id: "w".into(),
            failure_code: Some("render_failed".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            failed,
            r#"{"worker_id":"w","failure_code":"render_failed"}"#
        );
        let ok = serde_json::to_string(&RenderFinishInput {
            worker_id: "w".into(),
            size_bytes: Some(10),
            rendered_ms: Some(2000),
            parts: vec![UploadedPart {
                number: 1,
                etag: "\"e\"".into(),
            }],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            ok,
            r#"{"worker_id":"w","size_bytes":10,"rendered_ms":2000,"parts":[{"number":1,"etag":"\"e\""}]}"#
        );
    }

    #[test]
    fn only_site_ids_go_into_an_address() {
        let client = ApiClient::new(
            "https://site.example/api/v4",
            Secret::new("t"),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(
            client
                .url("stream", "abcdefghij0123456789abcdef", "release")
                .unwrap(),
            "https://site.example/api/v4/lms/stream_worker/stream/abcdefghij0123456789abcdef/release"
        );
        for bad in [
            "",
            "short",
            "abcdefghij0123456789abcde/",
            "ABCDEFGHIJ0123456789ABCDEF",
            "abcdefghij0123456789abcdef0",
            "../../../etc/passwd/0000000",
            "abcdefghij0123456789abcd\n",
        ] {
            assert!(
                matches!(client.url("stream", bad, "finish"), Err(ApiError::BadId)),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_bad_address_is_a_config_error() {
        assert!(matches!(
            ApiClient::new("not a url", Secret::new("t"), Duration::from_secs(1)),
            Err(ApiError::Config)
        ));
    }
}
