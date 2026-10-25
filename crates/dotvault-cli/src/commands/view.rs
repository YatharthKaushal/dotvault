use std::collections::HashMap;
use std::io::Write;
use std::process::ExitCode;

use anyhow::{Context, Result};
use dotvault_core::dotenv::DotEnv;
use globset::GlobBuilder;
use owo_colors::OwoColorize;
use serde::Serialize;

use super::{read_text, resolve_target, Ctx, ResolveOpts};
use crate::cli::{DiffArgs, FindArgs, ShowArgs, TargetFlags};
use crate::filter::Filter;
use crate::keys;
use crate::output::{
    masked, print_json, render_pairs_warn, sanitize, shown, stdout, tilde, Cell, Format, Style,
    Table,
};

pub(super) fn pairs_of(env: &DotEnv) -> Vec<(String, String)> {
    env.pairs()
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

pub fn show(ctx: &Ctx, args: ShowArgs) -> Result<ExitCode> {
    let path = resolve_target(
        ctx,
        args.target.as_deref(),
        &args.flags,
        ResolveOpts {
            include_examples: args.examples,
            may_create: false,
        },
    )?;
    let env = DotEnv::parse(&read_text(&path, false)?);
    let mut pairs = pairs_of(&env);
    let mut exit = ExitCode::SUCCESS;

    if !args.keys.is_empty() {
        let picked = keys::select(&pairs, &args.keys)?;
        if !picked.missing.is_empty() {
            let what: Vec<String> = picked
                .missing
                .iter()
                .map(|m| keys::describe_missing(m))
                .collect();
            anstream::eprintln!(
                "{} {} in {}",
                "warning:".yellow().bold(),
                sanitize(&what.join("; ")),
                tilde(&path)
            );
            exit = ExitCode::from(1);
        }
        pairs = picked.chosen;
    }

    print_pairs(&tilde(&path), &pairs, args.format, args.reveal)?;
    Ok(exit)
}

/// Prints variables as a table (headed by `title`) or in a machine format.
pub(super) fn print_pairs(
    title: &str,
    pairs: &[(String, String)],
    format: Format,
    reveal: bool,
) -> Result<()> {
    let mut out = stdout().lock();
    if format != Format::Table {
        write!(out, "{}", render_pairs_warn(format, pairs, reveal))?;
        return Ok(());
    }

    writeln!(out, "{}", sanitize(title).dimmed())?;
    if pairs.is_empty() {
        writeln!(out, "{}", "(no variables)".dimmed())?;
        return Ok(());
    }
    let mut table = Table::new(&["KEY", "VALUE"]);
    for (key, value) in pairs {
        let cell = if reveal {
            Cell::new(value.replace('\n', "\\n"), Style::Plain)
        } else {
            Cell::new(masked(value), Style::Dim)
        };
        table.row(vec![Cell::new(key, Style::Bold), cell]);
    }
    table.render(&mut out)?;
    if !reveal {
        writeln!(
            out,
            "{}",
            "values are masked; add --reveal to see them".dimmed()
        )?;
    }
    Ok(())
}

struct Match {
    key: String,
    project: String,
    env: String,
    path: String,
    value: String,
    variant: usize,
}

/// `A`..`Z`, then `#27`, `#28`, ...
fn variant_label(index: usize) -> String {
    match u8::try_from(index) {
        Ok(i) if i < 26 => ((b'A' + i) as char).to_string(),
        _ => format!("#{}", index + 1),
    }
}

pub fn find(ctx: &Ctx, args: FindArgs) -> Result<ExitCode> {
    if ctx.config.roots.is_empty() {
        anstream::eprintln!(
            "No folders are being scanned and no disks or home folder could be found; run `dotvault scan <dir>` first."
        );
        return Ok(ExitCode::from(1));
    }
    // A plain name has no wildcards, so as a glob it is an exact (case-insensitive) match.
    let matcher = GlobBuilder::new(&args.pattern)
        .case_insensitive(true)
        .build()
        .with_context(|| format!("invalid pattern {:?}", args.pattern))?
        .compile_matcher();
    let filter = Filter::new(&args.filter)?.with_keys(std::slice::from_ref(&args.pattern))?;

    let records = ctx.records(false)?;
    let mut matches: Vec<Match> = Vec::new();
    let mut variants: HashMap<String, Vec<String>> = HashMap::new();
    let mut files = 0;

    for rec in records.iter().filter(|r| filter.matches(r)) {
        let Ok(text) = read_text(&rec.path, false) else {
            continue;
        };
        let env = DotEnv::parse(&text);
        let mut hit = false;
        for (key, value) in env.pairs() {
            if !matcher.is_match(key) {
                continue;
            }
            hit = true;
            let seen = variants.entry(key.to_string()).or_default();
            let variant = match seen.iter().position(|v| v == value) {
                Some(i) => i,
                None => {
                    seen.push(value.to_string());
                    seen.len() - 1
                }
            };
            matches.push(Match {
                key: key.to_string(),
                project: rec.project.clone(),
                env: rec.label.clone(),
                path: rec.path.display().to_string(),
                value: value.to_string(),
                variant,
            });
        }
        files += usize::from(hit);
    }

    matches.sort_by(|a, b| {
        (&a.key, a.project.to_lowercase(), &a.env).cmp(&(&b.key, b.project.to_lowercase(), &b.env))
    });

    if args.json {
        #[derive(Serialize)]
        struct Row<'a> {
            key: &'a str,
            project: &'a str,
            env: &'a str,
            path: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            value: Option<&'a str>,
            variant: String,
            distinct_values: usize,
        }
        let rows: Vec<Row> = matches
            .iter()
            .map(|m| Row {
                key: &m.key,
                project: &m.project,
                env: &m.env,
                path: &m.path,
                value: args.reveal.then_some(m.value.as_str()),
                variant: variant_label(m.variant),
                distinct_values: variants[&m.key].len(),
            })
            .collect();
        print_json(&rows)?;
    } else if matches.is_empty() {
        anstream::eprintln!("No variable matches {:?}.", args.pattern);
    } else {
        let mut table = Table::new(&["KEY", "PROJECT", "ENV", "VALUE", "VARIANT"]);
        for m in &matches {
            let distinct = variants[&m.key].len();
            let value = shown(&m.value, args.reveal);
            table.row(vec![
                Cell::new(&m.key, Style::Bold),
                m.project.clone().into(),
                Cell::new(&m.env, Style::Cyan),
                Cell::new(
                    value.replace('\n', "\\n"),
                    if args.reveal {
                        Style::Plain
                    } else {
                        Style::Dim
                    },
                ),
                if distinct > 1 {
                    Cell::new(variant_label(m.variant), Style::Yellow)
                } else {
                    Cell::new("-", Style::Dim)
                },
            ]);
        }
        table.render(&mut stdout().lock())?;

        let mut summary = format!(
            "{} in {} {}",
            if matches.len() == 1 {
                "1 match".to_string()
            } else {
                format!("{} matches", matches.len())
            },
            files,
            if files == 1 { "file" } else { "files" }
        );
        let mut differing: Vec<(&String, usize)> = variants
            .iter()
            .filter(|(_, v)| v.len() > 1)
            .map(|(k, v)| (k, v.len()))
            .collect();
        differing.sort();
        for (key, n) in differing {
            summary.push_str(&format!(" · {key} has {n} distinct values"));
        }
        anstream::eprintln!("{}", summary.dimmed());
    }

    // like grep: 1 when nothing matched
    Ok(if matches.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

pub fn diff(ctx: &Ctx, args: DiffArgs) -> Result<ExitCode> {
    let resolve = |target: &str| {
        resolve_target(
            ctx,
            Some(target),
            &TargetFlags::default(),
            ResolveOpts {
                include_examples: false,
                may_create: false,
            },
        )
    };
    let (left_path, right_path) = (resolve(&args.left)?, resolve(&args.right)?);
    let left = DotEnv::parse(&read_text(&left_path, false)?);
    let right = DotEnv::parse(&read_text(&right_path, false)?);
    let (left, right) = (pairs_of(&left), pairs_of(&right));
    let lookup = |pairs: &[(String, String)], key: &str| {
        pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    };

    let mut out = stdout().lock();
    writeln!(out, "{}", format!("--- {}", tilde(&left_path)).dimmed())?;
    writeln!(out, "{}", format!("+++ {}", tilde(&right_path)).dimmed())?;

    let mut differences = 0usize;
    let mut identical = 0usize;
    for (key, lv) in &left {
        match lookup(&right, key) {
            None => {
                differences += 1;
                writeln!(out, "{} {key}", "-".red().bold())?;
            }
            Some(rv) if rv != *lv => {
                differences += 1;
                if args.reveal {
                    writeln!(out, "{} {key}  {} → {}", "~".yellow().bold(), lv, rv)?;
                } else {
                    writeln!(
                        out,
                        "{} {key}  {}",
                        "~".yellow().bold(),
                        "(value differs)".dimmed()
                    )?;
                }
            }
            Some(_) => identical += 1,
        }
    }
    for (key, _) in &right {
        if lookup(&left, key).is_none() {
            differences += 1;
            writeln!(out, "{} {key}", "+".green().bold())?;
        }
    }

    if differences == 0 {
        writeln!(
            out,
            "{}",
            format!("identical ({identical} variables)").dimmed()
        )?;
        Ok(ExitCode::SUCCESS)
    } else {
        writeln!(
            out,
            "{}",
            format!("{differences} differences, {identical} identical").dimmed()
        )?;
        Ok(ExitCode::from(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_labels() {
        assert_eq!(variant_label(0), "A");
        assert_eq!(variant_label(25), "Z");
        assert_eq!(variant_label(26), "#27");
    }
}
