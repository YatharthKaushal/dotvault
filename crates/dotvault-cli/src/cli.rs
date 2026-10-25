use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;

use crate::filter::FilterArgs;
use crate::output::Format;

#[derive(Parser, Debug)]
#[command(
    name = "dotvault",
    version,
    about = "Find, view, edit, store and share your .env files from one place",
    disable_version_flag = true,
    propagate_version = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Print version
    // clap's own flag is only `-V`; people also reach for `-v`. The field is never read:
    // clap prints the version and exits when the flag is seen.
    #[arg(short = 'v', short_alias = 'V', long, action = clap::ArgAction::Version, global = true)]
    #[allow(dead_code)]
    version: Option<bool>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Find env files under folders and remember those folders (default: the whole computer)
    Scan(ScanArgs),
    /// Manage the folders that are searched
    Roots {
        #[command(subcommand)]
        command: RootsCommand,
    },
    /// List env files (examples are hidden unless asked for)
    #[command(visible_alias = "list")]
    Ls(LsArgs),
    /// Show the variables of one env file (values are masked unless --reveal)
    Show(ShowArgs),
    /// Find a variable across every env file
    Find(FindArgs),
    /// Print the value of one or more variables, or copy it to the clipboard
    Get(GetArgs),
    /// Set variables, keeping the rest of the file untouched
    Set(SetArgs),
    /// Remove variables
    Unset(UnsetArgs),
    /// Open an env file in $VISUAL / $EDITOR
    Edit(EditArgs),
    /// Compare two env files (exit code 1 if they differ)
    Diff(DiffArgs),
    /// Show where dotvault keeps its config and settings
    Config,
    /// Create an account on a dotvault server
    Register(RegisterArgs),
    /// Log in to a dotvault server
    Login(LoginArgs),
    /// Log out and forget the saved session
    Logout,
    /// Show who you are logged in as
    Whoami,
    /// Change your password
    Passwd,
    /// Get back into your account with the recovery key
    Recover(RecoverArgs),
    /// Store an env file in your encrypted cloud vault
    Push(PushArgs),
    /// Fetch an env file from your cloud vault
    Pull(PullArgs),
    /// Browse your cloud vault
    Vault {
        #[command(subcommand)]
        command: VaultCommand,
    },
    /// Create a link/code that gives someone access to chosen variables
    Share(ShareArgs),
    /// Open a share link or code someone sent you
    Open(OpenArgs),
    /// Manage the shares you created
    Shares {
        #[command(subcommand)]
        command: SharesCommand,
    },
    /// Print a shell completion script
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Internal: holds a copied value on the clipboard, then clears it (started by `get --copy`)
    #[command(name = "_clip-hold", hide = true)]
    ClipHold,
}

/// Spells out `-ckv` (copy KEY=value) before clap sees it. Short flags are one
/// letter, so clap would read `-ckv` as `-c -k -v`, and `-v` prints the version.
pub fn expand_shorthands<I>(args: I) -> Vec<std::ffi::OsString>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let mut after_dashes = false;
    let mut out = Vec::new();
    for arg in args {
        if after_dashes {
            out.push(arg);
        } else if arg == "--" {
            after_dashes = true;
            out.push(arg);
        } else if arg == "-ckv" {
            out.push("-c".into());
            out.push("--kv".into());
        } else {
            out.push(arg);
        }
    }
    out
}

#[derive(Args, Debug)]
pub struct ScanArgs {
    /// Folders to search and remember (default: the folders already remembered)
    pub paths: Vec<PathBuf>,
    /// How many directory levels to descend
    #[arg(long, value_name = "N")]
    pub depth: Option<usize>,
    /// Also search the whole computer, every drive (system folders are skipped; this can take a minute)
    #[arg(long)]
    pub system: bool,
}

