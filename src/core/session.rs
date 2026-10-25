//! Open-tab session persisted in `session.json` (app data dir, next to
//! `layout.json`), so the tabs come back after a restart or a crash.
//!
//! Only locations are stored: per tab the primary path, the optional split
//! path and whether the split side had focus, plus the active tab. History,
//! selection, filters and scroll positions are not part of the session.
//!
//! A missing, unreadable, corrupt or unknown-version file simply means "no
//! session": nothing is restored and no other setting is touched.

use crate::core::fs::MY_PC_PATH;
use crate::core::utils::files::path_compare_key;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Current `session.json` format. Files with any other version are ignored.
pub const SESSION_VERSION: u32 = 1;

/// Delay between the first unsaved change and the write. Changes made in the
/// meantime are included, so at most one write happens per interval.
pub const SESSION_SAVE_DELAY: Duration = Duration::from_secs(1);
/// Delay before retrying a failed write.
pub const SESSION_RETRY_DELAY: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionTab {
    pub primary: PathBuf,
    pub split: Option<PathBuf>,
    /// The split side had keyboard focus (only meaningful with `split`).
    pub focused_secondary: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSnapshot {
    pub version: u32,
    pub tabs: Vec<SessionTab>,
    pub active_tab: usize,
}

impl SessionSnapshot {
    /// Checks and repairs a deserialized snapshot. Returns `None` when there is
    /// nothing usable to restore (unknown version, no tabs).
    fn validated(mut self) -> Option<Self> {
        if self.version != SESSION_VERSION {
            return None;
        }
        self.tabs.retain(|tab| !tab.primary.as_os_str().is_empty());
        if self.tabs.is_empty() {
            return None;
        }
        for tab in &mut self.tabs {
            if tab
                .split
                .as_ref()
                .is_some_and(|split| split.as_os_str().is_empty())
            {
                tab.split = None;
            }
            if tab.split.is_none() {
                tab.focused_secondary = false;
            }
        }
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        Some(self)
    }
}

/// Maps a view's location to the path stored in the session. Locations that
/// cannot be reopened as a folder view fall back to This PC: raw physical
/// drives (they only open the "not NTFS" popup) and empty paths.
pub fn session_path_for(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text.is_empty() || text.starts_with(r"\\.\PhysicalDrive") {
        PathBuf::from(MY_PC_PATH)
    } else {
        path.to_path_buf()
    }
}

/// One tab as seen by the change detection: the tab id and the locations the
/// session would store for it.
#[derive(Clone, Copy, Debug)]
pub struct SessionTabKey<'a> {
    pub id: u64,
    pub primary: &'a Path,
    pub split: Option<&'a Path>,
    pub focused_secondary: bool,
}

/// Cheap fingerprint of everything the session stores (plus tab ids). Computed
/// every frame: O(tabs), hashing a few short paths, no allocation.
pub fn session_signature<'a>(
    tabs: impl IntoIterator<Item = SessionTabKey<'a>>,
    active_tab: usize,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut count = 0usize;
    for tab in tabs {
        tab.id.hash(&mut hasher);
        tab.primary.hash(&mut hasher);
        tab.split.hash(&mut hasher);
        tab.focused_secondary.hash(&mut hasher);
        count += 1;
    }
    count.hash(&mut hasher);
    active_tab.hash(&mut hasher);
    hasher.finish()
}

/// What the caller should do this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionSavePoll {
    /// The saved session matches the current state.
    Idle,
    /// A write is due at this instant (schedule a repaint for it).
    WaitUntil(Instant),
    /// Write the current state now, then report the result.
    SaveNow,
}

/// Decides when to write `session.json`: the first change after a save starts
/// a [`SESSION_SAVE_DELAY`] timer; when it expires the then-current state is
/// written. Continuous changes therefore cause at most one write per delay and
/// never postpone the write indefinitely.
#[derive(Debug, Default)]
pub struct SessionSaveScheduler {
    last_saved: Option<u64>,
    due: Option<Instant>,
}

