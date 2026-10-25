//! Secret files: finding and marking SSH keys, keystores, service-account JSON and
//! the like, and describing them without ever printing their contents.

use std::fs;
use std::path::PathBuf;

use assert_cmd::cargo::cargo_bin_cmd;
use assert_cmd::Command;
use base64::{engine::general_purpose::STANDARD, Engine};
use predicates::prelude::*;
use serde_json::{json, Value};
use tempfile::TempDir;

// Key material is built from fragments at run time so nothing here looks real to a
// secret scanner.
fn begin(label: &str) -> String {
    format!("{}BEGIN {label}{}", "-----", "-----")
}

fn pem(label: &str, body: &str) -> String {
    format!("{}\n{body}\n-----END {label}-----\n", begin(label))
}

fn string(out: &mut Vec<u8>, s: &[u8]) {
    out.extend((s.len() as u32).to_be_bytes());
    out.extend(s);
}

fn openssh_key(cipher: &str, key_type: &str) -> String {
    let mut b = b"openssh-key-v1\0".to_vec();
    string(&mut b, cipher.as_bytes());
    string(&mut b, if cipher == "none" { b"none" } else { b"bcrypt" });
    string(&mut b, b"");
    b.extend(1u32.to_be_bytes());
    let mut public = Vec::new();
    string(&mut public, key_type.as_bytes());
    string(&mut public, &[0u8; 32]);
    string(&mut b, &public);
    string(&mut b, &[0u8; 16]);
    let wrapped: Vec<String> = STANDARD
        .encode(&b)
        .as_bytes()
        .chunks(70)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect();
    pem("OPENSSH PRIVATE KEY", &wrapped.join("\n"))
}

fn service_account() -> String {
    json!({
        "type": "service_account",
        "project_id": "demo",
        "private_key": pem("PRIVATE KEY", "c2VjcmV0LWJvZHk="),
        "client_email": "bot@demo.example",
    })
    .to_string()
}

const JKS: [u8; 16] = [
    0xFE, 0xED, 0xFE, 0xED, 0, 0, 0, 2, 0xAA, 0xBB, 0xCC, 0xDD, 0, 1, 2, 3,
];

struct Fixture {
    tmp: TempDir,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let f = Fixture { tmp, home };
        f.write("app/package.json", "{}");
        f.write("app/.env", "A=1\nB=2\n");
        f.write("app/secrets/sa.json", &service_account());
        f.write("app/config.json", r#"{"port": 3000}"#);
        f.write("app/server.key", &pem("PRIVATE KEY", "c2VjcmV0LWJvZHk="));
        f.write("app/server.pem", &pem("CERTIFICATE", "Y2VydA=="));
        fs::create_dir_all(f.home.join("app/android")).unwrap();
        fs::write(f.home.join("app/android/release.keystore"), JKS).unwrap();
        f.write("keys/id_ed25519", &openssh_key("none", "ssh-ed25519"));
        f.write("keys/id_ed25519.pub", "ssh-ed25519 AAAA me@host\n");
        // a hidden folder in home: only found because it is added as a root,
        // the way a real default scan adds ~/.ssh
        f.write(".ssh/id_rsa", &openssh_key("aes256-ctr", "ssh-rsa"));
        f.write(".ssh/known_hosts", "github.com ssh-ed25519 AAAA\n");
        f
    }

    fn write(&self, rel: &str, contents: &str) {
        let p = self.home.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.home.join(rel)
    }

    fn data(&self) -> PathBuf {
        self.tmp.path().join("data")
    }

    fn config(&self) -> PathBuf {
        self.tmp.path().join("config")
    }

    fn dv(&self) -> Command {
        let roots = std::env::join_paths([self.home.clone(), self.home.join(".ssh")]).unwrap();
        let mut cmd = cargo_bin_cmd!("dotvault");
        cmd.env("DOTVAULT_HOME", &self.home)
            .env("DOTVAULT_SCAN_ROOTS", roots)
            .env("DOTVAULT_CONFIG_DIR", self.config())
            .env("DOTVAULT_DATA_DIR", self.data())
            .env("NO_COLOR", "1")
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            .current_dir(&self.home);
        cmd
    }

    fn out(&self, args: &[&str]) -> String {
        let out = self.dv().args(args).assert().success().get_output().clone();
        String::from_utf8(out.stdout).unwrap().replace('\\', "/")
    }

    fn json(&self, args: &[&str]) -> Vec<Value> {
        serde_json::from_str::<Value>(&self.out(args))
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }

    fn kinds(&self, args: &[&str]) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .json(&[args, &["--json"]].concat())
            .iter()
            .map(|r| {
                let path = r["path"].as_str().unwrap().replace('\\', "/");
                let name = path.rsplit('/').next().unwrap().to_string();
                (name, r["kind"].as_str().unwrap().to_string())
            })
            .collect();
        v.sort();
        v
    }
}

