//! Commands that change one env file at the same moment take turns.
//!
//! Without that, each starts from the text it read, and the last one to write wins:
//! twelve `set` commands used to keep five variables, all reporting success.
//!
//! Programs are started with their output going to files, not pipes, so that no
//! pipe is being created in this process while another is started.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
    file: PathBuf,
}

impl Fixture {
    fn new(start: &str) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("home").join("app");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("package.json"), "{}").unwrap();
        let file = dir.join(".env");
        fs::write(&file, start).unwrap();
        Fixture { tmp, file }
    }

    fn data(&self) -> PathBuf {
        self.tmp.path().join("data")
    }

    /// Starts `dotvault <args> -f <file>`. Its output lands in `out.<n>` next to the data.
    fn start(&self, n: usize, args: &[&str]) -> Child {
        let home = self.tmp.path().join("home");
        let out = File::create(self.tmp.path().join(format!("out.{n}"))).unwrap();
        Command::new(env!("CARGO_BIN_EXE_dotvault"))
            .args(args)
            .arg("-f")
            .arg(&self.file)
            .env("DOTVAULT_HOME", &home)
            .env("DOTVAULT_SCAN_ROOTS", &home)
            .env("DOTVAULT_CONFIG_DIR", self.tmp.path().join("config"))
            .env("DOTVAULT_DATA_DIR", self.data())
            .env("NO_COLOR", "1")
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            .stdin(Stdio::null())
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .spawn()
            .unwrap()
    }

    fn output(&self, n: usize) -> String {
        fs::read_to_string(self.tmp.path().join(format!("out.{n}"))).unwrap_or_default()
    }

    fn text(&self) -> String {
        fs::read_to_string(&self.file).unwrap()
    }

    /// Runs each command at once and checks that all of them succeeded.
    fn together(&self, commands: &[Vec<String>]) {
        let children: Vec<Child> = commands
            .iter()
            .enumerate()
            .map(|(n, args)| {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                self.start(n, &args)
            })
            .collect();
        for (n, mut child) in children.into_iter().enumerate() {
            let status = child.wait().unwrap();
            assert!(
                status.success(),
                "command {n}: {status}: {}",
                self.output(n)
            );
        }
    }
}

