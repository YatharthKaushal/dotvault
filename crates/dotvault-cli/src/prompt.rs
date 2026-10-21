//! Interactive input.
//!
//! Each prompt has a non-interactive route so scripts, CI and tests can drive
//! the CLI: an environment variable, or one line per prompt on piped stdin.
//! Passwords from the environment are visible to other processes of the same
//! user, so prefer the interactive prompt for day-to-day use.

use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::{bail, Context, Result};
use zeroize::Zeroizing;

pub const MIN_PASSWORD_CHARS: usize = 10;

fn env_value(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

fn read_stdin_line() -> Result<String> {
    let mut line = String::new();
    let n = io::stdin().lock().read_line(&mut line)?;
    if n == 0 {
        bail!("no input available (stdin is closed)");
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn ask(label: &str) {
    let mut err = io::stderr();
    let _ = write!(err, "{label}");
    let _ = err.flush();
}

/// A line of visible input, e.g. a username.
pub fn line(label: &str, env_var: Option<&str>) -> Result<String> {
    if let Some(v) = env_var.and_then(env_value) {
        return Ok(v);
    }
    ask(label);
    Ok(read_stdin_line()?.trim().to_string())
}

/// A secret that is not echoed.
pub fn password(label: &str, env_var: &str) -> Result<Zeroizing<String>> {
    if let Some(v) = env_value(env_var) {
        return Ok(Zeroizing::new(v));
    }
    if io::stdin().is_terminal() {
        return rpassword::prompt_password(label)
            .map(Zeroizing::new)
            .context("could not read the password");
    }
    ask(label);
    Ok(Zeroizing::new(read_stdin_line()?))
}

/// A new password, asked for twice.
pub fn new_password(label: &str, repeat_label: &str, env_var: &str) -> Result<Zeroizing<String>> {
    if let Some(v) = env_value(env_var) {
        return Ok(Zeroizing::new(v));
    }
    let first = password(label, env_var)?;
    let second = password(repeat_label, env_var)?;
    if *first != *second {
        bail!("the two passwords do not match");
    }
    Ok(first)
}

/// Rules for a *new* password. They are deliberately modest: the real
/// protection is Argon2id, and a long passphrase beats composition rules.
pub fn check_new_password(password: &str, username: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        bail!("choose a password of at least {MIN_PASSWORD_CHARS} characters (a few random words works well)");
    }
    if password.eq_ignore_ascii_case(username) {
        bail!("the password must not be the same as the username");
    }
    Ok(())
}

/// The server's username rules, checked early so a typo fails before the
/// (deliberately slow) key derivation.
pub fn check_username(username: &str) -> Result<()> {
    let ok = (3..=32).contains(&username.len())
        && username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    if ok {
        Ok(())
    } else {
        bail!("username must be 3-32 characters from A-Z a-z 0-9 _ . -")
    }
}

/// Asks a yes/no question; anything but y/yes is no. Without a terminal the
/// answer is no, so callers need a `--yes` flag for scripts.
pub fn confirm(question: &str) -> Result<bool> {
    if !io::stdin().is_terminal() {
        return Ok(false);
    }
    ask(&format!("{question} [y/N] "));
    let answer = read_stdin_line()?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// Lets the user pick from `items` by number. Returns zero-based indices in
/// ascending order. Empty input selects everything.
pub fn select(prompt: &str, items: &[String]) -> Result<Vec<usize>> {
    if !io::stdin().is_terminal() {
        bail!("interactive selection needs a terminal; use --keys to choose variables");
    }
    let mut err = io::stderr();
    writeln!(err, "{prompt}")?;
    for (i, item) in items.iter().enumerate() {
        writeln!(err, "  {:>2}) {item}", i + 1)?;
    }
    ask("Numbers (e.g. 1,3-5), Enter for all: ");
    parse_selection(&read_stdin_line()?, items.len())
}

pub fn parse_selection(input: &str, count: usize) -> Result<Vec<usize>> {
    let input = input.trim();
    if input.is_empty() || input.eq_ignore_ascii_case("all") {
        return Ok((0..count).collect());
    }
    let mut picked = std::collections::BTreeSet::new();
    for part in input.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (from, to) = match part.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (part, part),
        };
        let (from, to): (usize, usize) = (
            from.parse()
                .with_context(|| format!("{part:?} is not a number"))?,
            to.parse()
                .with_context(|| format!("{part:?} is not a number"))?,
        );
        if from == 0 || to > count || from > to {
            bail!("{part:?} is outside 1-{count}");
        }
        picked.extend(from - 1..to);
    }
    Ok(picked.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections() {
        assert_eq!(parse_selection("", 3).unwrap(), [0, 1, 2]);
        assert_eq!(parse_selection("all", 3).unwrap(), [0, 1, 2]);
        assert_eq!(parse_selection("2", 5).unwrap(), [1]);
        assert_eq!(parse_selection("1,3-4", 5).unwrap(), [0, 2, 3]);
        assert_eq!(parse_selection(" 3 , 1 ,3", 5).unwrap(), [0, 2]);
        for bad in ["0", "6", "x", "3-1", "1-9", "1,,x"] {
            assert!(parse_selection(bad, 5).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn username_rules() {
        for ok in ["abc", "Ada", "a.b-c_d", &"x".repeat(32)] {
            assert!(check_username(ok).is_ok(), "{ok}");
        }
        for bad in ["", "ab", "a b", "ünï", "semi;colon", &"x".repeat(33)] {
            assert!(check_username(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn password_rules() {
        assert!(check_new_password("short", "ada").is_err());
        assert!(check_new_password("123456789", "ada").is_err());
        assert!(check_new_password("correct horse battery", "ada").is_ok());
        assert!(check_new_password("ADA-LOVELACE", "ada-lovelace").is_err());
        // length counts characters, not bytes
        assert!(check_new_password("ééééééééé", "x").is_err());
        assert!(check_new_password("éééééééééé", "x").is_ok());
    }
}
