//! Git-like history per variable: every change dotvault makes to a variable in an
//! env file becomes a numbered version (v1, v2, ...) that can be listed and
//! restored.
//!
//! The history file keeps old and current values in plain text, so it is the one
//! place dotvault stores values on disk besides the env files themselves. It is
//! owner-only, separate from the disposable index, and never deleted on its own.
//!
//! Tracking a file begins the first time dotvault changes it. From then on every
//! change is diffed per variable. When the file turns out to differ from what was
//! last recorded, someone changed it outside dotvault, and that is recorded too
//! (`external`), so versions are not skipped. Variables that were already in the
//! file when tracking began get a `baseline` version first.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use dotvault_core::dotenv::DotEnv;
use rusqlite::{params, Connection, TransactionBehavior};

const SCHEMA_VERSION: i32 = 1;

/// What produced a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Set,
    Unset,
    Edit,
    Pull,
    Revert,
    /// The variable was already in the file when tracking began.
    Baseline,
    /// The file changed outside dotvault.
    External,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Set => "set",
            Action::Unset => "unset",
            Action::Edit => "edit",
            Action::Pull => "pull",
            Action::Revert => "revert",
            Action::Baseline => "baseline",
            Action::External => "external",
        }
    }
}

/// A variable's new value, or `None` when it was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub key: String,
    pub value: Option<String>,
}

fn planned(key: &str, value: Option<&str>) -> Planned {
    Planned {
        key: key.to_string(),
        value: value.map(str::to_string),
    }
}

/// What a write adds to the history, in the order it is recorded.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub baseline: Vec<Planned>,
    pub external: Vec<Planned>,
    pub changes: Vec<Planned>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.baseline.is_empty() && self.external.is_empty() && self.changes.is_empty()
    }
}

pub type Pairs = Vec<(String, String)>;

/// Works out what to record. `before` is the file as it was just before the
/// write (`None` if it did not exist), `after` as it is now, and `last` holds the
/// last recorded value of every variable that has history (`None` = removed).
///
/// Nothing is planned when no variable changed, so opening an editor and quitting
/// does not start tracking a file.
pub fn plan(
    before: Option<&[(String, String)]>,
    after: &[(String, String)],
    last: &HashMap<String, Option<String>>,
) -> Plan {
    let before_pairs = before.unwrap_or(&[]);
    let before_map: HashMap<&str, &str> = before_pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let after_map: HashMap<&str, &str> = after
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let mut changes = Vec::new();
    for (key, value) in after {
        if before_map.get(key.as_str()) != Some(&value.as_str()) {
            changes.push(planned(key, Some(value)));
        }
    }
    for (key, _) in before_pairs {
        if !after_map.contains_key(key.as_str()) {
            changes.push(planned(key, None));
        }
    }
    if changes.is_empty() {
        return Plan::default();
    }

    let mut plan = Plan {
        changes,
        ..Plan::default()
    };
    if before.is_some() {
        for (key, value) in before_pairs {
            match last.get(key) {
                None => plan.baseline.push(planned(key, Some(value))),
                Some(recorded) if recorded.as_deref() != Some(value.as_str()) => {
                    plan.external.push(planned(key, Some(value)));
                }
                Some(_) => {}
            }
        }
        // Recorded as present, but the file no longer has it.
        let mut gone: Vec<&String> = last
            .iter()
            .filter(|(key, value)| value.is_some() && !before_map.contains_key(key.as_str()))
            .map(|(key, _)| key)
            .collect();
        gone.sort();
        for key in gone {
            plan.external.push(planned(key, None));
        }
    }
    plan
}

/// Whether a write changed any variable. Decided before the history is opened, so a
/// command that changes nothing never creates the history file.
pub fn has_changes(before: Option<&Before>, after_text: &str) -> bool {
    let before_pairs = before.map(|b| pairs_of(&b.text));
    !plan(
        before_pairs.as_deref(),
        &pairs_of(after_text),
        &HashMap::new(),
    )
    .is_empty()
}

/// The file as it was just before dotvault wrote to it.
pub struct Before {
    pub text: String,
    /// Unix seconds. Used as the time of changes made outside dotvault.
    pub mtime: Option<i64>,
}

impl Before {
    /// `None` when the file does not exist yet.
    pub fn read(path: &Path) -> Option<Before> {
        let bytes = fs::read(path).ok()?;
        let mtime = fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        Some(Before {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            mtime,
        })
    }
}

