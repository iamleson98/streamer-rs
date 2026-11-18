//! The stream key: sealed by the browser to the runner's public key (RSA-OAEP, SHA-256, label =
//! the destination id), opened here, checked for characters and kept in a type that cannot be
//! printed by accident.

use std::fmt;

use aws_lc_rs::rsa::{OAEP_SHA256_MGF1SHA256, OaepPrivateDecryptingKey, PrivateDecryptingKey};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use zeroize::{Zeroize, Zeroizing};

/// The longest stream key accepted. Platforms issue keys of a few dozen characters.
pub const KEY_MAX_LEN: usize = 128;

/// The longest secret opened: a key, or a TikTok push address (the server's address and the key).
/// An RSA-3072 key seals at most 318 bytes.
pub const SECRET_MAX_LEN: usize = 300;

/// Why a key could not be used. The messages say what is wrong, never what the key is.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeyError {
    #[error("the runner's private key is not an unencrypted PKCS#8 RSA key in PEM form")]
    BadPrivateKey,
    #[error("the sealed key is not valid base64")]
    BadSealed,
    #[error("the sealed key could not be opened with the runner's key and the destination label")]
    Open,
    #[error("the stream key has characters or a length a stream key never has")]
    Invalid,
}

/// A value that never prints: `Debug` shows a marker and there is no `Display`. Its memory is
/// overwritten when it is dropped.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// The value, for the one place that must hand it on (an argument, a header).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

/// A stream key that passed the character check.
#[derive(Debug, Clone)]
pub struct StreamKey(Secret);

impl StreamKey {
    /// Accepts printable ASCII only and nothing that could change where the stream goes when the key
    /// is appended to an ingest address: no `/`, `@`, `:`, `?`, `#`, `%`, `\`, quotes or whitespace.
    pub fn new(value: String) -> Result<Self, KeyError> {
        let value = Zeroizing::new(value);
        let ok = !value.is_empty()
            && value.len() <= KEY_MAX_LEN
            && value.bytes().all(|b| {
                b.is_ascii_graphic()
                    && !matches!(
                        b,
                        b'/' | b'@' | b':' | b'?' | b'#' | b'%' | b'\\' | b'"' | b'\''
                    )
            });
        if ok {
            Ok(Self(Secret(value)))
        } else {
            Err(KeyError::Invalid)
        }
    }

    pub fn expose(&self) -> &str {
        self.0.expose()
    }
}

/// The lines around a PKCS#8 private key in a PEM file.
fn pem_markers() -> (String, String) {
    const LABEL: &str = "PRIVATE KEY";
    (
        format!("-----BEGIN {LABEL}-----"),
        format!("-----END {LABEL}-----"),
    )
}

/// The runner's RSA private key, loaded from a PKCS#8 PEM file's text.
#[derive(Debug)]
pub struct RunnerPrivateKey {
    key: OaepPrivateDecryptingKey,
}

impl RunnerPrivateKey {
    pub fn from_pem(pem: &str) -> Result<Self, KeyError> {
        let (begin, end) = pem_markers();
        let start = pem.find(&begin).ok_or(KeyError::BadPrivateKey)? + begin.len();
        let stop = pem[start..].find(&end).ok_or(KeyError::BadPrivateKey)? + start;
        let body: String = pem[start..stop].split_whitespace().collect();
        let der = STANDARD.decode(body).map_err(|_| KeyError::BadPrivateKey)?;
        let key = PrivateDecryptingKey::from_pkcs8(&der).map_err(|_| KeyError::BadPrivateKey)?;
        Self::from_key(key)
    }

    pub fn from_key(key: PrivateDecryptingKey) -> Result<Self, KeyError> {
        Ok(Self {
            key: OaepPrivateDecryptingKey::new(key).map_err(|_| KeyError::BadPrivateKey)?,
        })
    }

    /// Opens the key a browser sealed for the destination `destination_id` (the OAEP label): a plain
    /// stream key, checked for characters.
    pub fn open(&self, sealed_base64: &str, destination_id: &str) -> Result<StreamKey, KeyError> {
        let secret = self.open_secret(sealed_base64, destination_id)?;
        StreamKey::new(secret.expose().to_owned())
    }

