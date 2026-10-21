//! Decides which file names are env files and how to label them.

use std::path::{Path, PathBuf};

/// Files whose presence marks the root of a project.
pub const PROJECT_MARKERS: &[&str] = &[
    ".git",
    "package.json",
    "Cargo.toml",
    "composer.json",
    "go.mod",
    "pyproject.toml",
    "pom.xml",
    "Gemfile",
    "mix.exs",
];

/// Name segments that mark an env file as a template rather than real config.
pub const DEFAULT_EXAMPLE_SUFFIXES: &[&str] = &["example", "sample", "template", "dist"];

/// Editor swap and scratch files that sit next to real env files.
const NOISE_SUFFIXES: &[&str] = &["swp", "swo", "swx", "tmp"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvFileInfo {
    /// `default` for a bare `.env`, otherwise the part after `.env.`
    /// (`production`, `local`, `production.local`, ...).
    pub label: String,
    pub is_example: bool,
}

#[derive(Debug, Clone)]
pub struct ClassifyOptions {
    pub example_suffixes: Vec<String>,
}

impl Default for ClassifyOptions {
    fn default() -> Self {
        Self {
            example_suffixes: DEFAULT_EXAMPLE_SUFFIXES
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// Returns `Some` when `file_name` is `.env` or `.env.<something>`.
pub fn classify(file_name: &str, opts: &ClassifyOptions) -> Option<EnvFileInfo> {
    let suffix = if file_name == ".env" {
        ""
    } else {
        let s = file_name.strip_prefix(".env.")?;
        if s.is_empty() {
            return None;
        }
        s
    };

    let last_segment = suffix.rsplit('.').next().unwrap_or_default();
    if suffix.ends_with('~')
        || NOISE_SUFFIXES
            .iter()
            .any(|n| n.eq_ignore_ascii_case(last_segment))
    {
        return None;
    }

    let is_example = suffix.split('.').any(|seg| {
        opts.example_suffixes
            .iter()
            .any(|ex| ex.eq_ignore_ascii_case(seg))
    });

    let label = if suffix.is_empty() { "default" } else { suffix };
    Some(EnvFileInfo {
        label: label.to_string(),
        is_example,
    })
}

/// Walks up from `start` to the nearest directory for which `has_marker`
/// returns true. The filesystem check is injected so this stays pure (and
/// may cache, hence `FnMut`).
pub fn find_project_root(
    start: &Path,
    mut has_marker: impl FnMut(&Path) -> bool,
) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| has_marker(dir))
        .map(Path::to_path_buf)
}

/// Display name for a project rooted at `root` (its directory name), falling
/// back to the whole path for roots like `/`.
pub fn project_name(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str) -> Option<EnvFileInfo> {
        classify(name, &ClassifyOptions::default())
    }

    #[test]
    fn matches_env_family() {
        assert_eq!(c(".env").unwrap().label, "default");
        assert_eq!(c(".env.local").unwrap().label, "local");
        assert_eq!(c(".env.production").unwrap().label, "production");
        assert_eq!(c(".env.development").unwrap().label, "development");
        assert_eq!(c(".env.live").unwrap().label, "live");
        assert_eq!(
            c(".env.production.local").unwrap().label,
            "production.local"
        );
    }

    #[test]
    fn rejects_non_env_files() {
        assert!(c("env").is_none());
        assert!(c(".envrc").is_none());
        assert!(c(".env.").is_none());
        assert!(c("production.env").is_none());
        assert!(c("config.json").is_none());
        assert!(c(".environment").is_none());
    }

    #[test]
    fn ignores_editor_noise() {
        assert!(c(".env.swp").is_none());
        assert!(c(".env.production.swo").is_none());
        assert!(c(".env.local~").is_none());
        assert!(c(".env.tmp").is_none());
        // backups are still env files the user may care about
        assert!(c(".env.bak").is_some());
    }

    #[test]
    fn flags_examples() {
        assert!(c(".env.example").unwrap().is_example);
        assert!(c(".env.sample").unwrap().is_example);
        assert!(c(".env.template").unwrap().is_example);
        assert!(c(".env.dist").unwrap().is_example);
        assert!(c(".env.local.example").unwrap().is_example);
        assert!(c(".env.EXAMPLE").unwrap().is_example);
        assert!(!c(".env").unwrap().is_example);
        assert!(!c(".env.production").unwrap().is_example);
        assert!(!c(".env.examples").unwrap().is_example);
    }

    #[test]
    fn custom_example_suffixes() {
        let opts = ClassifyOptions {
            example_suffixes: vec!["tpl".into()],
        };
        assert!(classify(".env.tpl", &opts).unwrap().is_example);
        assert!(!classify(".env.example", &opts).unwrap().is_example);
    }

    #[test]
    fn finds_nearest_project_root() {
        let root = Path::new("/work/acme");
        let found = find_project_root(Path::new("/work/acme/services/api"), |d| d == root);
        assert_eq!(found.as_deref(), Some(root));
        assert_eq!(find_project_root(Path::new("/x/y"), |_| false), None);
    }

    #[test]
    fn names_projects() {
        assert_eq!(project_name(Path::new("/work/acme")), "acme");
        assert_eq!(project_name(Path::new("/")), "/");
    }
}
