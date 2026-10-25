//! Secret files other than env files: marking them by hand and looking at one.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use dotvault_core::classify::classify;
use dotvault_core::fingerprint::fingerprint_bytes;
use dotvault_core::secrets::Detected;
use owo_colors::OwoColorize;
use serde_json::Value;

use super::Ctx;
use crate::cli::MarkArgs;
use crate::fsutil;
use crate::output::{age, masked, plural, sanitize, stdout, tilde, Cell, Style, Table};
use crate::scan::{classify_options, detect_secret, Marks};

/// `mark` adds files to the ones treated as secrets and `unmark` removes them,
/// which also silences a file dotvault wrongly took for a secret. The next scan
/// picks the change up.
pub fn mark(ctx: &mut Ctx, args: MarkArgs, marking: bool) -> Result<ExitCode> {
    let mut changed = 0;
    for path in &args.paths {
        if path.is_dir() {
            bail!("{} is a folder; name the files to mark", path.display());
        }
        let canonical = match fsutil::canonical(path) {
            Ok(canonical) => canonical,
            Err(_) if !marking => std::path::absolute(path).unwrap_or_else(|_| path.clone()),
            Err(e) => return Err(e).with_context(|| format!("cannot access {}", path.display())),
        };
        let config = &mut ctx.config;
        let (add_to, take_from) = if marking {
            (&mut config.marked, &mut config.unmarked)
        } else {
            (&mut config.unmarked, &mut config.marked)
        };
        let before = (add_to.len(), take_from.len());
        take_from.retain(|p| *p != canonical);
        if !add_to.contains(&canonical) {
            add_to.push(canonical);
        }
        if (add_to.len(), take_from.len()) != before {
            changed += 1;
        }
    }
    if changed > 0 {
        ctx.config.save(&ctx.dirs)?;
    }
    let verb = if marking { "Marked" } else { "Unmarked" };
    anstream::println!(
        "{verb} {}. {}",
        plural(args.paths.len(), "file"),
        if marking {
            "They are listed by `dotvault ls --secrets`."
        } else {
            "They are no longer listed."
        }
    );
    Ok(ExitCode::SUCCESS)
}

/// Whether `show` should treat this path as a secret file rather than an env file.
pub fn secret_at(ctx: &Ctx, path: &Path) -> Option<Detected> {
    let name = path.file_name()?.to_string_lossy();
    if classify(&name, &classify_options(&ctx.config)).is_some() {
        return None;
    }
    let size = std::fs::metadata(path).ok()?.len();
    detect_secret(path, size, &Marks::from_config(&ctx.config))
}

const FINGERPRINT_LIMIT: u64 = 8 * 1024 * 1024;

/// Describes a secret file. Key files and keystores never have their contents
/// printed; a JSON secret shows its top-level fields, masked unless `reveal`.
pub fn show_secret(path: &Path, detected: &Detected, reveal: bool) -> Result<ExitCode> {
    let md = std::fs::metadata(path).with_context(|| format!("cannot read {}", tilde(path)))?;
    let mut out = stdout().lock();
    writeln!(out, "{}", tilde(path).dimmed())?;

    let kind = if detected.detail.is_empty() {
        detected.kind.as_str().to_string()
    } else {
        format!("{} ({})", detected.kind.as_str(), detected.detail)
    };
    let modified = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    let fingerprint = if md.len() <= FINGERPRINT_LIMIT {
        std::fs::read(path)
            .map(|b| fingerprint_bytes(&b))
            .unwrap_or_else(|_| "-".to_string())
    } else {
        "-".to_string()
    };
    for (label, value) in [
        ("kind", kind),
        ("size", format!("{} bytes", md.len())),
        ("modified", age(crate::output::now_secs(), modified)),
        ("fingerprint", fingerprint),
    ] {
        writeln!(out, "{:<12}{}", label.dimmed(), sanitize(&value))?;
    }

    if !detected.kind.is_json() {
        if reveal {
            writeln!(
                out,
                "{}",
                "contents of this kind of file are never printed; use cat to read it".dimmed()
            )?;
        }
        return Ok(ExitCode::SUCCESS);
    }

    let text = std::fs::read(path).with_context(|| format!("cannot read {}", tilde(path)))?;
    let text = text.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&text);
    let Ok(Value::Object(fields)) = serde_json::from_slice::<Value>(text) else {
        return Ok(ExitCode::SUCCESS);
    };
    writeln!(out)?;
    let mut table = Table::new(&["KEY", "VALUE"]);
    for (key, value) in &fields {
        let cell = match value {
            Value::String(s) if reveal => Cell::new(sanitize(s).into_owned(), Style::Plain),
            Value::String(s) => Cell::new(masked(s), Style::Dim),
            Value::Object(_) => Cell::new("{...}", Style::Dim),
            Value::Array(_) => Cell::new("[...]", Style::Dim),
            other if reveal => Cell::new(other.to_string(), Style::Plain),
            other => Cell::new(masked(&other.to_string()), Style::Dim),
        };
        table.row(vec![
            Cell::new(sanitize(key).into_owned(), Style::Bold),
            cell,
        ]);
    }
    table.render(&mut out)?;
    if !reveal {
        writeln!(
            out,
            "{}",
            "values are masked; add --reveal to see them".dimmed()
        )?;
    }
    Ok(ExitCode::SUCCESS)
}
