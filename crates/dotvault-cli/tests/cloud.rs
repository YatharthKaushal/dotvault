//! End-to-end tests of the account, vault and sharing commands: the real
//! `dotvault` binary talking to a real server (its own SQLite file, an
//! ephemeral port) with several users on separate config directories.

use std::fs;
use std::path::PathBuf;

use assert_cmd::cargo::cargo_bin_cmd;
use assert_cmd::Command;
use dotvault_core::api::{CreateShareRequest, CreateShareResponse};
use dotvault_core::crypto::{random_key, seal_json, AAD_SHARE};
use dotvault_core::payload::{EnvEntry, EnvPayload};
use dotvault_core::share_code;
use dotvault_server::config::RateLimit;
use dotvault_server::harness::{self, RunningServer};
use dotvault_server::Config;
use predicates::prelude::*;
use tempfile::TempDir;

// Drop order matters: the server must go before the runtime it runs on.
struct World {
    server: RunningServer,
    _rt: tokio::runtime::Runtime,
    tmp: TempDir,
    db: PathBuf,
}

impl World {
    fn new() -> World {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("server.db");
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let config = Config {
            db: db.display().to_string(),
            // every CLI login makes several auth calls from one address
            auth_rate: RateLimit::per_minute(100_000),
            share_open_rate: RateLimit::per_minute(100_000),
            ..Config::default()
        };
        let server = rt.block_on(harness::spawn(config)).unwrap();
        World {
            server,
            _rt: rt,
            tmp,
            db,
        }
    }

    fn url(&self) -> String {
        self.server.base_url()
    }

    /// A person with their own config dir and working directory.
    fn user(&self, name: &str, password: &str) -> User {
        let root = self
            .tmp
            .path()
            .join(format!("user-{name}-{}", rand_suffix()));
        let user = User {
            name: name.to_string(),
            password: password.to_string(),
            config: root.join("config"),
            data: root.join("data"),
            work: root.join("work"),
            home: root.join("home"),
            url: self.url(),
        };
        fs::create_dir_all(&user.work).unwrap();
        fs::create_dir_all(&user.home).unwrap();
        user
    }

    /// Everything the server has on disk, including the write-ahead log.
    fn db_bytes(&self) -> Vec<u8> {
        let mut all = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            let mut p = self.db.clone().into_os_string();
            p.push(suffix);
            if let Ok(b) = fs::read(PathBuf::from(p)) {
                all.extend(b);
            }
        }
        all
    }
}

fn rand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

struct User {
    name: String,
    password: String,
    config: PathBuf,
    data: PathBuf,
    work: PathBuf,
    /// An empty stand-in for the home folder, so no test ever scans a real one.
    home: PathBuf,
    url: String,
}

impl User {
    fn dv(&self) -> Command {
        let mut cmd = cargo_bin_cmd!("dotvault");
        cmd.env("DOTVAULT_CONFIG_DIR", &self.config)
            .env("DOTVAULT_DATA_DIR", &self.data)
            .env("DOTVAULT_HOME", &self.home)
            .env("DOTVAULT_SCAN_ROOTS", &self.home)
            .env("DOTVAULT_SERVER", &self.url)
            .env("DOTVAULT_USERNAME", &self.name)
            .env("DOTVAULT_PASSWORD", &self.password)
            .env("NO_COLOR", "1")
            .env_remove("DOTVAULT_TOKEN")
            .env_remove("DOTVAULT_NEW_PASSWORD")
            .env_remove("DOTVAULT_RECOVERY_KEY")
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            .current_dir(&self.work);
        cmd
    }

    /// Registers and returns the recovery key that was shown.
    fn register(&self) -> String {
        let out = self
            .dv()
            .arg("register")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let text = String::from_utf8(out).unwrap();
        text.split_whitespace()
            .find(|w| w.starts_with("DVR1-"))
            .unwrap_or_else(|| panic!("no recovery key in output:\n{text}"))
            .to_string()
    }

    fn login(&self) {
        self.dv().arg("login").assert().success();
    }

    fn write(&self, rel: &str, contents: &str) {
        let p = self.work.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.work.join(rel)).unwrap()
    }

    fn session_json(&self) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(self.config.join("session.json")).unwrap())
            .unwrap()
    }

    fn stdout_of(&self, args: &[&str]) -> String {
        let out = self
            .dv()
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out).unwrap()
    }

    /// Same account, another device: its own config dir and no session yet.
    fn other_device(&self, world: &World) -> User {
        let mut d = world.user(&self.name, &self.password);
        d.name = self.name.clone();
        d
    }
}

