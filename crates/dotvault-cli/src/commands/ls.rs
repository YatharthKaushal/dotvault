use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use owo_colors::OwoColorize;
use serde::Serialize;

use super::Ctx;
use crate::cli::{LsArgs, RootsCommand, ScanArgs};
use crate::discover;
use crate::filter::{Filter, FilterArgs};
use crate::index::FileRecord;
use crate::output::{age, now_secs, plural, print_json, stdout, tilde, Cell, Style, Table};

const FIRST_RUN_HINT: &str = "No folders are being scanned and no disks or home folder could be found. Point dotvault at the folder that holds your projects:\n\n  dotvault scan ~/code\n";

pub fn scan(ctx: &mut Ctx, args: ScanArgs) -> Result<ExitCode> {
    // Naming a folder as the very first thing keeps the search to that folder.
    // Otherwise start from the whole computer, or bring an automatic list up to date.
    if !(ctx.config.roots.is_empty() && !args.paths.is_empty()) {
        ctx.prepare_roots()?;
    }
    if args.system {
        // Adds what an automatic list already holds, so only a user-named list grows.
        add_roots(ctx, &discover::default_roots(), false)?;
        anstream::eprintln!(
            "{}",
            "Searching the whole computer (system folders are skipped). This can take a minute."
                .dimmed()
        );
    }
    if !args.paths.is_empty() {
        add_roots(ctx, &args.paths, true)?;
    }
    if ctx.config.roots.is_empty() {
        anstream::eprintln!("{FIRST_RUN_HINT}");
        return Ok(ExitCode::from(1));
    }

    let started = Instant::now();
    let mut index = ctx.open_index()?;
    let outcome = ctx.rescan(&mut index, args.depth)?;

    for missing in &outcome.missing_roots {
        anstream::eprintln!(
            "{} folder not found: {}",
            "warning:".yellow().bold(),
            tilde(missing)
        );
    }
    let (files, examples, projects) = counts(&outcome.records);
    let mut out = stdout().lock();
    writeln!(
        out,
        "Scanned {} in {:.2}s: {} env {} in {} {}{}",
        plural(ctx.config.roots.len(), "folder"),
        started.elapsed().as_secs_f64(),
        files,
        if files == 1 { "file" } else { "files" },
        projects,
        if projects == 1 { "project" } else { "projects" },
        if examples > 0 {
            format!(" ({} hidden by default)", plural(examples, "example file"))
        } else {
            String::new()
        },
    )?;
    if outcome.errors > 0 {
        writeln!(
            out,
            "{}",
            format!("{} entries could not be read", outcome.errors).dimmed()
        )?;
    }
    Ok(ExitCode::SUCCESS)
}