fn words(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

fn lines(text: &str) -> Vec<&str> {
    let mut v: Vec<&str> = text.lines().collect();
    v.sort_unstable();
    v
}

#[test]
fn many_set_commands_at_once_keep_every_variable() {
    for round in 0..3 {
        let b = Fixture::new("X=0\n");
        let commands: Vec<Vec<String>> = (1..=12)
            .map(|i| vec!["set".to_string(), format!("K{i}=v{i}")])
            .collect();
        b.together(&commands);

        let mut want: Vec<String> = (1..=12).map(|i| format!("K{i}=v{i}")).collect();
        want.push("X=0".into());
        want.sort_unstable();
        assert_eq!(
            lines(&b.text()),
            want.iter().map(String::as_str).collect::<Vec<_>>(),
            "round {round}"
        );
    }
}

#[test]
fn many_unset_commands_at_once_remove_every_variable() {
    let mut start = String::from("KEEP=1\n");
    for i in 1..=12 {
        start.push_str(&format!("K{i}=v{i}\n"));
    }
    let b = Fixture::new(&start);
    let commands: Vec<Vec<String>> = (1..=12)
        .map(|i| vec!["unset".to_string(), format!("K{i}")])
        .collect();
    b.together(&commands);
    assert_eq!(b.text(), "KEEP=1\n");
}

#[test]
fn set_and_unset_at_once_do_not_undo_each_other() {
    let b = Fixture::new("OLD1=a\nOLD2=b\nOLD3=c\n");
    b.together(&[
        words(&["set", "NEW1=1"]),
        words(&["unset", "OLD1"]),
        words(&["set", "NEW2=2"]),
        words(&["unset", "OLD2"]),
        words(&["set", "NEW3=3"]),
        words(&["unset", "OLD3"]),
    ]);
    assert_eq!(lines(&b.text()), ["NEW1=1", "NEW2=2", "NEW3=3"]);
}

#[test]
fn every_change_is_in_the_history_too() {
    let b = Fixture::new("X=0\n");
    let commands: Vec<Vec<String>> = (1..=8)
        .map(|i| vec!["set".to_string(), format!("K{i}=v{i}")])
        .collect();
    b.together(&commands);
    for n in 0..8 {
        assert!(
            !b.output(n).contains("warning"),
            "command {n}: {}",
            b.output(n)
        );
    }

    let log = Command::new(env!("CARGO_BIN_EXE_dotvault"))
        .args(["log", "--json", "-f"])
        .arg(&b.file)
        .env("DOTVAULT_HOME", b.tmp.path().join("home"))
        .env("DOTVAULT_SCAN_ROOTS", b.tmp.path().join("home"))
        .env("DOTVAULT_CONFIG_DIR", b.tmp.path().join("config"))
        .env("DOTVAULT_DATA_DIR", b.data())
        .output()
        .unwrap();
    assert!(log.status.success());
    let log = String::from_utf8(log.stdout).unwrap();
    for i in 1..=8 {
        assert!(log.contains(&format!("\"K{i}\"")), "K{i} missing: {log}");
    }
}

/// Holds every lock file, which is how a test can stand in for another command
/// without knowing which one a given file maps to.
struct AllLocks {
    _held: Vec<File>,
}

impl AllLocks {
    fn take(data: &Path) -> AllLocks {
        let dir = data.join("locks");
        fs::create_dir_all(&dir).unwrap();
        let held = (0..64)
            .map(|n| {
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(dir.join(format!("edit-{n:02x}.lock")))
                    .unwrap();
                file.lock().unwrap();
                file
            })
            .collect();
        AllLocks { _held: held }
    }
}

fn finishes_within(child: &mut Child, limit: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
fn a_command_waits_for_its_turn_and_then_goes_ahead() {
    for (args, start, after) in [
        (words(&["set", "B=2"]), "A=1\n", "A=1\nB=2\n"),
        (words(&["unset", "A"]), "A=1\nB=2\n", "B=2\n"),
    ] {
        let b = Fixture::new(start);
        let locks = AllLocks::take(&b.data());

        let mut child = b.start(0, &args.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(
            !finishes_within(&mut child, Duration::from_millis(700)),
            "{args:?} did not wait for its turn: {}",
            b.output(0)
        );
        assert_eq!(b.text(), start, "{args:?} wrote while the file was in use");

        drop(locks);
        assert!(
            finishes_within(&mut child, Duration::from_secs(20)),
            "{args:?} never went ahead"
        );
        assert!(child.wait().unwrap().success(), "{}", b.output(0));
        assert_eq!(b.text(), after, "{args:?}");
    }
}

#[test]
fn revert_waits_for_its_turn_too() {
    let b = Fixture::new("A=1\n");
    b.together(&[words(&["set", "A=2"])]);
    assert_eq!(b.text(), "A=2\n");

    let locks = AllLocks::take(&b.data());
    let mut child = b.start(1, &["revert", "A"]);
    assert!(
        !finishes_within(&mut child, Duration::from_millis(700)),
        "revert did not wait for its turn: {}",
        b.output(1)
    );
    assert_eq!(b.text(), "A=2\n");

    drop(locks);
    assert!(finishes_within(&mut child, Duration::from_secs(20)));
    assert!(child.wait().unwrap().success(), "{}", b.output(1));
    assert_eq!(b.text(), "A=1\n");
}

#[test]
fn reading_never_waits() {
    let b = Fixture::new("A=1\n");
    let _locks = AllLocks::take(&b.data());
    let mut child = b.start(0, &["get", "A"]);
    assert!(
        finishes_within(&mut child, Duration::from_secs(20)),
        "get waited for a lock it does not need"
    );
    assert!(child.wait().unwrap().success(), "{}", b.output(0));
    assert_eq!(b.output(0).trim(), "1");
}