fn pair(name: &str, kind: &str) -> (String, String) {
    (name.to_string(), kind.to_string())
}

#[test]
fn plain_ls_lists_only_env_files_and_hints_at_the_rest() {
    let f = Fixture::new();
    let out = f.dv().arg("ls").assert().success().get_output().clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stdout.contains("app"), "{stdout}");
    for hidden in ["service-account", "ssh-key", "keystore", "private-key"] {
        assert!(
            !stdout.contains(hidden),
            "plain ls must not list {hidden}: {stdout}"
        );
    }
    assert!(
        stderr.contains("5 secret files (--secrets to list)"),
        "{stderr}"
    );
    assert_eq!(f.kinds(&["ls"]), [pair(".env", "env")], "--json agrees");
}

#[test]
fn ls_secrets_lists_each_file_with_its_kind_and_details() {
    let f = Fixture::new();
    let out = f.out(&["ls", "--secrets"]);
    assert!(out.contains("KIND"), "{out}");
    assert!(out.contains("service-account"), "{out}");
    assert!(out.contains("keystore (jks)"), "{out}");
    assert!(out.contains("private-key (pkcs8)"), "{out}");
    assert!(out.contains("ssh-key (ed25519)"), "{out}");
    assert!(out.contains("ssh-key (rsa, passphrase)"), "{out}");
    assert!(
        out.contains("~/.ssh/id_rsa"),
        "found through the credential-folder root: {out}"
    );
    assert_eq!(
        f.kinds(&["ls", "--secrets"]),
        [
            pair("id_ed25519", "ssh-key"),
            pair("id_rsa", "ssh-key"),
            pair("release.keystore", "keystore"),
            pair("sa.json", "service-account"),
            pair("server.key", "private-key"),
        ],
        "lookalikes (config.json, a certificate, a .pub, known_hosts) are left out"
    );
}

