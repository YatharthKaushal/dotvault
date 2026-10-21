//! Turns what the user typed (`./.env`, `~/x/.env.local`, `api`, `api:production`)
//! into one file.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::index::FileRecord;
use crate::output::tilde;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Path(PathBuf),
    Project { name: String, env: Option<String> },
}

/// `project` or `project:env` when the text has no path characters and no
/// such file exists; anything else is a path.
pub fn parse_target(text: &str) -> Target {
    let looks_like_path =
        text.contains(['/', '\\']) || text.starts_with(['.', '~']) || Path::new(text).is_file();
    if !looks_like_path && text.matches(':').count() <= 1 {
        let (name, env) = match text.split_once(':') {
            Some((n, e)) if !n.is_empty() && !e.is_empty() => (n, Some(e.to_string())),
            Some(_) => return Target::Path(PathBuf::from(text)),
            None => (text, None),
        };
        return Target::Project {
            name: name.to_string(),
            env,
        };
    }
    Target::Path(expand_tilde(text))
}

pub fn expand_tilde(text: &str) -> PathBuf {
    if text == "~" || text.starts_with("~/") || text.starts_with("~\\") {
        if let Some(home) = crate::discover::home_dir() {
            return home.join(text[1..].trim_start_matches(['/', '\\']));
        }
    }
    PathBuf::from(text)
}

/// Picks the single file a `project[:env]` selector refers to.
///
/// Without an env, example files are not candidates and a bare `.env`
/// (label `default`) wins over other files. Naming an env explicitly can
/// reach example files too.
pub fn pick<'a>(
    records: &'a [FileRecord],
    name: &str,
    env: Option<&str>,
    include_examples: bool,
) -> Result<&'a FileRecord> {
    let in_project: Vec<&FileRecord> = records
        .iter()
        .filter(|r| r.project.eq_ignore_ascii_case(name))
        .collect();

    if in_project.is_empty() {
        let near: Vec<&str> = {
            let needle = name.to_lowercase();
            let mut v: Vec<&str> = records
                .iter()
                .filter(|r| r.project.to_lowercase().contains(&needle))
                .map(|r| r.project.as_str())
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        if near.is_empty() {
            bail!("no project named {name:?}; run `dotvault ls` to see what is indexed (new folders need `dotvault scan <dir>`)");
        }
        bail!(
            "no project named {name:?}; did you mean: {}?",
            near.join(", ")
        );
    }

    let candidates: Vec<&FileRecord> = in_project
        .iter()
        .copied()
        .filter(|r| match env {
            Some(e) => r.label.eq_ignore_ascii_case(e),
            None => include_examples || !r.is_example,
        })
        .collect();

    let mut roots: Vec<&Path> = candidates
        .iter()
        .map(|r| r.project_root.as_path())
        .collect();
    roots.sort_unstable();
    roots.dedup();
    if roots.len() > 1 {
        bail!(
            "{} projects are named {name:?}; pass a path instead:\n{}",
            roots.len(),
            candidates
                .iter()
                .map(|r| format!("  {}", tilde(&r.path)))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    match candidates.as_slice() {
        [] => {
            let envs = available_envs(&in_project, name);
            match env {
                Some(e) => bail!("project {name:?} has no {e:?} environment; available: {envs}"),
                None => bail!("project {name:?} has only example files; name one explicitly, e.g. {name}:example (available: {envs})"),
            }
        }
        [only] => Ok(only),
        many => {
            if env.is_none() {
                if let Some(default) = many.iter().find(|r| r.label == "default") {
                    return Ok(default);
                }
            }
            bail!(
                "project {name:?} has several environments; pick one: {}",
                available_envs(many, name)
            )
        }
    }
}

fn available_envs(records: &[&FileRecord], name: &str) -> String {
    let mut labels: Vec<String> = records
        .iter()
        .map(|r| format!("{name}:{}", r.label))
        .collect();
    labels.sort();
    labels.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(project: &str, root: &str, label: &str) -> FileRecord {
        FileRecord {
            path: PathBuf::from(root).join(if label == "default" {
                ".env".to_string()
            } else {
                format!(".env.{label}")
            }),
            project: project.into(),
            project_root: PathBuf::from(root),
            label: label.into(),
            is_example: label.contains("example"),
            size: 0,
            mtime: 0,
            keys: vec![],
        }
    }

    #[test]
    fn parses_targets() {
        assert_eq!(
            parse_target("api"),
            Target::Project {
                name: "api".into(),
                env: None
            }
        );
        assert_eq!(
            parse_target("api:production"),
            Target::Project {
                name: "api".into(),
                env: Some("production".into())
            }
        );
        assert_eq!(
            parse_target("./.env"),
            Target::Path(PathBuf::from("./.env"))
        );
        assert_eq!(
            parse_target(".env.local"),
            Target::Path(PathBuf::from(".env.local"))
        );
        assert_eq!(
            parse_target("a/b/.env"),
            Target::Path(PathBuf::from("a/b/.env"))
        );
        assert!(matches!(parse_target("a:b:c"), Target::Path(_)));
        assert!(matches!(parse_target("api:"), Target::Path(_)));
        assert!(matches!(parse_target("C:\\x\\.env"), Target::Path(_)));
    }

    #[test]
    fn existing_files_win_over_selectors() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("api");
        std::fs::write(&file, "").unwrap();
        assert_eq!(
            parse_target(file.to_str().unwrap()),
            Target::Path(file.clone())
        );
    }

    #[test]
    fn picks_default_when_env_omitted() {
        let recs = vec![
            rec("api", "/r/api", "default"),
            rec("api", "/r/api", "production"),
            rec("api", "/r/api", "example"),
        ];
        assert_eq!(pick(&recs, "api", None, false).unwrap().label, "default");
        assert_eq!(
            pick(&recs, "API", Some("Production"), false).unwrap().label,
            "production"
        );
        // explicit env reaches example files
        assert_eq!(
            pick(&recs, "api", Some("example"), false).unwrap().label,
            "example"
        );
    }

    #[test]
    fn single_non_default_file_is_chosen() {
        let recs = vec![
            rec("web", "/r/web", "local"),
            rec("web", "/r/web", "example"),
        ];
        assert_eq!(pick(&recs, "web", None, false).unwrap().label, "local");
    }

    #[test]
    fn ambiguous_environments_are_reported() {
        let recs = vec![
            rec("web", "/r/web", "local"),
            rec("web", "/r/web", "production"),
        ];
        let err = pick(&recs, "web", None, false).unwrap_err().to_string();
        assert!(
            err.contains("web:local") && err.contains("web:production"),
            "{err}"
        );
    }

    #[test]
    fn same_name_in_two_places_is_ambiguous() {
        let recs = vec![
            rec("api", "/r/a/api", "default"),
            rec("api", "/r/b/api", "default"),
        ];
        let err = pick(&recs, "api", None, false).unwrap_err().to_string();
        assert!(
            err.contains("2 projects") && err.contains("/r/a/api"),
            "{err}"
        );
    }

    #[test]
    fn helpful_errors() {
        let recs = vec![rec("acme-api", "/r/acme-api", "default")];
        let err = pick(&recs, "acme", None, false).unwrap_err().to_string();
        assert!(err.contains("did you mean: acme-api"), "{err}");
        let err = pick(&recs, "zzz", None, false).unwrap_err().to_string();
        assert!(err.contains("dotvault scan"), "{err}");
        let err = pick(&recs, "acme-api", Some("staging"), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("acme-api:default"), "{err}");
        let only_example = vec![rec("x", "/r/x", "example")];
        let err = pick(&only_example, "x", None, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("x:example"), "{err}");
    }
}