pub fn roots(ctx: &mut Ctx, command: RootsCommand) -> Result<ExitCode> {
    match command {
        RootsCommand::Add { paths } => {
            let added = add_roots(ctx, &paths, true)?;
            anstream::println!(
                "Remembered {}. Run `dotvault scan` to index {}.",
                plural(added, "new folder"),
                if added == 1 { "it" } else { "them" }
            );
        }
        RootsCommand::Rm { paths } => {
            let mut removed = 0;
            for p in &paths {
                let canonical = crate::fsutil::canonical(p).unwrap_or_else(|_| p.clone());
                let before = ctx.config.roots.len();
                ctx.config.roots.retain(|r| *r != canonical && r != p);
                if ctx.config.roots.len() == before {
                    bail!(
                        "{} is not one of the scanned folders (see `dotvault roots ls`)",
                        p.display()
                    );
                }
                removed += before - ctx.config.roots.len();
            }
            ctx.config.mark_roots_explicit();
            ctx.config.save(&ctx.dirs)?;
            anstream::println!("Forgot {}.", plural(removed, "folder"));
        }
        RootsCommand::Ls => {
            if ctx.config.roots.is_empty() {
                anstream::eprintln!(
                    "No folders set up yet. The first command that needs them searches your whole computer, or add one with `dotvault scan <dir>`."
                );
                return Ok(ExitCode::SUCCESS);
            }
            let mut out = stdout().lock();
            for root in &ctx.config.roots {
                if root.is_dir() {
                    writeln!(out, "{}", tilde(root))?;
                } else {
                    writeln!(out, "{}  {}", tilde(root), "(missing)".red())?;
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Canonicalizes, de-duplicates and saves new scan roots. Returns how many were new.
/// Adds folders to the saved list. `named_by_user` is for folders the user typed:
/// the list is then theirs and dotvault stops managing it.
fn add_roots(ctx: &mut Ctx, paths: &[PathBuf], named_by_user: bool) -> Result<usize> {
    let mut added = 0;
    for p in paths {
        let canonical = match crate::fsutil::canonical(p) {
            Ok(canonical) => canonical,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!("{}", not_a_folder(p)),
            Err(e) => return Err(e).with_context(|| format!("cannot access {}", p.display())),
        };
        if !canonical.is_dir() {
            bail!("{} is not a folder", p.display());
        }
        if !ctx.config.roots.contains(&canonical) {
            ctx.config.roots.push(canonical);
            added += 1;
        }
    }
    if added > 0 {
        if named_by_user {
            ctx.config.mark_roots_explicit();
        }
        ctx.config.save(&ctx.dirs)?;
    }
    Ok(added)
}

/// What to say when `scan NAME` finds no such folder. A bare word is most likely
/// a project name, which `scan` does not take.
fn not_a_folder(path: &std::path::Path) -> String {
    let shown = path.display();
    let bare = path.components().count() == 1 && !path.has_root();
    if bare {
        format!("{shown} is not a folder here. `scan` searches folders; to look at a project use `dotvault show {shown}`")
    } else {
        format!("no folder at {shown}")
    }
}

pub fn ls(ctx: &Ctx, args: LsArgs) -> Result<ExitCode> {
    if ctx.config.roots.is_empty() {
        if args.json {
            print_json(&Vec::<()>::new())?;
        } else {
            anstream::eprintln!("{FIRST_RUN_HINT}");
        }
        return Ok(ExitCode::SUCCESS);
    }

    let records = ctx.records(args.refresh)?;
    let under = match &args.folder {
        Some(p) => Some(
            crate::fsutil::canonical(p)
                .with_context(|| format!("cannot access {}", p.display()))?,
        ),
        None => None,
    };
    let build = |filter_args: &FilterArgs| -> Result<Filter> {
        let mut f = Filter::new(filter_args)?.with_keys(&args.has_key)?;
        if let Some(dir) = &under {
            f = f.under(dir.clone());
        }
        Ok(f)
    };

    let filter = build(&args.filter)?;
    let shown: Vec<&FileRecord> = records.iter().filter(|r| filter.matches(r)).collect();

    // How many example files the default view is hiding.
    let hidden_examples = if args.filter.examples || args.filter.only_examples {
        0
    } else {
        let only = FilterArgs {
            only_examples: true,
            ..args.filter.clone()
        };
        let f = build(&only)?;
        records.iter().filter(|r| f.matches(r)).count()
    };

    if args.json {
        print_json(&shown.iter().map(|r| JsonFile::from(*r)).collect::<Vec<_>>())?;
        return Ok(ExitCode::SUCCESS);
    }
    if args.paths {
        let mut out = stdout().lock();
        for r in &shown {
            writeln!(out, "{}", r.path.display())?;
        }
        return Ok(ExitCode::SUCCESS);
    }

    if shown.is_empty() {
        anstream::eprintln!("No env files match.");
        if records.is_empty() {
            anstream::eprintln!(
                "Nothing is indexed yet; check `dotvault roots ls` and run `dotvault scan`."
            );
        } else if hidden_examples > 0 {
            anstream::eprintln!(
                "({} hidden; add --examples to include them)",
                plural(hidden_examples, "example file")
            );
        }
        return Ok(ExitCode::SUCCESS);
    }

    let now = now_secs();
    let mut table = Table::new(&["PROJECT", "ENV", "KEYS", "MODIFIED", "PATH"]);
    let mut previous_root: Option<&PathBuf> = None;
    for r in &shown {
        let repeat = previous_root == Some(&r.project_root);
        previous_root = Some(&r.project_root);
        table.row(vec![
            Cell::new(&r.project, if repeat { Style::Dim } else { Style::Bold }),
            Cell::new(
                if r.is_example {
                    format!("{} (example)", r.label)
                } else {
                    r.label.clone()
                },
                if r.is_example {
                    Style::Dim
                } else {
                    Style::Cyan
                },
            ),
            r.keys.len().to_string().into(),
            Cell::new(age(now, r.mtime / 1000), Style::Dim),
            Cell::new(tilde(&r.path), Style::Dim),
        ]);
    }
    table.render(&mut stdout().lock())?;

    let projects = shown
        .iter()
        .map(|r| &r.project_root)
        .collect::<HashSet<_>>()
        .len();
    let mut summary = format!(
        "{} in {}",
        plural(shown.len(), "file"),
        plural(projects, "project")
    );
    if hidden_examples > 0 {
        summary.push_str(&format!(
            " · {} hidden (--examples to include)",
            plural(hidden_examples, "example file")
        ));
    }
    anstream::eprintln!("{}", summary.dimmed());
    Ok(ExitCode::SUCCESS)
}

pub fn config(ctx: &Ctx) -> Result<ExitCode> {
    let mut out = stdout().lock();
    let config_file = ctx.dirs.config_file();
    let index_file = ctx.dirs.index_file();
    writeln!(
        out,
        "config file   {}{}",
        tilde(&config_file),
        if config_file.exists() {
            ""
        } else {
            "  (not created yet)"
        }
    )?;
    writeln!(out, "index         {}", tilde(&index_file))?;
    writeln!(
        out,
        "server        {}",
        ctx.config.server.as_deref().unwrap_or("(not set)")
    )?;
    writeln!(out, "max depth     {}", ctx.config.max_depth)?;
    writeln!(out, "auto rescan   after {}s", ctx.config.auto_scan_secs)?;
    writeln!(
        out,
        "example names {}",
        ctx.config.example_suffixes.join(", ")
    )?;
    if !ctx.config.ignore.is_empty() {
        writeln!(out, "extra ignores {}", ctx.config.ignore.join(", "))?;
    }
    writeln!(out, "folders       {}", ctx.config.roots.len())?;
    for root in &ctx.config.roots {
        writeln!(out, "  {}", tilde(root))?;
    }
    Ok(ExitCode::SUCCESS)
}

fn counts(records: &[FileRecord]) -> (usize, usize, usize) {
    let examples = records.iter().filter(|r| r.is_example).count();
    let projects = records
        .iter()
        .filter(|r| !r.is_example)
        .map(|r| &r.project_root)
        .collect::<HashSet<_>>()
        .len();
    (records.len() - examples, examples, projects)
}

#[derive(Serialize)]
struct JsonFile<'a> {
    path: String,
    project: &'a str,
    project_root: String,
    env: &'a str,
    example: bool,
    modified: i64,
    key_count: usize,
    key_names: &'a [String],
}

impl<'a> From<&'a FileRecord> for JsonFile<'a> {
    fn from(r: &'a FileRecord) -> Self {
        JsonFile {
            path: r.path.display().to_string(),
            project: &r.project,
            project_root: r.project_root.display().to_string(),
            env: &r.label,
            example: r.is_example,
            modified: r.mtime / 1000,
            key_count: r.keys.len(),
            key_names: &r.keys,
        }
    }
}
