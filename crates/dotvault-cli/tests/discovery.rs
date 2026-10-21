//! First-run discovery: with no folders configured, commands that need the list
//! of env files look in the home folder on their own. A fixture folder stands in
//! for home through `DOTVAULT_HOME`, which also works on Windows, where the OS
//! decides where the real one is.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

const NOTICE: &str = "scanning your home folder";

fn write(root: &Path, rel: &str, contents: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, contents).unwrap();
}

struct Machine {
    _tmp: TempDir,
    home: PathBuf,
    config: PathBuf,
    data: PathBuf,
}

impl Machine {
    /// A home folder with real projects, a loose folder, and the kinds of
    /// folders a broad scan must leave alone.
    fn new() -> Machine {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        write(&home, "code/acme-api/package.json", "{}");
        write(&home, "code/acme-api/.env", "API_KEY=1\nPORT=3000\n");
        write(&home, "code/acme-api/.env.production", "API_KEY=2\n");
        write(&home, "code/acme-api/node_modules/dep/.env", "LEAK=1\n");
        write(&home, "code/web/package.json", "{}");
        write(&home, "code/web/.env.local", "WEB=1\n");
        write(&home, "Documents/notes/.env", "NOTES=1\n");
        write(&home, ".config/tool/.env", "HIDDEN=1\n");
        write(&home, ".local/share/app/.env", "HIDDEN=2\n");
        write(&home, "Pictures/trip/.env", "MEDIA=1\n");
        Machine {
            config: tmp.path().join("config"),
            data: tmp.path().join("data"),
            home,
            _tmp: tmp,
        }
    }

    fn dv(&self) -> Command {
        let mut cmd = cargo_bin_cmd!("dotvault");
        cmd.env("DOTVAULT_HOME", &self.home)
            .env("DOTVAULT_CONFIG_DIR", &self.config)
            .env("DOTVAULT_DATA_DIR", &self.data)
            .env("NO_COLOR", "1")
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            .current_dir(&self.home);
        cmd
    }

