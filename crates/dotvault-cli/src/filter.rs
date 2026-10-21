//! Filtering of indexed env files, shared by `ls` and `find`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use globset::{GlobBuilder, GlobMatcher};

use crate::index::FileRecord;

#[derive(Args, Clone, Default, Debug)]
pub struct FilterArgs {
    /// Only projects whose name contains TEXT (or matches a glob like 'acme-*')
    #[arg(short, long, value_name = "TEXT")]
    pub project: Vec<String>,

    /// Only this environment: default, local, production, ... (repeatable; globs allowed)
    #[arg(short, long, value_name = "NAME")]
    pub env: Vec<String>,

    /// Only files whose path contains TEXT (or matches a glob)
    #[arg(long, value_name = "TEXT")]
    pub path: Vec<String>,

    /// Include example/template files (.env.example, .env.sample, ...)
    #[arg(long)]
    pub examples: bool,

    /// Show only example/template files
    #[arg(long, conflicts_with = "examples")]
    pub only_examples: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Examples {
    Hide,
    Include,
    Only,
}

enum Pattern {
    /// Case-insensitive substring.
    Contains(String),
    /// Case-insensitive equality.
    Exact(String),
    Glob(GlobMatcher),
}

impl Pattern {
    fn new(text: &str, whole: bool) -> Result<Pattern> {
        if text.contains(['*', '?', '[', '{']) {
            let glob = GlobBuilder::new(text)
                .case_insensitive(true)
                .build()
                .with_context(|| format!("invalid pattern {text:?}"))?;
            Ok(Pattern::Glob(glob.compile_matcher()))
        } else if whole {
            Ok(Pattern::Exact(text.to_lowercase()))
        } else {
            Ok(Pattern::Contains(text.to_lowercase()))
        }
    }

    fn matches(&self, haystack: &str) -> bool {
        match self {
            Pattern::Contains(t) => haystack.to_lowercase().contains(t),
            Pattern::Exact(t) => haystack.to_lowercase() == *t,
            Pattern::Glob(g) => g.is_match(haystack),
        }
    }
}

pub struct Filter {
    projects: Vec<Pattern>,
    envs: Vec<Pattern>,
    paths: Vec<Pattern>,
    keys: Vec<Pattern>,
    examples: Examples,
    under: Option<PathBuf>,
}

impl Filter {
    pub fn new(args: &FilterArgs) -> Result<Filter> {
        let build = |items: &[String], whole: bool| -> Result<Vec<Pattern>> {
            items.iter().map(|t| Pattern::new(t, whole)).collect()
        };
        Ok(Filter {
            projects: build(&args.project, false)?,
            envs: build(&args.env, true)?,
            paths: build(&args.path, false)?,
            keys: Vec::new(),
            examples: if args.only_examples {
                Examples::Only
            } else if args.examples {
                Examples::Include
            } else {
                Examples::Hide
            },
            under: None,
        })
    }

    /// Only files that define a key matching one of `keys` (exact name or glob).
    pub fn with_keys(mut self, keys: &[String]) -> Result<Filter> {
        self.keys = keys
            .iter()
            .map(|t| Pattern::new(t, true))
            .collect::<Result<_>>()?;
        Ok(self)
    }

    /// Only files inside this directory.
    pub fn under(mut self, dir: PathBuf) -> Filter {
        self.under = Some(dir);
        self
    }

    pub fn matches(&self, rec: &FileRecord) -> bool {
        match (self.examples, rec.is_example) {
            (Examples::Hide, true) | (Examples::Only, false) => return false,
            _ => {}
        }
        if let Some(dir) = &self.under {
            if !rec.path.starts_with(dir) {
                return false;
            }
        }
        if !any_or_empty(&self.projects, |p| p.matches(&rec.project)) {
            return false;
        }
        if !any_or_empty(&self.envs, |p| p.matches(&rec.label)) {
            return false;
        }
        if !any_or_empty(&self.paths, |p| {
            p.matches(&rec.path.to_string_lossy().replace('\\', "/"))
        }) {
            return false;
        }
        any_or_empty(&self.keys, |p| rec.keys.iter().any(|k| p.matches(k)))
    }
}

fn any_or_empty(patterns: &[Pattern], f: impl Fn(&Pattern) -> bool) -> bool {
    patterns.is_empty() || patterns.iter().any(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(project: &str, label: &str, path: &str, keys: &[&str]) -> FileRecord {
        FileRecord {
            path: PathBuf::from(path),
            project: project.into(),
            project_root: PathBuf::from("/r").join(project),
            label: label.into(),
            is_example: label.contains("example"),
            size: 0,
            mtime: 0,
            keys: keys.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn filter(f: impl FnOnce(&mut FilterArgs)) -> Filter {
        let mut args = FilterArgs::default();
        f(&mut args);
        Filter::new(&args).unwrap()
    }

    #[test]
    fn hides_examples_by_default() {
        let api = rec("api", "default", "/r/api/.env", &[]);
        let ex = rec("api", "example", "/r/api/.env.example", &[]);
        let f = filter(|_| {});
        assert!(f.matches(&api) && !f.matches(&ex));
        let f = filter(|a| a.examples = true);
        assert!(f.matches(&api) && f.matches(&ex));
        let f = filter(|a| a.only_examples = true);
        assert!(!f.matches(&api) && f.matches(&ex));
    }

    #[test]
    fn project_is_substring_or_glob() {
        let r = rec("acme-api", "default", "/r/acme-api/.env", &[]);
        assert!(filter(|a| a.project = vec!["API".into()]).matches(&r));
        assert!(filter(|a| a.project = vec!["acme-*".into()]).matches(&r));
        assert!(!filter(|a| a.project = vec!["web".into()]).matches(&r));
        // multiple values are alternatives
        assert!(filter(|a| a.project = vec!["web".into(), "api".into()]).matches(&r));
    }

    #[test]
    fn env_is_exact_unless_glob() {
        let prod = rec("a", "production", "/r/a/.env.production", &[]);
        let prod_local = rec("a", "production.local", "/r/a/.env.production.local", &[]);
        let f = filter(|a| a.env = vec!["production".into()]);
        assert!(f.matches(&prod) && !f.matches(&prod_local));
        let f = filter(|a| a.env = vec!["production*".into()]);
        assert!(f.matches(&prod) && f.matches(&prod_local));
        let f = filter(|a| a.env = vec!["Production".into()]);
        assert!(f.matches(&prod), "case-insensitive");
    }

    #[test]
    fn path_and_under_and_keys() {
        let r = rec("a", "default", "/home/me/code/a/.env", &["DB_URL", "PORT"]);
        assert!(filter(|a| a.path = vec!["code/a".into()]).matches(&r));
        assert!(filter(|a| a.path = vec!["**/code/**".into()]).matches(&r));
        assert!(!filter(|a| a.path = vec!["work".into()]).matches(&r));

        assert!(filter(|_| {}).under(PathBuf::from("/home/me")).matches(&r));
        assert!(!filter(|_| {}).under(PathBuf::from("/srv")).matches(&r));

        let has = |k: &str| {
            filter(|_| {})
                .with_keys(&[k.to_string()])
                .unwrap()
                .matches(&r)
        };
        assert!(has("DB_URL") && has("db_url") && has("DB_*") && has("*PORT"));
        assert!(!has("DB") && !has("SECRET"));
    }

    #[test]
    fn rejects_bad_globs() {
        let args = FilterArgs {
            project: vec!["[unclosed*".into()],
            ..FilterArgs::default()
        };
        assert!(Filter::new(&args).is_err());
    }
}
