mod account;
mod cloud;
mod edit;
mod history;
mod ls;
mod secrets;
mod share;
mod vault;
mod view;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context, Result};
use owo_colors::OwoColorize;

use crate::cli::{Cli, Command, TargetFlags};
use crate::clipboard;
use crate::config::{Config, Dirs, RootsPlan};
use dotvault_core::secrets::Kind;

use crate::discover;
use crate::fsutil;
use crate::index::{FileRecord, Index};
use crate::output::{humanize_secs, now_secs, tilde};
use crate::resolve::{self, parse_target, Target};
use crate::scan::{self, ScanOptions, ScanOutcome};

pub fn run(cli: Cli) -> Result<ExitCode> {
    // The clipboard helper needs nothing from the config, so it starts without it.
    if matches!(cli.command, Command::ClipHold) {
        return clipboard::hold();
    }
    let mut ctx = Ctx::load()?;
    if cli.command.needs_index() {
        ctx.prepare_roots()?;
    }
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
        Command::Mark(a) => secrets::mark(&mut ctx, a, true),
        Command::Unmark(a) => secrets::mark(&mut ctx, a, false),
        Command::Log(a) => history::log(&ctx, a),
        Command::Revert(a) => history::revert(&ctx, a),
        Command::Forget(a) => history::forget(&ctx, a),
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
        Command::ClipHold => clipboard::hold(),
    }
}

impl Command {
    /// Whether the command looks things up in the list of env files, so that a
    /// first run must have folders to look in. Commands given a path never do,
    /// so scripts and CI never trigger a scan they did not ask for.
    fn needs_index(&self) -> bool {
        match self {
            Command::Ls(_) | Command::Find(_) => true,
            Command::Show(a) => selects_project(a.target.as_deref(), &a.flags),
            Command::Get(a) => selects_project(None, &a.flags),
            Command::Set(a) => selects_project(None, &a.flags),
            Command::Unset(a) => selects_project(None, &a.flags),
            Command::Edit(a) => selects_project(a.target.as_deref(), &a.flags),
            Command::Log(a) => selects_project(None, &a.flags),
            Command::Revert(a) => selects_project(None, &a.flags),
            Command::Forget(a) => selects_project(None, &a.flags),
            Command::Push(a) => selects_project(a.target.as_deref(), &a.flags),
            Command::Share(a) => {
                a.vault.is_none() && selects_project(a.target.as_deref(), &a.flags)
            }
            Command::Diff(a) => is_project_selector(&a.left) || is_project_selector(&a.right),
            _ => false,
        }
    }
}

fn is_project_selector(text: &str) -> bool {
    matches!(parse_target(text), Target::Project { .. })
}

