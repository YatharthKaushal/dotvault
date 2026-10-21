//! Where dotvault looks when nobody told it, and what it leaves alone.
//!
//! With no folders configured the first command that needs them scans the home
//! folder. `scan --system` adds the whole machine. Both are broad, so a
//! [`Profile`] keeps them fast and safe: it skips hidden and application-data
//! folders inside home folders, operating-system folders, and network mounts
//! (a hung mount would freeze the very first command a user types).

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// The platforms whose system folders are known. A value, not `cfg!`, so every
/// platform's rules can be tested on whichever machine runs the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Mac,
    Windows,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Linux
        }
    }
}

/// The user's home folder. `DOTVAULT_HOME` overrides it, for containers and for
/// tests (on Windows the OS is asked and environment variables are ignored).
/// Canonical when it can be, because scanned paths are, and `~` shortening only
/// works when both sides agree (`/var` vs `/private/var`, short Windows names).
pub fn home_dir() -> Option<PathBuf> {
    let home = home_from(
        std::env::var_os("DOTVAULT_HOME"),
        directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()),
    )?;
    Some(crate::fsutil::canonical(&home).unwrap_or(home))
}

/// The override wins when it is set and not empty.
fn home_from(
    override_home: Option<std::ffi::OsString>,
    os_home: Option<PathBuf>,
) -> Option<PathBuf> {
    match override_home.filter(|v| !v.is_empty()) {
        Some(home) => Some(PathBuf::from(home)),
        None => os_home,
    }
}

/// What is scanned when no folders are configured: the home folder.
pub fn default_roots() -> Vec<PathBuf> {
    home_dir()
        .and_then(|h| crate::fsutil::canonical(&h).ok())
        .filter(|h| h.is_dir())
        .into_iter()
        .collect()
}

/// The root of the filesystem that holds the system: `/`, or the system drive.
pub fn machine_root(os: Os) -> PathBuf {
    match os {
        Os::Windows => {
            let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
            PathBuf::from(format!("{}\\", drive.trim_end_matches(['\\', '/'])))
        }
        Os::Linux | Os::Mac => PathBuf::from("/"),
    }
}

/// What `scan --system` adds. On Windows only the system drive: other drives may
/// be removable or network shares, so they are opt-in (`scan D:\`).
pub fn system_roots() -> Vec<PathBuf> {
    vec![machine_root(Os::current())]
}

/// Folders, relative to the filesystem root, that never hold project env files.
fn system_folders(os: Os) -> &'static [&'static str] {
    match os {
        Os::Linux => &[
            "proc",
            "sys",
            "dev",
            "run",
            "boot",
            "snap",
            "lost+found",
            "usr",
            "lib",
            "lib32",
            "lib64",
            "libx32",
            "bin",
            "sbin",
            "etc",
            "var/lib",
            "var/cache",
            "var/log",
            "var/spool",
            "var/tmp",
            "tmp",
            "media",
            "mnt",
            // full-system backups: root-owned, and copies of every file
            "timeshift",
        ],
        Os::Mac => &[
            "System",
            "Library",
            "private",
            "Applications",
            "Volumes",
            "cores",
            "usr",
            "opt/homebrew",
        ],
        Os::Windows => &[
            "Windows",
            "Program Files",
            "Program Files (x86)",
            "ProgramData",
            "$Recycle.Bin",
            "System Volume Information",
            "Recovery",
        ],
    }
}

/// Folders directly inside a home folder that are application data or media.
fn home_noise(os: Os) -> &'static [&'static str] {
    match os {
        Os::Linux => &["snap", "Music", "Pictures", "Videos"],
        Os::Mac => &["Library", "Applications", "Movies", "Music", "Pictures"],
        Os::Windows => &["AppData", "Music", "Pictures", "Videos"],
    }
}

