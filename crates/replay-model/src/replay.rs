//! The whiteboard of a recording, at any moment of it. Playing forward applies the records since
//! the last position; seeking back rebuilds from the start. Port of `BoardReplay` in
//! `player/board-replay.svelte.ts`.

use crate::element::BoardElement;
use crate::log::{BoardRecord, RecordKind};
use crate::presence::{PRESENCE_TTL_MS, RemotePresence, apply_presence};
use crate::scene::{BoardScene, Envelope};

#[derive(Debug)]
pub struct BoardReplay {
    scene: BoardScene,
    /// Pointers and strokes being drawn, by person, in the order they first appeared (a person
    /// updated in place keeps their place; one who left and came back goes last).
    remote: Vec<(String, RemotePresence)>,
    locked: bool,
    records: Vec<BoardRecord>,
    /// Records before this index are applied.
    next: usize,
    time: f64,
}

impl BoardReplay {
    /// `records` as `parse_board_log` returns them (sorted by time).
    pub fn new(records: Vec<BoardRecord>) -> Self {
        Self {
            scene: BoardScene::new(),
            remote: Vec::new(),
            locked: false,
            records,
            next: 0,
            time: -1.0,
        }
    }

    pub fn scene(&self) -> &BoardScene {
        &self.scene
    }

    /// Whether the teacher locked the board for everyone else.
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Pointers and strokes being drawn, by person.
    pub fn remote(&self) -> impl Iterator<Item = (&str, &RemotePresence)> {
        self.remote.iter().map(|(u, p)| (u.as_str(), p))
    }

    /// What is being drawn right now, as the painter draws it over the board.
    pub fn overlays(&self) -> Vec<&BoardElement> {
        self.remote
            .iter()
            .filter_map(|(_, p)| p.preview.as_ref())
            .collect()
    }