    /// Opens what a browser sealed for the destination, as printable ASCII of a bounded length and
    /// nothing more: what it is (a key, or a TikTok push address) is the platform's to check.
    pub fn open_secret(
        &self,
        sealed_base64: &str,
        destination_id: &str,
    ) -> Result<Secret, KeyError> {
        let sealed = STANDARD
            .decode(sealed_base64.trim())
            .map_err(|_| KeyError::BadSealed)?;
        let mut out = vec![0u8; self.key.min_output_size()];
        let plain = self
            .key
            .decrypt(
                &OAEP_SHA256_MGF1SHA256,
                &sealed,
                &mut out,
                Some(destination_id.as_bytes()),
            )
            .map_err(|_| KeyError::Open)?;
        let text = std::str::from_utf8(plain)
            .map(str::to_owned)
            .map_err(|_| KeyError::Invalid);
        out.zeroize();
        let text = Zeroizing::new(text?);
        if text.is_empty()
            || text.len() > SECRET_MAX_LEN
            || !text.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(KeyError::Invalid);
        }
        Ok(Secret::new(text.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use aws_lc_rs::rsa::{KeySize, OaepPublicEncryptingKey};

    use super::*;

    const LABEL: &str = "01hdestination00000000001";

    fn pair() -> (RunnerPrivateKey, OaepPublicEncryptingKey) {
        let private = PrivateDecryptingKey::generate(KeySize::Rsa2048).unwrap();
        let public = OaepPublicEncryptingKey::new(private.public_key()).unwrap();
        (RunnerPrivateKey::from_key(private).unwrap(), public)
    }

    fn seal(public: &OaepPublicEncryptingKey, plain: &[u8], label: &str) -> String {
        let mut out = vec![0u8; public.ciphertext_size()];
        let sealed = public
            .encrypt(
                &OAEP_SHA256_MGF1SHA256,
                plain,
                &mut out,
                Some(label.as_bytes()),
            )
            .unwrap();
        STANDARD.encode(sealed)
    }

    #[test]
    fn opens_a_key_sealed_with_the_destination_label() {
        let (private, public) = pair();
        let key = private
            .open(&seal(&public, b"abcd-efgh-ijkl-mnop-qrst", LABEL), LABEL)
            .unwrap();
        assert_eq!(key.expose(), "abcd-efgh-ijkl-mnop-qrst");
    }

    #[test]
    fn a_wrong_label_does_not_open() {
        let (private, public) = pair();
        let sealed = seal(&public, b"abcd-efgh-ijkl-mnop-qrst", LABEL);
        assert_eq!(
            private
                .open(&sealed, "01hdestination00000000002")
                .unwrap_err(),
            KeyError::Open
        );
        assert_eq!(private.open(&sealed, "").unwrap_err(), KeyError::Open);
    }

    #[test]
    fn another_runner_key_does_not_open() {
        let (_, public) = pair();
        let (other, _) = pair();
        assert_eq!(
            other
                .open(&seal(&public, b"abcd-efgh", LABEL), LABEL)
                .unwrap_err(),
            KeyError::Open
        );
    }

    #[test]
    fn garbage_is_refused() {
        let (private, _) = pair();
        assert_eq!(
            private.open("not base64!", LABEL).unwrap_err(),
            KeyError::BadSealed
        );
        assert_eq!(
            private.open(&STANDARD.encode([1u8; 7]), LABEL).unwrap_err(),
            KeyError::Open
        );
    }

    #[test]
    fn a_sealed_key_with_bad_characters_is_refused_after_opening() {
        let (private, public) = pair();
        for bad in [
            "a/b", "a@b", "a:b", "a b", "a\nb", "a?b", "a#b", "a%2fb", "", "ключ",
        ] {
            let err = private
                .open(&seal(&public, bad.as_bytes(), LABEL), LABEL)
                .unwrap_err();
            assert_eq!(err, KeyError::Invalid, "{bad:?}");
        }
    }

    #[test]
    fn open_secret_takes_a_push_address_and_nothing_unprintable_or_too_long() {
        let (private, public) = pair();
        let address = "rtmp://push.tiktokcdn.com/game/stream-1234?expire=1&sign=ab";
        let secret = private
            .open_secret(&seal(&public, address.as_bytes(), LABEL), LABEL)
            .unwrap();
        assert_eq!(secret.expose(), address);
        // 190 bytes is what a 2048-bit key seals; 300 is the limit of the runner.
        let long = "k".repeat(190);
        assert!(
            private
                .open_secret(&seal(&public, long.as_bytes(), LABEL), LABEL)
                .is_ok()
        );
        for bad in ["", "a b", "a\nb", "ключ"] {
            let err = private
                .open_secret(&seal(&public, bad.as_bytes(), LABEL), LABEL)
                .unwrap_err();
            assert_eq!(err, KeyError::Invalid, "{bad:?}");
        }
    }

    #[test]
    fn key_characters() {
        assert!(StreamKey::new("abcd-efgh-ijkl-mnop-qrst".into()).is_ok());
        assert!(StreamKey::new("AbC_123.x".into()).is_ok());
        for bad in [
            "a/b", "a@b", "a:b", "a b", "a\tb", "a\0b", "a?b", "a#b", "a\\b", "a\"b", "a'b", "",
        ] {
            assert!(StreamKey::new(bad.into()).is_err(), "{bad:?}");
        }
        assert!(StreamKey::new("k".repeat(KEY_MAX_LEN)).is_ok());
        assert!(StreamKey::new("k".repeat(KEY_MAX_LEN + 1)).is_err());
    }

    #[test]
    fn debug_never_shows_the_key() {
        let key = StreamKey::new("abcd-efgh".into()).unwrap();
        assert!(!format!("{key:?}").contains("abcd"));
    }

    #[test]
    fn a_pkcs8_pem_file_loads_and_opens() {
        use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};

        let private = PrivateDecryptingKey::generate(KeySize::Rsa2048).unwrap();
        let public = OaepPublicEncryptingKey::new(private.public_key()).unwrap();
        let der: Pkcs8V1Der<'static> = private.as_der().unwrap();
        let wrapped: Vec<String> = STANDARD
            .encode(der.as_ref())
            .as_bytes()
            .chunks(64)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect();
        let (begin, end) = pem_markers();
        let pem = format!("{begin}\n{}\n{end}\n", wrapped.join("\n"));
        let loaded = RunnerPrivateKey::from_pem(&pem).unwrap();
        assert_eq!(
            loaded
                .open(&seal(&public, b"abcd-efgh", LABEL), LABEL)
                .unwrap()
                .expose(),
            "abcd-efgh"
        );
    }

    #[test]
    fn pem_loading() {
        assert_eq!(
            RunnerPrivateKey::from_pem("nothing").unwrap_err(),
            KeyError::BadPrivateKey
        );
        let (begin, end) = pem_markers();
        for body in ["!!!", "AAAA"] {
            let pem = format!("{begin}\n{body}\n{end}\n");
            assert_eq!(
                RunnerPrivateKey::from_pem(&pem).unwrap_err(),
                KeyError::BadPrivateKey,
                "{body}"
            );
        }
    }
}
