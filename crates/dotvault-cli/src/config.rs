use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use dotvault_core::classify::DEFAULT_EXAMPLE_SUFFIXES;
use serde::{Deserialize, Serialize};

use crate::fsutil;

/// Directories never descended into while scanning.
pub const DEFAULT_IGNORE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    ".hg",
    ".svn",
    "target",
    "vendor",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".nuxt",
    ".cache",
    ".tox",
    ".gradle",
    ".idea",
    ".vscode",
    ".terraform",
    "bower_components",
    ".pnpm-store",
    ".yarn",
    ".npm",
    ".cargo",
    ".rustup",
    ".nvm",
    ".pyenv",
    "Pods",
    "DerivedData",
    ".Trash",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Base URL of the dotvault server used for cloud features.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// Folders searched for env files.
    pub roots: Vec<PathBuf>,
    /// Who chose `roots`: `true` means dotvault did (the whole computer, kept up to
    /// date as drives come and go), `false` means you did (never touched). Missing
    /// in files written before this key existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots_auto: Option<bool>,
    pub max_depth: usize,
    /// Rescan automatically when the index is older than this many seconds.
    pub auto_scan_secs: u64,
    /// Extra directory names to skip, on top of the built-in list.
    pub ignore: Vec<String>,
    /// File-name segments that mark an env file as an example/template.
    pub example_suffixes: Vec<String>,
    /// Keep a history of every variable dotvault changes (`dotvault log`). It holds
    /// old and current values, so turn it off here if you do not want that.
    pub history: bool,
    /// How many versions of each variable the history keeps.
    pub history_keep: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server: None,
            roots: Vec::new(),
            roots_auto: None,
            max_depth: 10,
            auto_scan_secs: 600,
            ignore: Vec::new(),
            example_suffixes: DEFAULT_EXAMPLE_SUFFIXES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            history: true,
            history_keep: 50,
        }
    }
}