    /// Shows the board as it was at `t` (ms into the recording).
    pub fn seek(&mut self, t: f64) {
        if t < self.time {
            self.scene.clear();
            self.remote.clear();
            self.locked = false;
            self.next = 0;
        }
        self.time = t;
        let mut pending: Vec<Envelope> = Vec::new();
        while self.next < self.records.len() && self.records[self.next].t <= t {
            let r = &self.records[self.next];
            self.next += 1;
            match r.kind {
                RecordKind::Snapshot | RecordKind::Elements => {
                    // Consecutive writes merge in one pass (one redraw).
                    pending.extend(r.elements.iter().flatten().cloned());
                    if r.kind == RecordKind::Snapshot {
                        self.locked = r.locked == Some(true);
                    }
                    if let (RecordKind::Elements, Some(u)) = (r.kind, &r.user)
                        && let Some((_, p)) = self.remote.iter_mut().find(|(id, _)| id == u)
                    {
                        p.preview = None;
                    }
                }
                RecordKind::Clear => {
                    pending.clear();
                    self.scene.clear();
                }
                RecordKind::Lock => self.locked = r.locked == Some(true),
                RecordKind::Presence => {
                    let Some(u) = &r.user else { continue };
                    let payload = r.presence.clone().unwrap_or_default();
                    let at = self.remote.iter().position(|(id, _)| id == u);
                    // The element a pointer edits is looked up on the scene as merged so far in
                    // this pass: the web player's pending writes are not in it yet either.
                    let lookup = |id: &str| self.scene.element(id).cloned();
                    let presence =
                        apply_presence(at.map(|i| &self.remote[i].1), &payload, r.t, &lookup);
                    match (presence, at) {
                        (Some(p), Some(i)) => self.remote[i].1 = p,
                        (Some(p), None) => self.remote.push((u.clone(), p)),
                        (None, Some(i)) => {
                            self.remote.remove(i);
                        }
                        (None, None) => {}
                    }
                }
                RecordKind::Quiz => {}
            }
        }
        if !pending.is_empty() {
            self.scene.merge(pending);
        }
        // A pointer that went quiet left the room's screens too.
        self.remote.retain(|(_, p)| p.at >= t - PRESENCE_TTL_MS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::parse_board_log;

    fn replay(lines: &[&str]) -> BoardReplay {
        BoardReplay::new(parse_board_log(&lines.join("\n")))
    }

    fn el(id: &str, version: u32, x2: u32) -> String {
        format!(
            r#"{{"id":"{id}","version":{version},"version_nonce":1,"is_deleted":false,"data":{{"type":"line","color":"ink","size":2,"z":{version},"x1":0,"y1":0,"x2":{x2},"y2":0}}}}"#
        )
    }

    fn ids(r: &BoardReplay) -> Vec<String> {
        r.scene().ordered().iter().map(|e| e.id.clone()).collect()
    }

    #[test]
    fn seeking_forward_applies_only_the_records_up_to_the_time() {
        let a = el("a", 1, 1);
        let b = el("b", 1, 2);
        let l1 = format!(r#"{{"t":100,"k":"snapshot","elements":[{a}]}}"#);
        let l2 = format!(r#"{{"t":200,"k":"elements","elements":[{b}]}}"#);
        let mut r = replay(&[&l1, &l2]);
        r.seek(99.0);
        assert!(ids(&r).is_empty());
        r.seek(100.0);
        assert_eq!(ids(&r), ["a"]);
        r.seek(500.0);
        assert_eq!(ids(&r), ["a", "b"]);
    }

    #[test]
    fn seeking_back_rebuilds_from_the_start() {
        let a = el("a", 1, 1);
        let b = el("b", 2, 2);
        let l1 = format!(r#"{{"t":100,"k":"snapshot","locked":true,"elements":[{a}]}}"#);
        let l2 = format!(r#"{{"t":200,"k":"elements","elements":[{b}]}}"#);
        let mut r = replay(&[&l1, &l2, r#"{"t":300,"k":"lock","locked":false}"#]);
        r.seek(400.0);
        assert!(!r.locked());
        r.seek(150.0);
        assert_eq!(ids(&r), ["a"]);
        assert!(r.locked());
        r.seek(0.0);
        assert!(ids(&r).is_empty() && !r.locked());
    }

    #[test]
    fn a_clear_drops_what_was_written_before_it_in_the_same_pass() {
        let a = el("a", 1, 1);
        let b = el("b", 2, 2);
        let l1 = format!(r#"{{"t":1,"k":"elements","elements":[{a}]}}"#);
        let l3 = format!(r#"{{"t":3,"k":"elements","elements":[{b}]}}"#);
        let mut r = replay(&[&l1, r#"{"t":2,"k":"clear"}"#, &l3]);
        r.seek(10.0);
        assert_eq!(ids(&r), ["b"]);
    }

    #[test]
    fn a_snapshot_sets_the_lock_and_a_missing_flag_unlocks() {
        let mut r = replay(&[
            r#"{"t":1,"k":"lock","locked":true}"#,
            r#"{"t":2,"k":"snapshot","elements":[]}"#,
        ]);
        r.seek(1.0);
        assert!(r.locked());
        r.seek(2.0);
        assert!(!r.locked());
    }

    #[test]
    fn a_pointer_that_went_quiet_disappears_after_the_ttl() {
        let mut r = replay(&[r#"{"t":1000,"k":"presence","u":"u1","p":{"cursor":[5,5]}}"#]);
        r.seek(1000.0 + PRESENCE_TTL_MS);
        assert_eq!(r.remote().count(), 1);
        r.seek(1000.0 + PRESENCE_TTL_MS + 1.0);
        assert_eq!(r.remote().count(), 0);
    }

    #[test]
    fn a_presence_with_nothing_in_it_removes_the_person() {
        let mut r = replay(&[
            r#"{"t":1,"k":"presence","u":"u1","p":{"cursor":[5,5]}}"#,
            r#"{"t":2,"k":"presence","u":"u1","p":{"cursor":null}}"#,
        ]);
        r.seek(1.0);
        assert_eq!(r.remote().count(), 1);
        r.seek(2.0);
        assert_eq!(r.remote().count(), 0);
    }

    #[test]
    fn the_preview_goes_when_the_users_own_write_arrives() {
        let a = el("a", 1, 1);
        let preview = r#"{"t":1,"k":"presence","u":"u1","p":{"cursor":[1,1],"preview":{"type":"line","color":"red","size":2,"z":1,"x1":0,"y1":0,"x2":9,"y2":9}}}"#;
        let write = format!(r#"{{"t":2,"k":"elements","u":"u1","elements":[{a}]}}"#);
        let other = format!(r#"{{"t":3,"k":"elements","u":"u2","elements":[{a}]}}"#);
        let mut r = replay(&[preview, &write, &other]);
        r.seek(1.0);
        assert_eq!(r.overlays().len(), 1);
        r.seek(2.0);
        assert!(r.overlays().is_empty());
        // The person's pointer stays; only the preview was cleared.
        assert_eq!(r.remote().count(), 1);
    }

    #[test]
    fn another_users_write_leaves_a_preview_alone() {
        let a = el("a", 1, 1);
        let preview = r#"{"t":1,"k":"presence","u":"u1","p":{"preview":{"type":"line","color":"red","size":2,"z":1,"x1":0,"y1":0,"x2":9,"y2":9}}}"#;
        let write = format!(r#"{{"t":2,"k":"elements","u":"u2","elements":[{a}]}}"#);
        let mut r = replay(&[preview, &write]);
        r.seek(2.0);
        assert_eq!(r.overlays().len(), 1);
    }

    #[test]
    fn overlays_keep_the_order_people_first_appeared_in() {
        let p = |t: u32, u: &str, x2: u32| {
            format!(
                r#"{{"t":{t},"k":"presence","u":"{u}","p":{{"preview":{{"type":"line","color":"red","size":2,"z":1,"x1":0,"y1":0,"x2":{x2},"y2":0}}}}}}"#
            )
        };
        let (l1, l2, l3) = (p(1, "u1", 1), p(2, "u2", 2), p(3, "u1", 3));
        let mut r = replay(&[&l1, &l2, &l3]);
        r.seek(3.0);
        let names: Vec<&str> = r.remote().map(|(u, _)| u).collect();
        assert_eq!(names, ["u1", "u2"]);
    }

    #[test]
    fn a_ref_preview_reads_the_scene_merged_so_far() {
        let graph = r#"{"id":"g","version":1,"version_nonce":1,"is_deleted":false,"data":{"type":"graph","color":"blue","size":2,"z":1,"x1":0,"y1":0,"x2":4,"y2":4,"view":[-1,1,-1,1],"series":[],"params":{"a":1}}}"#;
        let l1 = format!(r#"{{"t":1,"k":"elements","elements":[{graph}]}}"#);
        let upd = r#"{"t":2,"k":"presence","u":"u1","p":{"preview":{"ref":"g","set":{"params":{"a":2}}}}}"#;
        let mut r = replay(&[&l1, upd]);
        // Both in one pass: the write is still pending when the pointer asks for it, as on the web.
        r.seek(2.0);
        assert!(r.overlays().is_empty());
        let mut r = replay(&[&l1, upd]);
        r.seek(1.0);
        r.seek(2.0);
        assert_eq!(r.overlays().len(), 1);
    }
}
