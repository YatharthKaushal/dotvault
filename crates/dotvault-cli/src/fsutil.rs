use std::fs::{self, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Like `fs::canonicalize`, but on Windows without the `\\?\` prefix, which
/// would show up in every path dotvault prints and defeat the `~` shortening.
pub fn canonical(path: &Path) -> io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// Replaces `path` with `contents` without ever leaving a half-written file:
/// the data goes to a temp file in the same directory which is then renamed
/// over the target. Permissions of an existing file are kept; new files are
/// created private (0600 on Unix). Symlinks are followed, so a linked `.env`
/// stays a link.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_with(path, contents, Perms::KeepExisting)
}

/// Like [`write_atomic`] but the result is always private (0600 on Unix),
/// even if an older, more permissive file is being replaced. For credentials.
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_with(path, contents, Perms::Private)
}

#[derive(Clone, Copy)]
enum Perms {
    KeepExisting,
    Private,
}

fn write_with(path: &Path, contents: &[u8], perms: Perms) -> io::Result<()> {
    let target = match fs::canonicalize(path) {
        Ok(p) => p,
        Err(e) if e.kind() == io::ErrorKind::NotFound => absolute(path)?,
        Err(e) => return Err(e),
    };
    let dir = target
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent directory"))?;
    let existing_perms = match perms {
        Perms::KeepExisting => fs::metadata(&target).ok().map(|m| m.permissions()),
        // the temp file is created 0600 and that is what we want to end up with
        Perms::Private => None,
    };

    let tmp = temp_path(dir);
    let result = (|| {
        let mut file = new_private_file(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        if let Some(perms) = existing_perms {
            fs::set_permissions(&tmp, perms)?;
        }
        fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// How long a command waits for another one that is changing the same file.
pub const EDIT_LOCK_WAIT: Duration = Duration::from_secs(30);

/// How many lock files there are. A file always maps to the same one; two files
/// that share one only wait for each other for a moment.
const LOCK_STRIPES: u64 = 64;

/// Held while one dotvault command reads an env file, changes it and writes it
/// back, so that commands running at the same moment take turns instead of each
/// starting from the same old text and one change silently winning. Released when
/// dropped, or when the process ends, however it ends.
///
/// The lock lives in `lock_dir`, not next to the env file: a file in a project
/// folder would end up in git. It is advisory, which is enough because every
/// dotvault command that changes an env file asks for it.
#[derive(Debug)]
pub struct EditLock {
    _file: fs::File,
}

/// Waits for the turn to change `target`, up to [`EDIT_LOCK_WAIT`]. A wait that runs
/// out is [`io::ErrorKind::TimedOut`]; any other error means the lock could not be
/// set up at all.
pub fn lock_for_edit(lock_dir: &Path, target: &Path) -> io::Result<EditLock> {
    lock_for_edit_within(lock_dir, target, EDIT_LOCK_WAIT)
}

fn lock_for_edit_within(lock_dir: &Path, target: &Path, wait: Duration) -> io::Result<EditLock> {
    fs::create_dir_all(lock_dir)?;
    let file = open_lock_file(&lock_dir.join(format!("edit-{:02x}.lock", stripe(target))))?;
    let started = Instant::now();
    let mut pause = Duration::from_millis(2);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(EditLock { _file: file }),
            Err(TryLockError::Error(err)) => return Err(err),
            Err(TryLockError::WouldBlock) if started.elapsed() >= wait => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "another dotvault command is still changing this file",
                ));
            }
            Err(TryLockError::WouldBlock) => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_millis(50));
            }
        }
    }
}

/// Which lock file `target` uses. Hashed by hand (FNV-1a) so that every version of
/// dotvault, whatever Rust built it, picks the same one. The text is lowercased:
/// on a case-insensitive disk two spellings of one path must not get two locks,
/// and elsewhere the worst case is two files sharing a lock.
fn stripe(target: &Path) -> u64 {
    let resolved = match fs::canonicalize(target) {
        Ok(p) => p,
        Err(_) => match (target.parent(), target.file_name()) {
            (Some(dir), Some(name)) if !dir.as_os_str().is_empty() => fs::canonicalize(dir)
                .map(|d| d.join(name))
                .unwrap_or_else(|_| target.to_path_buf()),
            _ => absolute(target).unwrap_or_else(|_| target.to_path_buf()),
        },
    };
    let text = resolved.to_string_lossy().to_lowercase();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash % LOCK_STRIPES
}

#[cfg(unix)]
fn open_lock_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_lock_file(path: &Path) -> io::Result<fs::File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn absolute(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

/// Starts with `.dotvault` so it is never mistaken for an env file.
fn temp_path(dir: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    dir.join(format!(".dotvault-tmp-{}-{nanos}", std::process::id()))
}

#[cfg(unix)]
fn new_private_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn new_private_file(path: &Path) -> io::Result<fs::File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_paths_with_different_locks(dir: &Path) -> (PathBuf, PathBuf) {
        let first = dir.join("first.env");
        let other = (0..)
            .map(|n| dir.join(format!("other-{n}.env")))
            .find(|p| stripe(p) != stripe(&first))
            .unwrap();
        (first, other)
    }

    #[test]
    fn a_second_lock_on_the_same_file_waits_and_then_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let locks = dir.path().join("locks");
        let target = dir.path().join(".env");
        let held = lock_for_edit(&locks, &target).unwrap();

        let started = Instant::now();
        let err = lock_for_edit_within(&locks, &target, Duration::from_millis(150)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(150));

        drop(held);
        lock_for_edit_within(&locks, &target, Duration::from_millis(150)).unwrap();
    }

    #[test]
    fn the_same_file_under_another_spelling_has_the_same_lock() {
        let dir = tempfile::tempdir().unwrap();
        let locks = dir.path().join("locks");
        let target = dir.path().join(".env");
        fs::write(&target, "").unwrap();
        let roundabout = dir.path().join("sub").join("..").join(".env");
        fs::create_dir(dir.path().join("sub")).unwrap();

        let _held = lock_for_edit(&locks, &target).unwrap();
        let err =
            lock_for_edit_within(&locks, &roundabout, Duration::from_millis(100)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn a_file_that_does_not_exist_yet_can_be_locked_and_keeps_its_lock() {
        let dir = tempfile::tempdir().unwrap();
        let locks = dir.path().join("locks");
        let target = dir.path().join(".env");
        let before = stripe(&target);
        let _held = lock_for_edit(&locks, &target).unwrap();
        fs::write(&target, "A=1\n").unwrap();
        assert_eq!(stripe(&target), before);
    }

    #[test]
    fn different_files_do_not_wait_for_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let locks = dir.path().join("locks");
        let (a, b) = two_paths_with_different_locks(dir.path());
        let _held = lock_for_edit(&locks, &a).unwrap();
        lock_for_edit_within(&locks, &b, Duration::from_millis(100)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn lock_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locks = dir.path().join("locks");
        let _held = lock_for_edit(&locks, &dir.path().join(".env")).unwrap();
        for entry in fs::read_dir(&locks).unwrap() {
            let mode = entry.unwrap().metadata().unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "{mode:o}");
        }
    }

    /// The bug itself: read, change, write back, from many threads at once.
    #[test]
    fn taking_turns_loses_no_update() {
        let dir = tempfile::tempdir().unwrap();
        let locks = dir.path().join("locks");
        let target = dir.path().join("counter");
        fs::write(&target, "0").unwrap();

        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..10 {
                        let _turn = lock_for_edit(&locks, &target).unwrap();
                        let n: u32 = fs::read_to_string(&target).unwrap().parse().unwrap();
                        std::thread::sleep(Duration::from_millis(1));
                        write_atomic(&target, (n + 1).to_string().as_bytes()).unwrap();
                    }
                });
            }
        });
        assert_eq!(fs::read_to_string(&target).unwrap(), "80");
    }

    #[test]
    fn creates_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".env");
        write_atomic(&p, b"A=1\n").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "A=1\n");
        write_atomic(&p, b"A=2\n").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "A=2\n");
        // no temp files left behind
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [".env"]);
    }

    #[cfg(unix)]
    #[test]
    fn new_files_are_private_and_existing_modes_are_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".env");
        write_atomic(&p, b"x").unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&p, b"y").unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_writes_tighten_loose_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("session.json");
        fs::write(&p, "old").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&p, b"new").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "new");
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.env");
        let link = dir.path().join(".env");
        fs::write(&real, "A=1\n").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, b"A=2\n").unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "A=2\n");
    }
}
