//! The site's stream worker routes under `{api base}/lms/stream_worker/stream/`: the request and
//! response bodies as the API describes them, and a client that speaks them with the runner's
//! bearer token.

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
    pub lease_until: i64,
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

/// Why a call did not give an answer. None of the messages carries a body, an address or a token.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The server or the network failed (a 5xx such as 503, a timeout, a dropped connection): ask again.
    #[error("the site is not answering")]
    Unavailable,
    /// The runner's token was refused.
    #[error("the site refused the runner token")]
    Unauthorized,
    /// The request is unknown to the site.
    #[error("the site does not know the request")]
    NotFound,
    /// The site refused the call for another reason.
    #[error("the site answered with status {0}")]
    Rejected(u16),
    /// The site's answer was not what the API describes.
    #[error("the site's answer could not be read")]
    Malformed,
    /// The client could not be built.
    #[error("the API address is not usable")]
    Config,
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
        let base = format!(
            "{}/lms/stream_worker/stream",
            base_url.trim_end_matches('/')
        );
        reqwest::Url::parse(&base).map_err(|_| ApiError::Config)?;
        Ok(Self { http, base, token })
    }

    /// Takes the next stream due, `None` when nothing waits (204).
    pub async fn claim(&self, worker_id: &str) -> Result<Option<StreamClaim>, ApiError> {
        let body = ClaimInput {
            worker_id: worker_id.to_owned(),
        };
        let response = self.post(&format!("{}/claim", self.base), &body).await?;
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
            .post(&format!("{}/{request_id}/heartbeat", self.base), input)
            .await?;
        response.json().await.map_err(|_| ApiError::Malformed)
    }

    pub async fn finish(&self, request_id: &str, input: &FinishInput) -> Result<(), ApiError> {
        self.post(&format!("{}/{request_id}/finish", self.base), input)
            .await
            .map(|_| ())
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
        assert_eq!(claim.mp4_url.expose(), "https://store.example/f.mp4?sig=1");
        assert!(!format!("{claim:?}").contains("sig=1"));
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
    fn a_bad_address_is_a_config_error() {
        assert!(matches!(
            ApiClient::new("not a url", Secret::new("t"), Duration::from_secs(1)),
            Err(ApiError::Config)
        ));
    }
}
