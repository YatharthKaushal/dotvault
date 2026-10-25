//! Per-variable history: `log`, `revert`, `forget`, and what records it.

use std::fs;
use std::path::PathBuf;

use assert_cmd::cargo::cargo_bin_cmd;
use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

const START: &str = "DATABASE_URL=postgres://old\nAPI_KEY=k1\n";

struct Fixture {
    tmp: TempDir,
    app: PathBuf,
    file: PathBuf,
}

/// One version of a variable: number, action, and value (`None` when removed).
type Version = (i64, String, Option<String>);

impl Fixture {
    fn new() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("home").join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("package.json"), "{}").unwrap();
        let file = app.join(".env.local");
        fs::write(&file, START).unwrap();
        Fixture { tmp, app, file }
    }

    fn data(&self) -> PathBuf {
        self.tmp.path().join("data")
    }

    fn history_file(&self) -> PathBuf {
        self.data().join("history.db")
    }

    fn dv(&self) -> Command {
        let home = self.tmp.path().join("home");
        let mut cmd = cargo_bin_cmd!("dotvault");
        cmd.env("DOTVAULT_HOME", &home)
            .env("DOTVAULT_SCAN_ROOTS", &home)
            .env("DOTVAULT_CONFIG_DIR", self.tmp.path().join("config"))
            .env("DOTVAULT_DATA_DIR", self.data())
            .env("NO_COLOR", "1")
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            .current_dir(&self.app);
        cmd
    }

    /// `dotvault <args> -f <the file>`.
    fn on_file(&self, args: &[&str]) -> Command {
        let mut cmd = self.dv();
        cmd.args(args).arg("-f").arg(&self.file);
        cmd
    }

    fn set(&self, assignment: &str) {
        self.on_file(&["set", assignment]).assert().success();
    }

    fn text(&self) -> String {
        fs::read_to_string(&self.file).unwrap()
    }

    fn json(&self, args: &[&str]) -> Vec<Value> {
        let out = self
            .on_file(&[&["log"], args, &["--json", "--reveal"]].concat())
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice::<Value>(&out)
            .unwrap_or_else(|e| {
                panic!(
                    "log --json must print JSON ({e}): {}",
                    String::from_utf8_lossy(&out)
                )
            })
            .as_array()
            .unwrap()
            .clone()
    }

    /// A variable's versions, oldest first.
    fn versions(&self, key: &str) -> Vec<Version> {
        let mut rows: Vec<Version> = self
            .json(&[key])
            .iter()
            .map(|r| {
                (
                    r["version"].as_i64().unwrap(),
                    r["action"].as_str().unwrap().to_string(),
                    r["value"].as_str().map(str::to_string),
                )
            })
            .collect();
        rows.reverse();
        rows
    }
}

fn v(version: i64, action: &str, value: Option<&str>) -> Version {
    (version, action.to_string(), value.map(str::to_string))
}

#[test]
fn set_twice_shows_a_baseline_then_each_change_in_order() {
    let f = Fixture::new();
    f.set("DATABASE_URL=postgres://new");
    f.set("DATABASE_URL=postgres://newer");
    assert_eq!(
        f.versions("DATABASE_URL"),
        [
            v(1, "baseline", Some("postgres://old")),
            v(2, "set", Some("postgres://new")),
            v(3, "set", Some("postgres://newer")),
        ]
    );
    // API_KEY was never changed, but it was already there when tracking began
    assert_eq!(f.versions("API_KEY"), [v(1, "baseline", Some("k1"))]);
}

#[test]
fn a_command_that_changes_several_variables_is_one_change() {
    let f = Fixture::new();
    f.on_file(&["set", "DATABASE_URL=x", "API_KEY=y", "NEW=z"])
        .assert()
        .success();
    let rows = f.json(&[]);
    let set: Vec<&Value> = rows.iter().filter(|r| r["action"] == "set").collect();
    assert_eq!(set.len(), 3);
    assert!(
        set.iter().all(|r| r["change"] == set[0]["change"]),
        "{set:?}"
    );
}

