//! `get` with several keys or globs, and `get --copy`.
//!
//! Copying never touches a real clipboard here: debug builds send the value to a
//! file named by `DOTVAULT_CLIPBOARD_FILE`, and `DOTVAULT_CLIPBOARD_SECS` shortens
//! the time before it is cleared.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin_cmd;
use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

const ENV_FILE: &str = "\
DATABASE_URL=postgres://user:pw-1f3a@host/db
RAZORPAY_KEY_ID=rzp_id
RAZORPAY_KEY_SECRET=\"has space & 'quote'\"
LIVEKIT_URL=wss://livekit.example
";

struct Fixture {
    tmp: TempDir,
    file: PathBuf,
    clip: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("app.env");
        fs::write(&file, ENV_FILE).unwrap();
        Fixture {
            clip: tmp.path().join("clipboard.txt"),
            file,
            tmp,
        }
    }

    /// `dotvault`, with the clipboard redirected to a file that clears after `secs`.
    fn dv(&self, secs: u64) -> Command {
        let mut cmd = cargo_bin_cmd!("dotvault");
        cmd.env("DOTVAULT_HOME", self.tmp.path().join("home"))
            .env("DOTVAULT_CONFIG_DIR", self.tmp.path().join("config"))
            .env("DOTVAULT_DATA_DIR", self.tmp.path().join("data"))
            .env("DOTVAULT_CLIPBOARD_FILE", &self.clip)
            .env("DOTVAULT_CLIPBOARD_SECS", secs.to_string())
            .env("NO_COLOR", "1")
            .current_dir(self.tmp.path());
        cmd
    }

    fn clipboard(&self) -> Option<String> {
        fs::read_to_string(&self.clip).ok()
    }

    fn wait_for_clipboard(&self, want: &str, within: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < within {
            if self.clipboard().as_deref() == Some(want) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.clipboard().as_deref() == Some(want)
    }
}

fn stdout_of(cmd: &mut Command) -> String {
    String::from_utf8(cmd.assert().success().get_output().stdout.clone()).unwrap()
}

#[test]
fn one_name_still_prints_the_bare_value() {
    let b = Fixture::new();
    let out = stdout_of(b.dv(1).args(["get", "DATABASE_URL", "-f"]).arg(&b.file));
    assert_eq!(out, "postgres://user:pw-1f3a@host/db\n");
}

#[test]
fn several_names_print_key_value_lines_in_the_order_given() {
    let b = Fixture::new();
    let out = stdout_of(
        b.dv(1)
            .args(["get", "LIVEKIT_URL", "DATABASE_URL", "-f"])
            .arg(&b.file),
    );
    assert_eq!(
        out,
        "LIVEKIT_URL=wss://livekit.example\nDATABASE_URL=postgres://user:pw-1f3a@host/db\n"
    );
}

#[test]
fn a_glob_prints_every_match_and_quotes_what_needs_it() {
    let b = Fixture::new();
    let out = stdout_of(b.dv(1).args(["get", "RAZORPAY_*", "-f"]).arg(&b.file));
    assert_eq!(
        out,
        "RAZORPAY_KEY_ID=rzp_id\nRAZORPAY_KEY_SECRET=\"has space & 'quote'\"\n"
    );
    // what it printed can be read back as a dotenv file
    let back = dotvault_core::dotenv::DotEnv::parse(&out);
    assert_eq!(back.get("RAZORPAY_KEY_SECRET"), Some("has space & 'quote'"));
}

#[test]
fn a_glob_matching_one_variable_still_prints_a_line_not_a_bare_value() {
    let b = Fixture::new();
    let out = stdout_of(b.dv(1).args(["get", "LIVEKIT_*", "-f"]).arg(&b.file));
    assert_eq!(out, "LIVEKIT_URL=wss://livekit.example\n");
}

#[test]
fn a_missing_name_fails_and_prints_nothing() {
    let b = Fixture::new();
    b.dv(1)
        .args(["get", "DATABASE_URL", "NOPE", "ZZ_*", "-f"])
        .arg(&b.file)
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains("NOPE is not set"))
        .stderr(predicate::str::contains("no variable matches ZZ_*"));
}

#[test]
fn names_and_globs_are_case_sensitive() {
    let b = Fixture::new();
    b.dv(1)
        .args(["get", "database_url", "-f"])
        .arg(&b.file)
        .assert()
        .failure();
    b.dv(1)
        .args(["get", "razorpay_*", "-f"])
        .arg(&b.file)
        .assert()
        .failure();
}

#[test]
fn show_keys_accepts_globs() {
    let b = Fixture::new();
    let out = stdout_of(
        b.dv(1)
            .arg("show")
            .arg(&b.file)
            .args(["--keys", "RAZORPAY_*,LIVEKIT_URL"]),
    );
    for key in ["RAZORPAY_KEY_ID", "RAZORPAY_KEY_SECRET", "LIVEKIT_URL"] {
        assert!(out.contains(key), "{out}");
    }
    assert!(!out.contains("DATABASE_URL"), "{out}");
}