#[derive(Subcommand, Debug)]
pub enum RootsCommand {
    /// Remember folders without scanning them yet
    Add { paths: Vec<PathBuf> },
    /// Forget folders
    Rm { paths: Vec<PathBuf> },
    /// List the remembered folders
    Ls,
}

#[derive(Args, Debug)]
pub struct LsArgs {
    /// Only files inside this folder
    pub folder: Option<PathBuf>,

    #[command(flatten)]
    pub filter: FilterArgs,

    /// Only files that define this variable (exact name or glob, repeatable)
    #[arg(short = 'k', long = "has-key", value_name = "KEY")]
    pub has_key: Vec<String>,

    /// Print only file paths, one per line
    #[arg(long, conflicts_with = "json")]
    pub paths: bool,

    /// Machine-readable output (key names are included, values never)
    #[arg(long)]
    pub json: bool,

    /// Rescan the remembered folders first
    #[arg(long)]
    pub refresh: bool,
}

/// How to pick a single env file. Without any of these, `./.env` is used.
#[derive(Args, Debug, Default, Clone)]
pub struct TargetFlags {
    /// Path to an env file
    #[arg(short, long, value_name = "PATH", conflicts_with_all = ["project", "env"])]
    pub file: Option<PathBuf>,

    /// Project name (see `dotvault ls`)
    #[arg(short, long, value_name = "NAME")]
    pub project: Option<String>,

    /// Environment of the project: default, local, production, ...
    #[arg(short, long, value_name = "ENV", requires = "project")]
    pub env: Option<String>,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// A path, `project` or `project:env` (default: ./.env)
    pub target: Option<String>,

    #[command(flatten)]
    pub flags: TargetFlags,

    /// Only these variables (comma separated; names or globs like 'RAZORPAY_*')
    #[arg(short, long, value_delimiter = ',', value_name = "KEYS")]
    pub keys: Vec<String>,

    /// Print real values instead of masks
    #[arg(long)]
    pub reveal: bool,

    #[arg(long, value_enum, default_value = "table")]
    pub format: Format,

    /// Let a bare `project` selector resolve to an example file
    #[arg(long)]
    pub examples: bool,
}

#[derive(Args, Debug)]
pub struct FindArgs {
    /// Variable name or glob, e.g. DATABASE_URL or 'AWS_*'
    pub pattern: String,

    #[command(flatten)]
    pub filter: FilterArgs,

    /// Print real values instead of masks
    #[arg(long)]
    pub reveal: bool,

    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct GetArgs {
    /// Variables to print: one name prints the bare value, several (or a glob like
    /// 'RAZORPAY_*') print KEY=value lines
    #[arg(required = true, value_name = "KEY")]
    pub keys: Vec<String>,

    #[command(flatten)]
    pub flags: TargetFlags,

    /// Copy to the clipboard instead of printing, and clear it again after 30 seconds
    #[arg(short = 'c', long)]
    pub copy: bool,

    /// Print or copy KEY=value instead of just the value (`-ckv` is short for `-c --kv`)
    #[arg(short = 'k', long)]
    pub kv: bool,
}

#[derive(Args, Debug)]
pub struct SetArgs {
    /// KEY=VALUE pairs. A bare KEY prompts for the value so it stays out of shell history.
    #[arg(required = true, value_name = "KEY[=VALUE]")]
    pub assignments: Vec<String>,

    #[command(flatten)]
    pub flags: TargetFlags,
}

#[derive(Args, Debug)]
pub struct UnsetArgs {
    #[arg(required = true)]
    pub keys: Vec<String>,

    #[command(flatten)]
    pub flags: TargetFlags,
}

#[derive(Args, Debug)]
pub struct EditArgs {
    /// A path, `project` or `project:env` (default: ./.env)
    pub target: Option<String>,

    #[command(flatten)]
    pub flags: TargetFlags,
}

#[derive(Args, Debug)]
pub struct DiffArgs {
    /// A path, `project` or `project:env`
    pub left: String,
    /// A path, `project` or `project:env`
    pub right: String,

