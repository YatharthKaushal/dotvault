//! End-to-end tests of the local (no account) commands, run against the real
//! binary and a fixture tree of projects.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

struct Sandbox {
    _tmp: TempDir,
    code: PathBuf,
    config: PathBuf,
    data: PathBuf,
    /// An empty stand-in for the home folder, so no test ever scans a real one.
    home: PathBuf,
}

fn write(root: &Path, rel: &str, contents: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, contents).unwrap();
}

impl Sandbox {
    /// code/
    ///   acme-api/  package.json .env .env.production .env.example node_modules/dep/.env
    ///   web/       package.json .env.local .env.development
    ///   loose/     .env
    fn new() -> Sandbox {
        let tmp = tempfile::tempdir().unwrap();
        let code = tmp.path().join("code");
        write(&code, "acme-api/package.json", "{}");
        write(
            &code,
            "acme-api/.env",
            "# local dev\nDATABASE_URL=postgres://localhost/dev\nAPI_KEY=dev-key # shared\nPORT=3000\n",
        );
        write(
            &code,
            "acme-api/.env.production",
            "DATABASE_URL=postgres://prod.internal/db\nAPI_KEY=prod-key\n",
        );
        write(&code, "acme-api/.env.example", "DATABASE_URL=\nAPI_KEY=\n");
        write(&code, "acme-api/node_modules/dep/.env", "LEAK=1\n");
        write(&code, "web/package.json", "{}");
        write(
            &code,
            "web/.env.local",
            "NEXT_PUBLIC_URL=http://localhost:3000\nAPI_KEY=dev-key\n",
        );
        write(
            &code,
            "web/.env.development",
            "NEXT_PUBLIC_URL=http://dev.example\n",
        );
        write(&code, "loose/.env", "SOLO=1\n");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        Sandbox {
            config: tmp.path().join("config"),
            data: tmp.path().join("data"),
            home,
            code,
            _tmp: tmp,
        }
    }

    fn scanned() -> Sandbox {
        let s = Sandbox::new();
        s.dv().args(["scan", "."]).assert().success();
        s
    }

    fn dv(&self) -> Command {
        let mut cmd = cargo_bin_cmd!("dotvault");
        cmd.env("DOTVAULT_CONFIG_DIR", &self.config)
            .env("DOTVAULT_DATA_DIR", &self.data)
            .env("DOTVAULT_HOME", &self.home)
            .env("DOTVAULT_SCAN_ROOTS", &self.home)
            .env("NO_COLOR", "1")
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            .current_dir(&self.code);
        cmd
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.code.join(rel)).unwrap()
    }
}

fn stdout_of(cmd: &mut Command) -> String {
    String::from_utf8(cmd.assert().success().get_output().stdout.clone()).unwrap()
}

// --- scan / ls ------------------------------------------------------------------

