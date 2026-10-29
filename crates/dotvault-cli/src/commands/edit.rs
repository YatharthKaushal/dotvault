use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, ExitCode};

use anyhow::{anyhow, bail, Context, Result};
use dotvault_core::dotenv::DotEnv;
use owo_colors::OwoColorize;

use super::{read_text, resolve_target, Ctx, ResolveOpts};
use crate::cli::{EditArgs, GetArgs, SetArgs, UnsetArgs};
use crate::clipboard;
use crate::fsutil;
use crate::history::{Action, Before};
use crate::keys;
use crate::output::{render_pairs_for, sanitize, skipped_names, stdout, tilde, Format};

fn existing() -> ResolveOpts {
    ResolveOpts {
        include_examples: false,
        may_create: false,
    }
}

fn creatable() -> ResolveOpts {
    ResolveOpts {
        include_examples: false,
        may_create: true,
    }
}

pub fn get(ctx: &Ctx, args: GetArgs) -> Result<ExitCode> {
    let path = resolve_target(ctx, None, &args.flags, existing())?;
    let env = DotEnv::parse(&read_text(&path, false)?);
    let picked = keys::select(&super::view::pairs_of(&env), &args.keys)?;
    if !picked.missing.is_empty() {
        let what: Vec<String> = picked
            .missing
            .iter()
            .map(|m| keys::describe_missing(m))
            .collect();
        bail!("{} in {}", what.join("; "), tilde(&path));
    }

    let text = text_for(
        &picked.chosen,
        &args.keys,
        args.kv,
        args.copy,
        io::stdout().is_terminal(),
    )?;

    if args.copy {
        clipboard::copy(&text)?;
        let what = match picked.chosen.as_slice() {
            [(key, _)] if args.kv => format!("{} as KEY=value", sanitize(key)),
            [(key, _)] => sanitize(key).into_owned(),
            many => format!("{} variables", many.len()),
        };
        anstream::eprintln!(
            "Copied {what} to the clipboard. It clears in {}s.",
            clipboard::CLEAR_AFTER.as_secs()
        );
        return Ok(ExitCode::SUCCESS);
    }

    writeln!(stdout().lock(), "{text}")?;
    Ok(ExitCode::SUCCESS)
}

/// What `get` prints or copies. One plain name gives the bare value, so
/// `$(dotvault get KEY)` keeps working; several names, a glob or `--kv` give
/// `KEY=value` lines. Only what is printed to a terminal has control characters
/// made visible: a copy is always the exact data.
fn text_for(
    chosen: &[(String, String)],
    patterns: &[String],
    kv: bool,
    copy: bool,
    terminal: bool,
) -> Result<String> {
    if let ([pattern], [(_, value)]) = (patterns, chosen) {
        if !kv && !keys::is_glob(pattern) {
            return Ok(value.clone());
        }
    }
    if skipped_names(Format::Dotenv, chosen) == chosen.len() {
        let names: Vec<String> = chosen
            .iter()
            .map(|(k, _)| sanitize(k).into_owned())
            .collect();
        bail!(
            "{} cannot be written as KEY=value (names look like MY_VAR)",
            names.join(", ")
        );
    }
    let rendered = render_pairs_for(terminal && !copy, Format::Dotenv, chosen, true);
    Ok(rendered.trim_end_matches('\n').to_string())
}

pub fn set(ctx: &Ctx, args: SetArgs) -> Result<ExitCode> {
    let path = resolve_target(ctx, None, &args.flags, creatable())?;

    // Values are asked for before the file is locked, so that someone typing never
    // holds up another command. A file that cannot be read, or a name that cannot be
    // set, is refused before a secret is typed for it.
    let mut assignments = Vec::new();
    for assignment in &args.assignments {
        let (key, value) = match assignment.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => {
                if path.exists() {
                    read_text(&path, true)?;
                }
                DotEnv::new().set(assignment, "")?;
                (assignment.clone(), prompt_value(assignment)?)
            }
        };
        assignments.push((key, value));
    }

    let _lock = ctx.lock_file(&path)?;
    let text = if path.exists() {
        read_text(&path, true)?
    } else {
        String::new()
    };
    let before = Before::read(&path);
    let mut env = DotEnv::parse(&text);

    let mut names = Vec::new();
    for (key, value) in &assignments {
        env.set(key, value)?;
        names.push(key.clone());
    }

    // Nothing is written unless every key was valid.
    let after = env.to_string();
    fsutil::write_atomic(&path, after.as_bytes())
        .with_context(|| format!("cannot write {}", tilde(&path)))?;
    ctx.touch_index(&path);
    ctx.record(&path, before.as_ref(), &after, Action::Set);
    anstream::println!("Set {} in {}", names.join(", "), tilde(&path));
    Ok(ExitCode::SUCCESS)
}