/// The folders a broad scan skips.
#[derive(Debug, Clone)]
pub struct Profile {
    os: Os,
    /// Home folders: the current user's, plus `/root`.
    homes: Vec<PathBuf>,
    /// Folders whose children are home folders: `/home`, `/Users`, `C:\Users`.
    home_parents: Vec<PathBuf>,
    /// Folders never entered, compared by exact path.
    skipped: HashSet<PathBuf>,
}

impl Profile {
    /// `fs_root` is the filesystem root the system folders hang off; tests pass
    /// a temporary directory standing in for `/`.
    pub fn new(os: Os, fs_root: &Path, home: Option<&Path>) -> Profile {
        let canon = |p: PathBuf| crate::fsutil::canonical(&p).unwrap_or(p);
        let mut homes: Vec<PathBuf> = home.map(|h| canon(h.to_path_buf())).into_iter().collect();
        let mut home_parents = Vec::new();
        match os {
            Os::Linux => {
                home_parents.push(fs_root.join("home"));
                homes.push(fs_root.join("root"));
            }
            Os::Mac => home_parents.push(fs_root.join("Users")),
            Os::Windows => home_parents.push(fs_root.join("Users")),
        }
        Profile {
            os,
            homes,
            home_parents: home_parents.into_iter().map(canon).collect(),
            skipped: system_folders(os)
                .iter()
                .map(|rel| fs_root.join(rel))
                .collect(),
        }
    }

    /// The profile for this machine, including network mounts.
    pub fn detect() -> Profile {
        let os = Os::current();
        Profile::new(os, &machine_root(os), home_dir().as_deref()).skipping(read_network_mounts())
    }

    /// Also never enter these folders (exact paths).
    pub fn skipping(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Profile {
        self.skipped.extend(paths);
        self
    }

    /// Whether `dir` is a home folder. A home folder is never a project, even
    /// when it holds `.git` or `package.json` (a dotfiles repository is common).
    pub fn is_home(&self, dir: &Path) -> bool {
        self.homes.iter().any(|h| h == dir)
            || dir
                .parent()
                .is_some_and(|p| self.home_parents.iter().any(|hp| hp == p))
    }

    /// Whether the walker should not enter the directory `dir`.
    pub fn skips(&self, dir: &Path) -> bool {
        if self.skipped.contains(dir) {
            return true;
        }
        let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
            return false;
        };
        if !self.is_home(parent) {
            return false;
        }
        self.is_hidden_or_noise(name)
    }

    fn is_hidden_or_noise(&self, name: &OsStr) -> bool {
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            return true;
        }
        home_noise(self.os).iter().any(|noise| {
            if self.os == Os::Windows {
                noise.eq_ignore_ascii_case(&name)
            } else {
                *noise == name
            }
        })
    }
}

/// Mount points of network and FUSE file systems, from the text of
/// `/proc/mounts`. Entering one can block for a long time.
pub fn network_mount_points(proc_mounts: &str) -> Vec<PathBuf> {
    proc_mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (_device, mount, fstype) = (fields.next()?, fields.next()?, fields.next()?);
            is_network_fs(fstype).then(|| PathBuf::from(unescape_mount(mount)))
        })
        .collect()
}

fn is_network_fs(fstype: &str) -> bool {
    fstype.starts_with("nfs")
        || matches!(
            fstype,
            "cifs"
                | "smb3"
                | "smbfs"
                | "9p"
                | "davfs"
                | "afs"
                | "ceph"
                | "fuse.sshfs"
                | "fuse.gvfsd-fuse"
                | "fuse.rclone"
                | "fuse.s3fs"
                | "fuse.glusterfs"
        )
}

