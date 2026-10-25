//! Local cache of discovered env files.
//!
//! Stores where files are and which key *names* they define, never values, so
//! `ls` and `find` don't have to walk the disk. The whole file is disposable:
//! if it is unreadable or from another version it is simply rebuilt.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use dotvault_core::secrets::Kind;

use crate::scan::{self, Refresh};

const SCHEMA_VERSION: i32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
    pub path: PathBuf,
    pub project: String,
    pub project_root: PathBuf,
    /// `default`, `production`, `local`, ...
    pub label: String,
    pub is_example: bool,
    pub size: u64,
    /// Unix milliseconds.
    pub mtime: i64,
    /// Variable names of an env file, or top-level key names of a JSON secret.
    pub keys: Vec<String>,
    /// `Env` for env files; for anything else the label is the kind's name.
    pub kind: Kind,
    /// A short note about a secret file, such as `ed25519, passphrase`.
    pub detail: String,
}

pub struct Index {
    conn: Connection,
}

impl Index {
    pub fn open(path: &Path) -> Result<Index> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        match Self::try_open(path) {
            Ok(index) => Ok(index),
            Err(_) => {
                // It is only a cache: start over rather than fail.
                let _ = fs::remove_file(path);
                Self::try_open(path)
                    .with_context(|| format!("cannot open index {}", path.display()))
            }
        }
    }

    fn try_open(path: &Path) -> Result<Index> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.pragma_update(None, "synchronous", "OFF")?;
        let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            conn.execute_batch("DROP TABLE IF EXISTS files; DROP TABLE IF EXISTS meta;")?;
            conn.execute_batch(
                "CREATE TABLE files (
                    path TEXT PRIMARY KEY,
                    project TEXT NOT NULL,
                    project_root TEXT NOT NULL,
                    label TEXT NOT NULL,
                    is_example INTEGER NOT NULL,
                    size INTEGER NOT NULL,
                    mtime INTEGER NOT NULL,
                    keys TEXT NOT NULL,
                    kind TEXT NOT NULL DEFAULT 'env',
                    detail TEXT NOT NULL DEFAULT ''
                );
                CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(Index { conn })
    }

    fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn last_scan(&self) -> Result<Option<i64>> {
        Ok(self.meta("last_scan")?.and_then(|v| v.parse().ok()))
    }

    /// How long the last scan took, so a slow one is not repeated too often.
    pub fn last_scan_millis(&self) -> Result<Option<u64>> {
        Ok(self.meta("last_scan_ms")?.and_then(|v| v.parse().ok()))
    }

    pub fn signature(&self) -> Result<Option<String>> {
        self.meta("signature")
    }

    /// Swaps the whole index for the result of a fresh scan that took `scan_millis`.
    pub fn replace_all(
        &mut self,
        records: &[FileRecord],
        signature: &str,
        now: i64,
        scan_millis: u64,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM files", [])?;
        {
            let mut insert = tx.prepare(INSERT_SQL)?;
            for r in records {
                insert_record(&mut insert, r)?;
            }
        }
        for (k, v) in [
            ("last_scan", now.to_string()),
            ("last_scan_ms", scan_millis.to_string()),
            ("signature", signature.to_string()),
        ] {
            tx.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
                [k, &v],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn all(&self) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, project, project_root, label, is_example, size, mtime, keys, kind, detail
             FROM files ORDER BY project COLLATE NOCASE, project_root, label",
        )?;
        let rows = stmt.query_map([], |r| {
            let keys: String = r.get(7)?;
            Ok(FileRecord {
                path: PathBuf::from(r.get::<_, String>(0)?),
                project: r.get(1)?,
                project_root: PathBuf::from(r.get::<_, String>(2)?),
                label: r.get(3)?,
                is_example: r.get::<_, i64>(4)? != 0,
                size: r.get::<_, i64>(5)? as u64,
                mtime: r.get(6)?,
                keys: keys
                    .split('\n')
                    .filter(|k| !k.is_empty())
                    .map(String::from)
                    .collect(),
                kind: Kind::parse(&r.get::<_, String>(8)?).unwrap_or(Kind::Env),
                detail: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn upsert(&self, r: &FileRecord) -> Result<()> {
        let mut insert = self.conn.prepare_cached(INSERT_SQL)?;
        insert_record(&mut insert, r)?;
        Ok(())
    }

    pub fn remove(&self, path: &Path) -> Result<()> {
        self.conn.execute(
            "DELETE FROM files WHERE path = ?1",
            [path.to_string_lossy()],
        )?;
        Ok(())
    }

    /// Brings `records` up to date with the disk: files that vanished are
    /// dropped and files that changed are re-read. Discovering *new* files
    /// needs a scan.
    pub fn refresh_stale(&self, records: Vec<FileRecord>) -> Result<Vec<FileRecord>> {
        let mut out = Vec::with_capacity(records.len());
        for rec in records {
            match scan::refresh_record(&rec) {
                Refresh::Unchanged => out.push(rec),
                Refresh::Changed(new) => {
                    self.upsert(&new)?;
                    out.push(new);
                }
                Refresh::Gone => self.remove(&rec.path)?,
            }
        }
        Ok(out)
    }

    /// Re-reads one file if it is already indexed (after `set`, `edit`, ...).
    pub fn touch(&self, path: &Path) -> Result<()> {
        let canonical = crate::fsutil::canonical(path).unwrap_or_else(|_| path.to_path_buf());
        let existing = self
            .all()?
            .into_iter()
            .find(|r| r.path == canonical || r.path == path);
        if let Some(rec) = existing {
            match scan::refresh_record(&rec) {
                Refresh::Unchanged => {}
                Refresh::Changed(new) => self.upsert(&new)?,
                Refresh::Gone => self.remove(&rec.path)?,
            }
        }
        Ok(())
    }
}

const INSERT_SQL: &str = "INSERT OR REPLACE INTO files
     (path, project, project_root, label, is_example, size, mtime, keys, kind, detail)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)";

fn insert_record(stmt: &mut rusqlite::Statement<'_>, r: &FileRecord) -> rusqlite::Result<usize> {
    stmt.execute(params![
        r.path.to_string_lossy(),
        r.project,
        r.project_root.to_string_lossy(),
        r.label,
        r.is_example as i64,
        r.size as i64,
        r.mtime,
        r.keys.join("\n"),
        r.kind.as_str(),
        r.detail,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(path: &str, project: &str, label: &str, keys: &[&str]) -> FileRecord {
        FileRecord {
            path: PathBuf::from(path),
            project: project.into(),
            project_root: PathBuf::from("/p").join(project),
            label: label.into(),
            is_example: label == "example",
            size: 10,
            mtime: 5,
            keys: keys.iter().map(|s| s.to_string()).collect(),
            kind: Kind::Env,
            detail: String::new(),
        }
    }

    #[test]
    fn stores_and_returns_records() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = Index::open(&dir.path().join("i.db")).unwrap();
        assert_eq!(idx.last_scan().unwrap(), None);
        let recs = vec![
            rec("/p/b/.env", "b", "default", &["X", "Y"]),
            rec("/p/a/.env.example", "A", "example", &[]),
        ];
        idx.replace_all(&recs, "sig", 123, 45).unwrap();
        assert_eq!(idx.last_scan_millis().unwrap(), Some(45));
        assert_eq!(idx.last_scan().unwrap(), Some(123));
        assert_eq!(idx.signature().unwrap().as_deref(), Some("sig"));
        let back = idx.all().unwrap();
        // ordered by project, case-insensitively
        assert_eq!(back[0].project, "A");
        assert_eq!(back[1].keys, ["X", "Y"]);
        assert!(back[0].is_example);
    }

    #[test]
    fn secret_kinds_and_details_round_trip_and_sort_with_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = Index::open(&dir.path().join("i.db")).unwrap();
        let mut key = rec("/h/.ssh/id_ed25519", ".ssh", "ssh-key", &[]);
        key.kind = Kind::SshKey;
        key.detail = "ed25519, passphrase".to_string();
        let mut sa = rec(
            "/p/a/sa.json",
            "a",
            "service-account",
            &["client_email", "private_key"],
        );
        sa.kind = Kind::ServiceAccount;
        idx.replace_all(
            &[
                key.clone(),
                sa.clone(),
                rec("/p/a/.env", "a", "default", &["X"]),
            ],
            "s",
            1,
            0,
        )
        .unwrap();
        let back = idx.all().unwrap();
        assert_eq!(back.len(), 3);
        let find = |kind: Kind| back.iter().find(|r| r.kind == kind).unwrap();
        assert_eq!(find(Kind::SshKey).detail, "ed25519, passphrase");
        assert_eq!(
            find(Kind::ServiceAccount).keys,
            ["client_email", "private_key"]
        );
        assert_eq!(find(Kind::Env).detail, "");
        // one project's env file and secret file sort together
        let projects: Vec<&str> = back.iter().map(|r| r.project.as_str()).collect();
        assert_eq!(projects, [".ssh", "a", "a"]);
    }

    #[test]
    fn an_index_from_before_secret_files_is_rebuilt_not_broken() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("i.db");
        {
            // the layout version 1 wrote: no kind or detail column
            let old = rusqlite::Connection::open(&path).unwrap();
            old.execute_batch(
                "CREATE TABLE files (path TEXT PRIMARY KEY, project TEXT NOT NULL,
                    project_root TEXT NOT NULL, label TEXT NOT NULL, is_example INTEGER NOT NULL,
                    size INTEGER NOT NULL, mtime INTEGER NOT NULL, keys TEXT NOT NULL);
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO files VALUES ('/p/.env', 'p', '/p', 'default', 0, 1, 1, 'A');
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }
        let mut idx = Index::open(&path).unwrap();
        assert!(
            idx.all().unwrap().is_empty(),
            "the old rows are dropped; a scan refills them"
        );
        assert_eq!(
            idx.signature().unwrap(),
            None,
            "so the next command rescans"
        );
        let mut key = rec("/k/id_rsa", "k", "ssh-key", &[]);
        key.kind = Kind::SshKey;
        idx.replace_all(&[key], "s", 1, 0).unwrap();
        assert_eq!(idx.all().unwrap()[0].kind, Kind::SshKey);
    }

    #[test]
    fn replace_all_discards_old_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = Index::open(&dir.path().join("i.db")).unwrap();
        idx.replace_all(&[rec("/p/a/.env", "a", "default", &[])], "s", 1, 0)
            .unwrap();
        idx.replace_all(&[rec("/p/b/.env", "b", "default", &[])], "s", 2, 0)
            .unwrap();
        let all = idx.all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].project, "b");
    }

    #[test]
    fn recovers_from_a_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("i.db");
        fs::write(&path, b"this is not a sqlite database at all, just text").unwrap();
        let idx = Index::open(&path).unwrap();
        assert!(idx.all().unwrap().is_empty());
    }
}
