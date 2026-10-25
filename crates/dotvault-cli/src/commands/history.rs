//! `log`, `revert` and `forget`: the history of variables dotvault has changed.

use std::collections::HashMap;
use std::io::Write;
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context, Result};
use dotvault_core::dotenv::DotEnv;
use dotvault_core::fingerprint::fingerprint;
use owo_colors::OwoColorize;
use serde::Serialize;

use super::{read_text, resolve_target, Ctx, ResolveOpts};
use crate::cli::{ForgetArgs, LogArgs, RevertArgs};
use crate::fsutil;
use crate::history::{self as store, Action, Before, History, Row};
use crate::output::{
    age, masked, now_secs, plural, print_json, sanitize, stdout, tilde, Cell, Style, Table,
};

/// The history outlives a moved or deleted file, so `log` and `forget` accept a
/// path that no longer exists.
fn lenient() -> ResolveOpts {
    ResolveOpts {
        include_examples: false,
        may_create: true,
    }
}

fn existing() -> ResolveOpts {
    ResolveOpts {
        include_examples: false,
        may_create: false,
    }
}

/// A value as `log` shows it: masked with a tag that tells equal values apart, or
/// the real value with `--reveal`.
fn value_text(value: &Option<String>, reveal: bool) -> String {
    match value {
        None => "(removed)".to_string(),
        Some(v) if reveal => sanitize(v).into_owned(),
        Some(v) => format!("{} {}", masked(v), fingerprint(v)),
    }
}

fn action_style(action: &str) -> Style {
    match action {
        "external" => Style::Yellow,
        "revert" => Style::Cyan,
        "baseline" => Style::Dim,
        _ => Style::Plain,
    }
}

#[derive(Serialize)]
struct JsonRow<'a> {
    change: i64,
    at: i64,
    action: &'a str,
    key: &'a str,
    version: i64,
    removed: bool,
    fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<&'a str>,
}

pub fn log(ctx: &Ctx, args: LogArgs) -> Result<ExitCode> {
    let path = resolve_target(ctx, None, &args.flags, lenient())?;
    let file = store::key_for(&path);
    let rows = match History::open_if_exists(&ctx.dirs.history_file())? {
        Some(history) => history.rows(&file, args.key.as_deref())?,
        None => Vec::new(),
    };

    if args.json {
        let mut newest_first: Vec<&Row> = rows.iter().rev().collect();
        if let Some(n) = args.limit {
            newest_first.truncate(n);
        }
        let out: Vec<JsonRow> = newest_first
            .into_iter()
            .map(|r| JsonRow {
                change: r.change,
                at: r.at,
                action: &r.action,
                key: &r.key,
                version: r.version,
                removed: r.value.is_none(),
                fingerprint: r.value.as_deref().map(fingerprint),
                value: if args.reveal {
                    r.value.as_deref()
                } else {
                    None
                },
            })
            .collect();
        print_json(&out)?;
        return Ok(ExitCode::SUCCESS);
    }

    if rows.is_empty() {
        let what = match &args.key {
            Some(key) => format!("{} in {}", sanitize(key), tilde(&path)),
            None => tilde(&path),
        };
        anstream::eprintln!(
            "{}",
            format!(
                "No history for {what} yet. It starts the first time dotvault changes the file (set, unset, edit, pull)."
            )
            .dimmed()
        );
        return Ok(ExitCode::SUCCESS);
    }

    let now = now_secs();
    let mut out = stdout().lock();
    match &args.key {
        Some(key) => {
            writeln!(
                out,
                "{}",
                format!("{}  {}", tilde(&path), sanitize(key)).dimmed()
            )?;
            let mut table = Table::new(&["VERSION", "WHEN", "ACTION", "VALUE"]);
            let limit = args.limit.unwrap_or(usize::MAX);
            for r in rows.iter().rev().take(limit) {
                table.row(vec![
                    Cell::new(format!("v{}", r.version), Style::Bold),
                    Cell::new(age(now, r.at), Style::Dim),
                    Cell::new(r.action.clone(), action_style(&r.action)),
                    Cell::new(value_text(&r.value, args.reveal), Style::Plain),
                ]);
            }
            table.render(&mut out)?;
        }
        None => {
            writeln!(out, "{}", tilde(&path).dimmed())?;
            file_view(
                &mut out,
                &rows,
                args.limit.unwrap_or(usize::MAX),
                now,
                args.reveal,
            )?;
        }
    }
    if !args.reveal {
        writeln!(
            out,
            "{}",
            "values are masked; add --reveal to see them".dimmed()
        )?;
    }
    Ok(ExitCode::SUCCESS)
}