impl Config {
    pub fn load(dirs: &Dirs) -> Result<Config> {
        let path = dirs.config_file();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("invalid config file {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    pub fn save(&self, dirs: &Dirs) -> Result<()> {
        let path = dirs.config_file();
        std::fs::create_dir_all(&dirs.config)
            .with_context(|| format!("cannot create {}", dirs.config.display()))?;
        let text = toml::to_string_pretty(self).context("cannot serialize config")?;
        fsutil::write_atomic(&path, text.as_bytes())
            .with_context(|| format!("cannot write {}", path.display()))
    }

    /// What to do with the saved folders before a command that needs them.
    pub fn plan_roots(&self, default: &[PathBuf], home: Option<&Path>) -> RootsPlan {
        plan_roots(&self.roots, self.roots_auto, default, home)
    }

    /// The folders were named by the user, so dotvault stops managing them.
    pub fn mark_roots_explicit(&mut self) {
        self.roots_auto = Some(false);
    }

    /// Everything that changes which files a scan finds. When it changes the
    /// index is rebuilt.
    pub fn scan_signature(&self) -> String {
        format!(
            "{:?}|{}|{:?}|{:?}",
            self.roots, self.max_depth, self.ignore, self.example_suffixes
        )
    }
}

/// What [`plan_roots`] decided.
#[derive(Debug, PartialEq, Eq)]
pub enum RootsPlan {
    /// Leave the saved folders alone.
    Keep,
    /// Nothing is saved yet: start with these.
    Start(Vec<PathBuf>),
    /// dotvault chose the saved folders and the computer's disks changed.
    Refresh(Vec<PathBuf>),
    /// An older version saved only the home folder as its default: search the
    /// whole computer now, like a new install does.
    Widen(Vec<PathBuf>),
}

/// Decides whether the saved `roots` should be replaced by the current `default`.
///
/// Folders the user chose (`auto == Some(false)`) are never touched. Folders
/// dotvault chose (`Some(true)`) follow the computer. An older version saved
/// `[home]` as its automatic default without saying so (`None`); that exact value
/// is treated as automatic once, and anything else in an older file is the user's.
pub fn plan_roots(
    roots: &[PathBuf],
    auto: Option<bool>,
    default: &[PathBuf],
    home: Option<&Path>,
) -> RootsPlan {
    if default.is_empty() {
        return RootsPlan::Keep;
    }
    if roots.is_empty() {
        return RootsPlan::Start(default.to_vec());
    }
    match auto {
        Some(true) if roots != default => RootsPlan::Refresh(default.to_vec()),
        None if roots != default && roots.len() == 1 && home == Some(roots[0].as_path()) => {
            RootsPlan::Widen(default.to_vec())
        }
        _ => RootsPlan::Keep,
    }
}

#[derive(Debug, Clone)]
pub struct Dirs {
    pub config: PathBuf,
    pub data: PathBuf,
}

impl Dirs {
    /// `DOTVAULT_CONFIG_DIR` / `DOTVAULT_DATA_DIR` override the platform defaults.
    pub fn detect() -> Result<Dirs> {
        let project = ProjectDirs::from("", "", "dotvault");
        let config = env_dir("DOTVAULT_CONFIG_DIR")
            .or_else(|| project.as_ref().map(|p| p.config_dir().to_path_buf()))
            .context("cannot determine a config directory; set DOTVAULT_CONFIG_DIR")?;
        let data = env_dir("DOTVAULT_DATA_DIR")
            .or_else(|| project.as_ref().map(|p| p.data_dir().to_path_buf()))
            .unwrap_or_else(|| config.clone());
        Ok(Dirs { config, data })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    pub fn index_file(&self) -> PathBuf {
        self.data.join("index.db")
    }

    /// Unlike the index this is not a cache and is never deleted on its own.
    pub fn history_file(&self) -> PathBuf {
        self.data.join("history.db")
    }
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(items: &[&str]) -> Vec<PathBuf> {
        items.iter().map(PathBuf::from).collect()
    }

    const HOME: &str = "/home/me";

    fn plan(roots: &[&str], auto: Option<bool>, default: &[&str]) -> RootsPlan {
        plan_roots(&paths(roots), auto, &paths(default), Some(Path::new(HOME)))
    }

    #[test]
    fn nothing_saved_starts_with_the_default_whatever_the_flag_says() {
        for auto in [None, Some(true), Some(false)] {
            assert_eq!(
                plan(&[], auto, &["/", "/mnt/data"]),
                RootsPlan::Start(paths(&["/", "/mnt/data"])),
                "{auto:?}"
            );
        }
    }

    #[test]
    fn with_no_default_to_offer_nothing_changes() {
        assert_eq!(plan(&[], None, &[]), RootsPlan::Keep);
        assert_eq!(plan(&[HOME], Some(true), &[]), RootsPlan::Keep);
    }

    #[test]
    fn folders_dotvault_chose_follow_the_computer() {
        assert_eq!(plan(&["/"], Some(true), &["/"]), RootsPlan::Keep);
        assert_eq!(
            plan(&["/"], Some(true), &["/", "/media/me/USB"]),
            RootsPlan::Refresh(paths(&["/", "/media/me/USB"]))
        );
        // a drive that was unplugged disappears
        assert_eq!(
            plan(&["/", "/media/me/USB"], Some(true), &["/"]),
            RootsPlan::Refresh(paths(&["/"]))
        );
    }

    #[test]
    fn folders_the_user_chose_are_never_touched() {
        assert_eq!(plan(&["/srv/app"], Some(false), &["/"]), RootsPlan::Keep);
        // even when they are exactly the old default
        assert_eq!(plan(&[HOME], Some(false), &["/"]), RootsPlan::Keep);
    }

    #[test]
    fn the_home_folder_an_older_version_saved_is_widened_once() {
        assert_eq!(
            plan(&[HOME], None, &["C:\\", "D:\\"]),
            RootsPlan::Widen(paths(&["C:\\", "D:\\"]))
        );
        // already the default: nothing to do
        assert_eq!(plan(&[HOME], None, &[HOME]), RootsPlan::Keep);
    }

    #[test]
    fn other_folders_in_an_older_file_are_the_users() {
        assert_eq!(plan(&["/srv/app"], None, &["/"]), RootsPlan::Keep);
        assert_eq!(plan(&[HOME, "/srv/app"], None, &["/"]), RootsPlan::Keep);
        assert_eq!(plan(&["/home/other"], None, &["/"]), RootsPlan::Keep);
    }

    #[test]
    fn without_a_known_home_an_older_file_is_left_alone() {
        let plan = plan_roots(&paths(&[HOME]), None, &paths(&["/"]), None);
        assert_eq!(plan, RootsPlan::Keep);
    }

    #[test]
    fn the_flag_round_trips_and_an_old_file_has_none() {
        let old: Config = toml::from_str("roots = [\"/home/me\"]\n").unwrap();
        assert_eq!(old.roots_auto, None);
        let mut config = Config::default();
        assert!(!toml::to_string_pretty(&config)
            .unwrap()
            .contains("roots_auto"));
        config.roots_auto = Some(true);
        let text = toml::to_string_pretty(&config).unwrap();
        assert!(text.contains("roots_auto = true"), "{text}");
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.roots_auto, Some(true));
        config.mark_roots_explicit();
        assert_eq!(config.roots_auto, Some(false));
    }
}
