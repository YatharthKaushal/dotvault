//! Where dotvault looks when nobody told it, and what it leaves alone.
//!
//! With no folders configured the first command that needs them searches the
//! whole computer: `/` on Linux and macOS, plus other local disks mounted in the
//! usual places, and every fixed or removable drive on Windows. That is broad, so
//! a [`Profile`] keeps it fast and safe: it skips hidden and application-data
//! folders inside home folders, operating-system folders, and network mounts
//! (a hung mount would freeze the very first command a user types).

use std::borrow::Cow;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
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

/// What is searched when no folders are configured: every disk of this machine,
/// or the folders in `DOTVAULT_SCAN_ROOTS` (a PATH-style list that replaces them,
/// for containers and CI, and how tests keep away from the real machine).
pub fn default_roots() -> Vec<PathBuf> {
    let from_env = roots_from_override(std::env::var_os("DOTVAULT_SCAN_ROOTS"));
    let roots = match from_env {
        Some(list) => list,
        None => add_home_if_outside(disks(Os::current()), home_dir()),
    };
    let mut out: Vec<PathBuf> = Vec::new();
    for root in roots {
        if let Ok(canonical) = crate::fsutil::canonical(&root) {
            if canonical.is_dir() && !out.contains(&canonical) {
                out.push(canonical);
            }
        }
    }
    out
}

/// Said in the first-run notice, so it matches what `default_roots` will do.
pub fn default_roots_label() -> &'static str {
    if std::env::var_os("DOTVAULT_SCAN_ROOTS").is_some_and(|v| !v.is_empty()) {
        "the folders in DOTVAULT_SCAN_ROOTS"
    } else {
        "this whole computer, every drive"
    }
}

/// An empty or unset value is no override.
fn roots_from_override(value: Option<OsString>) -> Option<Vec<PathBuf>> {
    let value = value.filter(|v| !v.is_empty())?;
    Some(
        std::env::split_paths(&value)
            .filter(|p| !p.as_os_str().is_empty())
            .collect(),
    )
}

/// A home folder on some other kind of disk (a network home) is still searched.
fn add_home_if_outside(mut roots: Vec<PathBuf>, home: Option<PathBuf>) -> Vec<PathBuf> {
    if let Some(home) = home {
        if !roots.iter().any(|r| home.starts_with(r)) {
            roots.push(home);
        }
    }
    roots
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

/// Every disk worth searching: the system root plus other local disks.
fn disks(os: Os) -> Vec<PathBuf> {
    match os {
        Os::Windows => windows_drives(),
        Os::Linux => {
            let mut roots = vec![machine_root(os)];
            for mount in local_disk_mounts(&read_proc_mounts()) {
                if !roots.contains(&mount) {
                    roots.push(mount);
                }
            }
            roots
        }
        Os::Mac => {
            let mut roots = vec![machine_root(os)];
            roots.extend(mac_volumes());
            roots
        }
    }
}

/// `GetDriveType` results for a disk that is part of this computer.
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;

/// Drive roots (`C:\`) for the letters set in a `GetLogicalDrives` bit mask whose
/// type is fixed or removable. Network shares are left out because a dead one can
/// hang the first command; optical and RAM drives hold no projects. A named
/// folder on any drive still works: `scan Z:\`.
fn drive_roots(mask: u32, kind_of: impl Fn(&str) -> u32) -> Vec<PathBuf> {
    (0..26u8)
        .filter(|bit| mask & (1u32 << bit) != 0)
        .map(|bit| format!("{}:\\", (b'A' + bit) as char))
        .filter(|root| matches!(kind_of(root), DRIVE_FIXED | DRIVE_REMOVABLE))
        .map(PathBuf::from)
        .collect()
}

fn windows_drives() -> Vec<PathBuf> {
    drive_roots(logical_drive_mask(), drive_kind)
}

#[cfg(windows)]
fn logical_drive_mask() -> u32 {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;
    use windows_sys::Win32::System::Diagnostics::Debug::{SetErrorMode, SEM_FAILCRITICALERRORS};
    // SAFETY: both calls take plain values and only read or set this process's
    // error mode and the machine's drive list. The error mode stops an empty card
    // reader from raising a "no disk" dialog while its drive is looked at.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS);
        GetLogicalDrives()
    }
}

#[cfg(windows)]
fn drive_kind(root: &str) -> u32 {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
    unsafe { GetDriveTypeW(wide.as_ptr()) }
}

#[cfg(not(windows))]
fn logical_drive_mask() -> u32 {
    0
}

#[cfg(not(windows))]
fn drive_kind(_root: &str) -> u32 {
    0
}

