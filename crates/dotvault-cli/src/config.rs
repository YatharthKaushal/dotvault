use std::path::PathBuf;

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
    pub max_depth: usize,
    /// Rescan automatically when the index is older than this many seconds.
    pub auto_scan_secs: u64,
    /// Extra directory names to skip, on top of the built-in list.
    pub ignore: Vec<String>,
    /// File-name segments that mark an env file as an example/template.
    pub example_suffixes: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server: None,
            roots: Vec::new(),
            max_depth: 10,
            auto_scan_secs: 600,
            ignore: Vec::new(),
            example_suffixes: DEFAULT_EXAMPLE_SUFFIXES
                .iter()
                .map(|s| s.to_string())
                .collect(),
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

    /// Everything that changes which files a scan finds. When it changes the
    /// index is rebuilt.
    pub fn scan_signature(&self) -> String {
        format!(
            "{:?}|{}|{:?}|{:?}",
            self.roots, self.max_depth, self.ignore, self.example_suffixes
        )
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
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}