    /// Show real values for changed variables
    #[arg(long)]
    pub reveal: bool,
}

#[derive(Args, Debug)]
pub struct RegisterArgs {
    /// Server to register on (default: your config, your last session, or the hosted server)
    #[arg(long, value_name = "URL", env = "DOTVAULT_SERVER")]
    pub server: Option<String>,
    #[arg(short, long)]
    pub username: Option<String>,
}

#[derive(Args, Debug)]
pub struct LoginArgs {
    /// Server to log in to (default: your config, your last session, or the hosted server)
    #[arg(long, value_name = "URL", env = "DOTVAULT_SERVER")]
    pub server: Option<String>,
    #[arg(short, long)]
    pub username: Option<String>,
    /// Do not keep the vault key on disk; vault commands will ask for the password instead
    #[arg(long)]
    pub no_cache_key: bool,
}

#[derive(Args, Debug)]
pub struct RecoverArgs {
    /// Server to recover on (default: your config, your last session, or the hosted server)
    #[arg(long, value_name = "URL", env = "DOTVAULT_SERVER")]
    pub server: Option<String>,
    #[arg(short, long)]
    pub username: Option<String>,
    /// Do not keep the vault key on disk
    #[arg(long)]
    pub no_cache_key: bool,
}

#[derive(Args, Debug)]
pub struct PushArgs {
    /// A path, `project` or `project:env` (default: ./.env)
    pub target: Option<String>,

    #[command(flatten)]
    pub flags: TargetFlags,

    /// Name in the vault (default: project/env, e.g. acme-api/production)
    #[arg(short, long)]
    pub name: Option<String>,

    /// Only these variables (comma separated)
    #[arg(short, long, value_delimiter = ',', value_name = "KEYS")]
    pub keys: Vec<String>,
}

#[derive(Args, Debug)]
pub struct PullArgs {
    /// Vault item name (see `dotvault vault ls`)
    pub name: String,

    /// Write to this file
    #[arg(short, long, value_name = "PATH", conflicts_with = "stdout")]
    pub out: Option<PathBuf>,

    /// Print the variables instead of writing a file
    #[arg(long)]
    pub stdout: bool,

    /// Replace the file if it exists
    #[arg(long, requires = "out", conflicts_with = "merge")]
    pub force: bool,

    /// Add to the file if it exists, keeping its comments and other variables
    #[arg(long, requires = "out")]
    pub merge: bool,

    /// Only these variables (comma separated)
    #[arg(short, long, value_delimiter = ',', value_name = "KEYS")]
    pub keys: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum VaultCommand {
    /// List your vault items
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// Show an item's variables (masked unless --reveal)
    Show {
        name: String,
        #[arg(long)]
        reveal: bool,
        #[arg(long, value_enum, default_value = "table")]
        format: Format,
        #[arg(short, long, value_delimiter = ',', value_name = "KEYS")]
        keys: Vec<String>,
    },
    /// Delete an item
    Rm {
        name: String,
        /// Do not ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Args, Debug)]
pub struct ShareArgs {
    /// A path, `project` or `project:env` (default: ./.env)
    #[arg(conflicts_with = "vault")]
    pub target: Option<String>,

    #[command(flatten)]
    pub flags: TargetFlags,

    /// Share a vault item instead of a local file
    #[arg(long, value_name = "NAME", conflicts_with_all = ["file", "project"])]
    pub vault: Option<String>,

    /// Only these variables (comma separated)
    #[arg(
        short,
        long,
        value_delimiter = ',',
        value_name = "KEYS",
        conflicts_with = "interactive"
    )]
    pub keys: Vec<String>,

    /// Pick the variables from a list
    #[arg(short, long)]
    pub interactive: bool,

    /// How long the share works: 30m, 24h, 7d (max 30d)
    #[arg(long, default_value = "24h", value_name = "TIME")]
    pub expires: String,

    /// Stop working after this many opens
    #[arg(long, value_name = "N")]
    pub max_views: Option<u32>,

    /// A message the recipient sees
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(Args, Debug)]
pub struct OpenArgs {
    /// The link or code you were sent
    pub share: String,

