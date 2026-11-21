//! The board log of a recording: gzipped NDJSON, one record per line, in arrival order. Port of
//! `parseBoardLog`/`readBoardLog` in `player/board-replay.svelte.ts`. The recorder writes it
//! (`rtcd`), the player and this crate read it.

use std::io::Read;

use flate2::read::GzDecoder;
use serde_json::Value;

use crate::presence::PresencePayload;
use crate::scene::Envelope;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    Snapshot,
    Elements,
    Clear,
    Lock,
    Presence,
    /// The lesson's quiz game: the board skips these.
    Quiz,
}

impl RecordKind {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "snapshot" => Self::Snapshot,
            "elements" => Self::Elements,
            "clear" => Self::Clear,
            "lock" => Self::Lock,
            "presence" => Self::Presence,
            "quiz" => Self::Quiz,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoardRecord {
    /// Offset into the recording (ms).
    pub t: f64,
    pub kind: RecordKind,
    /// The person the record is about.
    pub user: Option<String>,
    pub elements: Option<Vec<Envelope>>,
    pub locked: Option<bool>,
    pub presence: Option<PresencePayload>,
}

#[derive(Debug, thiserror::Error)]
#[error("the board log is not readable gzip")]
pub struct LogError(#[from] std::io::Error);

fn record_of(line: &str) -> Option<BoardRecord> {
    let r: Value = serde_json::from_str(line).ok()?;
    let t = r.get("t")?.as_f64()?;
    let kind = RecordKind::parse(r.get("k")?.as_str()?)?;
    // An envelope that does not read is skipped, as the web player would not draw it.
    let elements = r.get("elements").and_then(Value::as_array).map(|list| {
        list.iter()
            .filter_map(|e| serde_json::from_value::<Envelope>(e.clone()).ok())
            .collect()
    });
    Some(BoardRecord {
        t,
        kind,
        // `if (r.u)`: an empty id is no id.
        user: r
            .get("u")
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty())
            .map(str::to_owned),
        elements,
        locked: r.get("locked").map(|l| *l == Value::Bool(true)),
        presence: r.get("p").map(PresencePayload::from_value),
    })
}

/// Reads the log's lines; a malformed line is skipped, not fatal (a truncated last line: the
/// recorder stopped mid-write). Sorted by time with a stable sort, which only guards against a
/// clock step.
pub fn parse_board_log(text: &str) -> Vec<BoardRecord> {
    let mut records: Vec<BoardRecord> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(record_of)
        .collect();
    records.sort_by(|a, b| a.t.total_cmp(&b.t));
    records
}

/// Unzips the board log (gzip, as served) and parses it.
pub fn read_board_log(gz: impl Read) -> Result<Vec<BoardRecord>, LogError> {
    let mut text = String::new();
    GzDecoder::new(gz).read_to_string(&mut text)?;
    Ok(parse_board_log(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;

    #[test]
    fn lines_that_do_not_parse_or_are_not_records_are_skipped() {
        let text = [
            r#"{"t":5,"k":"clear"}"#,
            "",
            "not json",
            r#"{"t":"x","k":"clear"}"#,
            r#"{"t":1,"k":"mystery"}"#,
            r#"{"t":2,"k":"lock","locked":true}"#,
            r#"{"t":9,"k":"snapshot","elements":[{"id":"a","version":1,"version_nonce":2,"is_deleted":false,"data":{}},{"id":"b"}]}"#,
            r#"{"t":10,"k":"elements","elements":[]"#,
        ]
        .join("\n");
        let records = parse_board_log(&text);
        let kinds: Vec<_> = records.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            [RecordKind::Lock, RecordKind::Clear, RecordKind::Snapshot]
        );
        // The envelope that is not whole is dropped, the other kept.
        assert_eq!(records[2].elements.as_ref().unwrap().len(), 1);
        assert_eq!(records[0].locked, Some(true));
    }

    #[test]
    fn records_are_put_in_time_order_and_equal_times_keep_their_order() {
        let text = [
            r#"{"t":5,"k":"lock","locked":true,"u":"first"}"#,
            r#"{"t":3,"k":"clear"}"#,
            r#"{"t":5,"k":"lock","locked":false,"u":"second"}"#,
        ]
        .join("\n");
        let users: Vec<_> = parse_board_log(&text).into_iter().map(|r| r.user).collect();
        assert_eq!(users, [None, Some("first".into()), Some("second".into())]);
    }

    #[test]
    fn an_empty_user_is_no_user_and_quiz_records_are_kept_for_the_quiz() {
        let records = parse_board_log("{\"t\":1,\"k\":\"quiz\",\"u\":\"\",\"q\":[1]}");
        assert_eq!(records[0].kind, RecordKind::Quiz);
        assert_eq!(records[0].user, None);
    }

    #[test]
    fn a_gzipped_log_reads_and_garbage_is_an_error() {
        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        gz.write_all(b"{\"t\":1,\"k\":\"clear\"}\n").unwrap();
        let bytes = gz.finish().unwrap();
        assert_eq!(read_board_log(bytes.as_slice()).unwrap().len(), 1);
        assert!(read_board_log(&b"plain text"[..]).is_err());
    }
}
