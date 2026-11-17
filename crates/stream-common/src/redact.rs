//! Keeping secrets out of everything a workload writes: log lines, error chains, panic messages and
//! anything it sends. A `Redactor` replaces the secrets it was given (and the query string of every
//! web address, where a presigned address keeps its signature); a process-wide registry feeds the
//! logging writer, so a line is cleaned whoever wrote it.

use std::borrow::Cow;
use std::fmt;
use std::io::{self, Write};
use std::sync::{Arc, RwLock};

use tracing_subscriber::fmt::MakeWriter;
use zeroize::Zeroizing;

/// What a secret is replaced with.
pub const REDACTED: &str = "[redacted]";

/// The shortest secret that is searched for; anything shorter would be cut out of unrelated words.
const MIN_SECRET_LEN: usize = 4;

/// Replaces secrets in text.
#[derive(Debug, Default, Clone)]
pub struct Redactor {
    secrets: Vec<Zeroizing<String>>,
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a secret, in the forms it may appear: as given and percent-encoded.
    pub fn add(&mut self, secret: &str) {
        if secret.len() < MIN_SECRET_LEN {
            return;
        }
        for form in [secret.to_owned(), percent_encode(secret)] {
            let form = Zeroizing::new(form);
            if !self.secrets.contains(&form) {
                self.secrets.push(form);
            }
        }
        // Longest first, so a secret that holds another is cut out whole.
        self.secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    }

    /// The text with every known secret and every web address's query string replaced.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut out = Cow::Borrowed(text);
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = Cow::Owned(out.replace(secret.as_str(), REDACTED));
            }
        }
        match strip_url_queries(&out) {
            Some(stripped) => Cow::Owned(stripped),
            None => out,
        }
    }
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Cuts the query string (`?` to the end of the address) out of every `http://` and `https://`
/// address in the text, or `None` when there is none.
fn strip_url_queries(text: &str) -> Option<String> {
    if !text.contains("http") || !text.contains('?') {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut changed = false;
    while let Some(at) = ["https://", "http://"]
        .iter()
        .filter_map(|p| rest.find(p))
        .min()
    {
        let (head, tail) = rest.split_at(at);
        out.push_str(head);
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')' | ']'))
            .unwrap_or(tail.len());
        let (url, after) = tail.split_at(end);
        match url.find('?') {
            Some(q) => {
                out.push_str(&url[..q]);
                out.push_str("?[redacted]");
                changed = true;
            }
            None => out.push_str(url),
        }
        rest = after;
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// The secrets registered now, and the redactor built from them (rebuilt when the set changes, not
/// for every line).
struct Registry {
    secrets: Vec<Zeroizing<String>>,
    redactor: Option<Arc<Redactor>>,
}

impl Registry {
    fn rebuild(&mut self) {
        let mut redactor = Redactor::new();
        for secret in &self.secrets {
            redactor.add(secret);
        }
        self.redactor = Some(Arc::new(redactor));
    }
}

static REGISTRY: RwLock<Registry> = RwLock::new(Registry {
    secrets: Vec::new(),
    redactor: None,
});

/// A secret in the process-wide registry; it leaves when this is dropped.
pub struct Registered(Zeroizing<String>);

impl fmt::Debug for Registered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Registered([redacted])")
    }
}

/// Puts a secret in the registry: from now on `redact_global` and the logging writer cut it out.
pub fn register_secret(secret: &str) -> Registered {
    if secret.len() >= MIN_SECRET_LEN {
        let mut registry = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
        registry.secrets.push(Zeroizing::new(secret.to_owned()));
        registry.rebuild();
    }
    Registered(Zeroizing::new(secret.to_owned()))
}

impl Drop for Registered {
    fn drop(&mut self) {
        let mut registry = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = registry.secrets.iter().position(|s| *s == self.0) {
            registry.secrets.swap_remove(i);
            registry.rebuild();
        }
    }
}

/// Redacts with the registry's secrets.
pub fn redact_global(text: &str) -> String {
    let redactor = REGISTRY
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .redactor
        .clone();
    match redactor {
        Some(redactor) => redactor.redact(text).into_owned(),
        None => Redactor::new().redact(text).into_owned(),
    }
}

/// Passes lines to `W` after `redact_global`. A line is cleaned whole: bytes wait for the newline.
#[derive(Debug)]
pub struct RedactingWriter<W: Write> {
    inner: W,
    pending: Vec<u8>,
}

