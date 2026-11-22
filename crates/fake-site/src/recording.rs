//! A recording on disk: its files, the board log and the deck pictures, and the manifest that
//! lists them.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Value, json};

pub const RECORDING_ID: &str = "recording0000000000001";
pub const CLASS_ID: &str = "classfixture00000000000001";
pub const TEACHER_ID: &str = "teacherfixture0000000001";
pub const DECK_ID: &str = "deckfixture00000000000001a";

/// One stream of the recording.
#[derive(Debug, Clone)]
pub struct RecFile {
    /// `voice`, `camera`, `screen`, `screen_audio` or `board`.
    pub kind: &'static str,
    pub user: &'static str,
    pub session: &'static str,
    pub start_ms: u64,
    pub end_ms: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub path: PathBuf,
    pub size: u64,
}

/// Everything the fake site serves of one recording.
#[derive(Debug, Clone)]
pub struct Recording {
    pub name: String,
    pub duration_ms: u64,
    pub files: Vec<RecFile>,
    /// Deck page pictures by (deck, page), as PNG bytes.
    pub pictures: HashMap<(String, u32), &'static [u8]>,
}

impl Recording {
    /// The manifest as the site sends it.
    pub fn manifest(&self) -> Value {
        let files: Vec<Value> = self
            .files
            .iter()
            .map(|f| {
                let mut v = json!({
                    "kind": f.kind, "user_id": f.user, "session_id": f.session,
                    "start_ms": f.start_ms, "end_ms": f.end_ms, "size": f.size,
                });
                if let (Some(w), Some(h)) = (f.width, f.height) {
                    v["width"] = w.into();
                    v["height"] = h.into();
                }
                v
            })
            .collect();
        json!({
            "duration_ms": self.duration_ms, "files": files,
            "teacher_id": TEACHER_ID, "class_id": CLASS_ID,
        })
    }
}
