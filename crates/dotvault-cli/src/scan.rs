//! Finds env files on disk.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, UNIX_EPOCH};

use dotvault_core::classify::{
    classify, find_project_root, project_name, ClassifyOptions, EnvFileInfo, PROJECT_MARKERS,
};
use dotvault_core::dotenv::DotEnv;
use dotvault_core::secrets::{self, Detected, Kind};
use ignore::{WalkBuilder, WalkState};

use crate::config::{Config, DEFAULT_IGNORE_DIRS};
use crate::discover::Profile;
use crate::index::FileRecord;

/// An env file larger than this is indexed but its keys are not read.
const MAX_PARSE_BYTES: u64 = 1024 * 1024;

/// Files the user marked or unmarked. Looked up by file name first, so the many
/// files that match neither cost one hash lookup.
#[derive(Debug, Default, Clone)]
pub struct Marks {
    marked: HashSet<PathBuf>,
    unmarked: HashSet<PathBuf>,
    names: HashSet<OsString>,
}

#[derive(Debug, PartialEq, Eq)]
enum Mark {
    Marked,
    Unmarked,
    Neither,
}

impl Marks {
    pub fn from_config(config: &Config) -> Marks {
        let names = config
            .marked
            .iter()
            .chain(&config.unmarked)
            .filter_map(|p| p.file_name().map(OsString::from))
            .collect();
        Marks {
            marked: config.marked.iter().cloned().collect(),
            unmarked: config.unmarked.iter().cloned().collect(),
            names,
        }
    }

    fn has_name(&self, name: &OsStr) -> bool {
        !self.names.is_empty() && self.names.contains(name)
    }

    fn lookup(&self, path: &Path) -> Mark {
        if self.unmarked.contains(path) {
            Mark::Unmarked
        } else if self.marked.contains(path) {
            Mark::Marked
        } else {
            Mark::Neither
        }
    }
}

pub struct ScanOptions {
    pub roots: Vec<PathBuf>,
    pub max_depth: usize,
    pub ignore: HashSet<OsString>,
    pub classify: ClassifyOptions,
    /// Broad-scan rules: hidden folders in homes, system folders, network mounts.
    pub profile: Profile,
    /// Secret files the user marked or unmarked by hand.
    pub marks: Marks,
}

impl ScanOptions {
    pub fn from_config(config: &Config, depth_override: Option<usize>) -> Self {
        ScanOptions {
            roots: config.roots.clone(),
            max_depth: depth_override.unwrap_or(config.max_depth),
            ignore: DEFAULT_IGNORE_DIRS
                .iter()
                .map(|s| s.to_string())
                .chain(config.ignore.iter().cloned())
                .map(OsString::from)
                .collect(),
            classify: classify_options(config),
            profile: Profile::detect(),
            marks: Marks::from_config(config),
        }
    }
}

pub fn classify_options(config: &Config) -> ClassifyOptions {
    ClassifyOptions {
        example_suffixes: config.example_suffixes.clone(),
    }
}

#[derive(Debug, Default)]
pub struct ScanOutcome {
    pub records: Vec<FileRecord>,
    /// Entries that could not be read (permissions, races).
    pub errors: usize,
    pub missing_roots: Vec<PathBuf>,
    pub elapsed: Duration,
}

