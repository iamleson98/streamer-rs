//! A recording's table of contents, as the site's manifest endpoint sends it. Port of the types in
//! `lesson-replay`'s `types.ts` and of `readManifest`'s checks.

use serde::Deserialize;

/// What a stream is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Voice,
    Camera,
    Screen,
    ScreenAudio,
    Board,
}

/// One stream of a recording, placed on its timeline.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ManifestFile {
    pub kind: FileKind,
    /// The call session the stream came from (not set for the board).
    #[serde(default)]
    pub session_id: Option<String>,
    /// The person the stream came from, when known.
    #[serde(default)]
    pub user_id: Option<String>,
    /// Where the file starts on the recording's timeline (ms).
    pub start_ms: f64,
    /// Where it ends (ms).
    pub end_ms: f64,
    /// A video's size when it started.
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// Its size in bytes.
    pub size: u64,
}

/// A file with its number in the manifest (the file's address).
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingFile {
    pub index: u32,
    pub file: ManifestFile,
}

impl std::ops::Deref for RecordingFile {
    type Target = ManifestFile;
    fn deref(&self) -> &ManifestFile {
        &self.file
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RecordingManifest {
    pub duration_ms: f64,
    pub files: Vec<ManifestFile>,
    /// The class's teacher.
    pub teacher_id: String,
    /// The recording's class: imported deck pages are read through it.
    #[serde(default)]
    pub class_id: Option<String>,
    /// Where to read the board log straight from storage.
    #[serde(default)]
    pub board_url: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("the manifest is not a recording's")]
pub struct ManifestError;

impl RecordingManifest {
    /// Reads and checks a manifest: a positive duration, a teacher and files that end after they
    /// begin.
    pub fn parse(json: &[u8]) -> Result<Self, ManifestError> {
        let manifest: Self = serde_json::from_slice(json).map_err(|_| ManifestError)?;
        let sane = manifest.duration_ms.is_finite()
            && manifest.duration_ms > 0.0
            && manifest
                .files
                .iter()
                .all(|f| f.start_ms.is_finite() && f.end_ms.is_finite() && f.end_ms >= f.start_ms);
        if sane {
            Ok(manifest)
        } else {
            Err(ManifestError)
        }
    }

    /// The files with their numbers.
    pub fn numbered(&self) -> Vec<RecordingFile> {
        self.files
            .iter()
            .enumerate()
            .map(|(index, file)| RecordingFile {
                index: index as u32,
                file: file.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_reads_and_numbers_its_files() {
        let m = RecordingManifest::parse(
            br#"{"duration_ms": 5000, "teacher_id": "t1", "class_id": "c",
                "files": [{"kind":"board","start_ms":0,"end_ms":5000,"size":10},
                          {"kind":"screen_audio","user_id":"t1","session_id":"s","start_ms":1,"end_ms":2,"size":3}]}"#,
        )
        .unwrap();
        let files = m.numbered();
        assert_eq!(files[1].index, 1);
        assert_eq!(files[1].kind, FileKind::ScreenAudio);
        assert_eq!(m.class_id.as_deref(), Some("c"));
    }

    #[test]
    fn a_manifest_that_is_not_a_recordings_is_refused() {
        for bad in [
            &br#"{"duration_ms": 0, "teacher_id": "t", "files": []}"#[..],
            br#"{"duration_ms": 5, "files": []}"#,
            br#"{"duration_ms": 5, "teacher_id": "t", "files": [{"kind":"camera","start_ms":9,"end_ms":1,"size":1}]}"#,
            br#"{"duration_ms": 5, "teacher_id": "t", "files": [{"kind":"other","start_ms":0,"end_ms":1,"size":1}]}"#,
            b"[]",
        ] {
            assert!(RecordingManifest::parse(bad).is_err());
        }
    }
}
