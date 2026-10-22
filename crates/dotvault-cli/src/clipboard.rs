//! `get --copy`: puts a value on the system clipboard and takes it off again later.
//!
//! A helper process (this binary, run as the hidden command `_clip-hold`) owns the
//! clipboard for a while. On Linux it has to exist, because the program that
//! copied must keep answering paste requests and the clipboard is empty again as
//! soon as it exits. On every platform it is what clears the value afterwards,
//! and only if the clipboard still holds it, so something copied in the meantime
//! is left alone. The value reaches the helper on stdin, never on the command
//! line, where `ps` would show it. The helper shares no output with the caller,
//! so `$(dotvault get KEY -c)` does not wait for it.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

/// How long a copied value stays on the clipboard.
pub const CLEAR_AFTER: Duration = Duration::from_secs(30);

/// How long to wait for the helper to say whether the copy worked.
const HANDSHAKE: Duration = Duration::from_secs(10);

/// The hidden command that runs [`hold`].
pub const HELPER_COMMAND: &str = "_clip-hold";

/// Puts `text` on the clipboard and arranges for it to be cleared after
/// [`CLEAR_AFTER`]. Returns once the value is on the clipboard.
pub fn copy(text: &str) -> Result<()> {
    let exe = std::env::current_exe()
        .context("cannot find the dotvault program to start the clipboard helper")?;
    let mut command = Command::new(exe);
    command
        .arg(HELPER_COMMAND)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    detach(&mut command);
    let mut child = command
        .spawn()
        .context("cannot start the clipboard helper")?;

    if let Some(mut stdin) = child.stdin.take() {
        // Closing stdin ends the value. If the helper already died, its answer
        // below says why, so a failed write needs no message of its own.
        let _ = stdin.write_all(text.as_bytes());
    }
    let stdout = child.stdout.take().context("no output from the helper")?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = sender.send(line);
    });

    match receiver.recv_timeout(HANDSHAKE) {
        Ok(line) if line.trim() == "ok" => Ok(()),
        Ok(line) => {
            let _ = child.wait();
            match line.trim().strip_prefix("error: ") {
                Some(why) => bail!("{why}"),
                None => bail!("the clipboard helper stopped before it copied anything"),
            }
        }
        Err(_) => {
            let _ = child.kill();
            bail!(
                "the clipboard did not answer within {} seconds",
                HANDSHAKE.as_secs()
            )
        }
    }
}

/// Runs in the helper process: copy what arrives on stdin, say `ok` (or
/// `error: ...`) on stdout, then wait and clear.
pub fn hold() -> Result<ExitCode> {
    let mut text = String::new();
    io::stdin()
        .read_to_string(&mut text)
        .context("cannot read the value to copy")?;

    let mut board = match Board::open() {
        Ok(board) => board,
        Err(err) => return Ok(refuse(&err)),
    };
    if let Err(err) = board.set(&text) {
        return Ok(refuse(&err));
    }
    say("ok");

    thread::sleep(hold_time());
    if board.get().as_deref() == Some(text.as_str()) {
        board.clear();
    }
    Ok(ExitCode::SUCCESS)
}

fn refuse(err: &anyhow::Error) -> ExitCode {
    say(&format!("error: {err}"));
    ExitCode::FAILURE
}

fn say(line: &str) {
    let mut out = io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

#[cfg(debug_assertions)]
fn hold_time() -> Duration {
    // Lets tests watch the clearing without waiting half a minute.
    std::env::var("DOTVAULT_CLIPBOARD_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map_or(CLEAR_AFTER, Duration::from_secs)
}

#[cfg(not(debug_assertions))]
fn hold_time() -> Duration {
    CLEAR_AFTER
}

/// Keeps the helper from being killed with the terminal's foreground job
/// (Ctrl-C) or its console, so the value is still cleared on time.
#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::Foundation::{
        SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    // Rust starts every child with all the handles this program may pass on. The
    // helper would then keep this program's own output pipe open, and whatever
    // waits for that pipe to close (a script capturing the output) would wait for
    // the helper too. The helper's own stdin, stdout and stderr are set
    // explicitly, so they are not affected.
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: both calls only look at, and flag, a handle of this process, and
        // a missing or invalid handle is skipped.
        unsafe {
            let handle = GetStdHandle(which);
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn detach(_command: &mut Command) {}

/// Where the value goes: the system clipboard, or in debug builds a file named
/// by `DOTVAULT_CLIPBOARD_FILE`, so that tests never touch a real clipboard.
enum Board {
    System(arboard::Clipboard),
    #[cfg(debug_assertions)]
    File(std::path::PathBuf),
}

impl Board {
    fn open() -> Result<Board> {
        #[cfg(debug_assertions)]
        if let Some(path) = std::env::var_os("DOTVAULT_CLIPBOARD_FILE") {
            return Ok(Board::File(path.into()));
        }
        arboard::Clipboard::new().map(Board::System).map_err(|err| {
            anyhow!(
                "no clipboard is available here ({err}). Copying needs a desktop session; \
                 it does not work over SSH, in a container or on a server"
            )
        })
    }

    fn set(&mut self, text: &str) -> Result<()> {
        match self {
            Board::System(board) => {
                set_system(board, text).map_err(|err| anyhow!("could not copy: {err}"))
            }
            #[cfg(debug_assertions)]
            Board::File(path) => {
                std::fs::write(path, text).map_err(|err| anyhow!("could not copy: {err}"))
            }
        }
    }

    fn get(&mut self) -> Option<String> {
        match self {
            Board::System(board) => board.get_text().ok(),
            #[cfg(debug_assertions)]
            Board::File(path) => std::fs::read_to_string(path).ok(),
        }
    }

    fn clear(&mut self) {
        match self {
            Board::System(board) => {
                let _ = board.clear();
            }
            #[cfg(debug_assertions)]
            Board::File(path) => {
                let _ = std::fs::write(path, "");
            }
        }
    }
}

/// Copies with the platform's request that clipboard history (and, on Windows,
/// cloud sync) leave the value out.
fn set_system(board: &mut arboard::Clipboard, text: &str) -> Result<(), arboard::Error> {
    let set = board.set();
    #[cfg(windows)]
    let set = {
        use arboard::SetExtWindows;
        set.exclude_from_monitoring()
            .exclude_from_cloud()
            .exclude_from_history()
    };
    #[cfg(target_os = "macos")]
    let set = {
        use arboard::SetExtApple;
        set.exclude_from_history()
    };
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
    ))]
    let set = {
        use arboard::SetExtLinux;
        set.exclude_from_history()
    };
    set.text(text.to_owned())
}