#[test]
fn a_new_file_starts_at_v1_with_no_baseline() {
    let f = Fixture::new();
    let fresh = f.app.join(".env.new");
    f.dv()
        .args(["set", "A=1", "-f"])
        .arg(&fresh)
        .assert()
        .success();
    let out = f
        .dv()
        .args(["log", "A", "--json", "--reveal", "-f"])
        .arg(&fresh)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rows: Vec<Value> = serde_json::from_slice(&out).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0]["version"].as_i64(), rows[0]["action"].as_str()),
        (Some(1), Some("set"))
    );
}

#[test]
fn unset_is_a_version_with_no_value() {
    let f = Fixture::new();
    f.on_file(&["unset", "API_KEY"]).assert().success();
    assert_eq!(
        f.versions("API_KEY"),
        [v(1, "baseline", Some("k1")), v(2, "unset", None)]
    );
    let removed = f.json(&["API_KEY"]);
    assert_eq!(removed[0]["removed"], true);
    assert_eq!(removed[0]["fingerprint"], Value::Null);
}

#[test]
fn setting_the_same_value_or_unsetting_a_missing_one_records_nothing() {
    let f = Fixture::new();
    f.set("API_KEY=k1");
    f.on_file(&["unset", "NOT_THERE"]).assert().code(1);
    assert!(
        !f.history_file().exists(),
        "nothing changed, so tracking never started"
    );
}