impl SessionSaveScheduler {
    pub fn poll(&mut self, signature: u64, now: Instant) -> SessionSavePoll {
        if self.last_saved == Some(signature) {
            self.due = None;
            return SessionSavePoll::Idle;
        }
        match self.due {
            Some(due) if now >= due => SessionSavePoll::SaveNow,
            Some(due) => SessionSavePoll::WaitUntil(due),
            None => {
                let due = now + SESSION_SAVE_DELAY;
                self.due = Some(due);
                SessionSavePoll::WaitUntil(due)
            }
        }
    }

    /// `true` when `signature` differs from the last successful write.
    pub fn is_dirty(&self, signature: u64) -> bool {
        self.last_saved != Some(signature)
    }

    pub fn mark_saved(&mut self, signature: u64) {
        self.last_saved = Some(signature);
        self.due = None;
    }

    pub fn mark_failed(&mut self, now: Instant) {
        self.due = Some(now + SESSION_RETRY_DELAY);
    }
}

/// Result of merging launch paths into restored tabs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaunchMerge {
    /// Launch paths to open as new tabs, appended in this order.
    pub append: Vec<PathBuf>,
    /// Index (into the tab list after appending) of the tab to activate, if
    /// any launch path was given.
    pub activate: Option<usize>,
}

/// Merges launch paths into the tabs restored from the session: a launch path
/// that is already open (same folder, compared as a key only: case, `/` vs
/// `\`, repeated/trailing separators and `\\?\` are ignored) activates that tab instead of opening a duplicate;
/// the others are appended. The tab of the last launch path ends up active.
pub fn merge_launch_paths(existing: &[PathBuf], launch: &[PathBuf]) -> LaunchMerge {
    let mut keys: Vec<String> = existing.iter().map(|p| path_compare_key(p)).collect();
    let mut merge = LaunchMerge::default();
    for path in launch {
        let key = path_compare_key(path);
        let index = match keys.iter().position(|existing| *existing == key) {
            Some(index) => index,
            None => {
                keys.push(key);
                merge.append.push(path.clone());
                keys.len() - 1
            }
        };
        merge.activate = Some(index);
    }
    merge
}

fn session_file_path() -> Option<PathBuf> {
    Some(crate::core::indexer::app_data_dir()?.join("session.json"))
}

/// Loads the saved session, or `None` when there is nothing valid to restore.
pub fn load_session() -> Option<SessionSnapshot> {
    load_session_from(&session_file_path()?)
}

fn load_session_from(path: &Path) -> Option<SessionSnapshot> {
    let data = std::fs::read_to_string(path).ok()?;
    parse_session(&data)
}

fn parse_session(data: &str) -> Option<SessionSnapshot> {
    serde_json::from_str::<SessionSnapshot>(data)
        .ok()?
        .validated()
}

/// Writes the session atomically (temporary file + rename), so a crash or a
/// kill mid-write leaves the previous file intact.
pub fn save_session(snapshot: &SessionSnapshot) -> std::io::Result<()> {
    let path = session_file_path().ok_or_else(|| std::io::Error::other("no app data directory"))?;
    save_session_to(&path, snapshot)
}