pub fn unset(ctx: &Ctx, args: UnsetArgs) -> Result<ExitCode> {
    let path = resolve_target(ctx, None, &args.flags, existing())?;
    let _lock = ctx.lock_file(&path)?;
    let before = Before::read(&path);
    let mut env = DotEnv::parse(&read_text(&path, true)?);

    let mut removed = Vec::new();
    let mut missing = Vec::new();
    for key in &args.keys {
        if env.unset(key) > 0 {
            removed.push(key.as_str());
        } else {
            missing.push(key.as_str());
        }
    }

    if !removed.is_empty() {
        let after = env.to_string();
        fsutil::write_atomic(&path, after.as_bytes())
            .with_context(|| format!("cannot write {}", tilde(&path)))?;
        ctx.touch_index(&path);
        ctx.record(&path, before.as_ref(), &after, Action::Unset);
        anstream::println!("Removed {} from {}", removed.join(", "), tilde(&path));
    }
    if missing.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        anstream::eprintln!(
            "{} not set in {}: {}",
            "warning:".yellow().bold(),
            tilde(&path),
            missing.join(", ")
        );
        Ok(ExitCode::from(1))
    }
}

pub fn edit(ctx: &Ctx, args: EditArgs) -> Result<ExitCode> {
    let path = resolve_target(ctx, args.target.as_deref(), &args.flags, creatable())?;
    let before = Before::read(&path);
    let status = run_editor(&path)?;
    ctx.touch_index(&path);
    // Whatever the editor saved is diffed per variable, even if it then exited badly.
    if let Ok(bytes) = std::fs::read(&path) {
        ctx.record(
            &path,
            before.as_ref(),
            &String::from_utf8_lossy(&bytes),
            Action::Edit,
        );
    }
    if status.success() {
        Ok(ExitCode::SUCCESS)
    } else {
        bail!("the editor exited with {status}");
    }
}

fn run_editor(path: &Path) -> Result<std::process::ExitStatus> {
    let configured = ["VISUAL", "EDITOR"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.trim().is_empty()));

    let candidates: Vec<String> = match configured {
        Some(cmd) => vec![cmd],
        None if cfg!(windows) => vec!["notepad".into()],
        None => vec!["nano".into(), "vi".into()],
    };

    for candidate in &candidates {
        let mut parts = split_command(candidate);
        if parts.is_empty() {
            continue;
        }
        let program = parts.remove(0);
        match Command::new(&program).args(&parts).arg(path).status() {
            Ok(status) => return Ok(status),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("cannot run editor {program:?}")),
        }
    }
    Err(anyhow!("no editor found; set $VISUAL or $EDITOR"))
}

/// Splits `code --wait` or `"C:\Program Files\x\edit.exe" -w` into words.
fn split_command(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut started = false;
    for c in command.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                started = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(current);
    }
    words
}

/// Reads a secret without echoing it, or one line from a pipe.
fn prompt_value(key: &str) -> Result<String> {
    if io::stdin().is_terminal() {
        rpassword::prompt_password(format!("Value for {key}: ")).context("could not read the value")
    } else {
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_editor_commands() {
        assert_eq!(split_command("vim"), ["vim"]);
        assert_eq!(split_command("code --wait"), ["code", "--wait"]);
        assert_eq!(
            split_command(r#""C:\Program Files\Notepad++\notepad++.exe" -multiInst"#),
            [r"C:\Program Files\Notepad++\notepad++.exe", "-multiInst"]
        );
        assert_eq!(split_command("  a   b  "), ["a", "b"]);
        assert!(split_command("   ").is_empty());
    }

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn one_plain_name_is_the_bare_value() {
        let chosen = pairs(&[("DB", "postgres://x")]);
        let text = text_for(&chosen, &names(&["DB"]), false, false, false).unwrap();
        assert_eq!(text, "postgres://x");
    }

    #[test]
    fn kv_gives_the_whole_pair_quoted_when_needed() {
        let chosen = pairs(&[("DB", "postgres://x"), ("MSG", "has space")]);
        let one = text_for(&chosen[..1], &names(&["DB"]), true, false, false).unwrap();
        assert_eq!(one, "DB=postgres://x");
        let spaced = text_for(&chosen[1..], &names(&["MSG"]), true, true, false).unwrap();
        assert_eq!(spaced, "MSG=\"has space\"");
    }

    #[test]
    fn several_names_or_a_glob_give_lines_without_a_trailing_newline() {
        let chosen = pairs(&[("A", "1"), ("B", "2")]);
        let several = text_for(&chosen, &names(&["A", "B"]), false, false, false).unwrap();
        assert_eq!(several, "A=1\nB=2");
        let glob = text_for(&chosen[..1], &names(&["A*"]), false, false, false).unwrap();
        assert_eq!(glob, "A=1");
    }

    #[test]
    fn a_copy_is_exact_even_from_a_terminal_but_printing_is_made_visible() {
        let chosen = pairs(&[("ESC", "a\u{1b}[31mb"), ("OK", "fine")]);
        let patterns = names(&["ESC", "OK"]);
        let copied = text_for(&chosen, &patterns, false, true, true).unwrap();
        assert!(copied.contains('\u{1b}'), "{copied:?}");
        let printed = text_for(&chosen, &patterns, false, false, true).unwrap();
        assert!(!printed.contains('\u{1b}'), "{printed:?}");
        assert!(printed.contains("\\u{1b}"), "{printed:?}");
    }

    #[test]
    fn a_name_that_cannot_be_written_as_a_pair_is_an_error_not_an_empty_copy() {
        let chosen = pairs(&[("bad name", "x")]);
        let err = text_for(&chosen, &names(&["bad name"]), true, true, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("cannot be written as KEY=value"), "{err}");
        // the bare value does not need a valid name
        assert_eq!(
            text_for(&chosen, &names(&["bad name"]), false, true, false).unwrap(),
            "x"
        );
    }
}