#[test]
fn show_keys_warns_about_what_matched_nothing() {
    let b = Fixture::new();
    b.dv(1)
        .arg("show")
        .arg(&b.file)
        .args(["--keys", "DATABASE_URL,NOPE_*"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no variable matches NOPE_*"))
        .stdout(predicate::str::contains("DATABASE_URL"));
}

#[test]
fn copy_puts_the_exact_value_on_the_clipboard_and_prints_nothing_else() {
    let b = Fixture::new();
    b.dv(30)
        .args(["get", "DATABASE_URL", "-c", "-f"])
        .arg(&b.file)
        .assert()
        .success()
        .stdout("")
        .stderr(predicate::str::contains(
            "Copied DATABASE_URL to the clipboard",
        ))
        .stderr(predicate::str::contains("pw-1f3a").not());
    // no trailing newline, so pasting into a password field or a terminal adds nothing
    assert_eq!(
        b.clipboard().as_deref(),
        Some("postgres://user:pw-1f3a@host/db")
    );
}

#[test]
fn copy_of_several_variables_is_dotenv_lines_without_a_trailing_newline() {
    let b = Fixture::new();
    b.dv(30)
        .args(["get", "RAZORPAY_*", "LIVEKIT_URL", "--copy", "-f"])
        .arg(&b.file)
        .assert()
        .success()
        .stdout("")
        .stderr(predicate::str::contains("Copied 3 variables"));
    assert_eq!(
        b.clipboard().as_deref(),
        Some("RAZORPAY_KEY_ID=rzp_id\nRAZORPAY_KEY_SECRET=\"has space & 'quote'\"\nLIVEKIT_URL=wss://livekit.example")
    );
}

#[test]
fn copy_is_cleared_after_the_delay() {
    let b = Fixture::new();
    b.dv(1)
        .args(["get", "DATABASE_URL", "-c", "-f"])
        .arg(&b.file)
        .assert()
        .success();
    assert_eq!(
        b.clipboard().as_deref(),
        Some("postgres://user:pw-1f3a@host/db")
    );
    assert!(
        b.wait_for_clipboard("", Duration::from_secs(10)),
        "still holds {:?}",
        b.clipboard()
    );
}

#[test]
fn copy_leaves_alone_what_was_copied_in_the_meantime() {
    let b = Fixture::new();
    b.dv(2)
        .args(["get", "DATABASE_URL", "-c", "-f"])
        .arg(&b.file)
        .assert()
        .success();
    fs::write(&b.clip, "something else").unwrap();
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(b.clipboard().as_deref(), Some("something else"));
}

#[test]
fn copy_does_not_wait_for_the_helper_that_clears_it_later() {
    // The helper lives for 20 s. Capturing output waits for every holder of the
    // pipe, so a helper that inherited it would make this call take that long.
    let b = Fixture::new();
    let start = Instant::now();
    b.dv(20)
        .args(["get", "DATABASE_URL", "-c", "-f"])
        .arg(&b.file)
        .assert()
        .success();
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "took {:?}",
        start.elapsed()
    );
}

#[test]
fn copy_of_a_missing_variable_touches_nothing() {
    let b = Fixture::new();
    fs::write(&b.clip, "keep me").unwrap();
    b.dv(1)
        .args(["get", "NOPE", "-c", "-f"])
        .arg(&b.file)
        .assert()
        .failure()
        .stderr(predicate::str::contains("NOPE is not set"));
    assert_eq!(b.clipboard().as_deref(), Some("keep me"));
}

#[test]
fn copy_with_no_clipboard_says_so_and_never_prints_the_value() {
    // Linux only: with no display there is certainly no clipboard to reach.
    if !cfg!(target_os = "linux") {
        return;
    }
    let b = Fixture::new();
    let mut cmd = b.dv(1);
    cmd.env_remove("DOTVAULT_CLIPBOARD_FILE")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY");
    cmd.args(["get", "DATABASE_URL", "-c", "-f"])
        .arg(&b.file)
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains("no clipboard is available"))
        .stderr(predicate::str::contains("SSH"))
        .stderr(predicate::str::contains("pw-1f3a").not());
}

#[test]
fn the_value_never_appears_on_the_helpers_command_line() {
    // Linux only: command lines are readable under /proc.
    if !cfg!(target_os = "linux") {
        return;
    }
    let b = Fixture::new();
    b.dv(4)
        .args(["get", "DATABASE_URL", "-c", "-f"])
        .arg(&b.file)
        .assert()
        .success();
    let mut helpers = 0;
    for entry in fs::read_dir("/proc").unwrap().flatten() {
        let Ok(raw) = fs::read(Path::new(&entry.path()).join("cmdline")) else {
            continue;
        };
        let line = String::from_utf8_lossy(&raw).replace('\0', " ");
        if line.contains("_clip-hold") {
            helpers += 1;
            assert!(!line.contains("pw-1f3a"), "{line}");
        }
    }
    assert!(helpers >= 1, "the helper should still be holding the value");
}