/// File systems of disks inside the computer or plugged into it.
fn is_disk_fs(fstype: &str) -> bool {
    matches!(
        fstype,
        "ext2"
            | "ext3"
            | "ext4"
            | "btrfs"
            | "xfs"
            | "f2fs"
            | "jfs"
            | "reiserfs"
            | "ntfs"
            | "ntfs3"
            | "fuseblk"
            | "exfat"
            | "vfat"
            | "msdos"
            | "hfsplus"
    )
}

/// Where desktop systems mount extra and removable disks. These folders are
/// skipped as system folders when walking `/`, so a disk mounted in one needs a
/// root of its own. Disks mounted anywhere else (`/data`, `/home`) are reached
/// from `/`.
fn is_extra_disk_spot(mount: &Path) -> bool {
    ["/mnt", "/media", "/run/media"]
        .iter()
        .any(|spot| mount.starts_with(spot) && mount != Path::new(spot))
}

/// Mount points of local disks that sit where [`is_extra_disk_spot`] says, from
/// the text of `/proc/mounts`.
pub fn local_disk_mounts(proc_mounts: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for line in proc_mounts.lines() {
        let mut fields = line.split_whitespace();
        let (Some(device), Some(mount), Some(fstype)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let mount = PathBuf::from(unescape_mount(mount));
        if device.starts_with("/dev/")
            && is_disk_fs(fstype)
            && is_extra_disk_spot(&mount)
            && !out.contains(&mount)
        {
            out.push(mount);
        }
    }
    out
}

/// File systems of disks plugged into a Mac (network shares show up in
/// `/Volumes` too, and a dead one can hang whatever touches it).
// Used on macOS only, but tested everywhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn is_local_mac_fs(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "apfs" | "hfs" | "exfat" | "msdos" | "ntfs" | "ufs"
    )
}

/// Disks plugged into a Mac, other than the startup disk (which appears in
/// `/Volumes` as a symlink to `/`).
#[cfg(target_os = "macos")]
fn mac_volumes() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir("/Volumes") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
            let path = entry.path();
            (kind.is_dir() && !kind.is_symlink() && is_local_mac_fs(&fs_type_name(&path)?))
                .then_some(path)
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn mac_volumes() -> Vec<PathBuf> {
    Vec::new()
}