#[test]
fn values_are_masked_unless_revealed() {
    let f = Fixture::new();
    f.set("DATABASE_URL=postgres://hunter2-secret");

    f.on_file(&["log", "DATABASE_URL"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hunter2-secret").not())
        .stdout(predicate::str::contains("********"))
        .stdout(predicate::str::contains("masked"));
    f.on_file(&["log", "DATABASE_URL", "--reveal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("postgres://hunter2-secret"))
        .stdout(predicate::str::contains("postgres://old"));

    // JSON never carries a value unless asked to
    let out = f
        .on_file(&["log", "DATABASE_URL", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(
        !text.contains("hunter2-secret") && !text.contains("\"value\""),
        "{text}"
    );
    assert!(text.contains("\"fingerprint\""), "{text}");
}

#[test]
fn the_file_view_groups_by_change_and_shows_old_to_new_when_revealed() {
    let f = Fixture::new();
    f.set("DATABASE_URL=postgres://new");
    f.on_file(&["log"])
        .assert()
        .success()
        .stdout(predicate::str::contains("baseline"))
        .stdout(predicate::str::contains("set"))
        .stdout(predicate::str::contains("DATABASE_URL"))
        .stdout(predicate::str::contains("postgres://new").not());
    f.on_file(&["log", "--reveal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("postgres://old → postgres://new"));
}

#[test]
fn limit_shows_only_the_newest_entries() {
    let f = Fixture::new();
    for i in 1..=4 {
        f.set(&format!("DATABASE_URL=v{i}"));
    }
    let out = f
        .on_file(&["log", "DATABASE_URL", "--json", "-n", "2"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rows: Vec<Value> = serde_json::from_slice(&out).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["version"], 5, "newest first");
}

#[test]
fn history_is_an_alias_and_an_untracked_file_says_how_tracking_starts() {
    let f = Fixture::new();
    f.on_file(&["history"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "starts the first time dotvault changes the file",
        ));
    f.on_file(&["log", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[]"));
}

#[cfg(unix)]
#[test]
fn an_edit_is_diffed_per_variable_under_one_change() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let script = f.tmp.path().join("editor.sh");
    fs::write(
        &script,
        "#!/bin/sh\nprintf 'DATABASE_URL=edited\\nAPI_KEY=k1\\nADDED=1\\n' > \"$1\"\n",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    f.dv()
        .args(["edit", "-f"])
        .arg(&f.file)
        .env("EDITOR", &script)
        .assert()
        .success();
    assert_eq!(
        f.versions("DATABASE_URL"),
        [
            v(1, "baseline", Some("postgres://old")),
            v(2, "edit", Some("edited"))
        ]
    );
    assert_eq!(f.versions("ADDED"), [v(1, "edit", Some("1"))]);
    let rows = f.json(&[]);
    let edits: Vec<&Value> = rows.iter().filter(|r| r["action"] == "edit").collect();
    assert_eq!(edits.len(), 2, "API_KEY did not change");
    assert_eq!(edits[0]["change"], edits[1]["change"]);
}

#[cfg(unix)]
#[test]
fn opening_the_editor_and_changing_nothing_does_not_start_tracking() {
    let f = Fixture::new();
    f.dv()
        .args(["edit", "-f"])
        .arg(&f.file)
        .env("EDITOR", "true")
        .assert()
        .success();
    assert!(!f.history_file().exists());
}

#[test]
fn an_edit_made_by_hand_between_two_sets_is_recorded_as_external() {
    let f = Fixture::new();
    f.set("DATABASE_URL=postgres://via-dotvault");
    fs::write(&f.file, "DATABASE_URL=postgres://by-hand\nAPI_KEY=k1\n").unwrap();
    f.set("API_KEY=k2");
    assert_eq!(
        f.versions("DATABASE_URL"),
        [
            v(1, "baseline", Some("postgres://old")),
            v(2, "set", Some("postgres://via-dotvault")),
            v(3, "external", Some("postgres://by-hand")),
        ]
    );
}

#[test]
fn revert_goes_back_one_version_and_is_itself_a_new_version() {
    let f = Fixture::new();
    f.set("DATABASE_URL=postgres://new");
    f.set("DATABASE_URL=postgres://newer");
    f.on_file(&["revert", "DATABASE_URL"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Reverted DATABASE_URL to v2"))
        .stdout(predicate::str::contains("now v4"));
    assert!(
        f.text().contains("DATABASE_URL=postgres://new\n"),
        "{}",
        f.text()
    );
    assert_eq!(
        f.versions("DATABASE_URL").last().unwrap(),
        &v(4, "revert", Some("postgres://new"))
    );
    // reverting again goes back past the revert: to the value before it
    f.on_file(&["revert", "DATABASE_URL"]).assert().success();
    assert!(
        f.text().contains("DATABASE_URL=postgres://newer\n"),
        "{}",
        f.text()
    );
}

#[test]
fn revert_to_a_chosen_version_keeps_the_rest_of_the_file_intact() {
    let f = Fixture::new();
    fs::write(
        &f.file,
        "# the database\nDATABASE_URL=postgres://old\n\nAPI_KEY=k1 # key\n",
    )
    .unwrap();
    f.set("DATABASE_URL=postgres://new");
    f.set("DATABASE_URL=postgres://newer");
    f.on_file(&["revert", "DATABASE_URL", "--to", "1"])
        .assert()
        .success();
    assert_eq!(
        f.text(),
        "# the database\nDATABASE_URL=postgres://old\n\nAPI_KEY=k1 # key\n"
    );
}

#[test]
fn reverting_to_a_version_that_had_no_value_removes_the_variable() {
    let f = Fixture::new();
    f.set("ADDED=1"); // v1 of ADDED is a set
    f.on_file(&["unset", "ADDED"]).assert().success(); // v2: removed
    f.set("ADDED=2"); // v3
    f.on_file(&["revert", "ADDED", "--to", "2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed ADDED"))
        .stdout(predicate::str::contains("v2 had no value"));
    assert!(!f.text().contains("ADDED"), "{}", f.text());
    assert_eq!(f.versions("ADDED").last().unwrap(), &v(4, "revert", None));
}

#[test]
fn a_variable_that_revert_brings_back_goes_to_the_end_of_the_file() {
    // History keeps values, not layout, so the old position is not restored.
    // This pins that on purpose: changing it should be a decision.
    let f = Fixture::new();
    fs::write(&f.file, "FIRST=1\nMIDDLE=2 # kept?\nLAST=3\n").unwrap();
    f.on_file(&["unset", "MIDDLE"]).assert().success();
    assert_eq!(f.text(), "FIRST=1\nLAST=3\n");
    f.on_file(&["revert", "MIDDLE", "--to", "1"])
        .assert()
        .success();
    assert_eq!(f.text(), "FIRST=1\nLAST=3\nMIDDLE=2\n");
}

#[test]
fn revert_undoes_an_edit_made_outside_dotvault() {
    let f = Fixture::new();
    f.set("DATABASE_URL=postgres://recorded");
    fs::write(&f.file, "DATABASE_URL=postgres://by-hand\nAPI_KEY=k1\n").unwrap();
    // the file differs from the last record, so the last record is what to go back to
    f.on_file(&["revert", "DATABASE_URL"]).assert().success();
    assert!(
        f.text().contains("DATABASE_URL=postgres://recorded\n"),
        "{}",
        f.text()
    );
    let seen: Vec<String> = f
        .versions("DATABASE_URL")
        .into_iter()
        .map(|(_, action, _)| action)
        .collect();
    assert_eq!(seen, ["baseline", "set", "external", "revert"]);
}

#[test]
fn revert_explains_what_it_cannot_do_and_changes_nothing() {
    let f = Fixture::new();
    f.on_file(&["revert", "DATABASE_URL"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no history for DATABASE_URL"))
        .stderr(predicate::str::contains("first time dotvault changes it"));

    f.set("DATABASE_URL=postgres://new");
    let before = f.text();
    f.on_file(&["revert", "DATABASE_URL", "--to", "9"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("versions v1 to v2, no v9"));
    f.on_file(&["revert", "DATABASE_URL", "--to", "2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already has the value of v2"));
    assert_eq!(f.text(), before);

    // a variable with a single version has nothing earlier
    f.on_file(&["revert", "API_KEY"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("only one version"));
    assert_eq!(f.text(), before);
}

#[test]
fn forget_deletes_one_variable_or_everything_for_the_file() {
    let f = Fixture::new();
    f.set("DATABASE_URL=postgres://new");
    f.on_file(&["forget", "DATABASE_URL"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Forgot 2 versions of DATABASE_URL",
        ));
    assert!(f.versions("DATABASE_URL").is_empty());
    assert!(
        !f.versions("API_KEY").is_empty(),
        "other variables keep their history"
    );
    f.on_file(&["forget", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Forgot 1 version of every variable",
        ));
    assert!(f.json(&[]).is_empty());
    f.on_file(&["forget", "--all"])
        .assert()
        .success()
        .stderr(predicate::str::contains("No history recorded"));
}

#[test]
fn forget_needs_to_be_told_what_to_forget() {
    let f = Fixture::new();
    f.set("DATABASE_URL=x");
    f.on_file(&["forget"]).assert().failure();
    f.on_file(&["forget", "DATABASE_URL", "--all"])
        .assert()
        .failure();
    assert!(
        !f.versions("DATABASE_URL").is_empty(),
        "nothing was deleted"
    );
}

#[test]
fn a_forgotten_value_does_not_remain_in_the_history_file() {
    let f = Fixture::new();
    f.set("DATABASE_URL=postgres://needle-in-a-haystack-7");
    f.set("DATABASE_URL=postgres://other");
    f.on_file(&["forget", "--all"]).assert().success();
    let raw = fs::read(f.history_file()).unwrap();
    assert!(
        !raw.windows(20).any(|w| w == b"needle-in-a-haystack"),
        "forget must overwrite the values, not just unlink them"
    );
}

#[cfg(unix)]
#[test]
fn the_history_file_is_owner_only_with_no_extra_files() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.set("DATABASE_URL=x");
    let mode = fs::metadata(f.history_file()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let names: Vec<String> = fs::read_dir(f.data())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names.iter().any(|n| n.starts_with("history.db-")),
        "no sidecar files may hold values: {names:?}"
    );
}

#[test]
fn history_can_be_turned_off_in_the_config() {
    let f = Fixture::new();
    let config = f.tmp.path().join("config");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("config.toml"), "history = false\n").unwrap();
    f.set("DATABASE_URL=postgres://new");
    assert!(f.text().contains("postgres://new"));
    assert!(
        !f.history_file().exists(),
        "nothing may be recorded when it is off"
    );
}

#[test]
fn only_the_newest_versions_are_kept() {
    let f = Fixture::new();
    let config = f.tmp.path().join("config");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("config.toml"), "history_keep = 3\n").unwrap();
    for i in 1..=6 {
        f.set(&format!("DATABASE_URL=v{i}"));
    }
    let versions: Vec<i64> = f.versions("DATABASE_URL").iter().map(|r| r.0).collect();
    assert_eq!(versions, [5, 6, 7], "the newest three of seven");
}

#[test]
fn a_history_that_cannot_be_written_never_blocks_the_change() {
    let f = Fixture::new();
    // a directory where the history file should be makes every write to it fail
    fs::create_dir_all(f.history_file()).unwrap();
    f.on_file(&["set", "DATABASE_URL=postgres://still-saved"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "the change was saved, but its history could not be recorded",
        ));
    assert!(f.text().contains("postgres://still-saved"));
}

#[test]
fn only_changes_start_tracking_never_looking() {
    let f = Fixture::new();
    f.dv().args(["scan"]).assert().success();
    f.dv().arg("ls").assert().success();
    f.dv().args(["show", "-f"]).arg(&f.file).assert().success();
    f.on_file(&["get", "DATABASE_URL"]).assert().success();
    f.on_file(&["log"]).assert().success();
    f.on_file(&["log", "--json"]).assert().success();
    f.dv().args(["find", "API_KEY"]).assert().success();
    assert!(
        !f.history_file().exists(),
        "nothing was changed, so no history file"
    );
}

#[test]
fn a_project_selector_and_a_file_path_mean_the_same_file() {
    let f = Fixture::new();
    f.dv()
        .args([
            "set",
            "DATABASE_URL=via-project",
            "-p",
            "app",
            "-e",
            "local",
        ])
        .assert()
        .success();
    f.set("DATABASE_URL=via-path");
    let via_project = f
        .dv()
        .args([
            "log",
            "DATABASE_URL",
            "--json",
            "--reveal",
            "-p",
            "app",
            "-e",
            "local",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rows: Vec<Value> = serde_json::from_slice(&via_project).unwrap();
    assert_eq!(rows.len(), 3, "baseline and both sets: {rows:?}");
    assert_eq!(rows[0]["value"], "via-path");
}

#[test]
fn history_stays_reachable_after_the_file_is_moved_away() {
    let f = Fixture::new();
    f.set("DATABASE_URL=postgres://new");
    let moved = f.app.join(".env.moved");
    fs::rename(&f.file, &moved).unwrap();
    // the old path no longer exists, but its history can still be read and cleared
    f.dv()
        .args(["log", "DATABASE_URL", "--json", "--reveal", "-f"])
        .arg(&f.file)
        .assert()
        .success()
        .stdout(predicate::str::contains("postgres://new"));
    // the new location is a new file as far as history is concerned
    f.dv()
        .args(["log", "--json", "-f"])
        .arg(&moved)
        .assert()
        .success()
        .stdout(predicate::str::contains("[]"));
}

#[test]
fn log_json_is_pure_json_on_stdout() {
    let f = Fixture::new();
    f.set("DATABASE_URL=postgres://new");
    let out = f
        .on_file(&["log", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(serde_json::from_slice::<Value>(&out.stdout).is_ok());
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