/// Every change to the file, newest first, with the variables each one touched.
fn file_view(
    out: &mut impl Write,
    rows: &[Row],
    limit: usize,
    now: i64,
    reveal: bool,
) -> Result<()> {
    let mut changes: Vec<Vec<&Row>> = Vec::new();
    for row in rows {
        match changes.last_mut() {
            Some(group) if group[0].change == row.change => group.push(row),
            _ => changes.push(vec![row]),
        }
    }
    let earlier: HashMap<(&str, i64), &Option<String>> = rows
        .iter()
        .map(|r| ((r.key.as_str(), r.version), &r.value))
        .collect();
    let width = rows
        .iter()
        .map(|r| r.key.chars().count())
        .max()
        .unwrap_or(0);

    for group in changes.iter().rev().take(limit) {
        let first = group[0];
        writeln!(
            out,
            "{}  {}  {}",
            age(now, first.at).dimmed(),
            first.action.bold(),
            plural(group.len(), "variable").dimmed()
        )?;
        for r in group {
            let shown = match (reveal, earlier.get(&(r.key.as_str(), r.version - 1))) {
                (true, Some(old)) => {
                    format!("{} → {}", value_text(old, true), value_text(&r.value, true))
                }
                _ => value_text(&r.value, reveal),
            };
            writeln!(
                out,
                "    {:<width$}  v{:<3} {}",
                sanitize(&r.key),
                r.version,
                shown,
            )?;
        }
    }
    Ok(())
}

pub fn revert(ctx: &Ctx, args: RevertArgs) -> Result<ExitCode> {
    let path = resolve_target(ctx, None, &args.flags, existing())?;
    let file = store::key_for(&path);
    let text = read_text(&path, true)?;
    let before = Before::read(&path).ok_or_else(|| anyhow!("cannot read {}", tilde(&path)))?;

    let history = History::open_if_exists(&ctx.dirs.history_file())?;
    let rows = match &history {
        Some(h) => h.rows(&file, Some(&args.key))?,
        None => Vec::new(),
    };
    let Some(latest) = rows.last() else {
        bail!(
            "no history for {} in {}; history starts the first time dotvault changes it",
            sanitize(&args.key),
            tilde(&path)
        );
    };

    let mut env = DotEnv::parse(&text);
    let current = env.get(&args.key).map(str::to_string);

    let target = match args.to {
        Some(n) => rows.iter().find(|r| r.version == n).ok_or_else(|| {
            anyhow!(
                "{} has versions v{} to v{}, no v{n}",
                sanitize(&args.key),
                rows[0].version,
                latest.version
            )
        })?,
        // Undo the latest change, whether dotvault made it or someone edited the
        // file since (then the latest recorded value is what to go back to).
        None if latest.value != current => latest,
        None => rows.iter().rev().nth(1).ok_or_else(|| {
            anyhow!(
                "{} has only one version (v{}); there is nothing earlier to go back to",
                sanitize(&args.key),
                latest.version
            )
        })?,
    };

    if target.value == current {
        anstream::println!(
            "{} already has the value of v{} in {}; nothing to do.",
            sanitize(&args.key),
            target.version,
            tilde(&path)
        );
        return Ok(ExitCode::SUCCESS);
    }

    match &target.value {
        Some(value) => env.set(&args.key, value)?,
        None => {
            env.unset(&args.key);
        }
    }
    let after = env.to_string();
    fsutil::write_atomic(&path, after.as_bytes())
        .with_context(|| format!("cannot write {}", tilde(&path)))?;
    ctx.touch_index(&path);
    ctx.record(&path, Some(&before), &after, Action::Revert);

    let now_at = match History::open_if_exists(&ctx.dirs.history_file()) {
        Ok(Some(h)) => h
            .rows(&file, Some(&args.key))
            .ok()
            .and_then(|r| r.last().map(|row| row.version)),
        _ => None,
    };
    let now = match now_at {
        Some(v) if v > latest.version => format!(" (now v{v})"),
        _ => String::new(),
    };
    match &target.value {
        Some(_) => anstream::println!(
            "Reverted {} to v{} in {}{now}",
            sanitize(&args.key),
            target.version,
            tilde(&path)
        ),
        None => anstream::println!(
            "Removed {} from {}: v{} had no value{now}",
            sanitize(&args.key),
            tilde(&path),
            target.version
        ),
    }
    Ok(ExitCode::SUCCESS)
}

pub fn forget(ctx: &Ctx, args: ForgetArgs) -> Result<ExitCode> {
    let path = resolve_target(ctx, None, &args.flags, lenient())?;
    let file = store::key_for(&path);
    let what = match &args.key {
        Some(key) => format!("{} in {}", sanitize(key), tilde(&path)),
        None => format!("every variable in {}", tilde(&path)),
    };
    let Some(mut history) = History::open_if_exists(&ctx.dirs.history_file())? else {
        anstream::eprintln!("{}", format!("No history recorded for {what}.").dimmed());
        return Ok(ExitCode::SUCCESS);
    };
    let removed = history.forget(&file, args.key.as_deref())?;
    if removed == 0 {
        anstream::eprintln!("{}", format!("No history recorded for {what}.").dimmed());
    } else {
        anstream::println!("Forgot {} of {what}.", plural(removed, "version"));
    }
    Ok(ExitCode::SUCCESS)
}