impl<W: Write> RedactingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            pending: Vec::new(),
        }
    }

    fn emit(&mut self, upto: usize) -> io::Result<()> {
        let line: Vec<u8> = self.pending.drain(..upto).collect();
        let clean = redact_global(&String::from_utf8_lossy(&line));
        self.inner.write_all(clean.as_bytes())
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        while let Some(i) = self.pending.iter().position(|b| *b == b'\n') {
            self.emit(i + 1)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write> Drop for RedactingWriter<W> {
    fn drop(&mut self) {
        let rest = self.pending.len();
        // Nothing can be reported from a drop; a line that cannot be written is lost.
        let _ = self.emit(rest).and_then(|()| self.inner.flush());
    }
}

/// A `MakeWriter` whose writers redact.
#[derive(Debug, Clone)]
pub struct RedactingMakeWriter<M>(pub M);

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for RedactingMakeWriter<M> {
    type Writer = RedactingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter::new(self.0.make_writer())
    }
}

/// Starts logging to stderr, redacted, at the level in `RUST_LOG` (default `info`), and routes
/// panics through the same writer: the default panic hook would print past it.
pub fn init_logging() {
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(RedactingMakeWriter(io::stderr))
        .with_ansi(false)
        .init();
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
        tracing::error!(location, message = message.unwrap_or("no message"), "panic");
    }));
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn replaces_every_occurrence() {
        let mut r = Redactor::new();
        r.add("abcd-efgh-ijkl");
        assert_eq!(
            r.redact("push to rtmps://a.example/live/abcd-efgh-ijkl failed, key abcd-efgh-ijkl"),
            "push to rtmps://a.example/live/[redacted] failed, key [redacted]"
        );
    }

    #[test]
    fn replaces_the_percent_encoded_form() {
        let mut r = Redactor::new();
        r.add("ab cd+ef");
        assert_eq!(r.redact("x ab%20cd%2Bef y"), "x [redacted] y");
    }

    #[test]
    fn a_secret_that_holds_another_goes_whole() {
        let mut r = Redactor::new();
        r.add("abcdef");
        r.add("abcdefghij");
        assert_eq!(r.redact("k=abcdefghij"), "k=[redacted]");
    }

    #[test]
    fn short_secrets_are_not_searched_for() {
        let mut r = Redactor::new();
        r.add("a");
        assert_eq!(r.redact("a banana"), "a banana");
    }

    #[test]
    fn cuts_the_query_out_of_every_web_address() {
        let r = Redactor::new();
        assert_eq!(
            r.redact("GET https://bucket.example/f.mp4?X-Amz-Signature=abc&X-Amz-Credential=def failed (retrying)"),
            "GET https://bucket.example/f.mp4?[redacted] failed (retrying)"
        );
        assert_eq!(
            r.redact("a http://h/p?q=1 b https://h2/p2?r=2\" end"),
            "a http://h/p?[redacted] b https://h2/p2?[redacted]\" end"
        );
        assert_eq!(
            r.redact("https://h/p no query, what? yes"),
            "https://h/p no query, what? yes"
        );
    }

    #[test]
    fn a_registration_never_prints_its_secret() {
        let guard = register_secret("topsecret-key-2");
        assert!(!format!("{guard:?}").contains("topsecret"));
    }

    #[test]
    fn leaves_clean_text_alone() {
        let mut r = Redactor::new();
        r.add("abcd-efgh");
        assert!(matches!(r.redact("nothing to hide"), Cow::Borrowed(_)));
    }

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_writer_cleans_lines_split_across_writes_and_forgets_dropped_secrets() {
        let sink = Sink::default();
        {
            let _guard = register_secret("topsecret-key-1");
            let mut w = RedactingWriter::new(sink.clone());
            w.write_all(b"error: bad key topsec").unwrap();
            w.write_all(b"ret-key-1 here\nsecond top").unwrap();
            w.write_all(b"secret-key-1").unwrap();
        }
        let text = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        assert_eq!(text, "error: bad key [redacted] here\nsecond [redacted]");

        let sink = Sink::default();
        let mut w = RedactingWriter::new(sink.clone());
        w.write_all(b"topsecret-key-1\n").unwrap();
        assert_eq!(
            String::from_utf8(sink.0.lock().unwrap().clone()).unwrap(),
            "topsecret-key-1\n"
        );
    }
}
