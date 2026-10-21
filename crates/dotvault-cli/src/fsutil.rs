use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