#[test]
fn kind_narrows_the_list_and_a_wrong_kind_says_what_exists() {
    let f = Fixture::new();
    assert_eq!(
        f.kinds(&["ls", "--kind", "ssh-key"]),
        [pair("id_ed25519", "ssh-key"), pair("id_rsa", "ssh-key")]
    );
    assert_eq!(
        f.kinds(&["ls", "--kind", "keystore", "--kind", "service-account"]),
        [
            pair("release.keystore", "keystore"),
            pair("sa.json", "service-account")
        ]
    );
    f.dv()
        .args(["ls", "--kind", "ssh"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown kind"))
        .stderr(predicate::str::contains("ssh-key"))
        .stderr(predicate::str::contains("keystore"));
    f.dv()
        .args(["ls", "--secrets", "--kind", "netrc"])
        .assert()
        .success()
        .stderr(predicate::str::contains("No secret files match"));
}

#[test]
fn json_secrets_can_be_found_by_their_key_names() {
    let f = Fixture::new();
    assert_eq!(
        f.kinds(&["ls", "--secrets", "-k", "private_key"]),
        [pair("sa.json", "service-account")]
    );
    assert!(f
        .kinds(&["ls", "--secrets", "-k", "no_such_key"])
        .is_empty());
    let rows = f.json(&["ls", "--secrets", "--json"]);
    let sa = rows
        .iter()
        .find(|r| r["kind"] == "service-account")
        .unwrap();
    let names: Vec<&str> = sa["key_names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k.as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"client_email") && names.contains(&"private_key"),
        "{names:?}"
    );
    assert_eq!(sa["key_count"], 4);
    let key = rows.iter().find(|r| r["kind"] == "ssh-key").unwrap();
    assert_eq!(key["key_count"], 0);
    assert!(
        key["detail"].as_str().unwrap().contains("ed25519")
            || key["detail"].as_str().unwrap().contains("rsa")
    );
}

#[test]
fn the_index_and_every_listing_hold_names_and_never_contents() {
    let f = Fixture::new();
    let mut everything = String::new();
    for args in [
        &["ls", "--secrets", "--json"][..],
        &["ls", "--secrets"],
        &["ls", "--json"],
        &["scan"],
    ] {
        let out = f.dv().args(args).assert().success().get_output().clone();
        everything.push_str(&String::from_utf8_lossy(&out.stdout));
        everything.push_str(&String::from_utf8_lossy(&out.stderr));
    }
    for needle in [
        "c2VjcmV0LWJvZHk",
        "bot@demo.example",
        "BEGIN",
        "openssh-key",
    ] {
        assert!(
            !everything.contains(needle),
            "{needle} leaked: {everything}"
        );
    }
    let raw = fs::read(f.data().join("index.db")).unwrap();
    for needle in [&b"c2VjcmV0LWJvZHk"[..], b"bot@demo.example", b"BEGIN"] {
        assert!(
            !raw.windows(needle.len()).any(|w| w == needle),
            "the index must not hold {:?}",
            String::from_utf8_lossy(needle)
        );
    }
}

#[test]
fn env_features_ignore_secret_files() {
    let f = Fixture::new();
    // a project's env file is still the only thing a project name means
    f.dv()
        .args(["get", "A", "-p", "app"])
        .assert()
        .success()
        .stdout("1\n");
    f.dv()
        .args(["show", "-p", "app", "--reveal"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("A")
                .and(predicate::str::contains("service-account").not())
                .and(predicate::str::contains("private").not()),
        );
    f.dv()
        .args(["show", "-p", "app", "-e", "service-account"])
        .assert()
        .failure();
    // `find` looks through variables, not through key files
    f.dv()
        .args(["find", "private_key"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("sa.json").not());
}

#[test]
fn show_describes_a_key_without_ever_printing_it() {
    let f = Fixture::new();
    let key = f.path("keys/id_ed25519");
    for extra in [&[][..], &["--reveal"]] {
        let out = f
            .dv()
            .arg("show")
            .arg(&key)
            .args(extra)
            .assert()
            .success()
            .get_output()
            .clone();
        let shown = String::from_utf8_lossy(&out.stdout).replace('\\', "/");
        assert!(shown.contains("ssh-key (ed25519)"), "{shown}");
        assert!(shown.contains("bytes"), "{shown}");
        assert!(shown.contains("fingerprint"), "{shown}");
        assert!(
            !shown.contains("BEGIN") && !shown.contains("openssh"),
            "{shown}"
        );
    }
    f.dv()
        .arg("show")
        .arg(&key)
        .arg("--reveal")
        .assert()
        .success()
        .stdout(predicate::str::contains("never printed"));
    // a keystore too
    f.dv()
        .arg("show")
        .arg(f.path("app/android/release.keystore"))
        .assert()
        .success()
        .stdout(predicate::str::contains("keystore (jks)"));
}

#[test]
fn show_a_json_secret_masks_its_fields_until_revealed() {
    let f = Fixture::new();
    let sa = f.path("app/secrets/sa.json");
    let masked = f
        .dv()
        .arg("show")
        .arg(&sa)
        .assert()
        .success()
        .get_output()
        .clone();
    let masked = String::from_utf8_lossy(&masked.stdout);
    assert!(masked.contains("service-account"), "{masked}");
    assert!(
        masked.contains("client_email") && masked.contains("private_key"),
        "{masked}"
    );
    assert!(masked.contains("********"), "{masked}");
    assert!(
        !masked.contains("bot@demo.example") && !masked.contains("c2VjcmV0LWJvZHk"),
        "{masked}"
    );

    f.dv()
        .arg("show")
        .arg(&sa)
        .arg("--reveal")
        .assert()
        .success()
        .stdout(predicate::str::contains("bot@demo.example"));
}

#[test]
fn show_a_secret_file_only_as_a_table_of_details() {
    let f = Fixture::new();
    let sa = f.path("app/secrets/sa.json");
    for extra in [&["--format", "json"][..], &["--keys", "private_key"]] {
        f.dv()
            .arg("show")
            .arg(&sa)
            .args(extra)
            .assert()
            .failure()
            .stderr(predicate::str::contains("service-account file"))
            .stderr(predicate::str::contains("table"));
    }
    // plain config JSON and ordinary files still go through the env path as before
    f.dv()
        .arg("show")
        .arg(f.path("app/.env"))
        .arg("--reveal")
        .assert()
        .success();
}

#[test]
fn mark_adds_a_file_by_hand_and_unmark_removes_it_or_silences_a_wrong_guess() {
    let f = Fixture::new();
    f.write("notes/passwords.txt", "bank: hunter2\n");
    let notes = f.path("notes/passwords.txt");
    let sa = f.path("app/secrets/sa.json");
    assert!(!f
        .kinds(&["ls", "--secrets"])
        .iter()
        .any(|(n, _)| n == "passwords.txt"));

    f.dv()
        .arg("mark")
        .arg(&notes)
        .assert()
        .success()
        .stdout(predicate::str::contains("Marked 1 file"));
    assert!(f
        .kinds(&["ls", "--secrets"])
        .contains(&pair("passwords.txt", "marked")));
    f.dv()
        .args(["ls", "--kind", "marked", "--paths"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("passwords.txt")
                .and(predicate::str::contains("sa.json").not()),
        );

    // silence a file dotvault found by itself
    f.dv().arg("unmark").arg(&sa).assert().success();
    assert!(!f
        .kinds(&["ls", "--secrets"])
        .iter()
        .any(|(n, _)| n == "sa.json"));
    // and bring it back
    f.dv().arg("mark").arg(&sa).assert().success();
    assert!(f
        .kinds(&["ls", "--secrets"])
        .contains(&pair("sa.json", "service-account")));

    // unmarking a hand-marked file removes it
    f.dv().arg("unmark").arg(&notes).assert().success();
    assert!(!f
        .kinds(&["ls", "--secrets"])
        .iter()
        .any(|(n, _)| n == "passwords.txt"));
    let config = fs::read_to_string(f.config().join("config.toml")).unwrap();
    assert!(config.contains("unmarked"), "{config}");
}

#[test]
fn mark_refuses_folders_and_missing_files_but_unmark_forgives_a_deleted_one() {
    let f = Fixture::new();
    f.dv()
        .arg("mark")
        .arg(f.path("app"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("is a folder"));
    f.dv()
        .arg("mark")
        .arg(f.path("nothing-here.txt"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot access"));
    f.dv()
        .arg("unmark")
        .arg(f.path("long-gone.pem"))
        .assert()
        .success();
    f.dv().arg("mark").assert().failure();
}

#[test]
fn a_secret_that_changes_is_re_read_without_a_new_scan() {
    let f = Fixture::new();
    let sa = f.path("app/secrets/sa.json");
    assert!(f
        .kinds(&["ls", "--secrets"])
        .contains(&pair("sa.json", "service-account")));
    std::thread::sleep(std::time::Duration::from_millis(30));
    fs::write(&sa, r#"{"port": 8080}"#).unwrap();
    assert!(
        !f.kinds(&["ls", "--secrets"])
            .iter()
            .any(|(n, _)| n == "sa.json"),
        "once it holds no secret it is no longer listed"
    );
}

#[test]
fn scan_counts_secret_files_apart_from_env_files() {
    let f = Fixture::new();
    f.dv()
        .arg("scan")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 env file in 1 project"))
        .stdout(predicate::str::contains("5 secret files found"));
}

#[test]
fn an_index_written_by_an_older_version_is_rebuilt() {
    let f = Fixture::new();
    fs::create_dir_all(f.data()).unwrap();
    {
        let old = rusqlite::Connection::open(f.data().join("index.db")).unwrap();
        old.execute_batch(
            "CREATE TABLE files (path TEXT PRIMARY KEY, project TEXT NOT NULL,
                project_root TEXT NOT NULL, label TEXT NOT NULL, is_example INTEGER NOT NULL,
                size INTEGER NOT NULL, mtime INTEGER NOT NULL, keys TEXT NOT NULL);
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO files VALUES ('/gone/.env', 'gone', '/gone', 'default', 0, 1, 1, 'A');
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let out = f.out(&["ls", "--paths"]);
    assert!(out.contains("app/.env") && !out.contains("/gone/"), "{out}");
    assert_eq!(f.kinds(&["ls", "--secrets"]).len(), 5);
}

#[cfg(unix)]
#[test]
fn a_hostile_file_name_cannot_drive_the_terminal() {
    let f = Fixture::new();
    let nasty = f.path("app/\u{1b}[31mevil.pem");
    fs::write(&nasty, pem("PRIVATE KEY", "c2VjcmV0")).unwrap();
    for args in [&["ls", "--secrets"][..], &["scan"]] {
        let out = f.dv().args(args).assert().success().get_output().clone();
        for stream in [&out.stdout, &out.stderr] {
            assert!(
                !String::from_utf8_lossy(stream).contains('\u{1b}'),
                "{args:?} printed a raw escape"
            );
        }
    }
    let shown = f
        .dv()
        .arg("show")
        .arg(&nasty)
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(!String::from_utf8_lossy(&shown.stdout).contains('\u{1b}'));
}

#[test]
fn ls_secrets_with_nothing_found_says_how_files_are_recognised() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join("app")).unwrap();
    fs::write(home.join("app/.env"), "A=1\n").unwrap();
    let mut cmd = cargo_bin_cmd!("dotvault");
    cmd.env("DOTVAULT_HOME", &home)
        .env("DOTVAULT_SCAN_ROOTS", &home)
        .env("DOTVAULT_CONFIG_DIR", tmp.path().join("c"))
        .env("DOTVAULT_DATA_DIR", tmp.path().join("d"))
        .env("NO_COLOR", "1")
        .args(["ls", "--secrets"])
        .assert()
        .success()
        .stderr(predicate::str::contains("No secret files match"))
        .stderr(predicate::str::contains("dotvault mark"));
}
