//! First-run discovery: with no folders configured, commands that need the list
//! of env files search the whole computer on their own. Tests never do: a fixture
//! folder stands in for home through `DOTVAULT_HOME`, and `DOTVAULT_SCAN_ROOTS`
//! stands in for the computer's disks (both also work on Windows, where the OS
//! decides where the real ones are).

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

const NOTICE: &str = "No folders set up yet";

fn write(root: &Path, rel: &str, contents: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, contents).unwrap();
}

struct Machine {
    _tmp: TempDir,
    home: PathBuf,
    /// A second "disk": a folder outside home with one project in it.
    disk: PathBuf,
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
        let disk = tmp.path().join("disk");
        write(&disk, "shop/package.json", "{}");
        write(&disk, "shop/.env", "SHOP=1\n");
        Machine {
            config: tmp.path().join("config"),
            data: tmp.path().join("data"),
            home,
            disk,
            _tmp: tmp,
        }
    }

    fn dv(&self) -> Command {
        let mut cmd = cargo_bin_cmd!("dotvault");
        cmd.env("DOTVAULT_HOME", &self.home)
            .env("DOTVAULT_SCAN_ROOTS", &self.home)
            .env("DOTVAULT_CONFIG_DIR", &self.config)
            .env("DOTVAULT_DATA_DIR", &self.data)
            .env("NO_COLOR", "1")
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            .current_dir(&self.home);
        cmd
    }

    /// Like [`Machine::dv`] on a computer whose disks are `roots`.
    fn dv_on(&self, roots: &[&Path]) -> Command {
        let mut cmd = self.dv();
        cmd.env("DOTVAULT_SCAN_ROOTS", std::env::join_paths(roots).unwrap());
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

#[test]
fn an_ambiguous_project_name_lists_the_paths_on_one_readable_line() {
    let m = Machine::new();
    write(&m.home, "code/shop/backend/package.json", "{}");
    write(&m.home, "code/shop/backend/.env", "SHOP=1\n");
    write(&m.home, "work/blog/backend/package.json", "{}");
    write(&m.home, "work/blog/backend/.env", "BLOG=1\n");

    let out = m
        .dv()
        .args(["show", "backend"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let errors: Vec<&str> = stderr.lines().filter(|l| l.starts_with("error:")).collect();
    assert_eq!(errors.len(), 1, "{stderr}");
    let line = errors[0].replace('\\', "/");
    assert!(line.contains("2 projects are named"), "{line}");
    assert!(line.contains("code/shop/backend/.env"), "{line}");
    assert!(line.contains("work/blog/backend/.env"), "{line}");
    // the old message put the paths after an escaped newline: `instead:\n  ~/...`
    assert!(!errors[0].contains("instead:\\n"), "{}", errors[0]);
}

#[test]
fn a_path_given_to_dash_p_says_to_use_dash_f_and_dash_f_works() {
    let m = Machine::new();
    let file = m.home.join("code/acme-api/.env");

    m.dv()
        .args(["get", "PORT", "-p"])
        .arg(&file)
        .assert()
        .failure()
        .stderr(predicate::str::contains("pass it with -f instead"));

    m.dv()
        .args(["get", "PORT", "-f"])
        .arg(&file)
        .assert()
        .success()
        .stdout("3000\n");
}

/// Forgets who chose the saved folders, the way a config written by 0.2 or 0.3 is.
fn make_legacy(m: &Machine) {
    let path = m.config.join("config.toml");
    let text = fs::read_to_string(&path).unwrap();
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| !l.starts_with("roots_auto"))
        .collect();
    fs::write(&path, kept.join("\n") + "\n").unwrap();
    assert!(!m.config_text().contains("roots_auto"));
}

fn project_files(lines: &[String]) -> Vec<String> {
    let mut v: Vec<String> = lines
        .iter()
        .map(|l| l.rsplit('/').nth(1).unwrap_or("").to_string())
        .collect();
    v.sort();
    v.dedup();
    v
}

#[test]
fn the_first_run_saves_that_dotvault_chose_the_folders() {
    let m = Machine::new();
    m.dv().arg("ls").assert().success();
    assert!(
        m.config_text().contains("roots_auto = true"),
        "{}",
        m.config_text()
    );
}

#[test]
fn a_home_only_config_from_an_older_version_is_widened_once_and_says_so() {
    let m = Machine::new();
    let home = m.home.clone();
    m.dv().arg("scan").arg(&home).assert().success(); // what 0.3 saved for a first run
    make_legacy(&m);

    let on = [m.home.as_path(), m.disk.as_path()];
    m.dv_on(&on)
        .args(["ls", "--paths"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Now searching"))
        .stderr(predicate::str::contains("it was your home folder only"));
    let lines = {
        let out = m
            .dv_on(&on)
            .args(["ls", "--paths"])
            .assert()
            .success()
            .get_output()
            .clone();
        assert!(
            !String::from_utf8_lossy(&out.stderr).contains("Now searching"),
            "the notice is shown once"
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(|l| l.replace('\\', "/"))
            .collect::<Vec<_>>()
    };
    assert!(has(&lines, "shop/.env"), "{lines:#?}");
    assert!(has(&lines, "code/acme-api/.env"), "{lines:#?}");
    assert!(
        m.config_text().contains("roots_auto = true"),
        "{}",
        m.config_text()
    );
}

#[test]
fn a_bare_scan_widens_an_older_home_only_config_too() {
    let m = Machine::new();
    let home = m.home.clone();
    m.dv().arg("scan").arg(&home).assert().success();
    make_legacy(&m);
    let on = [m.home.as_path(), m.disk.as_path()];
    m.dv_on(&on)
        .arg("scan")
        .assert()
        .success()
        .stderr(predicate::str::contains("Now searching"))
        .stdout(predicate::str::contains("2 folders"));
}

#[test]
fn other_folders_in_an_older_config_are_left_alone() {
    let m = Machine::new();
    let web = m.home.join("code/web");
    m.dv().arg("scan").arg(&web).assert().success();
    make_legacy(&m);
    let on = [m.home.as_path(), m.disk.as_path()];
    let out = m
        .dv_on(&on)
        .args(["ls", "--paths"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(!String::from_utf8_lossy(&out.stderr).contains("Now searching"));
    let lines: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(project_files(&lines).len(), 1, "{lines:#?}");
}

#[test]
fn folders_the_user_named_are_never_replaced_by_the_computers_disks() {
    let m = Machine::new();
    let web = m.home.join("code/web");
    m.dv().arg("scan").arg(&web).assert().success();
    assert!(
        m.config_text().contains("roots_auto = false"),
        "{}",
        m.config_text()
    );
    let on = [m.home.as_path(), m.disk.as_path()];
    let out = m
        .dv_on(&on)
        .args(["ls", "--paths"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(!String::from_utf8_lossy(&out.stderr).contains("Now searching"));
    let lines: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| l.replace('\\', "/"))
        .collect();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(has(&lines, "code/web/.env.local"));
}

#[test]
fn naming_a_folder_after_the_automatic_start_takes_the_list_over() {
    let m = Machine::new();
    m.dv().arg("ls").assert().success(); // automatic: [home]
    let disk = m.disk.clone();
    m.dv().arg("scan").arg(&disk).assert().success(); // the user adds a folder
    assert!(
        m.config_text().contains("roots_auto = false"),
        "{}",
        m.config_text()
    );
    // a disk appearing later is not added behind the user's back
    let both = [m.home.as_path(), m.disk.as_path()];
    m.dv_on(&both)
        .args(["roots", "rm"])
        .arg(&disk)
        .assert()
        .success();
    let out = m
        .dv_on(&both)
        .args(["ls", "--paths"])
        .assert()
        .success()
        .get_output()
        .clone();
    let lines: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| l.replace('\\', "/"))
        .collect();
    assert!(!has(&lines, "shop/.env"), "{lines:#?}");
}

#[test]
fn an_automatic_list_follows_disks_as_they_come_and_go() {
    let m = Machine::new();
    m.dv().args(["ls", "--paths"]).assert().success(); // [home]

    let both = [m.home.as_path(), m.disk.as_path()];
    let plugged = {
        let out = m
            .dv_on(&both)
            .args(["ls", "--paths"])
            .assert()
            .success()
            .get_output()
            .clone();
        assert!(
            !String::from_utf8_lossy(&out.stderr).contains(NOTICE),
            "following a new disk is quiet"
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(|l| l.replace('\\', "/"))
            .collect::<Vec<_>>()
    };
    assert!(has(&plugged, "shop/.env"), "{plugged:#?}");

    // unplugged again
    assert!(!has(&m.listed(), "shop/.env"));
}

#[test]
fn scan_system_adds_the_computers_disks_to_a_list_the_user_named() {
    let m = Machine::new();
    let web = m.home.join("code/web");
    m.dv().arg("scan").arg(&web).assert().success();
    let disk_only = [m.disk.as_path()];
    m.dv_on(&disk_only)
        .args(["scan", "--system"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Searching the whole computer"));
    let out = m
        .dv()
        .args(["ls", "--paths"])
        .assert()
        .success()
        .get_output()
        .clone();
    let lines: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| l.replace('\\', "/"))
        .collect();
    assert!(has(&lines, "code/web/.env.local"), "{lines:#?}");
    assert!(has(&lines, "shop/.env"), "{lines:#?}");
    assert!(
        m.config_text().contains("roots_auto = false"),
        "{}",
        m.config_text()
    );
}

#[test]
fn scan_with_a_project_name_says_scan_takes_folders() {
    let m = Machine::new();
    m.dv().arg("ls").assert().success();
    m.dv()
        .args(["scan", "acme-api"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("acme-api is not a folder here"))
        .stderr(predicate::str::contains("dotvault show acme-api"))
        .stderr(predicate::str::contains("os error").not());
}

#[test]
fn scan_with_a_path_that_does_not_exist_names_the_path() {
    let m = Machine::new();
    m.dv()
        .args(["scan", "no/such/folder"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no folder at"))
        .stderr(predicate::str::contains("show").not());
}

#[test]
fn the_first_run_notice_says_where_it_is_looking_and_that_it_can_take_a_while() {
    let m = Machine::new();
    m.dv()
        .arg("ls")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "the folders in DOTVAULT_SCAN_ROOTS",
        ))
        .stderr(predicate::str::contains("can take a minute"));
}