fn save_session_to(path: &Path, snapshot: &SessionSnapshot) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(snapshot).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data)?;
    // Replaces an existing file on Windows (MoveFileExW REPLACE_EXISTING).
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    fn sample() -> SessionSnapshot {
        SessionSnapshot {
            version: SESSION_VERSION,
            tabs: vec![
                SessionTab {
                    primary: p(r"C:\Windows"),
                    split: Some(p(r"C:\Windows\System32")),
                    focused_secondary: true,
                },
                SessionTab {
                    primary: p(MY_PC_PATH),
                    split: None,
                    focused_secondary: false,
                },
            ],
            active_tab: 1,
        }
    }

    #[test]
    fn round_trip_through_file() {
        let dir = std::env::temp_dir().join(format!("eden-session-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("session.json");

        // Missing file -> no session.
        assert_eq!(load_session_from(&path), None);

        // Round trip (also creates the missing parent dir), no temp file left.
        save_session_to(&path, &sample()).unwrap();
        assert_eq!(load_session_from(&path), Some(sample()));
        assert!(!path.with_extension("json.tmp").exists());

        // Overwriting an existing file works.
        let mut changed = sample();
        changed.active_tab = 0;
        save_session_to(&path, &changed).unwrap();
        assert_eq!(load_session_from(&path), Some(changed));

        // Corrupt file -> no session.
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_session_from(&path), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_unusable_files() {
        assert_eq!(parse_session(""), None);
        assert_eq!(parse_session("[]"), None);
        assert_eq!(parse_session("{}"), None); // version 0
        assert_eq!(
            parse_session(r#"{ "version": 2, "tabs": [{ "primary": "C:\\" }] }"#),
            None
        );
        assert_eq!(parse_session(r#"{ "version": 1, "tabs": [] }"#), None);
        assert_eq!(
            parse_session(r#"{ "version": 1, "tabs": [{ "primary": "" }] }"#),
            None
        );
        assert_eq!(
            parse_session(r#"{ "version": 1, "tabs": [{ "primary": 5 }] }"#),
            None
        );
    }

    #[test]
    fn repairs_partial_files() {
        // Missing fields take defaults, out-of-range active tab is clamped,
        // focus on a missing split is dropped, unknown fields are ignored.
        let parsed = parse_session(
            r#"{ "version": 1, "active_tab": 9, "future": true,
                 "tabs": [ { "primary": "" },
                           { "primary": "C:\\Windows", "focused_secondary": true },
                           { "primary": "D:\\", "split": "", "focused_secondary": true } ] }"#,
        )
        .unwrap();
        assert_eq!(
            parsed.tabs,
            vec![
                SessionTab {
                    primary: p(r"C:\Windows"),
                    split: None,
                    focused_secondary: false,
                },
                SessionTab {
                    primary: p(r"D:\"),
                    split: None,
                    focused_secondary: false,
                },
            ]
        );
        assert_eq!(parsed.active_tab, 1);
    }

    #[test]
    fn normalizes_unrestorable_locations() {
        assert_eq!(session_path_for(Path::new("")), p(MY_PC_PATH));
        assert_eq!(
            session_path_for(Path::new(r"\\.\PhysicalDrive1")),
            p(MY_PC_PATH)
        );
        assert_eq!(session_path_for(Path::new(r"C:\Windows")), p(r"C:\Windows"));
        assert_eq!(
            session_path_for(Path::new("::RECYCLE_BIN::")),
            p("::RECYCLE_BIN::")
        );
    }

    #[test]
    fn signature_detects_every_stored_change() {
        fn key<'a>(
            id: u64,
            primary: &'a Path,
            split: Option<&'a PathBuf>,
            focused_secondary: bool,
        ) -> SessionTabKey<'a> {
            SessionTabKey {
                id,
                primary,
                split: split.map(PathBuf::as_path),
                focused_secondary,
            }
        }
        let a = p(r"C:\A");
        let b = p(r"C:\B");
        let base = session_signature([key(1, &a, None, false), key(2, &b, None, false)], 0);
        let same = session_signature([key(1, &a, None, false), key(2, &b, None, false)], 0);
        assert_eq!(base, same);

        let changes = [
            // path change
            session_signature([key(1, &b, None, false), key(2, &b, None, false)], 0),
            // split toggled
            session_signature([key(1, &a, Some(&a), false), key(2, &b, None, false)], 0),
            // split path change
            session_signature([key(1, &a, Some(&b), false), key(2, &b, None, false)], 0),
            // focused side
            session_signature([key(1, &a, Some(&a), true), key(2, &b, None, false)], 0),
            // active tab
            session_signature([key(1, &a, None, false), key(2, &b, None, false)], 1),
            // order
            session_signature([key(2, &b, None, false), key(1, &a, None, false)], 0),
            // tab closed
            session_signature([key(1, &a, None, false)], 0),
        ];
        for (index, changed) in changes.iter().enumerate() {
            assert_ne!(*changed, base, "change #{index} not detected");
        }
        let split_on = changes[1];
        assert_ne!(split_on, changes[3]);
    }

    #[test]
    fn scheduler_throttles_writes() {
        let t0 = Instant::now();
        let mut scheduler = SessionSaveScheduler::default();

        // First state is unsaved: a write is scheduled one delay later.
        let due = t0 + SESSION_SAVE_DELAY;
        assert_eq!(scheduler.poll(1, t0), SessionSavePoll::WaitUntil(due));
        // A change in the meantime keeps the original deadline.
        let later = t0 + Duration::from_millis(500);
        assert_eq!(scheduler.poll(2, later), SessionSavePoll::WaitUntil(due));
        assert_eq!(scheduler.poll(2, due), SessionSavePoll::SaveNow);
        scheduler.mark_saved(2);
        assert!(!scheduler.is_dirty(2));
        assert_eq!(scheduler.poll(2, due), SessionSavePoll::Idle);

        // A change back to the saved state cancels the pending write.
        let t1 = due + Duration::from_secs(10);
        assert!(scheduler.is_dirty(3));
        assert!(matches!(
            scheduler.poll(3, t1),
            SessionSavePoll::WaitUntil(_)
        ));
        assert_eq!(scheduler.poll(2, t1), SessionSavePoll::Idle);

        // A failed write is retried after the retry delay.
        let t2 = t1 + Duration::from_secs(10);
        scheduler.poll(4, t2);
        assert_eq!(
            scheduler.poll(4, t2 + SESSION_SAVE_DELAY),
            SessionSavePoll::SaveNow
        );
        let failed_at = t2 + SESSION_SAVE_DELAY;
        scheduler.mark_failed(failed_at);
        assert_eq!(
            scheduler.poll(4, failed_at),
            SessionSavePoll::WaitUntil(failed_at + SESSION_RETRY_DELAY)
        );
        assert_eq!(
            scheduler.poll(4, failed_at + SESSION_RETRY_DELAY),
            SessionSavePoll::SaveNow
        );
    }

    #[test]
    fn merge_launch_paths_dedups_and_activates_last() {
        let existing = vec![p(r"C:\Windows"), p(MY_PC_PATH), p(r"D:\")];

        // No launch paths: nothing to do.
        assert_eq!(merge_launch_paths(&existing, &[]), LaunchMerge::default());

        // Already open (case/separator-insensitive): activate it.
        assert_eq!(
            merge_launch_paths(&existing, &[p(r"c:\windows\")]),
            LaunchMerge {
                append: vec![],
                activate: Some(0),
            }
        );
        assert_eq!(
            merge_launch_paths(&existing, &[p(r"\\?\C:\Windows")]).activate,
            Some(0)
        );
        assert_eq!(
            merge_launch_paths(&existing, &[p("D:/")]),
            LaunchMerge {
                append: vec![],
                activate: Some(2),
            }
        );

        // New paths are appended once; the last launch path is activated.
        assert_eq!(
            merge_launch_paths(
                &existing,
                &[p(r"E:\Work"), p(r"C:\Windows"), p(r"e:\work\"), p(r"F:\")]
            ),
            LaunchMerge {
                append: vec![p(r"E:\Work"), p(r"F:\")],
                activate: Some(4),
            }
        );
        assert_eq!(
            merge_launch_paths(&existing, &[p(r"E:\Work"), p(MY_PC_PATH)]),
            LaunchMerge {
                append: vec![p(r"E:\Work")],
                activate: Some(1),
            }
        );
    }
}