/// The file system type of the volume at `path`, or `None` if it cannot be read
/// within two seconds (a stale network mount blocks `statfs`).
#[cfg(target_os = "macos")]
fn fs_type_name(path: &Path) -> Option<String> {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    use std::sync::mpsc;
    use std::time::Duration;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        // SAFETY: an all-zero `statfs` is a valid value to be filled in.
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c_path` is NUL-terminated and `stat` is valid for writes.
        let ok = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) } == 0;
        let name = ok.then(|| {
            // SAFETY: on success the kernel fills `f_fstypename` with a NUL-terminated name.
            let name = unsafe { CStr::from_ptr(stat.f_fstypename.as_ptr()) };
            name.to_string_lossy().into_owned()
        });
        let _ = sender.send(name);
    });
    receiver.recv_timeout(Duration::from_secs(2)).ok().flatten()
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
    /// Folders never entered, compared by exact path (ignoring case on Windows).
    skipped: HashSet<PathBuf>,
}

impl Profile {
    /// `fs_roots` are the filesystem roots the system folders hang off: `/`, or
    /// every drive on Windows, where each drive has its own recycle bin and
    /// system folders. Tests pass temporary directories standing in for them.
    pub fn new(os: Os, fs_roots: &[PathBuf], home: Option<&Path>) -> Profile {
        let canon = |p: PathBuf| crate::fsutil::canonical(&p).unwrap_or(p);
        let mut homes: Vec<PathBuf> = home.map(|h| canon(h.to_path_buf())).into_iter().collect();
        let mut home_parents = Vec::new();
        // Homes live on the main filesystem; only Windows has several roots to look in.
        let home_roots = if os == Os::Windows {
            fs_roots
        } else {
            &fs_roots[..fs_roots.len().min(1)]
        };
        for root in home_roots {
            match os {
                Os::Linux => {
                    home_parents.push(root.join("home"));
                    homes.push(root.join("root"));
                }
                Os::Mac | Os::Windows => home_parents.push(root.join("Users")),
            }
        }
        let mut profile = Profile {
            os,
            homes,
            home_parents: home_parents.into_iter().map(canon).collect(),
            skipped: HashSet::new(),
        };
        let system: Vec<PathBuf> = fs_roots
            .iter()
            .flat_map(|root| system_folders(os).iter().map(move |rel| root.join(rel)))
            .collect();
        let keys: Vec<PathBuf> = system.iter().map(|p| profile.key(p).into_owned()).collect();
        profile.skipped.extend(keys);
        profile
    }

    /// How a path is compared: Windows names ignore case (`$RECYCLE.BIN`). Only
    /// Windows pays for a copy; this runs for every directory a scan meets.
    fn key<'a>(&self, path: &'a Path) -> Cow<'a, Path> {
        if self.os == Os::Windows {
            Cow::Owned(PathBuf::from(path.to_string_lossy().to_lowercase()))
        } else {
            Cow::Borrowed(path)
        }
    }

    /// The profile for this machine, including network mounts.
    pub fn detect() -> Profile {
        let os = Os::current();
        let mut roots = if os == Os::Windows {
            windows_drives()
        } else {
            Vec::new()
        };
        let main = machine_root(os);
        if !roots.contains(&main) {
            roots.insert(0, main);
        }
        Profile::new(os, &roots, home_dir().as_deref()).skipping(read_network_mounts())
    }

    /// Also never enter these folders (exact paths).
    pub fn skipping(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Profile {
        let keys: Vec<PathBuf> = paths
            .into_iter()
            .map(|p| self.key(&p).into_owned())
            .collect();
        self.skipped.extend(keys);
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
        if self.skipped.contains(&*self.key(dir)) {
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
fn read_proc_mounts() -> String {
    fs::read_to_string("/proc/mounts").unwrap_or_default()
}

#[cfg(not(target_os = "linux"))]
fn read_proc_mounts() -> String {
    String::new()
}

fn read_network_mounts() -> Vec<PathBuf> {
    network_mount_points(&read_proc_mounts())
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
        Profile::new(os, std::slice::from_ref(&root.to_path_buf()), Some(&home))
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
            let p = Profile::new(os, std::slice::from_ref(&root), None);
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
        let p = Profile::new(Os::Linux, std::slice::from_ref(&root), None)
            .skipping([root.join("srv/nfs")]);
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
    }

    /// `GetDriveType`: 0 unknown, 1 no root, 2 removable, 3 fixed, 4 network, 5 optical, 6 RAM.
    fn kinds(root: &str) -> u32 {
        match root {
            "A:\\" => 2, // floppy or card reader
            "C:\\" => 3,
            "D:\\" => 3,
            "E:\\" => 2, // USB stick
            "F:\\" => 5, // DVD
            "Y:\\" => 6, // RAM disk
            "Z:\\" => 4, // network share
            _ => 0,
        }
    }

    #[test]
    fn every_fixed_and_removable_drive_is_searched_in_letter_order() {
        let mask = (1 << 0) | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 24) | (1 << 25);
        let drives = drive_roots(mask, kinds);
        assert_eq!(
            drives,
            [
                PathBuf::from("A:\\"),
                PathBuf::from("C:\\"),
                PathBuf::from("D:\\"),
                PathBuf::from("E:\\")
            ],
            "network, optical and RAM drives are left out"
        );
    }

    #[test]
    fn drives_that_are_not_in_the_mask_are_never_asked_about() {
        let asked = std::cell::RefCell::new(Vec::new());
        let drives = drive_roots(1 << 3, |root| {
            asked.borrow_mut().push(root.to_string());
            3
        });
        assert_eq!(drives, [PathBuf::from("D:\\")]);
        assert_eq!(*asked.borrow(), ["D:\\"]);
        assert!(drive_roots(0, |_| 3).is_empty());
        // bits above Z are not drives
        assert!(drive_roots(1 << 31, |_| 3).is_empty());
    }

    #[test]
    fn local_disks_in_the_usual_mount_places_get_their_own_root() {
        let mounts = "\
/dev/nvme0n1p2 / ext4 rw,relatime 0 0
/dev/nvme0n1p1 /boot/efi vfat rw 0 0
/dev/nvme0n1p3 /home ext4 rw 0 0
/dev/sdb1 /mnt/data ext4 rw 0 0
/dev/sdc1 /media/me/BACKUP\\040DISK ntfs3 rw 0 0
/dev/sdd1 /run/media/me/USB exfat rw 0 0
/dev/sde1 /media/me/old fuseblk rw 0 0
/dev/loop3 /snap/core/1 squashfs ro 0 0
server:/export /mnt/team nfs4 rw 0 0
//nas/share /media/nas cifs rw 0 0
/dev/sdf1 /mnt ext4 rw 0 0
tmpfs /run/media tmpfs rw 0 0
/dev/sdb1 /mnt/data ext4 rw 0 0
";
        assert_eq!(
            local_disk_mounts(mounts),
            [
                PathBuf::from("/mnt/data"),
                PathBuf::from("/media/me/BACKUP DISK"),
                PathBuf::from("/run/media/me/USB"),
                PathBuf::from("/media/me/old"),
            ],
            "the system disk, /boot/efi, /home, loop devices, network shares and a mount \
             of /mnt itself are not extra disks, and a repeated mount is listed once"
        );
        assert!(local_disk_mounts("").is_empty());
        assert!(local_disk_mounts("short line\n\n").is_empty());
    }

    #[test]
    fn only_local_mac_volumes_are_searched() {
        for local in ["apfs", "APFS", "hfs", "exfat", "msdos", "ntfs"] {
            assert!(is_local_mac_fs(local), "{local}");
        }
        for network in ["smbfs", "nfs", "afpfs", "webdav", "autofs", "devfs", ""] {
            assert!(!is_local_mac_fs(network), "{network}");
        }
    }

    #[test]
    fn the_scan_roots_override_replaces_the_disks() {
        let list = std::env::join_paths(["/srv/a", "/srv/b"]).unwrap();
        assert_eq!(
            roots_from_override(Some(list)),
            Some(vec![PathBuf::from("/srv/a"), PathBuf::from("/srv/b")])
        );
        assert_eq!(roots_from_override(Some("".into())), None);
        assert_eq!(roots_from_override(None), None);
    }

    #[test]
    fn the_home_folder_is_added_only_when_no_root_contains_it() {
        let home = Some(PathBuf::from("/home/me"));
        let covered = add_home_if_outside(vec![PathBuf::from("/")], home.clone());
        assert_eq!(covered, [PathBuf::from("/")]);
        let outside = add_home_if_outside(vec![PathBuf::from("/data")], home.clone());
        assert_eq!(outside, [PathBuf::from("/data"), PathBuf::from("/home/me")]);
        assert_eq!(
            add_home_if_outside(vec![PathBuf::from("/data")], None),
            [PathBuf::from("/data")]
        );
    }

    #[test]
    fn windows_system_folders_are_skipped_on_every_drive_whatever_their_case() {
        let (_t, first) = fake_root();
        let (_u, second) = fake_root();
        let roots = [first.clone(), second.clone()];
        let p = Profile::new(Os::Windows, &roots, None);
        for drive in [&first, &second] {
            // the recycle bin is really named $RECYCLE.BIN, and the config says $Recycle.Bin
            for name in [
                "$RECYCLE.BIN",
                "$Recycle.Bin",
                "WINDOWS",
                "windows",
                "Program Files",
            ] {
                assert!(p.skips(&drive.join(name)), "{name} on {}", drive.display());
            }
            assert!(!p.skips(&drive.join("Users")));
            assert!(
                !p.skips(&drive.join("dev/Windows")),
                "only directly under a drive"
            );
        }
        // Linux names are case-sensitive
        let linux = Profile::new(Os::Linux, &roots[..1], None);
        assert!(linux.skips(&first.join("usr")));
        assert!(!linux.skips(&first.join("USR")));
    }

    #[test]
    fn users_folders_on_any_windows_drive_count_as_homes() {
        let (_t, first) = fake_root();
        let (_u, second) = fake_root();
        let p = Profile::new(Os::Windows, &[first.clone(), second.clone()], None);
        assert!(p.skips(&second.join("Users/me/AppData")));
        assert!(p.skips(&first.join("Users/other/APPDATA")));
        assert!(p.is_home(&second.join("Users/me")));
    }

    #[test]
    fn extra_skipped_paths_ignore_case_on_windows_only() {
        let (_t, root) = fake_root();
        let win = Profile::new(Os::Windows, std::slice::from_ref(&root), None)
            .skipping([root.join("Backups")]);
        assert!(win.skips(&root.join("BACKUPS")));
        let linux = Profile::new(Os::Linux, std::slice::from_ref(&root), None)
            .skipping([root.join("Backups")]);
        assert!(!linux.skips(&root.join("BACKUPS")));
    }

    #[test]
    fn a_machine_with_no_roots_skips_nothing_and_does_not_panic() {
        let p = Profile::new(Os::Linux, &[], None);
        assert!(!p.skips(Path::new("/usr")));
        let w = Profile::new(Os::Windows, &[], None);
        assert!(!w.skips(Path::new("C:\\Windows")));
    }

    #[cfg(windows)]
    #[test]
    fn the_real_system_drive_is_among_the_drives_found() {
        let drives = windows_drives();
        let system = machine_root(Os::Windows);
        assert!(
            drives.iter().any(|d| d
                .to_string_lossy()
                .eq_ignore_ascii_case(&system.to_string_lossy())),
            "{drives:?} should include {system:?}"
        );
        assert!(system.is_dir(), "{system:?}");
        // an empty card reader is a drive too, so only the system drive must exist
        let roots = default_roots();
        assert!(!roots.is_empty());
        assert!(
            roots
                .iter()
                .all(|r| r.is_dir() && !r.to_string_lossy().starts_with("\\\\?\\")),
            "{roots:?}"
        );
    }
}
