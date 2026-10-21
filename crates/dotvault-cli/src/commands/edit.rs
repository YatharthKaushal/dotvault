use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, ExitCode};

use anyhow::{anyhow, bail, Context, Result};
use dotvault_core::dotenv::DotEnv;
use owo_colors::OwoColorize;

use super::{read_text, resolve_target, Ctx, ResolveOpts};
use crate::cli::{EditArgs, GetArgs, SetArgs, UnsetArgs};
use crate::fsutil;
use crate::output::{stdout, tilde};

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
    match env.get(&args.key) {
        Some(value) => {
            let mut out = stdout().lock();
            writeln!(out, "{value}")?;
            Ok(ExitCode::SUCCESS)
        }
        None => bail!("{} is not set in {}", args.key, tilde(&path)),
    }
}

pub fn set(ctx: &Ctx, args: SetArgs) -> Result<ExitCode> {
    let path = resolve_target(ctx, None, &args.flags, creatable())?;
    let text = if path.exists() {
        read_text(&path, true)?
    } else {
        String::new()
    };
    let mut env = DotEnv::parse(&text);

    let mut names = Vec::new();
    for assignment in &args.assignments {
        let (key, value) = match assignment.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (assignment.clone(), prompt_value(assignment)?),
        };
        env.set(&key, &value)?;
        names.push(key);
    }

    // Nothing is written unless every key was valid.
    fsutil::write_atomic(&path, env.to_string().as_bytes())
        .with_context(|| format!("cannot write {}", tilde(&path)))?;
    ctx.touch_index(&path);
    anstream::println!("Set {} in {}", names.join(", "), tilde(&path));
    Ok(ExitCode::SUCCESS)
}

pub fn unset(ctx: &Ctx, args: UnsetArgs) -> Result<ExitCode> {
    let path = resolve_target(ctx, None, &args.flags, existing())?;
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
        fsutil::write_atomic(&path, env.to_string().as_bytes())
            .with_context(|| format!("cannot write {}", tilde(&path)))?;
        ctx.touch_index(&path);
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
    let status = run_editor(&path)?;
    ctx.touch_index(&path);
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
}