#[test]
fn scan_reports_and_remembers_folders() {
    let s = Sandbox::new();
    s.dv()
        .args(["scan", "."])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 folder"))
        .stdout(predicate::str::contains("5 env files in 3 projects"))
        .stdout(predicate::str::contains("1 example file hidden by default"));
    // remembered: a plain `scan` works without arguments
    s.dv().arg("scan").assert().success();
    s.dv()
        .args(["roots", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("code"));
}

#[test]
fn ls_groups_projects_hides_examples_and_skips_node_modules() {
    let s = Sandbox::scanned();
    s.dv()
        .arg("ls")
        .assert()
        .success()
        .stdout(predicate::str::contains("acme-api"))
        .stdout(predicate::str::contains("web"))
        .stdout(predicate::str::contains("loose"))
        .stdout(predicate::str::contains("production"))
        .stdout(predicate::str::contains("example").not())
        .stdout(predicate::str::contains("node_modules").not())
        .stderr(predicate::str::contains("1 example file hidden"));
}

#[test]
fn ls_examples_flags() {
    let s = Sandbox::scanned();
    s.dv()
        .args(["ls", "--examples"])
        .assert()
        .success()
        .stdout(predicate::str::contains("default (example)").not())
        .stdout(predicate::str::contains("example (example)"));
    let only = stdout_of(s.dv().args(["ls", "--only-examples", "--paths"]));
    assert_eq!(only.lines().count(), 1);
    assert!(only.trim_end().ends_with(".env.example"));
}

#[test]
fn ls_filters() {
    let s = Sandbox::scanned();
    let paths = |args: &[&str]| -> Vec<String> {
        let mut cmd = s.dv();
        cmd.arg("ls").args(args).arg("--paths");
        stdout_of(&mut cmd)
            .lines()
            .map(|l| l.rsplit(['/', '\\']).next().unwrap().to_string())
            .collect()
    };
    assert_eq!(paths(&["-e", "production"]), [".env.production"]);
    assert_eq!(paths(&["-p", "web"]).len(), 2);
    assert_eq!(paths(&["-p", "web", "-e", "local"]), [".env.local"]);
    assert_eq!(paths(&["-k", "NEXT_PUBLIC_*"]).len(), 2);
    assert_eq!(paths(&["-k", "SOLO"]), [".env"]);
    assert_eq!(paths(&["-k", "API_KEY"]).len(), 3);
    assert_eq!(paths(&["--path", "acme-api"]).len(), 2);
    assert_eq!(paths(&["web"]).len(), 2, "positional folder");
    assert!(paths(&["-p", "nothing-like-this"]).is_empty());
}

#[test]
fn ls_json_has_key_names_but_never_values() {
    let s = Sandbox::scanned();
    let out = stdout_of(s.dv().args(["ls", "--json"]));
    assert!(
        !out.contains("postgres") && !out.contains("dev-key"),
        "{out}"
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let api = v
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["project"] == "acme-api" && f["env"] == "default")
        .unwrap();
    assert_eq!(api["key_count"], 3);
    assert_eq!(
        api["key_names"],
        serde_json::json!(["DATABASE_URL", "API_KEY", "PORT"])
    );
}

#[test]
fn ls_notices_deleted_and_edited_files_without_rescan_but_needs_rescan_for_new_ones() {
    let s = Sandbox::scanned();
    fs::remove_file(s.code.join("loose/.env")).unwrap();
    s.dv()
        .arg("ls")
        .assert()
        .success()
        .stdout(predicate::str::contains("loose").not());

    write(
        &s.code,
        "web/.env.local",
        "NEXT_PUBLIC_URL=x\nBRAND_NEW_KEY=1\n",
    );
    s.dv()
        .args(["ls", "-k", "BRAND_NEW_KEY", "--paths"])
        .assert()
        .success()
        .stdout(predicate::str::contains(".env.local"));

    write(&s.code, "fresh/.env", "FRESH=1\n");
    s.dv()
        .args(["ls", "-p", "fresh", "--paths"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
    s.dv()
        .args(["ls", "--refresh", "-p", "fresh", "--paths"])
        .assert()
        .success()
        .stdout(predicate::str::contains("fresh"));
}

#[test]
fn roots_add_and_rm() {
    let s = Sandbox::new();
    s.dv().args(["roots", "add", "."]).assert().success();
    s.dv()
        .args(["roots", "rm", "does-not-exist"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not one of the scanned folders"));
    s.dv().args(["roots", "rm", "."]).assert().success();
    s.dv()
        .args(["roots", "ls"])
        .assert()
        .success()
        .stderr(predicate::str::contains("dotvault scan"));
    s.dv()
        .args(["scan", "no-such-dir"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no-such-dir is not a folder here"));
}

// --- show / get ----------------------------------------------------------------

#[test]
fn show_masks_values_unless_revealed() {
    let s = Sandbox::scanned();
    s.dv()
        .args(["show", "acme-api/.env"])
        .assert()
        .success()
        .stdout(predicate::str::contains("DATABASE_URL"))
        .stdout(predicate::str::contains("********"))
        .stdout(predicate::str::contains("postgres").not())
        .stdout(predicate::str::contains("dev-key").not());
    s.dv()
        .args(["show", "acme-api/.env", "--reveal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("postgres://localhost/dev"))
        .stdout(predicate::str::contains("dev-key"))
        .stdout(predicate::str::contains("shared").not()); // trailing comment is not part of the value
}

#[test]
fn show_formats_keys_and_default_target() {
    let s = Sandbox::scanned();
    let dotenv = stdout_of(s.dv().args([
        "show",
        "acme-api/.env",
        "--format",
        "dotenv",
        "--reveal",
        "--keys",
        "PORT,API_KEY",
    ]));
    assert_eq!(dotenv, "PORT=3000\nAPI_KEY=dev-key\n");

    let json = stdout_of(s.dv().args(["show", "acme-api/.env", "--format", "json"]));
    assert!(!json.contains("postgres"));
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["PORT"], "********");

    let shell = stdout_of(s.dv().args([
        "show",
        "acme-api/.env",
        "--format",
        "shell",
        "--reveal",
        "-k",
        "PORT",
    ]));
    assert_eq!(shell, "export PORT='3000'\n");

    // no argument: ./.env of the current directory
    let mut cmd = s.dv();
    cmd.current_dir(s.code.join("loose"))
        .args(["show", "--reveal"]);
    assert!(stdout_of(&mut cmd).contains("SOLO"));

    s.dv()
        .args(["show", "acme-api/.env", "--keys", "PORT,NOPE"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("NOPE"));
}

#[test]
fn show_resolves_project_selectors() {
    let s = Sandbox::scanned();
    // bare project -> its .env
    s.dv()
        .args(["show", "acme-api", "--reveal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dev-key"));
    // project:env
    s.dv()
        .args(["show", "acme-api:production", "--reveal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prod-key"));
    // flags work the same way
    s.dv()
        .args(["show", "-p", "acme-api", "-e", "production", "--reveal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prod-key"));
    // examples are reachable only explicitly
    s.dv()
        .args(["show", "acme-api:example"])
        .assert()
        .success()
        .stdout(predicate::str::contains("DATABASE_URL"));
    // ambiguous: web has .env.local and .env.development, no plain .env
    s.dv()
        .args(["show", "web"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("web:local"))
        .stderr(predicate::str::contains("web:development"));
    // typo gets a suggestion, unknown gets a pointer to scan
    s.dv()
        .args(["show", "acme"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("did you mean: acme-api"));
    s.dv()
        .args(["show", "zzz"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("dotvault scan"));
}

#[test]
fn get_prints_raw_value_and_fails_for_missing() {
    let s = Sandbox::scanned();
    s.dv()
        .args(["get", "API_KEY", "-p", "acme-api", "-e", "production"])
        .assert()
        .success()
        .stdout("prod-key\n");
    s.dv()
        .args(["get", "PORT", "-f", "acme-api/.env"])
        .assert()
        .success()
        .stdout("3000\n");
    s.dv()
        .args(["get", "NOPE", "-f", "acme-api/.env"])
        .assert()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("NOPE is not set"));
    // -e needs -p
    s.dv()
        .args(["get", "X", "-e", "production"])
        .assert()
        .failure();
}

// --- find -------------------------------------------------------------------------

#[test]
fn find_lists_every_definition_and_groups_distinct_values() {
    let s = Sandbox::scanned();
    let out = stdout_of(s.dv().args(["find", "API_KEY"]));
    assert!(
        !out.contains("dev-key") && !out.contains("prod-key"),
        "masked by default:\n{out}"
    );
    let rows: Vec<&str> = out.lines().skip(1).collect();
    assert_eq!(rows.len(), 3, "{out}");
    // dev-key appears twice (same variant), prod-key once (the other variant)
    let variant_of = |needle: &str| {
        rows.iter()
            .find(|r| r.contains(needle))
            .unwrap()
            .split_whitespace()
            .last()
            .unwrap()
            .to_string()
    };
    assert_eq!(variant_of("production"), "B");
    assert_eq!(variant_of("local"), "A");
    assert_eq!(variant_of("default"), "A");

    s.dv()
        .args(["find", "API_KEY"])
        .assert()
        .success()
        .stderr(predicate::str::contains("API_KEY has 2 distinct values"));

    s.dv()
        .args(["find", "api_key", "--reveal", "-e", "production"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prod-key"))
        .stdout(predicate::str::contains("dev-key").not());
}

#[test]
fn find_globs_filters_json_and_no_match() {
    let s = Sandbox::scanned();
    s.dv()
        .args(["find", "NEXT_PUBLIC_*", "-p", "web"])
        .assert()
        .success()
        .stderr(predicate::str::contains("2 matches in 2 files"));

    let out = stdout_of(s.dv().args(["find", "DATABASE_URL", "--json"]));
    assert!(!out.contains("postgres"));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2); // example file is excluded
    assert_eq!(v[0]["distinct_values"], 2);

    // examples are included only on request
    let with = stdout_of(
        s.dv()
            .args(["find", "DATABASE_URL", "--json", "--examples"]),
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&with)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        3
    );

    s.dv()
        .args(["find", "NO_SUCH_KEY"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("No variable matches"));
}

// --- set / unset -------------------------------------------------------------------

#[test]
fn set_updates_in_place_and_keeps_comments_and_layout() {
    let s = Sandbox::scanned();
    s.dv()
        .args([
            "set",
            "API_KEY=rotated",
            "NEW_ONE=hello world",
            "-f",
            "acme-api/.env",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("API_KEY, NEW_ONE"))
        .stdout(predicate::str::contains("rotated").not()); // values are never echoed
    assert_eq!(
        s.read("acme-api/.env"),
        "# local dev\nDATABASE_URL=postgres://localhost/dev\nAPI_KEY=rotated # shared\nPORT=3000\nNEW_ONE=\"hello world\"\n"
    );
    // the index followed along
    s.dv()
        .args(["ls", "-k", "NEW_ONE", "--paths"])
        .assert()
        .success()
        .stdout(predicate::str::contains("acme-api"));
}

#[test]
fn set_by_project_creates_missing_explicit_files_privately_and_validates_keys() {
    let s = Sandbox::scanned();
    s.dv()
        .args(["set", "A=1", "-p", "acme-api", "-e", "production"])
        .assert()
        .success();
    assert!(s.read("acme-api/.env.production").ends_with("A=1\n"));

    s.dv()
        .args(["set", "FRESH=1", "-f", "brand-new.env"])
        .assert()
        .success();
    assert_eq!(s.read("brand-new.env"), "FRESH=1\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(s.code.join("brand-new.env"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // one bad key aborts everything
    let before = s.read("acme-api/.env");
    s.dv()
        .args(["set", "GOOD=1", "1BAD=2", "-f", "acme-api/.env"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid variable name"));
    assert_eq!(s.read("acme-api/.env"), before);

    // an existing project that doesn't exist cannot be created by selector
    s.dv()
        .args(["set", "A=1", "-p", "ghost"])
        .assert()
        .failure();
}

#[test]
fn set_without_a_value_reads_it_from_stdin() {
    let s = Sandbox::scanned();
    s.dv()
        .args(["set", "SECRET", "-f", "loose/.env"])
        .write_stdin("hunter2\n")
        .assert()
        .success();
    assert_eq!(s.read("loose/.env"), "SOLO=1\nSECRET=hunter2\n");
}

#[test]
fn set_in_cwd_defaults_to_dot_env() {
    let s = Sandbox::scanned();
    let mut cmd = s.dv();
    cmd.current_dir(s.code.join("loose"))
        .args(["set", "ANOTHER=2"]);
    cmd.assert().success();
    assert_eq!(s.read("loose/.env"), "SOLO=1\nANOTHER=2\n");
}

#[test]
fn unset_removes_keys_and_reports_missing() {
    let s = Sandbox::scanned();
    s.dv()
        .args(["unset", "PORT", "-f", "acme-api/.env"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed PORT"));
    assert!(!s.read("acme-api/.env").contains("PORT"));
    assert!(s.read("acme-api/.env").contains("# local dev"));

    // partial success: removes what exists, exits 1 for what doesn't
    s.dv()
        .args(["unset", "API_KEY", "NOPE", "-f", "acme-api/.env"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("NOPE"));
    assert!(!s.read("acme-api/.env").contains("API_KEY"));
}

#[test]
fn refuses_to_rewrite_non_utf8_files() {
    let s = Sandbox::scanned();
    fs::write(s.code.join("latin1.env"), [b'A', b'=', 0xE9, b'\n']).unwrap();
    s.dv()
        .args(["set", "B=1", "-f", "latin1.env"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not valid UTF-8"));
    assert_eq!(
        fs::read(s.code.join("latin1.env")).unwrap(),
        [b'A', b'=', 0xE9, b'\n']
    );
}

// --- diff ----------------------------------------------------------------------------

#[test]
fn diff_reports_missing_and_changed_keys_with_exit_code() {
    let s = Sandbox::scanned();
    s.dv()
        .args(["diff", "acme-api", "acme-api:production"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("- PORT"))
        .stdout(predicate::str::contains("~ DATABASE_URL"))
        .stdout(predicate::str::contains("(value differs)"))
        .stdout(predicate::str::contains("postgres").not())
        .stdout(
            predicate::str::contains("2 differences").or(predicate::str::contains("3 differences")),
        );

    s.dv()
        .args(["diff", "acme-api", "acme-api:production", "--reveal"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "postgres://localhost/dev → postgres://prod.internal/db",
        ));

    // keys that exist only on the right
    s.dv()
        .args(["diff", "acme-api:example", "acme-api"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("+ PORT"));

    s.dv()
        .args(["diff", "acme-api/.env", "acme-api/.env"])
        .assert()
        .success()
        .stdout(predicate::str::contains("identical (3 variables)"));
}

// --- edit -----------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn edit_runs_the_editor_and_refreshes_the_index() {
    use std::os::unix::fs::PermissionsExt;
    let s = Sandbox::scanned();
    let script = s.code.join("fake-editor.sh");
    fs::write(&script, "#!/bin/sh\necho 'EDITED=yes' >> \"$1\"\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    s.dv()
        .args(["edit", "loose/.env"])
        .env("EDITOR", &script)
        .assert()
        .success();
    assert_eq!(s.read("loose/.env"), "SOLO=1\nEDITED=yes\n");
    s.dv()
        .args(["ls", "-k", "EDITED", "--paths"])
        .assert()
        .success()
        .stdout(predicate::str::contains("loose"));

    // editor failure is reported
    s.dv()
        .args(["edit", "loose/.env"])
        .env("EDITOR", "false")
        .assert()
        .failure()
        .stderr(predicate::str::contains("editor exited"));
    s.dv()
        .args(["edit", "loose/.env"])
        .env("EDITOR", "definitely-not-an-editor-xyz")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no editor found"));
}

// --- misc ------------------------------------------------------------------------------

#[test]
fn completions_and_config_and_help() {
    let s = Sandbox::new();
    s.dv()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dotvault"));
    s.dv()
        .arg("config")
        .assert()
        .success()
        .stdout(predicate::str::contains("config file"))
        .stdout(predicate::str::contains("(not set)"));
    s.dv().assert().failure(); // no subcommand: usage
    for flag in ["--version", "-v", "-V"] {
        s.dv()
            .arg(flag)
            .assert()
            .success()
            .stdout(predicate::str::starts_with("dotvault "));
    }
}