/// Whether a command's target is a project name (`acme`, `acme:prod`, `-p acme`)
/// and not a file path.
fn selects_project(positional: Option<&str>, flags: &TargetFlags) -> bool {
    flags.project.is_some() || positional.is_some_and(is_project_selector)
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

    /// Makes sure there are folders to search before a command that needs them:
    /// the whole computer when nothing is saved yet, and the computer's current
    /// disks while dotvault (not the user) chose the saved folders. Returns
    /// whether the saved folders changed.
    fn prepare_roots(&mut self) -> Result<bool> {
        let default = discover::default_roots();
        let home = discover::home_dir();
        let (roots, notice) = match self.config.plan_roots(&default, home.as_deref()) {
            RootsPlan::Keep => return Ok(false),
            RootsPlan::Start(roots) => (
                roots,
                Some(format!(
                    "No folders set up yet, so searching {}. The first run can take a minute. Change it with `dotvault roots`.",
                    discover::default_roots_label()
                )),
            ),
            RootsPlan::Widen(roots) => (
                roots,
                Some(format!(
                    "Now searching {} (it was your home folder only). The first run can take a minute. Change it with `dotvault roots`.",
                    discover::default_roots_label()
                )),
            ),
            RootsPlan::Refresh(roots) => (roots, None),
        };
        self.config.roots = roots;
        self.config.roots_auto = Some(true);
        self.config.save(&self.dirs)?;
        if let Some(notice) = notice {
            anstream::eprintln!("{}", notice.dimmed());
        }
        Ok(true)
    }

    /// Scans the remembered folders and stores the result in the index.
    fn rescan(&self, index: &mut Index, depth: Option<usize>) -> Result<ScanOutcome> {
        let outcome = scan::scan(&ScanOptions::from_config(&self.config, depth));
        index.replace_all(
            &outcome.records,
            &self.config.scan_signature(),
            now_secs(),
            outcome.elapsed.as_millis() as u64,
        )?;
        Ok(outcome)
    }

    /// Indexed env files, rescanning first when the index is old, was built
    /// for different settings, or `force` is set. Entries are checked against
    /// the disk, so deleted files disappear and edited ones are re-read. Secret
    /// files are not included: everything that picks an env file (projects,
    /// `find`, `diff`) works on these only.
    fn records(&self, force: bool) -> Result<Vec<FileRecord>> {
        let mut records = self.all_records(force)?;
        records.retain(|r| r.kind == Kind::Env);
        Ok(records)
    }

    /// Everything indexed: env files and secret files.
    fn all_records(&self, force: bool) -> Result<Vec<FileRecord>> {
        let mut index = self.open_index()?;
        if !self.config.roots.is_empty() {
            let gap = rescan_gap_secs(self.config.auto_scan_secs, index.last_scan_millis()?);
            let stale = force
                || index.signature()?.as_deref() != Some(self.config.scan_signature().as_str())
                || index
                    .last_scan()?
                    .is_none_or(|t| now_secs() - t > gap as i64);
            if stale {
                let outcome = self.rescan(&mut index, None)?;
                if outcome.elapsed.as_secs_f64() > 2.0 {
                    anstream::eprintln!(
                        "{}",
                        format!(
                            "Refreshed the env file list in {:.1}s. A scan this slow is repeated at most every {} (see auto_scan_secs in the config).",
                            outcome.elapsed.as_secs_f64(),
                            humanize_secs(rescan_gap_secs(
                                self.config.auto_scan_secs,
                                Some(outcome.elapsed.as_millis() as u64),
                            ))
                        )
                        .dimmed()
                    );
                }
            }
        }
        let records = index.all()?;
        index.refresh_stale(records)
    }

    /// Waits for the turn to change `path`, so that two commands working on the same
    /// env file at once cannot each start from the old text and lose the other's
    /// change. Hold the result until the new text is written and recorded. Gives up,
    /// before anything is written, if another command keeps the file for too long.
    /// If the lock cannot be set up at all (a read-only data folder), the command goes
    /// on without it, as it always did.
    pub fn lock_file(&self, path: &Path) -> Result<Option<fsutil::EditLock>> {
        match fsutil::lock_for_edit(&self.dirs.lock_dir(), path) {
            Ok(lock) => Ok(Some(lock)),
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => bail!(
                "another dotvault command has been changing {} for {} seconds; run this again in a moment",
                tilde(path),
                fsutil::EDIT_LOCK_WAIT.as_secs()
            ),
            Err(_) => Ok(None),
        }
    }

    /// Records what a write changed in the history. The file is already saved, so
    /// a failure here is a warning, never an error.
    pub fn record(
        &self,
        path: &Path,
        before: Option<&crate::history::Before>,
        after: &str,
        action: crate::history::Action,
    ) {
        if let Err(err) = self.try_record(path, before, after, action) {
            anstream::eprintln!(
                "{} the change was saved, but its history could not be recorded: {err:#}",
                "warning:".yellow().bold()
            );
        }
    }

    /// Like [`Ctx::record`] but reports failure, and does nothing if history is off.
    pub fn try_record(
        &self,
        path: &Path,
        before: Option<&crate::history::Before>,
        after: &str,
        action: crate::history::Action,
    ) -> Result<usize> {
        if !self.config.history || !crate::history::has_changes(before, after) {
            return Ok(0);
        }
        let mut history = crate::history::History::open(&self.dirs.history_file())?;
        history.record(
            &crate::history::key_for(path),
            before,
            after,
            action,
            self.config.history_keep,
            now_secs(),
        )
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

/// How old the list may get before it is refreshed on its own: the configured
/// interval, stretched to 50 times the last scan's duration so a slow scan (the
/// whole machine) costs about 2% of the time at most.
fn rescan_gap_secs(auto_scan_secs: u64, last_scan_millis: Option<u64>) -> u64 {
    let slow = (last_scan_millis.unwrap_or(0) / 1000).saturating_mul(50);
    auto_scan_secs.max(slow)
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
                bail!("no folders are being scanned and no disks or home folder could be found; run `dotvault scan <dir>` first, or pass a file path");
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn needs_index(args: &[&str]) -> bool {
        let mut full = vec!["dotvault"];
        full.extend_from_slice(args);
        Cli::try_parse_from(full).unwrap().command.needs_index()
    }

    #[test]
    fn commands_that_look_things_up_need_the_list_of_env_files() {
        for args in [
            &["ls"][..],
            &["find", "API_KEY"],
            &["show", "acme"],
            &["show", "acme:production"],
            &["show", "-p", "acme"],
            &["get", "KEY", "-p", "acme", "-e", "prod"],
            &["set", "A=1", "-p", "acme"],
            &["unset", "A", "-p", "acme"],
            &["edit", "acme"],
            &["push", "acme:prod"],
            &["share", "acme", "--keys", "A"],
            &["diff", "./.env", "acme"],
            &["diff", "acme", "acme:production"],
        ] {
            assert!(needs_index(args), "{args:?} should need the list");
        }
    }

    #[test]
    fn commands_given_a_path_or_nothing_to_look_up_do_not() {
        for args in [
            &["show"][..],
            &["show", "./.env"],
            &["show", "dir/.env.local"],
            &["show", "-f", "x/.env"],
            &["get", "KEY"],
            &["get", "KEY", "-f", "x/.env"],
            &["set", "A=1"],
            &["push", "./.env"],
            &["share", "--vault", "acme/prod"],
            &["share", "--vault", "acme", "--keys", "A"],
            &["diff", "./a.env", "./b.env"],
            &["scan"],
            &["roots", "ls"],
            &["config"],
            &["whoami"],
            &["login"],
            &["vault", "ls"],
            &[
                "open",
                "dv1.aaaaaaaaaaaaaaaaaaaaaa.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "--reveal",
            ],
            &["completions", "bash"],
        ] {
            assert!(!needs_index(args), "{args:?} must not trigger a scan");
        }
    }

    #[test]
    fn a_slow_scan_is_repeated_less_often() {
        // fast scans keep the configured interval
        assert_eq!(rescan_gap_secs(600, None), 600);
        assert_eq!(rescan_gap_secs(600, Some(900)), 600);
        assert_eq!(rescan_gap_secs(600, Some(11_000)), 600);
        // 26 s (the whole machine) is spaced to 50x
        assert_eq!(rescan_gap_secs(600, Some(26_000)), 1300);
        assert_eq!(rescan_gap_secs(600, Some(120_000)), 6000);
        // a larger configured interval is never shortened
        assert_eq!(rescan_gap_secs(86_400, Some(26_000)), 86_400);
        assert_eq!(rescan_gap_secs(0, Some(2_000)), 100);
    }
}