pub fn scan(opts: &ScanOptions) -> ScanOutcome {
    let started = Instant::now();
    let mut outcome = ScanOutcome::default();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let ignore = Arc::new(opts.ignore.clone());
    let profile = Arc::new(opts.profile.clone());
    let marks = Arc::new(opts.marks.clone());

    for root in &opts.roots {
        let Ok(root) = crate::fsutil::canonical(root) else {
            outcome.missing_roots.push(root.clone());
            continue;
        };
        let errors = AtomicUsize::new(0);
        let (tx, rx) = mpsc::channel::<Found>();

        let skip = Arc::clone(&ignore);
        let broad = Arc::clone(&profile);
        let walker = WalkBuilder::new(&root)
            .standard_filters(false)
            .follow_links(false)
            .max_depth(Some(opts.max_depth))
            .filter_entry(move |e| {
                // The root itself is always entered: `scan ~/.work` must work.
                if e.depth() == 0 || !e.file_type().is_some_and(|t| t.is_dir()) {
                    return true;
                }
                !(skip.contains(e.file_name()) || broad.skips(e.path()))
            })
            .build_parallel();

        walker.run(|| {
            let tx = tx.clone();
            let errors = &errors;
            let classify_opts = &opts.classify;
            let marks = Arc::clone(&marks);
            Box::new(move |entry| {
                let Ok(entry) = entry else {
                    errors.fetch_add(1, Ordering::Relaxed);
                    return WalkState::Continue;
                };
                let name = entry.file_name().to_string_lossy();
                let found = match classify(&name, classify_opts) {
                    Some(info) => env_found(&entry, info, errors),
                    None => secret_found(&entry, &name, &marks, errors),
                };
                if let Some(found) = found {
                    let _ = tx.send(found);
                }
                WalkState::Continue
            })
        });
        drop(tx);
        outcome.errors += errors.load(Ordering::Relaxed);

        let mut projects = ProjectCache::new(&root, &profile);
        for found in rx {
            if !seen.insert(found.path.clone()) {
                continue;
            }
            let dir = found.path.parent().unwrap_or(&root);
            let (project, project_root) = projects.lookup(dir);
            outcome.records.push(FileRecord {
                keys: found
                    .keys
                    .unwrap_or_else(|| read_keys(&found.path, found.size)),
                path: found.path,
                project,
                project_root,
                label: found.label,
                is_example: found.is_example,
                size: found.size,
                mtime: found.mtime,
                kind: found.kind,
                detail: found.detail,
            });
        }
    }

    outcome.records.sort_by(|a, b| {
        (&a.project, &a.project_root, &a.label).cmp(&(&b.project, &b.project_root, &b.label))
    });
    outcome.elapsed = started.elapsed();
    outcome
}

struct Found {
    path: PathBuf,
    label: String,
    is_example: bool,
    kind: Kind,
    detail: String,
    /// Known already for secret files; read later for env files.
    keys: Option<Vec<String>>,
    size: u64,
    mtime: i64,
}