/// `/proc/mounts` writes spaces and a few other bytes as `\040`-style octal.
fn unescape_mount(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = (bytes[i] == b'\\' && i + 3 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 4]).ok())
            .flatten()
            .and_then(|digits| u8::from_str_radix(digits, 8).ok());
        match octal {
            Some(byte) => {
                out.push(byte);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(target_os = "linux")]
fn read_network_mounts() -> Vec<PathBuf> {
    fs::read_to_string("/proc/mounts")
        .map(|text| network_mount_points(&text))
        .unwrap_or_default()
}

#[cfg(not(target_os = "linux"))]
fn read_network_mounts() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary directory standing in for `/`, with its canonical path.
    fn fake_root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::fsutil::canonical(dir.path()).unwrap();
        (dir, root)
    }

    fn profile(os: Os, root: &Path, home: &str) -> Profile {
        let home = root.join(home);
        fs::create_dir_all(&home).unwrap();
        Profile::new(os, root, Some(&home))
    }

    #[test]
    fn hidden_folders_inside_home_folders_are_skipped() {
        let (_t, root) = fake_root();
        let p = profile(Os::Linux, &root, "home/me");
        // the current user's home and another user's home under /home
        assert!(p.skips(&root.join("home/me/.config")));
        assert!(p.skips(&root.join("home/me/.local")));
        assert!(p.skips(&root.join("home/other/.cache")));
        // /root is a home too
        assert!(p.skips(&root.join("root/.ssh")));
        // visible folders are kept
        assert!(!p.skips(&root.join("home/me/code")));
        assert!(!p.skips(&root.join("home/other/work")));
        // hidden folders anywhere else are kept: only the top of a home is noise
        assert!(!p.skips(&root.join("home/me/code/.github")));
        assert!(!p.skips(&root.join("srv/.hidden-app")));
        assert!(!p.skips(&root.join("opt/.tool")));
    }

    #[test]
    fn application_data_inside_home_is_skipped_per_platform() {
        let (_t, root) = fake_root();
        let linux = profile(Os::Linux, &root, "home/me");
        assert!(linux.skips(&root.join("home/me/snap")));
        assert!(linux.skips(&root.join("home/me/Pictures")));
        assert!(
            !linux.skips(&root.join("home/me/Library")),
            "Library is only special on macOS"
        );

        let mac = profile(Os::Mac, &root, "Users/me");
        assert!(mac.skips(&root.join("Users/me/Library")));
        assert!(mac.skips(&root.join("Users/me/Movies")));
        assert!(!mac.skips(&root.join("Users/me/snap")));
        assert!(!mac.skips(&root.join("Users/me/Documents")));

        let win = profile(Os::Windows, &root, "Users/me");
        assert!(win.skips(&root.join("Users/me/AppData")));
        assert!(
            win.skips(&root.join("Users/me/appdata")),
            "Windows names are case-insensitive"
        );
        assert!(!win.skips(&root.join("Users/me/Documents")));
    }

    #[test]
    fn a_name_only_counts_directly_inside_a_home() {
        let (_t, root) = fake_root();
        let p = profile(Os::Linux, &root, "home/me");
        assert!(!p.skips(&root.join("home/me/code/Pictures")));
        assert!(!p.skips(&root.join("srv/snap")));
    }

    #[test]
    fn every_platforms_system_folders_are_skipped_under_the_root() {
        for (os, skipped, kept) in [
            (
                Os::Linux,
                [
                    "proc",
                    "sys",
                    "usr",
                    "var/lib",
                    "var/log",
                    "var/spool",
                    "tmp",
                    "mnt",
                    "etc",
                    "timeshift",
                ]
                .as_slice(),
                ["srv", "opt", "var/www", "var", "home", "data"].as_slice(),
            ),
            (
                Os::Mac,
                [
                    "System",
                    "Library",
                    "private",
                    "Applications",
                    "Volumes",
                    "opt/homebrew",
                ]
                .as_slice(),
                ["Users", "opt", "srv", "opt/work"].as_slice(),
            ),
            (
                Os::Windows,
                [
                    "Windows",
                    "Program Files",
                    "Program Files (x86)",
                    "ProgramData",
                    "$Recycle.Bin",
                ]
                .as_slice(),
                ["Users", "Projects", "dev", "inetpub"].as_slice(),
            ),
        ] {
            let (_t, root) = fake_root();
            let p = Profile::new(os, &root, None);
            for rel in skipped {
                assert!(p.skips(&root.join(rel)), "{os:?}: {rel} should be skipped");
            }
            for rel in kept {
                assert!(!p.skips(&root.join(rel)), "{os:?}: {rel} should be kept");
            }
        }
    }

    #[test]
    fn extra_skipped_paths_are_exact() {
        let (_t, root) = fake_root();
        let p = Profile::new(Os::Linux, &root, None).skipping([root.join("srv/nfs")]);
        assert!(p.skips(&root.join("srv/nfs")));
        assert!(!p.skips(&root.join("srv/nfs-backup")));
        assert!(!p.skips(&root.join("srv")));
    }

    #[test]
    fn home_folders_are_recognised_but_not_their_children() {
        let (_t, root) = fake_root();
        let p = profile(Os::Linux, &root, "home/me");
        assert!(p.is_home(&root.join("home/me")));
        assert!(p.is_home(&root.join("home/other")));
        assert!(p.is_home(&root.join("root")));
        assert!(!p.is_home(&root.join("home/me/code")));
        assert!(!p.is_home(&root.join("home")));
        assert!(!p.is_home(&root));
    }

    #[test]
    fn network_and_fuse_mounts_are_found_in_proc_mounts() {
        let mounts = "\
sysfs /sys sysfs rw,nosuid 0 0
/dev/nvme0n1p2 / ext4 rw,relatime 0 0
server:/export /mnt/team\\040share nfs4 rw,vers=4.2 0 0
//nas/media /home/me/nas cifs rw 0 0
me@host:/ /home/me/remote fuse.sshfs rw,nosuid 0 0
gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw 0 0
/dev/sdb1 /data ext4 rw 0 0
tmpfs /run tmpfs rw 0 0
ntfs /media/win fuseblk rw 0 0
";
        let found = network_mount_points(mounts);
        assert_eq!(
            found,
            [
                PathBuf::from("/mnt/team share"),
                PathBuf::from("/home/me/nas"),
                PathBuf::from("/home/me/remote"),
                PathBuf::from("/run/user/1000/gvfs"),
            ]
        );
    }

    #[test]
    fn mount_escapes_decode() {
        assert_eq!(unescape_mount("/mnt/a\\040b"), "/mnt/a b");
        assert_eq!(unescape_mount("/mnt/tab\\011x"), "/mnt/tab\tx");
        assert_eq!(unescape_mount("/mnt/back\\134slash"), "/mnt/back\\slash");
        assert_eq!(unescape_mount("/plain"), "/plain");
        // a backslash that is not an escape is kept
        assert_eq!(unescape_mount("/odd\\zz"), "/odd\\zz");
        assert_eq!(unescape_mount("/end\\"), "/end\\");
    }

    #[test]
    fn the_home_override_wins_only_when_set_and_not_empty() {
        let os_home = Some(PathBuf::from("/home/real"));
        assert_eq!(
            home_from(Some("/elsewhere".into()), os_home.clone()),
            Some(PathBuf::from("/elsewhere"))
        );
        assert_eq!(home_from(Some("".into()), os_home.clone()), os_home);
        assert_eq!(home_from(None, os_home.clone()), os_home);
        assert_eq!(home_from(None, None), None);
    }

    #[test]
    fn machine_roots_per_platform() {
        assert_eq!(machine_root(Os::Linux), PathBuf::from("/"));
        assert_eq!(machine_root(Os::Mac), PathBuf::from("/"));
        if cfg!(windows) {
            assert!(machine_root(Os::Windows).to_string_lossy().ends_with(":\\"));
        }
        assert_eq!(system_roots().len(), 1);
    }
}
