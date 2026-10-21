mod account;
mod cloud;
mod edit;
mod ls;
mod share;
mod vault;
mod view;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context, Result};

use crate::cli::{Cli, Command, TargetFlags};
use crate::config::{Config, Dirs};
use crate::index::{FileRecord, Index};
use crate::output::{now_secs, tilde};
use crate::resolve::{self, parse_target, Target};
use crate::scan::{self, ScanOptions, ScanOutcome};

pub fn run(cli: Cli) -> Result<ExitCode> {
    let mut ctx = Ctx::load()?;
    match cli.command {
        Command::Scan(a) => ls::scan(&mut ctx, a),
        Command::Roots { command } => ls::roots(&mut ctx, command),
        Command::Ls(a) => ls::ls(&ctx, a),
        Command::Config => ls::config(&ctx),
        Command::Show(a) => view::show(&ctx, a),
        Command::Find(a) => view::find(&ctx, a),
        Command::Diff(a) => view::diff(&ctx, a),
        Command::Get(a) => edit::get(&ctx, a),
        Command::Set(a) => edit::set(&ctx, a),
        Command::Unset(a) => edit::unset(&ctx, a),
        Command::Edit(a) => edit::edit(&ctx, a),
        Command::Register(a) => account::register(&mut ctx, a),
        Command::Login(a) => account::login(&mut ctx, a),
        Command::Logout => account::logout(&ctx),
        Command::Whoami => account::whoami(&ctx),
        Command::Passwd => account::passwd(&ctx),
        Command::Recover(a) => account::recover(&mut ctx, a),
        Command::Push(a) => vault::push(&ctx, a),
        Command::Pull(a) => vault::pull(&ctx, a),
        Command::Vault { command } => vault::vault(&ctx, command),
        Command::Share(a) => share::share(&ctx, a),
        Command::Open(a) => share::open(&ctx, a),
        Command::Shares { command } => share::shares(&ctx, command),
        Command::Completions { shell } => {
            use clap::CommandFactory;
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "dotvault", &mut std::io::stdout());
            Ok(ExitCode::SUCCESS)
        }
    }
}

pub struct Ctx {
    pub dirs: Dirs,
    pub config: Config,
}

impl Ctx {
    fn load() -> Result<Ctx> {
        let dirs = Dirs::detect()?;
        let config = Config::load(&dirs)?;
        Ok(Ctx { dirs, config })
    }

    fn open_index(&self) -> Result<Index> {
        Index::open(&self.dirs.index_file())
    }

    /// Scans the remembered folders and stores the result in the index.
    fn rescan(&self, index: &mut Index, depth: Option<usize>) -> Result<ScanOutcome> {
        let outcome = scan::scan(&ScanOptions::from_config(&self.config, depth));
        index.replace_all(&outcome.records, &self.config.scan_signature(), now_secs())?;
        Ok(outcome)
    }

    /// Indexed env files, rescanning first when the index is old, was built
    /// for different settings, or `force` is set. Entries are checked against
    /// the disk, so deleted files disappear and edited ones are re-read.
    fn records(&self, force: bool) -> Result<Vec<FileRecord>> {
        let mut index = self.open_index()?;
        if !self.config.roots.is_empty() {
            let age_limit = self.config.auto_scan_secs as i64;
            let stale = force
                || index.signature()?.as_deref() != Some(self.config.scan_signature().as_str())
                || index
                    .last_scan()?
                    .is_none_or(|t| now_secs() - t > age_limit);
            if stale {
                self.rescan(&mut index, None)?;
            }
        }
        let records = index.all()?;
        index.refresh_stale(records)
    }

    /// Keeps the index accurate after a file was changed. Best effort.
    fn touch_index(&self, path: &Path) {
        if self.dirs.index_file().exists() {
            if let Ok(index) = self.open_index() {
                let _ = index.touch(path);
            }
        }
    }
}

pub struct ResolveOpts {
    /// Let a bare `project` selector land on an example file.
    pub include_examples: bool,
    /// The file may not exist yet (set, edit).
    pub may_create: bool,
}

/// Resolves a positional target and/or `--file/--project/--env` to one path.
/// With neither, falls back to `./.env`.
pub fn resolve_target(
    ctx: &Ctx,
    positional: Option<&str>,
    flags: &TargetFlags,
    opts: ResolveOpts,
) -> Result<PathBuf> {
    if positional.is_some() && (flags.file.is_some() || flags.project.is_some()) {
        bail!("give either a target or --file/--project, not both");
    }

    let defaulted = positional.is_none() && flags.file.is_none() && flags.project.is_none();
    let target = if let Some(text) = positional {
        parse_target(text)
    } else if let Some(file) = &flags.file {
        Target::Path(resolve::expand_tilde(&file.to_string_lossy()))
    } else if let Some(project) = &flags.project {
        Target::Project {
            name: project.clone(),
            env: flags.env.clone(),
        }
    } else {
        Target::Path(PathBuf::from(".env"))
    };

    match target {
        Target::Path(path) => {
            if path.is_dir() {
                bail!("{} is a directory, not an env file", path.display());
            }
            if !path.exists() && !opts.may_create {
                if defaulted {
                    bail!("no .env in the current directory; pass a path, or a project name (see `dotvault ls`)");
                }
                bail!("{} does not exist", path.display());
            }
            Ok(path)
        }
        Target::Project { name, env } => {
            if ctx.config.roots.is_empty() {
                bail!("no folders are being scanned yet; run `dotvault scan <dir>` first, or pass a file path");
            }
            let records = ctx.records(false)?;
            let rec = resolve::pick(&records, &name, env.as_deref(), opts.include_examples)?;
            Ok(rec.path.clone())
        }
    }
}

/// Reads an env file. `strict` refuses non-UTF-8 content, which must not be
/// rewritten lossily.
pub fn read_text(path: &Path, strict: bool) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    if strict {
        String::from_utf8(bytes)
            .map_err(|_| anyhow!("{} is not valid UTF-8; refusing to modify it", tilde(path)))
    } else {
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}