fn env_found(entry: &ignore::DirEntry, info: EnvFileInfo, errors: &AtomicUsize) -> Option<Found> {
    // Symlinked env files count when they point at a file.
    let md = if entry.path_is_symlink() {
        fs::metadata(entry.path()).ok()
    } else {
        entry.metadata().ok()
    };
    match md {
        Some(md) if md.is_file() => Some(Found {
            path: entry.path().to_path_buf(),
            label: info.label,
            is_example: info.is_example,
            kind: Kind::Env,
            detail: String::new(),
            keys: None,
            size: md.len(),
            mtime: mtime_millis(&md),
        }),
        Some(_) => None,
        None => {
            errors.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

/// A secret file other than an env file. The name decides cheaply whether the file
/// is worth opening at all; only then is it read to be sure.
fn secret_found(
    entry: &ignore::DirEntry,
    name: &str,
    marks: &Marks,
    errors: &AtomicUsize,
) -> Option<Found> {
    let file_type = entry.file_type()?;
    if !(file_type.is_file() || file_type.is_symlink()) {
        return None;
    }
    let path = entry.path();
    if secrets::candidate_with(name, || parent_str(path), 1).is_none()
        && !marks.has_name(entry.file_name())
    {
        return None;
    }
    let md = if entry.path_is_symlink() {
        fs::metadata(path).ok()
    } else {
        entry.metadata().ok()
    };
    let Some(md) = md else {
        errors.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    if !md.is_file() {
        return None;
    }
    let detected = detect_secret(path, md.len(), marks)?;
    Some(Found {
        path: path.to_path_buf(),
        label: detected.kind.as_str().to_string(),
        is_example: false,
        kind: detected.kind,
        detail: detected.detail,
        keys: Some(detected.keys),
        size: md.len(),
        mtime: mtime_millis(&md),
    })
}

/// The folder's name without allocating; a name that is not UTF-8 reads as empty.
fn parent_str(path: &Path) -> &str {
    path.parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        .unwrap_or("")
}

fn parent_name(path: &Path) -> Cow<'_, str> {
    path.parent()
        .and_then(Path::file_name)
        .map_or(Cow::Borrowed(""), OsStr::to_string_lossy)
}

fn read_head(path: &Path, want: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(want.min(4096));
    fs::File::open(path)
        .ok()?
        .take(want as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// Decides whether the file at `path` is a secret file: the user's marks first,
/// then its name and a look at its contents. Reads only what the detector asks
/// for, and keeps none of it.
pub fn detect_secret(path: &Path, size: u64, marks: &Marks) -> Option<Detected> {
    let mark = marks.lookup(path);
    if mark == Mark::Unmarked {
        return None;
    }
    let name = path.file_name()?.to_string_lossy();
    let parent = parent_name(path);
    let detected = secrets::candidate(&name, &parent, size).and_then(|c| {
        let bytes = read_head(path, c.read)?;
        secrets::confirm(&c, &name, &bytes)
    });
    match (mark, detected) {
        (_, Some(d)) => Some(d),
        (Mark::Marked, None) => Some(Detected {
            kind: Kind::Marked,
            keys: Vec::new(),
            detail: String::new(),
        }),
        _ => None,
    }
}

/// Maps directories to their project, never looking above the scan root. A home
/// folder or a filesystem root is never a project, even with a `.git` in it (a
/// dotfiles repository is common), or every env file without a project folder
/// of its own would be filed under the user's name.
struct ProjectCache<'a> {
    root: &'a Path,
    profile: &'a Profile,
    is_root: HashMap<PathBuf, bool>,
    resolved: HashMap<PathBuf, (String, PathBuf)>,
}

impl<'a> ProjectCache<'a> {
    fn new(root: &'a Path, profile: &'a Profile) -> Self {
        ProjectCache {
            root,
            profile,
            is_root: HashMap::new(),
            resolved: HashMap::new(),
        }
    }

    fn lookup(&mut self, dir: &Path) -> (String, PathBuf) {
        if let Some(hit) = self.resolved.get(dir) {
            return hit.clone();
        }
        let boundary = self.root;
        let profile = self.profile;
        let is_root = &mut self.is_root;
        let found = find_project_root(dir, |candidate| {
            // Outside the scan root, a home folder or a filesystem root: not a project.
            if !candidate.starts_with(boundary)
                || candidate.parent().is_none()
                || profile.is_home(candidate)
            {
                return false;
            }
            *is_root
                .entry(candidate.to_path_buf())
                .or_insert_with(|| PROJECT_MARKERS.iter().any(|m| candidate.join(m).exists()))
        });
        let root = found.unwrap_or_else(|| dir.to_path_buf());
        let result = (project_name(&root), root);
        self.resolved.insert(dir.to_path_buf(), result.clone());
        result
    }
}

/// Milliseconds, so a same-size edit within one second is still noticed.
fn mtime_millis(md: &fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// Names of the variables a file defines. Values are deliberately not kept.
pub fn read_keys(path: &Path, size: u64) -> Vec<String> {
    if size > MAX_PARSE_BYTES {
        return Vec::new();
    }
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    let mut bytes = Vec::with_capacity(size as usize);
    if file.take(MAX_PARSE_BYTES).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    DotEnv::parse(&text)
        .pairs()
        .into_iter()
        .map(|(k, _)| k.to_string())
        .collect()
}

pub enum Refresh {
    Unchanged,
    Changed(FileRecord),
    Gone,
}

/// Compares a record with the file on disk (following symlinks).
pub fn refresh_record(old: &FileRecord) -> Refresh {
    let Ok(md) = fs::metadata(&old.path) else {
        return Refresh::Gone;
    };
    if !md.is_file() {
        return Refresh::Gone;
    }
    let (size, mtime) = (md.len(), mtime_millis(&md));
    if size == old.size && mtime == old.mtime {
        return Refresh::Unchanged;
    }
    if old.kind.is_secret() {
        // Marks changing rebuilds the index, so only the file itself can have changed.
        return match detect_secret(&old.path, size, &Marks::default()) {
            Some(d) => Refresh::Changed(FileRecord {
                size,
                mtime,
                label: d.kind.as_str().to_string(),
                keys: d.keys,
                kind: d.kind,
                detail: d.detail,
                ..old.clone()
            }),
            // A file that stopped being a secret, unless the user marked it by hand.
            None if old.kind == Kind::Marked => Refresh::Changed(FileRecord {
                size,
                mtime,
                ..old.clone()
            }),
            None => Refresh::Gone,
        };
    }
    Refresh::Changed(FileRecord {
        size,
        mtime,
        keys: read_keys(&old.path, size),
        ..old.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::Os;

    fn write(root: &Path, rel: &str, contents: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    fn opts(root: &Path) -> ScanOptions {
        ScanOptions::from_config(
            &Config {
                roots: vec![root.to_path_buf()],
                ..Config::default()
            },
            None,
        )
    }

    fn names(out: &ScanOutcome) -> Vec<String> {
        out.records
            .iter()
            .map(|r| format!("{}:{}", r.project, r.label))
            .collect()
    }

    /// A tree standing in for a whole machine: system folders, two homes, server folders.
    fn machine(root: &Path) {
        for system in [
            "proc/1",
            "usr/share/app",
            "var/lib/docker",
            "etc/app",
            "tmp/scratch",
        ] {
            write(root, &format!("{system}/.env"), "SYSTEM=1\n");
        }
        write(root, "var/www/site/.env", "WEB=1\n");
        write(root, "srv/app/.env", "SRV=1\n");
        write(root, "home/me/proj/package.json", "{}");
        write(root, "home/me/proj/.env", "ME=1\n");
        write(root, "home/me/.config/tool/.env", "HIDDEN=1\n");
        write(root, "home/me/.local/share/x/.env", "HIDDEN=2\n");
        write(root, "home/me/.work/secret/.env", "HIDDEN=3\n");
        write(root, "home/me/Pictures/trip/.env", "MEDIA=1\n");
        write(root, "home/other/work/.env", "OTHER=1\n");
        write(root, "home/other/.cache/.env", "HIDDEN=4\n");
        write(root, "root/ops/.env", "ROOT=1\n");
        write(root, "root/.ssh/.env", "HIDDEN=5\n");
    }

    fn machine_opts(root: &Path, scan_root: &Path) -> ScanOptions {
        let mut o = opts(scan_root);
        o.profile = Profile::new(
            Os::Linux,
            std::slice::from_ref(&root.to_path_buf()),
            Some(&root.join("home/me")),
        );
        o
    }

    fn relative(out: &ScanOutcome, root: &Path) -> Vec<String> {
        let mut found: Vec<String> = out
            .records
            .iter()
            .map(|r| {
                r.path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        found.sort();
        found
    }

    fn canonical_tmp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::fsutil::canonical(dir.path()).unwrap();
        (dir, root)
    }

    #[test]
    fn a_machine_wide_scan_finds_real_projects_and_skips_system_and_hidden_home_folders() {
        let (_t, root) = canonical_tmp();
        machine(&root);
        let out = scan(&machine_opts(&root, &root));
        assert_eq!(
            relative(&out, &root),
            [
                "home/me/proj/.env",
                "home/other/work/.env",
                "root/ops/.env",
                "srv/app/.env",
                "var/www/site/.env",
            ]
        );
    }

    #[test]
    fn an_explicit_folder_inside_a_hidden_home_folder_is_still_scanned() {
        let (_t, root) = canonical_tmp();
        machine(&root);
        let hidden = root.join("home/me/.work");
        let out = scan(&machine_opts(&root, &hidden));
        assert_eq!(relative(&out, &root), ["home/me/.work/secret/.env"]);
    }

    #[test]
    fn scanning_a_home_folder_directly_skips_its_hidden_and_media_folders() {
        let (_t, root) = canonical_tmp();
        machine(&root);
        let out = scan(&machine_opts(&root, &root.join("home/me")));
        assert_eq!(relative(&out, &root), ["home/me/proj/.env"]);
    }

    #[test]
    fn a_home_folder_is_never_a_project_even_with_a_git_repository_in_it() {
        let (_t, root) = canonical_tmp();
        // a dotfiles repository and a package.json in the home folder itself
        write(&root, "home/me/.git/HEAD", "");
        write(&root, "home/me/package.json", "{}");
        write(&root, "home/me/Downloads/foo/.env", "FOO=1\n");
        write(&root, "home/me/code/app/package.json", "{}");
        write(&root, "home/me/code/app/.env", "APP=1\n");

        for scan_root in [root.join("home/me"), root.clone()] {
            let out = scan(&machine_opts(&root, &scan_root));
            let mut projects: Vec<String> = out.records.iter().map(|r| r.project.clone()).collect();
            projects.sort();
            assert_eq!(
                projects,
                ["app", "foo"],
                "scan root {}",
                scan_root.display()
            );
        }
    }

    #[test]
    fn finds_env_family_and_flags_examples() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "api/package.json", "{}");
        write(root, "api/.env", "A=1\nB=2\n");
        write(root, "api/.env.production", "A=9\n");
        write(root, "api/.env.example", "A=\n");
        write(root, "api/.env.swp", "junk");
        write(root, "api/notenv.txt", "x");

        let out = scan(&opts(root));
        assert_eq!(
            names(&out),
            ["api:default", "api:example", "api:production"]
        );
        let default = out.records.iter().find(|r| r.label == "default").unwrap();
        assert_eq!(default.keys, ["A", "B"]);
        assert!(
            out.records
                .iter()
                .find(|r| r.label == "example")
                .unwrap()
                .is_example
        );
    }

    #[test]
    fn skips_ignored_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "app/.env", "A=1\n");
        write(root, "app/node_modules/dep/.env", "LEAK=1\n");
        write(root, "app/target/debug/.env", "LEAK=1\n");
        write(root, "app/dist/.env", "LEAK=1\n");
        let out = scan(&opts(root));
        assert_eq!(out.records.len(), 1);
        assert_eq!(out.records[0].keys, ["A"]);
    }

    #[test]
    fn respects_max_depth() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".env", "TOP=1\n");
        write(root, "a/b/c/.env", "DEEP=1\n");
        let mut o = opts(root);
        o.max_depth = 2;
        let shallow = scan(&o);
        assert_eq!(shallow.records.len(), 1);
        o.max_depth = 10;
        assert_eq!(scan(&o).records.len(), 2);
    }

    #[test]
    fn detects_nearest_project_but_not_above_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // a marker *above* the scan root must not capture the files
        write(root, "package.json", "{}");
        let scan_root = root.join("code");
        write(&scan_root, "mono/.git/HEAD", "");
        write(&scan_root, "mono/apps/web/package.json", "{}");
        write(&scan_root, "mono/apps/web/.env", "W=1\n");
        write(&scan_root, "mono/.env", "M=1\n");
        write(&scan_root, "loose/.env", "L=1\n");

        let out = scan(&opts(&scan_root));
        let mut got = names(&out);
        got.sort();
        assert_eq!(got, ["loose:default", "mono:default", "web:default"]);
    }

    #[test]
    fn reports_missing_roots_and_dedupes_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "p/.env", "A=1\n");
        let mut o = opts(root);
        o.roots = vec![root.to_path_buf(), root.join("p"), root.join("nope")];
        let out = scan(&o);
        assert_eq!(out.records.len(), 1);
        assert_eq!(out.missing_roots, [root.join("nope")]);
    }

    #[cfg(unix)]
    #[test]
    fn includes_symlinked_env_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "shared/real.env", "S=1\n");
        fs::create_dir_all(root.join("svc")).unwrap();
        std::os::unix::fs::symlink(root.join("shared/real.env"), root.join("svc/.env")).unwrap();
        let out = scan(&opts(root));
        assert_eq!(out.records.len(), 1);
        assert_eq!(out.records[0].keys, ["S"]);
    }

    #[test]
    fn refresh_notices_changes_and_deletions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "p/.env", "A=1\n");
        let rec = scan(&opts(root)).records.remove(0);
        assert!(matches!(refresh_record(&rec), Refresh::Unchanged));

        write(root, "p/.env", "A=1\nBRAND_NEW=2\n");
        match refresh_record(&rec) {
            Refresh::Changed(new) => assert_eq!(new.keys, ["A", "BRAND_NEW"]),
            _ => panic!("expected a change"),
        }

        fs::remove_file(&rec.path).unwrap();
        assert!(matches!(refresh_record(&rec), Refresh::Gone));
    }

    // --- secret files -----------------------------------------------------------

    // Built from fragments so nothing here looks like real key material.
    fn begin(label: &str) -> String {
        format!("{}BEGIN {label}{}", "-----", "-----")
    }

    fn pem(label: &str) -> String {
        format!("{}\nQUJDREVGR0g=\n-----END {label}-----\n", begin(label))
    }

    fn service_account() -> String {
        serde_json::json!({
            "type": "service_account",
            "project_id": "demo",
            "private_key": pem("PRIVATE KEY"),
            "client_email": "bot@demo.example",
        })
        .to_string()
    }

    fn jks() -> Vec<u8> {
        let mut bytes = vec![0xFE, 0xED, 0xFE, 0xED, 0, 0, 0, 2];
        bytes.extend([0u8; 64]);
        bytes
    }

    fn kind_of(out: &ScanOutcome, file: &str) -> Option<Kind> {
        out.records
            .iter()
            .find(|r| r.path.file_name().is_some_and(|n| n == file))
            .map(|r| r.kind)
    }

    fn secrets_tree(root: &Path) {
        write(root, "app/package.json", "{}");
        write(root, "app/.env", "A=1\n");
        write(root, "app/secrets/sa.json", &service_account());
        write(root, "app/config.json", r#"{"port": 3000}"#);
        write(root, "app/package-lock.json", &service_account());
        write(root, "app/server.key", &pem("PRIVATE KEY"));
        write(root, "app/server.pem", &pem("CERTIFICATE"));
        fs::create_dir_all(root.join("app/android")).unwrap();
        fs::write(root.join("app/android/release.keystore"), jks()).unwrap();
        fs::write(root.join("app/android/notes.keystore"), b"not a keystore").unwrap();
        write(root, "keys/id_ed25519", &pem("RSA PRIVATE KEY"));
        write(root, "keys/id_ed25519.pub", "ssh-ed25519 AAAA me@host\n");
        write(root, "node_modules/dep/leaked.key", &pem("PRIVATE KEY"));
        write(root, "pkg/testdata/fixture.key", &pem("PRIVATE KEY"));
    }

    #[test]
    fn secret_files_are_found_by_kind_and_env_files_are_unchanged() {
        let (_t, root) = canonical_tmp();
        secrets_tree(&root);
        let out = scan(&opts(&root));
        let kinds = [
            ("sa.json", Some(Kind::ServiceAccount)),
            ("server.key", Some(Kind::PrivateKey)),
            ("release.keystore", Some(Kind::Keystore)),
            ("id_ed25519", Some(Kind::SshKey)),
            (".env", Some(Kind::Env)),
            // looks like one, is not
            ("config.json", None),
            ("package-lock.json", None),
            ("server.pem", None),
            ("notes.keystore", None),
            ("id_ed25519.pub", None),
            // inside folders that are never searched
            ("leaked.key", None),
            ("fixture.key", None),
        ];
        for (file, want) in kinds {
            assert_eq!(kind_of(&out, file), want, "{file}");
        }
        let sa = out
            .records
            .iter()
            .find(|r| r.kind == Kind::ServiceAccount)
            .unwrap();
        assert_eq!(sa.label, "service-account");
        assert!(
            sa.keys.contains(&"private_key".to_string()),
            "{:?}",
            sa.keys
        );
        assert!(sa.project == "app", "{}", sa.project);
        let key = out.records.iter().find(|r| r.kind == Kind::SshKey).unwrap();
        assert_eq!(key.detail, "rsa");
        assert!(key.keys.is_empty());
        assert_eq!(
            key.project, "keys",
            "a key outside any project is filed under its folder"
        );
    }

    #[test]
    fn no_record_carries_the_contents_of_a_secret() {
        let (_t, root) = canonical_tmp();
        secrets_tree(&root);
        let out = scan(&opts(&root));
        for r in &out.records {
            let shown = format!("{r:?}");
            assert!(!shown.contains("QUJDREVGR0g"), "{shown}");
            assert!(!shown.contains("bot@demo.example"), "{shown}");
        }
    }

    #[test]
    fn marking_adds_a_file_and_unmarking_silences_one() {
        let (_t, root) = canonical_tmp();
        secrets_tree(&root);
        write(&root, "notes/passwords.txt", "bank: hunter2\n");
        let by_hand = root.join("notes/passwords.txt");
        let wrong_guess = root.join("app/secrets/sa.json");

        let mut config = Config {
            roots: vec![root.clone()],
            ..Config::default()
        };
        assert_eq!(
            kind_of(
                &scan(&ScanOptions::from_config(&config, None)),
                "passwords.txt"
            ),
            None
        );

        config.marked.push(by_hand.clone());
        config.unmarked.push(wrong_guess.clone());
        let out = scan(&ScanOptions::from_config(&config, None));
        assert_eq!(kind_of(&out, "passwords.txt"), Some(Kind::Marked));
        assert_eq!(
            kind_of(&out, "sa.json"),
            None,
            "unmarked wins over detection"
        );
        assert_eq!(
            kind_of(&out, "server.key"),
            Some(Kind::PrivateKey),
            "others are untouched"
        );

        // unmarked beats marked
        config.marked.push(wrong_guess);
        let out = scan(&ScanOptions::from_config(&config, None));
        assert_eq!(kind_of(&out, "sa.json"), None);
    }

    #[test]
    fn a_marked_file_that_is_also_recognised_keeps_its_specific_kind() {
        let (_t, root) = canonical_tmp();
        write(&root, "x/key.pem", &pem("PRIVATE KEY"));
        let config = Config {
            roots: vec![root.clone()],
            marked: vec![root.join("x/key.pem")],
            ..Config::default()
        };
        let out = scan(&ScanOptions::from_config(&config, None));
        assert_eq!(kind_of(&out, "key.pem"), Some(Kind::PrivateKey));
    }

    #[test]
    fn a_marked_file_is_found_even_when_its_name_is_ordinary_and_it_is_empty() {
        let (_t, root) = canonical_tmp();
        write(&root, "x/readme.md", "");
        let config = Config {
            roots: vec![root.clone()],
            marked: vec![root.join("x/readme.md")],
            ..Config::default()
        };
        let out = scan(&ScanOptions::from_config(&config, None));
        assert_eq!(kind_of(&out, "readme.md"), Some(Kind::Marked));
    }

    #[test]
    fn refresh_re_detects_a_secret_and_drops_one_that_stopped_being_secret() {
        let (_t, root) = canonical_tmp();
        write(&root, "x/package.json", "{}");
        write(&root, "x/creds.json", &service_account());
        let out = scan(&opts(&root));
        let record = out
            .records
            .iter()
            .find(|r| r.kind == Kind::ServiceAccount)
            .unwrap()
            .clone();
        assert!(matches!(refresh_record(&record), Refresh::Unchanged));

        // rewritten as an OAuth token file: still a secret, a different kind
        std::thread::sleep(Duration::from_millis(20));
        write(
            &root,
            "x/creds.json",
            r#"{"refresh_token": "1//refreshvalue", "client_id": "i"}"#,
        );
        match refresh_record(&record) {
            Refresh::Changed(new) => {
                assert_eq!(new.kind, Kind::AuthJson);
                assert_eq!(new.label, "auth-json");
                assert!(new.keys.contains(&"refresh_token".to_string()));
            }
            _ => panic!("expected a changed record"),
        }

        // rewritten as ordinary config: no longer a secret
        std::thread::sleep(Duration::from_millis(20));
        write(&root, "x/creds.json", r#"{"port": 8080, "debug": true}"#);
        assert!(matches!(refresh_record(&record), Refresh::Gone));

        // a file marked by hand stays after its contents change
        write(&root, "x/notes.txt", "one");
        let config = Config {
            roots: vec![root.clone()],
            marked: vec![root.join("x/notes.txt")],
            ..Config::default()
        };
        let out = scan(&ScanOptions::from_config(&config, None));
        let marked = out
            .records
            .iter()
            .find(|r| r.kind == Kind::Marked)
            .unwrap()
            .clone();
        std::thread::sleep(Duration::from_millis(20));
        write(&root, "x/notes.txt", "two, and longer");
        assert!(matches!(refresh_record(&marked), Refresh::Changed(r) if r.kind == Kind::Marked));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_key_counts_and_a_file_that_cannot_be_read_is_skipped() {
        use std::os::unix::fs::PermissionsExt;
        let (_t, root) = canonical_tmp();
        write(&root, "real/key.pem", &pem("PRIVATE KEY"));
        std::os::unix::fs::symlink(root.join("real/key.pem"), root.join("link.pem")).unwrap();
        write(&root, "locked/other.pem", &pem("PRIVATE KEY"));
        let locked = root.join("locked/other.pem");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let out = scan(&opts(&root));
        let found: Vec<String> = out
            .records
            .iter()
            .map(|r| r.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(found.contains(&"link.pem".to_string()), "{found:?}");
        assert!(found.contains(&"key.pem".to_string()), "{found:?}");
        if fs::File::open(&locked).is_err() {
            assert!(!found.contains(&"other.pem".to_string()), "{found:?}");
        }
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
    }
}