    /// Print the variables (real values)
    #[arg(long)]
    pub reveal: bool,

    #[arg(long, value_enum, default_value = "table", requires = "reveal")]
    pub format: Format,

    /// Write the variables to this file
    #[arg(short, long, value_name = "PATH")]
    pub write: Option<PathBuf>,

    /// Replace the file if it exists
    #[arg(long, requires = "write", conflicts_with = "merge")]
    pub force: bool,

    /// Add to the file if it exists, keeping its comments and other variables
    #[arg(long, requires = "write")]
    pub merge: bool,

    /// Keep a copy in your own vault, optionally under NAME
    #[arg(long, value_name = "NAME", num_args = 0..=1)]
    pub save: Option<Option<String>>,
}

#[derive(Subcommand, Debug)]
pub enum SharesCommand {
    /// List the shares you created
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// See who opened or saved a share
    Log {
        /// Share id (a unique prefix is enough)
        #[arg(allow_hyphen_values = true)]
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop a share from working
    Revoke {
        /// Share id (a unique prefix is enough)
        #[arg(allow_hyphen_values = true)]
        id: String,
    },
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    /// Catches duplicate argument ids and similar mistakes that clap would
    /// otherwise only report as a panic when the command runs.
    #[test]
    fn command_definition_is_valid() {
        super::Cli::command().debug_assert();
    }

    /// Share ids are base64url, so about 1 in 64 start with `-` (or `_`). They
    /// must be accepted as values, not mistaken for options.
    #[test]
    fn share_ids_starting_with_a_hyphen_are_accepted() {
        use super::{Command, SharesCommand};
        use clap::Parser;

        let id_of = |args: &[&str]| match super::Cli::try_parse_from(args).unwrap().command {
            Command::Shares {
                command: SharesCommand::Log { id, json },
            } => (id, json),
            Command::Shares {
                command: SharesCommand::Revoke { id },
            } => (id, false),
            _ => panic!("not a shares command"),
        };

        for id in ["-2abcde", "-abcdef", "-", "-_-_-_", "_-abcd", "-x"] {
            assert_eq!(
                id_of(&["dotvault", "shares", "log", id]),
                (id.to_string(), false),
                "{id}"
            );
            assert_eq!(id_of(&["dotvault", "shares", "revoke", id]).0, id, "{id}");
            // options may come before or after the id
            assert_eq!(
                id_of(&["dotvault", "shares", "log", "--json", id]),
                (id.to_string(), true),
                "{id}"
            );
            assert_eq!(
                id_of(&["dotvault", "shares", "log", id, "--json"]),
                (id.to_string(), true),
                "{id}"
            );
        }
    }

    #[test]
    fn scan_accepts_the_system_flag_with_or_without_folders() {
        use super::{Command, ScanArgs};
        use clap::Parser;
        let scan = |args: &[&str]| match super::Cli::try_parse_from(args).unwrap().command {
            Command::Scan(ScanArgs { paths, system, .. }) => (paths.len(), system),
            _ => panic!("not a scan"),
        };
        assert_eq!(scan(&["dotvault", "scan"]), (0, false));
        assert_eq!(scan(&["dotvault", "scan", "--system"]), (0, true));
        assert_eq!(
            scan(&["dotvault", "scan", "--system", "/srv/app"]),
            (1, true)
        );
        assert_eq!(
            scan(&["dotvault", "scan", "~/code", "--depth", "3"]),
            (1, false)
        );
    }

    fn expanded(args: &[&str]) -> Vec<String> {
        super::expand_shorthands(args.iter().map(std::ffi::OsString::from))
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn ckv_is_shorthand_for_copy_and_kv_and_other_flags_are_left_alone() {
        assert_eq!(
            expanded(&["dotvault", "get", "K", "-ckv"]),
            ["dotvault", "get", "K", "-c", "--kv"]
        );
        for untouched in ["-c", "-ck", "-kc", "--kv", "--copy", "-v", "-cv", "-ckvx"] {
            assert_eq!(
                expanded(&["dotvault", "get", "K", untouched]),
                ["dotvault", "get", "K", untouched]
            );
        }
        // after `--` everything is a plain argument
        assert_eq!(
            expanded(&["dotvault", "get", "--", "-ckv"]),
            ["dotvault", "get", "--", "-ckv"]
        );
    }

    #[test]
    fn get_parses_copy_and_kv_in_every_spelling() {
        use super::{Command, GetArgs};
        use clap::Parser;
        let get = |args: &[&str]| match super::Cli::try_parse_from(expanded(args)).unwrap().command
        {
            Command::Get(GetArgs { copy, kv, keys, .. }) => (keys.len(), copy, kv),
            _ => panic!("not a get"),
        };
        assert_eq!(get(&["dotvault", "get", "K"]), (1, false, false));
        assert_eq!(get(&["dotvault", "get", "K", "-c"]), (1, true, false));
        assert_eq!(get(&["dotvault", "get", "K", "-k"]), (1, false, true));
        assert_eq!(get(&["dotvault", "get", "K", "-ckv"]), (1, true, true));
        assert_eq!(get(&["dotvault", "get", "K", "-ck"]), (1, true, true));
        assert_eq!(
            get(&["dotvault", "get", "K", "--copy", "--kv"]),
            (1, true, true)
        );
        assert_eq!(
            get(&["dotvault", "get", "A", "B", "-ckv", "-f", "x"]),
            (2, true, true)
        );
        // `-v` on its own is still the version flag
        let err = super::Cli::try_parse_from(expanded(&["dotvault", "get", "K", "-v"]))
            .expect_err("-v prints the version");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    /// `-v`, `-V` and `--version` print the version on the root command and on every
    /// subcommand. A custom version flag that is inherited without a propagated version
    /// panics at parse time, so every command in the tree is tried.
    #[test]
    fn version_flags_work_on_every_command() {
        use clap::{error::ErrorKind, CommandFactory, Parser};

        fn collect(cmd: &clap::Command, path: Vec<String>, out: &mut Vec<Vec<String>>) {
            out.push(path.clone());
            for sub in cmd.get_subcommands().filter(|c| c.get_name() != "help") {
                let mut next = path.clone();
                next.push(sub.get_name().to_string());
                collect(sub, next, out);
            }
        }
        let mut paths = Vec::new();
        collect(&super::Cli::command(), Vec::new(), &mut paths);
        assert!(
            paths.len() > 25,
            "expected the whole command tree, got {}",
            paths.len()
        );

        for path in &paths {
            for flag in ["-v", "-V", "--version"] {
                let mut args = vec!["dotvault".to_string()];
                args.extend(path.iter().cloned());
                args.push(flag.to_string());
                let err = super::Cli::try_parse_from(&args).expect_err("version exits early");
                assert_eq!(err.kind(), ErrorKind::DisplayVersion, "{args:?}");
                assert!(err.to_string().starts_with("dotvault"), "{args:?}: {err}");
            }
        }
    }

    /// Accepting hyphens must not turn a real option into the id.
    #[test]
    fn a_missing_id_is_still_an_error() {
        use clap::Parser;
        for args in [
            &["dotvault", "shares", "log"][..],
            &["dotvault", "shares", "log", "--json"][..],
            &["dotvault", "shares", "revoke"][..],
        ] {
            assert!(
                super::Cli::try_parse_from(args).is_err(),
                "{args:?} should be rejected"
            );
        }
    }
}