const ENV: &str = "DB_URL=postgres://app:s3cr3t-DB-pw@db.internal/app\n# billing\nSTRIPE_KEY=sk_live_TOP_SECRET_STRIPE\nPORT=4000\n";

// --- accounts ------------------------------------------------------------------

#[test]
fn account_lifecycle() {
    let w = World::new();
    let ada = w.user("ada", "ada-long-password-1");

    let recovery = ada.register();
    assert!(recovery.starts_with("DVR1-"), "{recovery}");

    ada.dv()
        .arg("whoami")
        .assert()
        .success()
        .stdout(predicate::str::contains("ada"))
        .stdout(predicate::str::contains(w.url()))
        .stdout(predicate::str::contains("stored on this device"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(ada.config.join("session.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "session file must be private");
    }

    // the server remembered for next time
    ada.dv()
        .args(["config"])
        .assert()
        .success()
        .stdout(predicate::str::contains(w.url()));

    ada.dv()
        .arg("logout")
        .assert()
        .success()
        .stdout(predicate::str::contains("Logged out."));
    ada.dv()
        .arg("whoami")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not logged in"));

    ada.login();
    ada.dv().arg("whoami").assert().success();

    // wrong password and unknown user give the same message
    ada.dv()
        .arg("login")
        .env("DOTVAULT_PASSWORD", "definitely-not-it-0000")
        .assert()
        .failure()
        .stderr(predicate::str::contains("wrong username or password"));
    ada.dv()
        .arg("login")
        .env("DOTVAULT_USERNAME", "nobody-here")
        .assert()
        .failure()
        .stderr(predicate::str::contains("wrong username or password"));
}

#[test]
fn registration_rules() {
    let w = World::new();
    let ada = w.user("ada", "ada-long-password-1");
    ada.register();

    // same name, any case
    let clone = w.user("ADA", "another-long-password-2");
    clone
        .dv()
        .arg("register")
        .assert()
        .failure()
        .stderr(predicate::str::contains("already taken"));

    // weak password is rejected before any network call
    let weak = w.user("weakling", "short");
    weak.dv()
        .arg("register")
        .env("DOTVAULT_SERVER", "http://127.0.0.1:1")
        .assert()
        .failure()
        .stderr(predicate::str::contains("at least 10 characters"));

    // invalid usernames are rejected by the server
    let bad = w.user("a b", "long-enough-password-3");
    bad.dv()
        .args(["register", "--username", "a b"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("username must be"));
}

#[test]
fn refuses_plain_http_to_remote_servers() {
    // The built-in default server is covered by unit tests: an end-to-end test of it
    // would call the real hosted server, so these tests always name their own.
    let w = World::new();
    let u = w.user("ada", "ada-long-password-1");
    u.dv()
        .args(["login", "--server", "http://example.com"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("plain http"));
}

#[test]
fn unreachable_server_is_reported_clearly() {
    let w = World::new();
    let u = w.user("ada", "ada-long-password-1");
    u.dv()
        .arg("login")
        .env("DOTVAULT_SERVER", "http://127.0.0.1:1")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "could not reach http://127.0.0.1:1",
        ));
}

#[test]
fn password_change_keeps_the_vault_and_signs_other_devices_out() {
    let w = World::new();
    let ada = w.user("ada", "ada-old-password-1");
    ada.register();
    ada.write(".env", ENV);
    ada.dv().args(["push", "--name", "mine"]).assert().success();

    let phone = ada.other_device(&w);
    phone.login();
    phone.dv().arg("whoami").assert().success();

    ada.dv()
        .arg("passwd")
        .env("DOTVAULT_NEW_PASSWORD", "ada-brand-new-password-2")
        .assert()
        .success()
        .stdout(predicate::str::contains("Password changed"));

    // the old password is dead, the new one works, and the vault is intact
    ada.dv()
        .arg("login")
        .assert()
        .failure()
        .stderr(predicate::str::contains("wrong username or password"));
    let mut fresh = w.user("ada", "ada-brand-new-password-2");
    fresh.name = "ada".into();
    fresh.login();
    let shown = fresh.stdout_of(&["vault", "show", "mine", "--reveal", "--format", "dotenv"]);
    assert!(shown.contains("s3cr3t-DB-pw"), "{shown}");

    // the current device stays logged in; the other one does not
    ada.dv().arg("whoami").assert().success();
    phone
        .dv()
        .arg("whoami")
        .assert()
        .failure()
        .stderr(predicate::str::contains("dotvault login"));

    // wrong current password
    ada.dv()
        .arg("passwd")
        .env("DOTVAULT_PASSWORD", "not-the-current-password")
        .env("DOTVAULT_NEW_PASSWORD", "whatever-new-password-3")
        .assert()
        .failure()
        .stderr(predicate::str::contains("wrong current password"));
}

#[test]
fn recovery_key_restores_access_to_the_vault() {
    let w = World::new();
    let ada = w.user("ada", "ada-forgotten-password-1");
    let recovery = ada.register();
    ada.write(".env", ENV);
    ada.dv().args(["push", "--name", "mine"]).assert().success();

    let phone = ada.other_device(&w);
    phone.login();

    // wrong key first
    ada.dv()
        .arg("recover")
        .env("DOTVAULT_RECOVERY_KEY", "DVR1-AAAA-AAAA")
        .env("DOTVAULT_NEW_PASSWORD", "ada-recovered-password-2")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "does not look like a recovery key",
        ));

    let mut device = w.user("ada", "ada-recovered-password-2");
    device.name = "ada".into();
    device
        .dv()
        .arg("recover")
        .env("DOTVAULT_RECOVERY_KEY", &recovery)
        .env("DOTVAULT_NEW_PASSWORD", "ada-recovered-password-2")
        .assert()
        .success()
        .stdout(predicate::str::contains("Password reset"));

    let shown = device.stdout_of(&["vault", "show", "mine", "--reveal", "--format", "dotenv"]);
    assert!(
        shown.contains("s3cr3t-DB-pw"),
        "the recovered account must open the old vault: {shown}"
    );

    // every older session ended, and the forgotten password is gone
    phone.dv().arg("whoami").assert().failure();
    ada.dv().arg("login").assert().failure();
}

#[test]
fn login_without_caching_the_vault_key_asks_for_the_password() {
    let w = World::new();
    let ada = w.user("ada", "ada-long-password-1");
    ada.register();
    ada.write(".env", ENV);
    ada.dv().args(["push", "--name", "mine"]).assert().success();

    ada.dv()
        .args(["login", "--no-cache-key"])
        .assert()
        .success();
    let session = fs::read_to_string(ada.config.join("session.json")).unwrap();
    assert!(
        !session.contains("\"vault_key\""),
        "key must not be on disk: {session}"
    );

    ada.dv()
        .args(["vault", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("mine"));
    ada.dv()
        .args(["vault", "ls"])
        .env("DOTVAULT_PASSWORD", "the-wrong-password-9")
        .assert()
        .failure()
        .stderr(predicate::str::contains("wrong password"));

    // sharing needs no vault key, so no password
    ada.dv()
        .args(["shares", "ls"])
        .env("DOTVAULT_PASSWORD", "the-wrong-password-9")
        .assert()
        .success();
    ada.dv()
        .arg("whoami")
        .assert()
        .success()
        .stdout(predicate::str::contains("not stored"));
}

#[test]
fn tokens_from_the_environment_work_but_have_no_vault_key_and_logout_revokes_them() {
    let w = World::new();
    let ada = w.user("ada", "ada-long-password-1");
    ada.register();
    let token = ada.session_json()["token"].as_str().unwrap().to_string();

    let ci = w.user("ci", "unused-password-0");
    let with_token = |args: &[&str]| {
        let mut cmd = ci.dv();
        cmd.env("DOTVAULT_TOKEN", &token).args(args);
        cmd
    };
    with_token(&["whoami"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ada"))
        .stdout(predicate::str::contains("unavailable"));
    with_token(&["shares", "ls"]).assert().success();
    with_token(&["vault", "ls"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no vault key"));

    ada.dv().arg("logout").assert().success();
    with_token(&["whoami"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("dotvault login"));
}

// --- vault -----------------------------------------------------------------------

#[test]
fn pulling_into_a_file_records_each_changed_variable_in_its_history() {
    let w = World::new();
    let ada = w.user("ada", "ada-long-password-1");
    ada.register();
    ada.write("acme-api/package.json", "{}");
    ada.write("acme-api/.env", ENV);
    ada.dv()
        .current_dir(ada.work.join("acme-api"))
        .arg("push")
        .assert()
        .success();

    ada.write("target.env", "EXTRA=1\nPORT=1111\n");
    ada.dv()
        .args([
            "pull",
            "acme-api/default",
            "--out",
            "target.env",
            "--merge",
            "--keys",
            "PORT,DB_URL",
        ])
        .assert()
        .success();

    let versions = |key: &str| -> Vec<(String, Option<String>)> {
        let out = ada
            .dv()
            .args(["log", key, "--json", "--reveal", "-f"])
            .arg(ada.work.join("target.env"))
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let mut rows: Vec<serde_json::Value> = serde_json::from_slice(&out).unwrap();
        rows.reverse();
        rows.iter()
            .map(|r| {
                (
                    r["action"].as_str().unwrap().to_string(),
                    r["value"].as_str().map(str::to_string),
                )
            })
            .collect()
    };
    // PORT was already in the file at 1111, then the pull changed it
    assert_eq!(
        versions("PORT"),
        [
            ("baseline".to_string(), Some("1111".to_string())),
            ("pull".to_string(), Some("4000".to_string())),
        ]
    );
    // DB_URL arrived with the pull
    let db = versions("DB_URL");
    assert_eq!(db.len(), 1);
    assert_eq!(db[0].0, "pull");
    assert!(db[0].1.as_deref().unwrap().contains("s3cr3t-DB-pw"));
    // EXTRA was not touched, but it was there when tracking began
    assert_eq!(
        versions("EXTRA"),
        [("baseline".to_string(), Some("1".to_string()))]
    );

    // and the pull can be undone
    ada.dv()
        .args(["revert", "PORT", "-f"])
        .arg(ada.work.join("target.env"))
        .assert()
        .success();
    assert!(
        ada.read("target.env").contains("PORT=1111"),
        "{}",
        ada.read("target.env")
    );

    // a forced pull over a whole file records the variables it changes too
    ada.write("forced.env", "PORT=2222\n");
    ada.dv()
        .args([
            "pull",
            "acme-api/default",
            "--out",
            "forced.env",
            "--force",
            "--keys",
            "PORT",
        ])
        .assert()
        .success();
    let out = ada
        .dv()
        .args(["log", "PORT", "--json", "--reveal", "-f"])
        .arg(ada.work.join("forced.env"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out).unwrap();
    assert_eq!(rows[0]["action"], "pull");
    assert_eq!(rows[0]["value"], "4000");
}

#[test]
fn vault_push_pull_roundtrip_with_versions_merge_and_delete() {
    let w = World::new();
    let ada = w.user("ada", "ada-long-password-1");
    ada.register();
    ada.write("acme-api/package.json", "{}");
    ada.write("acme-api/.env", ENV);
    let proj = ada.work.join("acme-api");

    // default name comes from project and env
    ada.dv()
        .current_dir(&proj)
        .arg("push")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "acme-api/default (v1, 3 variables)",
        ));
    ada.dv()
        .args(["vault", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("acme-api/default"))
        .stdout(predicate::str::contains("v1"));

    // masked unless asked
    ada.dv()
        .args(["vault", "show", "acme-api/default"])
        .assert()
        .success()
        .stdout(predicate::str::contains("DB_URL"))
        .stdout(predicate::str::contains("s3cr3t").not());
    ada.dv()
        .args([
            "vault",
            "show",
            "acme-api/default",
            "--reveal",
            "--keys",
            "PORT",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("4000"))
        .stdout(predicate::str::contains("STRIPE").not());

    // pulling needs a destination and never overwrites silently
    ada.dv()
        .args(["pull", "acme-api/default"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--out"));
    ada.dv()
        .args(["pull", "acme-api/default", "--out", "restored.env"])
        .assert()
        .success();
    let restored = ada.read("restored.env");
    assert!(
        restored.contains("DB_URL=postgres://app:s3cr3t-DB-pw@db.internal/app"),
        "{restored}"
    );
    assert!(restored.contains("PORT=4000"));
    ada.dv()
        .args(["pull", "acme-api/default", "--out", "restored.env"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));

    // merge keeps what is already in the file
    ada.write("target.env", "# mine\nEXTRA=1\nPORT=1111\n");
    ada.dv()
        .args([
            "pull",
            "acme-api/default",
            "--out",
            "target.env",
            "--merge",
            "--keys",
            "PORT,DB_URL",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 added"))
        .stdout(predicate::str::contains("1 changed"));
    let merged = ada.read("target.env");
    assert!(
        merged.starts_with("# mine\nEXTRA=1\nPORT=4000\n"),
        "{merged}"
    );
    assert!(merged.contains("DB_URL="));
    assert!(
        !merged.contains("STRIPE"),
        "--keys must limit what is pulled"
    );

    // stdout form
    let printed = ada.stdout_of(&["pull", "acme-api/default", "--stdout", "--keys", "PORT"]);
    assert_eq!(printed, "PORT=4000\n");

    // pushing again creates a new version
    ada.write("acme-api/.env", &format!("{ENV}NEW_KEY=hello\n"));
    ada.dv()
        .current_dir(&proj)
        .arg("push")
        .assert()
        .success()
        .stdout(predicate::str::contains("v2, 4 variables"));
    ada.dv()
        .args(["vault", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("v2"));

    // a subset under another name
    ada.dv()
        .current_dir(&proj)
        .args(["push", "--name", "acme/just-port", "--keys", "PORT"])
        .assert()
        .success()
        .stdout(predicate::str::contains("(v1, 1 variable)"));
    ada.dv()
        .current_dir(&proj)
        .args(["push", "--name", "acme/bad", "--keys", "NOPE"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("NOPE"));

    // names can be abbreviated when unambiguous
    ada.dv()
        .args(["vault", "show", "just-port", "--reveal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("4000"));
    ada.dv()
        .args(["vault", "show", "acme"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("matches several items"));

    // delete needs confirmation, which a script must give explicitly
    ada.dv()
        .args(["vault", "rm", "just-port"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--yes"));
    ada.dv()
        .args(["vault", "rm", "just-port", "--yes"])
        .assert()
        .success();
    ada.dv()
        .args(["pull", "just-port", "--stdout"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no vault item"));
}

#[test]
fn vault_is_private_per_user() {
    let w = World::new();
    let ada = w.user("ada", "ada-long-password-1");
    let bob = w.user("bob", "bob-long-password-1");
    ada.register();
    bob.register();
    ada.write(".env", ENV);
    ada.dv()
        .args(["push", "--name", "ada-only"])
        .assert()
        .success();
    bob.dv()
        .args(["vault", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ada-only").not());
    bob.dv()
        .args(["vault", "show", "ada-only"])
        .assert()
        .failure();
}

// --- sharing ---------------------------------------------------------------------

#[test]
fn share_open_save_audit_and_revoke() {
    let w = World::new();
    let ada = w.user("ada", "ada-long-password-1");
    let bob = w.user("bob", "bob-long-password-1");
    let ada_recovery = ada.register();
    bob.register();
    ada.write(".env", ENV);

    // ada shares ONE variable, at most 3 opens
    let link = ada
        .stdout_of(&[
            "share",
            ".env",
            "--keys",
            "DB_URL",
            "--expires",
            "1h",
            "--max-views",
            "3",
            "--note",
            "for the migration",
        ])
        .trim()
        .to_string();
    let parsed = share_code::parse(&link).unwrap();
    assert_eq!(parsed.origin.as_deref(), Some(w.url().as_str()));
    let code = share_code::encode_code(&parsed.id, &parsed.key);

    // opening without saying what to do must not use up a view
    bob.dv()
        .args(["open", &link])
        .assert()
        .failure()
        .stderr(predicate::str::contains("say what to do"));
    ada.dv()
        .args(["shares", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0/3"))
        .stdout(predicate::str::contains("active"));

    // bob sees only the shared variable, by code, with the sender and note
    let out = bob
        .dv()
        .args(["open", &code, "--reveal"])
        .assert()
        .success()
        .get_output()
        .clone();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert!(
        stdout.contains("DB_URL") && stdout.contains("s3cr3t-DB-pw"),
        "{stdout}"
    );
    for hidden in ["STRIPE", "sk_live_TOP_SECRET_STRIPE", "PORT", "4000"] {
        assert!(
            !stdout.contains(hidden) && !stderr.contains(hidden),
            "{hidden} leaked:\n{stdout}\n{stderr}"
        );
    }
    assert!(stderr.contains("From ada"), "{stderr}");
    assert!(stderr.contains("for the migration"), "{stderr}");
    assert!(stderr.contains("2 opens left"), "{stderr}");

    // write it to a file, then keep a copy in bob's own vault via the link
    bob.dv()
        .args(["open", &link, "--write", ".env"])
        .assert()
        .success();
    assert_eq!(
        bob.read(".env"),
        "DB_URL=postgres://app:s3cr3t-DB-pw@db.internal/app\n"
    );
    bob.dv()
        .args(["open", &code, "--save"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Saved to your vault as shared/ada/",
        ));
    assert!(bob.stdout_of(&["vault", "ls"]).contains("shared/ada/"));
    let saved = bob.stdout_of(&[
        "vault",
        "show",
        "shared/ada",
        "--reveal",
        "--format",
        "dotenv",
    ]);
    assert_eq!(
        saved,
        "DB_URL=postgres://app:s3cr3t-DB-pw@db.internal/app\n"
    );

    // that was the 3rd open: the limit now applies
    bob.dv()
        .args(["open", &code, "--reveal"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("open limit"));

    // ada sees who did what, using a short id prefix
    let id_prefix = &parsed.id[..6];
    let log = ada.stdout_of(&["shares", "log", id_prefix]);
    assert!(
        log.contains("bob") && log.contains("opened") && log.contains("saved"),
        "{log}"
    );
    assert!(!log.contains("ada"), "ada never opened it: {log}");
    ada.dv()
        .args(["shares", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("3/3"));

    // revoke kills the link even for a share with views left
    let second = ada
        .stdout_of(&["share", ".env", "--keys", "PORT"])
        .trim()
        .to_string();
    let second_id = share_code::parse(&second).unwrap().id;
    ada.dv()
        .args(["shares", "revoke", &second_id[..5]])
        .assert()
        .success();
    bob.dv()
        .args(["open", &second, "--reveal"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("revoked"));
    ada.dv()
        .args(["shares", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("revoked"));

    // only the owner can see or revoke
    bob.dv()
        .args(["shares", "log", &parsed.id])
        .assert()
        .failure();
    bob.dv()
        .args(["shares", "revoke", &parsed.id])
        .assert()
        .failure();

    // The server never held anything readable: no secret, password, key or token.
    let db = String::from_utf8_lossy(&w.db_bytes()).into_owned();
    let tokens: Vec<String> = [&ada, &bob]
        .iter()
        .map(|u| u.session_json()["token"].as_str().unwrap().to_string())
        .collect();
    let mut needles: Vec<String> = vec![
        "s3cr3t-DB-pw".into(),
        "sk_live_TOP_SECRET_STRIPE".into(),
        "for the migration".into(),
        "postgres://".into(),
        "STRIPE_KEY".into(),
        "DB_URL".into(),
        "ada-long-password-1".into(),
        "bob-long-password-1".into(),
        ada_recovery.clone(),
        ada_recovery.replace('-', ""),
    ];
    needles.extend(tokens);
    needles.push(base64_url(&parsed.key[..]));
    for needle in &needles {
        assert!(
            !db.contains(needle.as_str()),
            "server database contains {needle:?}"
        );
    }
    // sanity: the scan can actually see plaintext metadata that IS stored
    assert!(
        db.contains("ada") && db.contains("bob"),
        "usernames are stored, so the scan works"
    );
}

fn base64_url(bytes: &[u8]) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD.encode(bytes)
}

#[test]
fn share_input_validation_and_failure_modes() {
    let w = World::new();
    let ada = w.user("ada", "ada-long-password-1");
    let bob = w.user("bob", "bob-long-password-1");
    ada.register();
    bob.register();
    ada.write(".env", ENV);

    // bad options fail before anything is created
    for (args, needle) in [
        (
            vec!["--expires", "10s"],
            "--expires must be between 1m and 30d",
        ),
        (vec!["--expires", "31d"], "--expires must be between"),
        (vec!["--expires", "soon"], "not a duration"),
        (vec!["--max-views", "0"], "--max-views must be between"),
        (vec!["--keys", "NOPE"], "not set in"),
    ] {
        ada.dv()
            .arg("share")
            .args(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(needle));
    }
    ada.dv()
        .args(["shares", "ls"])
        .assert()
        .success()
        .stderr(predicate::str::contains("not created any shares"));

    // sharing needs a login
    let stranger = w.user("stranger", "stranger-long-pw-1");
    stranger.write(".env", ENV);
    stranger
        .dv()
        .arg("share")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not logged in"));

    let link = ada
        .stdout_of(&["share", ".env", "--keys", "PORT", "--max-views", "1"])
        .trim()
        .to_string();
    let parsed = share_code::parse(&link).unwrap();
    let code = share_code::encode_code(&parsed.id, &parsed.key);

    // a damaged key part cannot decrypt
    let mut damaged = code.clone();
    let pos = damaged.len() - 20;
    let replacement = if damaged.as_bytes()[pos] == b'A' {
        "B"
    } else {
        "A"
    };
    damaged.replace_range(pos..pos + 1, replacement);
    bob.dv()
        .args(["open", &damaged, "--reveal"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("could not decrypt"));

    // garbage and unknown ids
    bob.dv()
        .args(["open", "not-a-code", "--reveal"])
        .assert()
        .failure();
    let unknown = share_code::encode_code(&share_code::new_id().unwrap(), &parsed.key);
    bob.dv()
        .args(["open", &unknown, "--reveal"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no such share"));

    // a link for another server is refused without sending anything there
    let foreign = format!(
        "https://other.example/s/{}#{}",
        parsed.id,
        base64_url(&parsed.key[..])
    );
    bob.dv()
        .args(["open", &foreign, "--reveal"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("log in there first"));

    // refuses to overwrite a file BEFORE opening (the view was used by the damaged attempt,
    // so use a fresh single-view share to prove nothing is consumed)
    let fresh = ada
        .stdout_of(&["share", ".env", "--keys", "PORT", "--max-views", "1"])
        .trim()
        .to_string();
    bob.write("taken.env", "KEEP=1\n");
    bob.dv()
        .args(["open", &fresh, "--write", "taken.env"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
    assert_eq!(bob.read("taken.env"), "KEEP=1\n");
    // ...and the single view is still available
    bob.dv()
        .args(["open", &fresh, "--reveal"])
        .assert()
        .success();
    bob.dv()
        .args(["open", &fresh, "--reveal"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("open limit"));

    // merging into an existing file works and keeps its content
    let third = ada
        .stdout_of(&["share", ".env", "--keys", "PORT,DB_URL"])
        .trim()
        .to_string();
    bob.dv()
        .args(["open", &third, "--write", "taken.env", "--merge"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 added"));
    let merged = bob.read("taken.env");
    assert!(
        merged.starts_with("KEEP=1\n") && merged.contains("PORT=4000"),
        "{merged}"
    );
}

#[test]
fn sharing_from_the_vault_and_from_project_selectors() {
    let w = World::new();
    let ada = w.user("ada", "ada-long-password-1");
    let bob = w.user("bob", "bob-long-password-1");
    ada.register();
    bob.register();
    ada.write("acme-api/package.json", "{}");
    ada.write("acme-api/.env.production", ENV);
    ada.dv()
        .current_dir(ada.work.join("acme-api"))
        .args(["push", ".env.production"])
        .assert()
        .success()
        .stdout(predicate::str::contains("acme-api/production"));

    // share straight from the vault item
    let link = ada
        .stdout_of(&[
            "share",
            "--vault",
            "acme-api/production",
            "--keys",
            "STRIPE_KEY",
        ])
        .trim()
        .to_string();
    let out = bob.stdout_of(&["open", &link, "--reveal", "--format", "dotenv"]);
    assert_eq!(out, "STRIPE_KEY=sk_live_TOP_SECRET_STRIPE\n");

    // and via a path
    let link = ada
        .stdout_of(&["share", "acme-api/.env.production", "--keys", "PORT"])
        .trim()
        .to_string();
    assert_eq!(
        bob.stdout_of(&["open", &link, "--reveal", "--format", "dotenv"]),
        "PORT=4000\n"
    );

    // vault and path are mutually exclusive
    ada.dv()
        .args(["share", ".env", "--vault", "x"])
        .assert()
        .failure();
}

// --- hostile content -------------------------------------------------------------

/// Uploads a payload no honest client would produce, using `user`'s real session,
/// and returns the share code.
fn upload_raw_share(world: &World, user: &User, payload: &EnvPayload) -> String {
    let token = user.session_json()["token"].as_str().unwrap().to_string();
    let key = random_key().unwrap();
    let body = serde_json::to_vec(&CreateShareRequest {
        payload_ct: seal_json(&key, payload, AAD_SHARE).unwrap(),
        ttl_secs: Some(3600),
        max_views: None,
    })
    .unwrap();
    let mut response = ureq::post(&format!("{}/v1/shares", world.url()))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .send(&body[..])
        .unwrap();
    let created: CreateShareResponse =
        serde_json::from_slice(&response.body_mut().read_to_vec().unwrap()).unwrap();
    share_code::encode_code(&created.id, &key)
}

#[test]
fn a_hostile_share_cannot_attack_the_recipient() {
    let w = World::new();
    let eve = w.user("eve", "eve-long-password-1");
    let bob = w.user("bob", "bob-long-password-1");
    eve.register();
    bob.register();

    let entry = |k: &str, v: &str| EnvEntry {
        key: k.into(),
        value: v.into(),
    };
    let code = upload_raw_share(
        &w,
        &eve,
        &EnvPayload {
            v: 1,
            // title-setting and screen-clearing escape sequences in the label and note
            source: "evil\u{1b}]0;pwned\u{7}".into(),
            note: Some("\u{1b}[2Jclear the screen".into()),
            entries: vec![
                entry("GOOD", "fine"),
                entry("X; touch PWNED", "v"),
                entry("ANSI", "red\u{1b}[31m clipboard\u{1b}]52;c;ZXZpbA==\u{7}"),
                entry("SUB", "$(touch PWNED)"),
            ],
        },
    );

    // Printed as a table: no raw control character reaches the terminal, on either stream.
    let out = bob
        .dv()
        .args(["open", &code, "--reveal"])
        .assert()
        .success()
        .get_output()
        .clone();
    for (name, bytes) in [("stdout", &out.stdout), ("stderr", &out.stderr)] {
        assert!(
            !bytes.contains(&0x1b) && !bytes.contains(&0x07),
            "raw control character on {name}: {:?}",
            String::from_utf8_lossy(bytes)
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("GOOD") && stdout.contains("fine"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\\u{1b}[31m"),
        "escapes should be visible, not executed: {stdout}"
    );

    // Shell format: only safe names are emitted, and the odd ones are reported.
    let shell_out = bob
        .dv()
        .args(["open", &code, "--reveal", "--format", "shell"])
        .assert()
        .success()
        .get_output()
        .clone();
    let shell = String::from_utf8_lossy(&shell_out.stdout).into_owned();
    assert!(!shell.contains("X; touch"), "unsafe name emitted:\n{shell}");
    assert!(shell.contains("export GOOD='fine'\n"), "{shell}");
    assert!(shell.contains("export SUB='$(touch PWNED)'\n"), "{shell}");
    assert!(
        String::from_utf8_lossy(&shell_out.stderr).contains("left out 1 variable"),
        "{}",
        String::from_utf8_lossy(&shell_out.stderr)
    );
    // ...and a real shell sourcing it runs nothing.
    #[cfg(unix)]
    {
        fs::write(bob.work.join("received.sh"), &shell).unwrap();
        let ran = std::process::Command::new("sh")
            .current_dir(&bob.work)
            .args(["-c", ". ./received.sh && printf %s \"$SUB\""])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&ran.stdout), "$(touch PWNED)");
        assert!(
            !bob.work.join("PWNED").exists(),
            "sourcing the received output executed code"
        );
    }

    // Writing a file refuses the bad name outright and leaves nothing behind.
    bob.dv()
        .args(["open", &code, "--write", "received.env"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid variable name"));
    assert!(!bob.work.join("received.env").exists());

    // Saving to the vault keeps the sender's label from reaching the terminal.
    let saved = bob
        .dv()
        .args(["open", &code, "--save"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(!saved.stdout.contains(&0x1b) && !saved.stdout.contains(&0x07));
    let listing = bob
        .dv()
        .args(["vault", "ls"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(!listing.contains(&0x1b) && !listing.contains(&0x07));
    assert!(String::from_utf8_lossy(&listing).contains("shared/eve/evil"));
}