fn pairs_of(text: &str) -> Pairs {
    DotEnv::parse(text)
        .pairs()
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// How a file is named in the history: its canonical path, so `-f ./.env`,
/// `-p project` and a symlink to it all mean the same file.
pub fn key_for(path: &Path) -> String {
    if let Ok(canonical) = crate::fsutil::canonical(path) {
        return canonical.to_string_lossy().into_owned();
    }
    // The file is gone (moved, deleted). Its folder usually is not: resolve that and
    // keep the name, so the path matches what was recorded while the file existed.
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if let (Some(folder), Some(name)) = (absolute.parent(), absolute.file_name()) {
        if let Ok(folder) = crate::fsutil::canonical(folder) {
            return folder.join(name).to_string_lossy().into_owned();
        }
    }
    absolute.to_string_lossy().into_owned()
}

/// One version of one variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Variables changed by one command share a change number.
    pub change: i64,
    /// Unix seconds.
    pub at: i64,
    pub action: String,
    pub key: String,
    pub version: i64,
    /// `None` when this version removed the variable.
    pub value: Option<String>,
}

pub struct History {
    conn: Connection,
}

impl History {
    /// Opens the history, creating it (owner-only) if it does not exist.
    pub fn open(path: &Path) -> Result<History> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        create_private(path)
            .with_context(|| format!("cannot create the history file {}", path.display()))?;
        Self::open_existing_file(path)
    }

    /// Opens the history only if it exists, so that merely looking never creates it.
    pub fn open_if_exists(path: &Path) -> Result<Option<History>> {
        if !path.exists() {
            return Ok(None);
        }
        Self::open_existing_file(path).map(Some)
    }

    fn open_existing_file(path: &Path) -> Result<History> {
        tighten(path);
        let conn = Connection::open(path)
            .with_context(|| format!("cannot open the history file {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(2))?;
        // Deleted values are overwritten, not just unlinked, so `forget` means it.
        conn.pragma_update(None, "secure_delete", true)?;
        // Not a cache: if this cannot be read it is left exactly as it is.
        let unusable = || {
            format!(
                "{} is not a usable history file; it was left alone",
                path.display()
            )
        };
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .with_context(unusable)?;
        if version > SCHEMA_VERSION {
            bail!(
                "{} was written by a newer dotvault; update dotvault to read it",
                path.display()
            );
        }
        if version == 0 {
            // Two commands starting at once must not both create the tables: take the
            // write lock first, then look again.
            conn.execute_batch("BEGIN IMMEDIATE")
                .with_context(unusable)?;
            let created = (|| -> rusqlite::Result<()> {
                let again: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
                if again == 0 {
                    conn.execute_batch(
                        "CREATE TABLE IF NOT EXISTS changes (
                            id     INTEGER PRIMARY KEY AUTOINCREMENT,
                            at     INTEGER NOT NULL,
                            action TEXT NOT NULL
                        );
                        CREATE TABLE IF NOT EXISTS versions (
                            change_id INTEGER NOT NULL REFERENCES changes(id) ON DELETE CASCADE,
                            path      TEXT NOT NULL,
                            key       TEXT NOT NULL,
                            version   INTEGER NOT NULL,
                            value     TEXT,
                            UNIQUE (path, key, version)
                        );",
                    )?;
                    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                }
                Ok(())
            })();
            match created {
                Ok(()) => conn.execute_batch("COMMIT").with_context(unusable)?,
                Err(e) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(e).with_context(unusable);
                }
            }
        }
        Ok(History { conn })
    }

    /// Records what a write changed in `file` (a [`key_for`] name). Returns how many
    /// versions were added. `keep` is how many versions of a variable to retain.
    pub fn record(
        &mut self,
        file: &str,
        before: Option<&Before>,
        after_text: &str,
        action: Action,
        keep: usize,
        now: i64,
    ) -> Result<usize> {
        let after = pairs_of(after_text);
        let before_pairs = before.map(|b| pairs_of(&b.text));
        // A writer from the start: a transaction that reads first and writes later
        // fails at once when another command holds the lock instead of waiting.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let last = latest_values(&tx, file)?;
        let plan = plan(before_pairs.as_deref(), &after, &last);
        if plan.is_empty() {
            return Ok(0);
        }

        let outside = before.and_then(|b| b.mtime).unwrap_or(now);
        let mut added = 0;
        for (kind, entries, at) in [
            (Action::Baseline, &plan.baseline, outside),
            (Action::External, &plan.external, outside),
            (action, &plan.changes, now),
        ] {
            if entries.is_empty() {
                continue;
            }
            tx.execute(
                "INSERT INTO changes (at, action) VALUES (?1, ?2)",
                params![at, kind.as_str()],
            )?;
            let change = tx.last_insert_rowid();
            for entry in entries {
                let next: i64 = tx.query_row(
                    "SELECT COALESCE(MAX(version), 0) + 1 FROM versions WHERE path = ?1 AND key = ?2",
                    params![file, entry.key],
                    |r| r.get(0),
                )?;
                tx.execute(
                    "INSERT INTO versions (change_id, path, key, version, value)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![change, file, entry.key, next, entry.value],
                )?;
                added += 1;
            }
        }

        let keep = keep.max(1) as i64;
        for entry in plan
            .baseline
            .iter()
            .chain(&plan.external)
            .chain(&plan.changes)
        {
            tx.execute(
                "DELETE FROM versions WHERE path = ?1 AND key = ?2 AND version <=
                 (SELECT MAX(version) FROM versions WHERE path = ?1 AND key = ?2) - ?3",
                params![file, entry.key, keep],
            )?;
        }
        tx.execute(
            "DELETE FROM changes WHERE id NOT IN (SELECT DISTINCT change_id FROM versions)",
            [],
        )?;
        tx.commit()?;
        Ok(added)
    }

    /// Every version of the file's variables (or of one variable), oldest first.
    pub fn rows(&self, file: &str, key: Option<&str>) -> Result<Vec<Row>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.at, c.action, v.key, v.version, v.value
             FROM versions v JOIN changes c ON c.id = v.change_id
             WHERE v.path = ?1 AND (?2 IS NULL OR v.key = ?2)
             ORDER BY c.id, v.rowid",
        )?;
        let rows = stmt
            .query_map(params![file, key], |r| {
                Ok(Row {
                    change: r.get(0)?,
                    at: r.get(1)?,
                    action: r.get(2)?,
                    key: r.get(3)?,
                    version: r.get(4)?,
                    value: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Deletes the history of one variable, or of the whole file. The values are
    /// overwritten and the file is compacted. Returns how many versions went.
    pub fn forget(&mut self, file: &str, key: Option<&str>) -> Result<usize> {
        let removed = self.conn.execute(
            "DELETE FROM versions WHERE path = ?1 AND (?2 IS NULL OR key = ?2)",
            params![file, key],
        )?;
        self.conn.execute(
            "DELETE FROM changes WHERE id NOT IN (SELECT DISTINCT change_id FROM versions)",
            [],
        )?;
        self.conn
            .execute_batch("VACUUM")
            .map_err(|e| anyhow!("history was cleared but could not be compacted: {e}"))?;
        Ok(removed)
    }
}

fn latest_values(conn: &Connection, file: &str) -> Result<HashMap<String, Option<String>>> {
    let mut stmt = conn.prepare(
        "SELECT v.key, v.value FROM versions v
         JOIN (SELECT key, MAX(version) AS top FROM versions WHERE path = ?1 GROUP BY key) x
           ON x.key = v.key AND x.top = v.version
         WHERE v.path = ?1",
    )?;
    let pairs = stmt
        .query_map(params![file], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<HashMap<String, Option<String>>, _>>()?;
    Ok(pairs)
}

/// Creates the file readable by its owner only, before SQLite touches it.
#[cfg(unix)]
fn create_private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .map(|_| ())
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> std::io::Result<()> {
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map(|_| ())
}

/// A history file that became readable by others (a copy, a restore) is made
/// private again.
#[cfg(unix)]
fn tighten(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn tighten(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(items: &[(&str, &str)]) -> Pairs {
        items
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn last(items: &[(&str, Option<&str>)]) -> HashMap<String, Option<String>> {
        items
            .iter()
            .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
            .collect()
    }

    fn keys(entries: &[Planned]) -> Vec<&str> {
        entries.iter().map(|p| p.key.as_str()).collect()
    }

    // --- plan ----------------------------------------------------------------

    #[test]
    fn a_new_file_records_its_variables_without_a_baseline() {
        let p = plan(None, &pairs(&[("A", "1"), ("B", "2")]), &HashMap::new());
        assert_eq!(keys(&p.changes), ["A", "B"]);
        assert!(p.baseline.is_empty() && p.external.is_empty());
    }

    #[test]
    fn the_first_change_to_an_existing_file_baselines_what_was_already_there() {
        let before = pairs(&[("A", "1"), ("B", "2")]);
        let after = pairs(&[("A", "9"), ("B", "2")]);
        let p = plan(Some(&before), &after, &HashMap::new());
        assert_eq!(keys(&p.baseline), ["A", "B"], "both were already there");
        assert_eq!(p.baseline[0].value.as_deref(), Some("1"));
        assert_eq!(keys(&p.changes), ["A"]);
        assert_eq!(p.changes[0].value.as_deref(), Some("9"));
    }

    #[test]
    fn has_changes_agrees_with_the_plan_whatever_the_history_holds() {
        let same = Before {
            text: "A=1\n".to_string(),
            mtime: None,
        };
        assert!(!has_changes(Some(&same), "A=1\n"));
        assert!(!has_changes(Some(&same), "# only a comment changed\nA=1\n"));
        assert!(has_changes(Some(&same), "A=2\n"));
        assert!(has_changes(Some(&same), ""), "a removal is a change");
        assert!(has_changes(None, "A=1\n"));
        assert!(!has_changes(None, ""));
    }

    #[test]
    fn nothing_changed_means_nothing_is_recorded_and_tracking_does_not_start() {
        let same = pairs(&[("A", "1")]);
        assert_eq!(plan(Some(&same), &same, &HashMap::new()), Plan::default());
        assert!(plan(None, &[], &HashMap::new()).is_empty());
        // even when history exists and has drifted: that waits for a real change
        let drifted = last(&[("A", Some("old"))]);
        assert!(plan(Some(&same), &same, &drifted).is_empty());
    }

    #[test]
    fn a_removed_variable_is_a_version_with_no_value() {
        let before = pairs(&[("A", "1"), ("B", "2")]);
        let after = pairs(&[("A", "1")]);
        let known = last(&[("A", Some("1")), ("B", Some("2"))]);
        let p = plan(Some(&before), &after, &known);
        assert_eq!(p.changes, vec![planned("B", None)]);
        assert!(p.baseline.is_empty() && p.external.is_empty());
    }

    #[test]
    fn a_value_that_differs_from_the_record_was_changed_outside_dotvault() {
        let before = pairs(&[("A", "edited-by-hand"), ("B", "2")]);
        let after = pairs(&[("A", "edited-by-hand"), ("B", "3")]);
        let known = last(&[("A", Some("1")), ("B", Some("2"))]);
        let p = plan(Some(&before), &after, &known);
        assert_eq!(p.external, vec![planned("A", Some("edited-by-hand"))]);
        assert_eq!(p.changes, vec![planned("B", Some("3"))]);
        assert!(p.baseline.is_empty());
    }

    #[test]
    fn a_variable_deleted_by_hand_is_noticed_and_one_added_back_is_too() {
        let before = pairs(&[("KEEP", "1"), ("BACK", "restored")]);
        let after = pairs(&[("KEEP", "2"), ("BACK", "restored")]);
        let known = last(&[
            ("KEEP", Some("1")),
            ("GONE", Some("x")),
            ("BACK", None),
            ("ALREADY_REMOVED", None),
        ]);
        let p = plan(Some(&before), &after, &known);
        assert_eq!(
            p.external,
            vec![planned("BACK", Some("restored")), planned("GONE", None)],
            "BACK reappeared and GONE vanished; ALREADY_REMOVED needs nothing"
        );
        assert_eq!(keys(&p.changes), ["KEEP"]);
    }

    #[test]
    fn several_variables_in_one_command_stay_together() {
        let before = pairs(&[("A", "1")]);
        let after = pairs(&[("A", "2"), ("B", "new"), ("C", "new")]);
        let known = last(&[("A", Some("1"))]);
        let p = plan(Some(&before), &after, &known);
        assert_eq!(keys(&p.changes), ["A", "B", "C"]);
    }

    // --- store ---------------------------------------------------------------

    fn store() -> (tempfile::TempDir, History) {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(&dir.path().join("history.db")).unwrap();
        (dir, history)
    }

    fn before(text: &str, mtime: i64) -> Before {
        Before {
            text: text.to_string(),
            mtime: Some(mtime),
        }
    }

    #[test]
    fn versions_count_up_per_variable_and_keep_their_values() {
        let (_dir, mut h) = store();
        h.record("/f", None, "A=1\n", Action::Set, 50, 100).unwrap();
        h.record(
            "/f",
            Some(&before("A=1\n", 100)),
            "A=2\nB=x\n",
            Action::Set,
            50,
            200,
        )
        .unwrap();
        h.record(
            "/f",
            Some(&before("A=2\nB=x\n", 200)),
            "A=3\nB=x\n",
            Action::Set,
            50,
            300,
        )
        .unwrap();
        let rows = h.rows("/f", Some("A")).unwrap();
        let seen: Vec<(i64, Option<&str>, i64)> = rows
            .iter()
            .map(|r| (r.version, r.value.as_deref(), r.at))
            .collect();
        assert_eq!(
            seen,
            [
                (1, Some("1"), 100),
                (2, Some("2"), 200),
                (3, Some("3"), 300)
            ]
        );
        assert_eq!(h.rows("/f", Some("B")).unwrap().len(), 1);
        assert!(
            h.rows("/other", None).unwrap().is_empty(),
            "files are separate"
        );
    }

    #[test]
    fn variables_changed_by_one_command_share_a_change_number() {
        let (_dir, mut h) = store();
        h.record("/f", None, "A=1\nB=2\n", Action::Set, 50, 100)
            .unwrap();
        let rows = h.rows("/f", None).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].change, rows[1].change);
        h.record(
            "/f",
            Some(&before("A=1\nB=2\n", 100)),
            "A=9\nB=2\n",
            Action::Set,
            50,
            200,
        )
        .unwrap();
        let rows = h.rows("/f", None).unwrap();
        assert_ne!(rows[0].change, rows.last().unwrap().change);
    }

    #[test]
    fn the_first_change_to_an_existing_file_adds_a_baseline_timed_at_the_files_mtime() {
        let (_dir, mut h) = store();
        let added = h
            .record(
                "/f",
                Some(&before("A=1\n", 50)),
                "A=2\n",
                Action::Set,
                50,
                900,
            )
            .unwrap();
        assert_eq!(added, 2);
        let rows = h.rows("/f", Some("A")).unwrap();
        assert_eq!((rows[0].action.as_str(), rows[0].at), ("baseline", 50));
        assert_eq!((rows[1].action.as_str(), rows[1].at), ("set", 900));
        assert_eq!(rows[0].value.as_deref(), Some("1"));
    }

    #[test]
    fn a_change_made_outside_dotvault_is_recorded_before_the_next_real_one() {
        let (_dir, mut h) = store();
        h.record("/f", None, "A=1\n", Action::Set, 50, 100).unwrap();
        // someone edits the file by hand at t=400, then dotvault sets B at t=500
        h.record(
            "/f",
            Some(&before("A=hand\n", 400)),
            "A=hand\nB=2\n",
            Action::Set,
            50,
            500,
        )
        .unwrap();
        let a = h.rows("/f", Some("A")).unwrap();
        assert_eq!(
            a.iter()
                .map(|r| (r.action.as_str(), r.at))
                .collect::<Vec<_>>(),
            [("set", 100), ("external", 400)]
        );
        assert_eq!(a[1].value.as_deref(), Some("hand"));
    }

    #[test]
    fn no_change_records_nothing_and_a_removal_stores_no_value() {
        let (_dir, mut h) = store();
        assert_eq!(
            h.record("/f", Some(&before("A=1\n", 1)), "A=1\n", Action::Set, 50, 2)
                .unwrap(),
            0
        );
        assert!(h.rows("/f", None).unwrap().is_empty());
        h.record("/f", None, "A=1\n", Action::Set, 50, 3).unwrap();
        h.record("/f", Some(&before("A=1\n", 3)), "", Action::Unset, 50, 4)
            .unwrap();
        let rows = h.rows("/f", Some("A")).unwrap();
        assert_eq!(rows.last().unwrap().value, None);
        assert_eq!(rows.last().unwrap().action, "unset");
    }

    #[test]
    fn only_the_newest_versions_are_kept_and_numbers_keep_counting() {
        let (_dir, mut h) = store();
        let mut text = String::new();
        for i in 1..=6 {
            let next = format!("A={i}\n");
            let b = (i > 1).then(|| before(&text, i));
            h.record("/f", b.as_ref(), &next, Action::Set, 3, 100 + i)
                .unwrap();
            text = next;
        }
        let rows = h.rows("/f", Some("A")).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.version).collect::<Vec<_>>(),
            [4, 5, 6],
            "the newest three"
        );
        assert_eq!(rows[0].value.as_deref(), Some("4"));
    }

    #[test]
    fn forget_removes_one_variable_or_the_whole_file_and_overwrites_the_values() {
        let (dir, mut h) = store();
        h.record(
            "/f",
            None,
            "KEEP=1\nSECRET=hunter2-needle\n",
            Action::Set,
            50,
            1,
        )
        .unwrap();
        h.record("/g", None, "SECRET=hunter2-needle\n", Action::Set, 50, 1)
            .unwrap();
        assert_eq!(h.forget("/f", Some("SECRET")).unwrap(), 1);
        assert_eq!(h.rows("/f", None).unwrap().len(), 1);
        assert_eq!(
            h.rows("/g", None).unwrap().len(),
            1,
            "another file is untouched"
        );
        assert_eq!(h.forget("/f", None).unwrap(), 1);
        assert!(h.rows("/f", None).unwrap().is_empty());
        assert_eq!(h.forget("/g", Some("SECRET")).unwrap(), 1);
        drop(h);
        let raw = fs::read(dir.path().join("history.db")).unwrap();
        assert!(
            !raw.windows(12).any(|w| w == b"hunter2-need"),
            "a forgotten value must not remain in the file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only_and_stays_that_way() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let mut h = History::open(&path).unwrap();
        h.record("/f", None, "A=1\n", Action::Set, 50, 1).unwrap();
        drop(h);
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        // loosened by a copy or a restore: made private again on the next open
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        drop(History::open(&path).unwrap());
        assert_eq!(mode(&path), 0o600);
        // the rollback journal leaves no other file behind
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["history.db"], "no -wal, -shm or -journal files");
    }

    #[test]
    fn a_file_that_is_not_a_history_is_an_error_and_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        fs::write(&path, b"this is not a sqlite database at all, just text").unwrap();
        let err = match History::open(&path) {
            Ok(_) => panic!("must not open"),
            Err(e) => format!("{e:#}"),
        };
        assert!(err.contains("left alone"), "{err}");
        assert_eq!(
            fs::read(&path).unwrap(),
            b"this is not a sqlite database at all, just text"
        );
    }

    #[test]
    fn looking_never_creates_the_file_and_a_newer_one_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        assert!(History::open_if_exists(&path).unwrap().is_none());
        assert!(!path.exists());
        drop(History::open(&path).unwrap());
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        let err = match History::open(&path) {
            Ok(_) => panic!("must refuse"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("newer dotvault"), "{err}");
    }

    #[test]
    fn two_writers_take_turns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        // Four commands at once, each changing its own file, all sharing one history.
        let threads: Vec<_> = (0..4)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let file = format!("/f{n}");
                    for i in 0..5 {
                        let mut h = History::open(&path).unwrap();
                        let b = (i > 0).then(|| before(&format!("K={i}\n"), 1));
                        h.record(
                            &file,
                            b.as_ref(),
                            &format!("K={}\n", i + 1),
                            Action::Set,
                            50,
                            1,
                        )
                        .unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let h = History::open(&path).unwrap();
        for n in 0..4 {
            let rows = h.rows(&format!("/f{n}"), Some("K")).unwrap();
            assert_eq!(
                rows.iter().map(|r| r.version).collect::<Vec<_>>(),
                [1, 2, 3, 4, 5],
                "/f{n}"
            );
        }
    }

    #[test]
    fn a_file_is_named_by_its_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.env");
        fs::write(&real, "A=1\n").unwrap();
        let roundabout = dir.path().join(".").join("real.env");
        assert_eq!(key_for(&real), key_for(&roundabout));
        assert!(key_for(&dir.path().join("missing.env")).ends_with("missing.env"));
    }

    #[test]
    fn a_file_keeps_its_name_after_it_is_deleted_or_moved() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("app.env");
        fs::write(&file, "A=1\n").unwrap();
        let while_it_existed = key_for(&file);
        fs::remove_file(&file).unwrap();
        assert_eq!(key_for(&file), while_it_existed);
    }

    #[cfg(unix)]
    #[test]
    fn a_deleted_file_reached_through_a_symlinked_folder_keeps_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let via_link = link.join("app.env");
        fs::write(&via_link, "A=1\n").unwrap();
        let while_it_existed = key_for(&via_link);
        fs::remove_file(&via_link).unwrap();
        assert_eq!(
            key_for(&via_link),
            while_it_existed,
            "recorded under the real folder"
        );
        assert!(while_it_existed.contains("real"), "{while_it_existed}");
    }
}