    /// The paths `ls --paths` prints, with `/` separators.
    fn listed(&self) -> Vec<String> {
        let out = self
            .dv()
            .args(["ls", "--paths"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| l.replace('\\', "/"))
            .collect()
    }

    fn config_text(&self) -> String {
        fs::read_to_string(self.config.join("config.toml")).unwrap_or_default()
    }
}

fn has(lines: &[String], suffix: &str) -> bool {
    lines.iter().any(|l| l.ends_with(suffix))
}

fn expect_the_real_projects(lines: &[String]) {
    assert_eq!(lines.len(), 4, "{lines:#?}");
    for suffix in [
        "code/acme-api/.env",
        "code/acme-api/.env.production",
        "code/web/.env.local",
        "Documents/notes/.env",
    ] {
        assert!(has(lines, suffix), "missing {suffix} in {lines:#?}");
    }
    for noise in [".config", ".local", "Pictures", "node_modules"] {
        let folder = format!("/{noise}/");
        assert!(
            !lines.iter().any(|l| l.contains(&folder)),
            "{noise} must not be listed: {lines:#?}"
        );
    }
}

#[test]
fn the_first_ls_finds_env_files_in_the_home_folder_with_no_setup() {
    let m = Machine::new();
    m.dv()
        .args(["ls", "--paths"])
        .assert()
        .success()
        .stderr(predicate::str::contains(NOTICE));
    expect_the_real_projects(&m.listed());

    // remembered: no notice the second time, same answer
    m.dv()
        .args(["ls", "--paths"])
        .assert()
        .success()
        .stderr(predicate::str::contains(NOTICE).not());
    assert!(m.config_text().contains("roots"), "{}", m.config_text());
}

#[test]
fn a_bare_scan_starts_with_the_home_folder() {
    let m = Machine::new();
    m.dv()
        .arg("scan")
        .assert()
        .success()
        .stderr(predicate::str::contains(NOTICE))
        .stdout(predicate::str::contains("4 env files in 3 projects"));
    expect_the_real_projects(&m.listed());
}

#[test]
fn a_project_name_as_the_first_argument_also_discovers() {
    let m = Machine::new();
    m.dv()
        .args(["show", "acme-api", "--reveal", "--keys", "API_KEY"])
        .assert()
        .success()
        .stderr(predicate::str::contains(NOTICE))
        .stdout(predicate::str::contains("API_KEY"));
}

#[test]
fn find_discovers_too() {
    let m = Machine::new();
    m.dv()
        .args(["find", "API_KEY"])
        .assert()
        .success()
        .stderr(predicate::str::contains(NOTICE))
        .stdout(predicate::str::contains("acme-api"));
}

#[test]
fn a_command_given_a_path_never_triggers_a_scan() {
    let m = Machine::new();
    let project = m.home.join("code/acme-api");
    for args in [
        vec!["show", "./.env"],
        vec!["get", "PORT", "-f", "./.env"],
        vec!["show"],
    ] {
        m.dv()
            .current_dir(&project)
            .args(&args)
            .assert()
            .success()
            .stderr(predicate::str::contains(NOTICE).not());
    }
    assert!(!m.config.exists(), "no config should have been written");
    assert!(!m.data.exists(), "no index should have been written");
}

#[test]
fn naming_a_folder_first_keeps_only_that_folder() {
    let m = Machine::new();
    m.dv().args(["scan", "code/web"]).assert().success();
    let lines = m.listed();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(has(&lines, "code/web/.env.local"));
    m.dv()
        .args(["ls", "--paths"])
        .assert()
        .stderr(predicate::str::contains(NOTICE).not());
}

#[test]
fn removing_the_home_folder_sticks_but_removing_the_last_root_starts_over() {
    let m = Machine::new();
    m.dv().args(["ls", "--paths"]).assert().success(); // discovers home
    m.dv().args(["scan", "code/web"]).assert().success(); // adds a folder

    m.dv().args(["roots", "rm", "."]).assert().success(); // drops home, keeps web
    let lines = m.listed();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(has(&lines, "code/web/.env.local"));
    m.dv()
        .args(["ls", "--paths"])
        .assert()
        .stderr(predicate::str::contains(NOTICE).not());

    m.dv().args(["roots", "rm", "code/web"]).assert().success(); // nothing left
    m.dv()
        .args(["ls", "--paths"])
        .assert()
        .success()
        .stderr(predicate::str::contains(NOTICE));
    expect_the_real_projects(&m.listed());
}

#[test]
fn json_output_stays_clean_when_the_first_run_discovers() {
    let m = Machine::new();
    let out = m
        .dv()
        .args(["ls", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(NOTICE), "{stderr}");
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout must be pure JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    assert_eq!(parsed.as_array().unwrap().len(), 4);
}

#[test]
fn a_dotfiles_repository_in_home_does_not_swallow_the_projects() {
    let m = Machine::new();
    write(&m.home, ".git/HEAD", "ref: refs/heads/main\n");
    write(&m.home, "package.json", "{}");
    write(&m.home, "Downloads/foo/.env", "FOO=1\n");

    let out = m
        .dv()
        .args(["ls", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Vec<serde_json::Value> = serde_json::from_slice(&out).unwrap();
    let projects: Vec<&str> = parsed
        .iter()
        .map(|f| f["project"].as_str().unwrap())
        .collect();
    assert!(projects.contains(&"foo"), "{projects:?}");
    assert!(
        !projects.contains(&"home"),
        "files must not be filed under the home folder's own name: {projects:?}"
    );
}

#[test]
fn no_output_shows_a_windows_verbatim_path_prefix() {
    // On Windows the standard canonicalize adds `\\?\`, which would appear in every
    // path and stop the home folder from being shortened to `~`.
    let m = Machine::new();
    for args in [
        vec!["ls"],
        vec!["ls", "--paths"],
        vec!["ls", "--json"],
        vec!["scan"],
        vec!["roots", "ls"],
    ] {
        let out = m.dv().args(&args).assert().success().get_output().clone();
        for stream in [&out.stdout, &out.stderr] {
            let text = String::from_utf8_lossy(stream);
            assert!(
                !text.contains("\\\\?\\"),
                "{args:?} printed a verbatim prefix: {text}"
            );
        }
    }
}

#[test]
fn roots_ls_shows_the_home_folder_after_discovery() {
    let m = Machine::new();
    m.dv().arg("scan").assert().success();
    m.dv()
        .args(["roots", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::diff("~\n"));
}
