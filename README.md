# dotvault

Find, read, edit, store and share your `.env` files from one place.

[![CI](https://img.shields.io/github/actions/workflow/status/YatharthKaushal/dotvault/ci.yml?branch=main&label=CI)](https://github.com/YatharthKaushal/dotvault/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

Every project keeps its own `.env` files in its own folder. Once you have more
than a handful of projects, finding a single value means opening folder after
folder, some files get forgotten along the way, and handing one to a teammate
means pasting it into WhatsApp or Teams.

dotvault is a command-line tool that finds the `.env` files across all of your
project folders and lets you list, search, read and edit them from one place.
Optionally it keeps an end-to-end encrypted copy in the cloud and shares chosen
variables through a link or short code. An account is only needed for the cloud
features.

> [!NOTE]
> Early development. The test suite passes in CI on Linux, macOS and Windows,
> but there is no published release yet and the cryptographic design has not
> been independently reviewed. See [Security](#security).

## Features

- Finds every `.env` and `.env.*` across your project folders, grouped by
  project and environment.
- Filters by project, environment, path or variable. Example files
  (`.env.example` and the like) are hidden unless you ask for them.
- Searches a variable across all files and shows which environments disagree.
- Masks values unless you pass `--reveal`. Prints as a table, dotenv, JSON, shell
  or PowerShell.
- Edits in place with `set`, `unset` and `edit`. Comments and order are kept and
  writes are atomic.
- Diffs two environments.
- Cloud vault with `push` and `pull`. Encryption happens on your machine; the
  server stores only ciphertext.
- Shares chosen variables through a link or short code, with an expiry, an open
  limit, revocation and a log of who opened it. The recipient needs an account.
- One native binary under 5 MB. It starts in about 2 ms and lists hundreds of env
  files in about 11 ms.

## Install

> [!IMPORTANT]
> There is no published release yet, and the repository is private for now. The
> install scripts work for everyone once the repository is public and a release
> exists. Until then, [build from source](#from-source).

**Linux and macOS** (x86_64 and aarch64; Linux needs glibc):

```bash
curl -fsSL https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.sh | sh
```

**Windows** (PowerShell, x64):

```powershell
irm https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.ps1 | iex
```

The scripts download the archive for your platform from GitHub Releases, verify
its SHA-256 checksum and copy the binary into place. They need no `sudo` or
administrator rights. Read them first: [install.sh](install.sh),
[install.ps1](install.ps1). You can also pick an archive yourself from the
[releases page](https://github.com/YatharthKaushal/dotvault/releases), extract it
and put `dotvault` on your `PATH`. The [site/](site/index.html) folder holds a
download page with the same commands and direct links; it is published with
GitHub Pages (`https://yatharthkaushal.github.io/dotvault/`) once the repository is public.

| | Linux and macOS | Windows |
| :--- | :--- | :--- |
| Installs to | `~/.local/bin/dotvault` | `%LOCALAPPDATA%\Programs\dotvault\dotvault.exe`, added to your user `PATH` |
| Options | `DOTVAULT_VERSION`, `DOTVAULT_INSTALL_DIR` (or `--version`, `--dir`) | `DOTVAULT_VERSION`, `DOTVAULT_INSTALL_DIR`, `DOTVAULT_NO_MODIFY_PATH=1` |

Set the variables on the `sh` side of the pipe, because that is the process that
reads them:

```bash
curl -fsSL https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.sh | DOTVAULT_VERSION=v0.1.0 sh
curl -fsSL https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.sh | sh -s -- --dir ~/bin
```

In PowerShell, set `$env:DOTVAULT_VERSION = "v0.1.0"` before running the installer.

If the script says the install folder is not on your `PATH`, add it, for example
`export PATH="$HOME/.local/bin:$PATH"` in your shell profile. On Windows, open a
new terminal after installing. The macOS binaries are not signed; files fetched
by the script are not quarantined, but one downloaded in a browser may be (clear
it with `xattr -d com.apple.quarantine dotvault`). On Alpine and other musl
systems, build from source.

**Update:** run the installer again. **Uninstall:** delete the binary (and, on
Windows, the folder and its `PATH` entry). Your env files, settings and saved
login are not touched; delete the folders listed under
[Configuration](#configuration) to remove those.

### From source

| Platform | Needs |
| :--- | :--- |
| Linux | Rust 1.85+, a C compiler (`build-essential`, `gcc` or `base-devel`) |
| macOS | Rust 1.85+, Xcode Command Line Tools (`xcode-select --install`) |
| Windows | Rust 1.85+, [C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) ("Desktop development with C++") |

Install Rust with [rustup](https://rustup.rs/), then on any platform:

```bash
git clone https://github.com/YatharthKaushal/dotvault.git
cd dotvault
cargo install --path crates/dotvault-cli --locked
dotvault --version
```

This puts the binary in Cargo's bin folder: `~/.cargo/bin/dotvault`, or
`%USERPROFILE%\.cargo\bin\dotvault.exe` on Windows. To update, `git pull` and
rerun the command with `--force`; to uninstall, `cargo uninstall dotvault`.

### Completions

`dotvault completions <bash|zsh|fish|elvish|powershell>` prints a script, for
example `dotvault completions bash > ~/.local/share/bash-completion/completions/dotvault`.

## Usage

A **target** is what a command works on: a path (`./.env`), a project
(`acme-api`) or `project:env` (`acme-api:production`). With no target, `./.env`
is used. The environment is the part of the file name after `.env.`, so
`.env.production` is `production` and a bare `.env` is `default`.

Local, no account:

```bash
dotvault scan ~/code                      # find env files and remember the folder
dotvault ls                               # list them, grouped by project
dotvault ls -p acme -e production         # filter
dotvault show acme-api:production         # values are masked
dotvault show acme-api:production --reveal --keys DATABASE_URL
dotvault find STRIPE_KEY                  # where is it defined, and do the copies agree?
dotvault diff acme-api:staging acme-api:production
dotvault set API_KEY=new -p acme-api -e production
dotvault set SECRET                       # prompts, so it stays out of shell history
dotvault unset OLD_KEY -f ./.env
dotvault edit acme-api:local              # opens $VISUAL / $EDITOR
```

Cloud vault:

```bash
dotvault register --server https://vault.example.com   # shows a recovery key once
dotvault push acme-api:production                      # stored as "acme-api/production"
dotvault vault ls

# on another machine
dotvault login --server https://vault.example.com
dotvault pull acme-api/production --out .env --merge
```

Sharing:

```bash
# you
dotvault share acme-api:production --keys DATABASE_URL,REDIS_URL --expires 1h --max-views 2
# prints a link and a short code (dv1.<id>.<key>)

# the recipient, who needs an account on the same server
dotvault open dv1.AbCd...xyz --write .env

# you, afterwards
dotvault shares ls
dotvault shares log <id>                  # who opened or saved it
dotvault shares revoke <id>
```

### Commands

| Command | Description |
| :--- | :--- |
| `scan [PATHS] [--depth N]` | Search folders for env files and remember them. With no paths, rescans the remembered folders. |
| `roots add\|rm\|ls` | Manage the remembered folders. |
| `ls [FOLDER]` | List env files. Filters: `-p/--project`, `-e/--env`, `--path`, `-k/--has-key`, `--examples`, `--only-examples`. Output: `--paths`, `--json`. `--refresh` rescans first. |
| `show [TARGET]` | Show a file's variables. `--reveal`, `--keys A,B`, `--format table\|dotenv\|json\|shell\|powershell`. |
| `find PATTERN` | Find a variable (name or glob) across all files. Same filters as `ls`, plus `--reveal`, `--json`. Rows with the same variant letter hold the same value. |
| `get KEY` | Print one raw value. Pick the file with `-f FILE` or `-p PROJECT [-e ENV]`. |
| `set KEY=VALUE...` | Set variables, keeping comments and layout. A bare `KEY` prompts for the value. Creates the file if needed. |
| `unset KEY...` | Remove variables. |
| `edit [TARGET]` | Open the file in `$VISUAL` / `$EDITOR`. |
| `diff A B` | Compare two files. `--reveal` shows values. |
| `register`, `login [--no-cache-key]`, `logout`, `whoami` | Account and session. `--server URL` selects the server. `--no-cache-key` keeps the vault key off disk, so vault commands ask for your password. |
| `passwd`, `recover` | Change your password, or reset it with the recovery key. |
| `push [TARGET] [--name N] [--keys A,B]` | Store a file in your vault. The default name is `project/env`. Pushing again adds a version. |
| `pull NAME --out PATH` | Fetch a vault item. Refuses to overwrite unless you add `--merge` or `--force`. `--stdout` prints instead. |
| `vault ls\|show\|rm` | List, show or delete vault items. Names can be abbreviated when unambiguous. |
| `share [TARGET]` | Create a link and code. `--keys A,B` or `-i` choose the variables, `--vault NAME` shares a vault item, `--expires 30m\|24h\|7d` (1 minute to 30 days, default 24 h), `--max-views N`, `--note TEXT`. |
| `open LINK\|CODE` | Open a share. Requires at least one of `--reveal`, `--write PATH` (with `--merge` or `--force`) or `--save [NAME]`, because opening uses up one of the allowed opens. |
| `shares ls\|log\|revoke` | Manage the shares you created. A short unique prefix of the id is enough. |
| `config` | Show where dotvault keeps its files. |
| `completions SHELL` | Print a completion script. |

Filters accept globs (`-p 'acme-*'`). `-p` and `--path` match by substring when
there is no wildcard; `-e` and `-k` match whole names.

Exit code `0` is success and `1` is an error. `diff` also returns `1` when the
files differ, `find` when nothing matches, and `unset` / `show --keys` when a
variable was not set.

To load an environment into your shell:

```bash
eval "$(dotvault show acme-api:local --format shell --reveal)"                           # bash, zsh
dotvault show acme-api:local --format powershell --reveal | Invoke-Expression            # PowerShell
```

## Configuration

Override the locations with `DOTVAULT_CONFIG_DIR` and `DOTVAULT_DATA_DIR`.
`dotvault config` prints the exact paths on your machine.

| | Linux | macOS | Windows |
| :--- | :--- | :--- | :--- |
| `config.toml`, `session.json` | `~/.config/dotvault` | `~/Library/Application Support/dotvault` | `%APPDATA%\dotvault\config` |
| `index.db` | `~/.local/share/dotvault` | `~/Library/Application Support/dotvault` | `%APPDATA%\dotvault\data` |

`index.db` is a cache of file locations and variable names (never values). It
refreshes itself; run `dotvault scan` to pick up new folders and
`dotvault ls --refresh` to pick up new files.

```toml
# config.toml
server = "https://vault.example.com"   # set on first login
roots = ["/home/you/code"]             # managed by `scan` and `roots`
max_depth = 10
auto_scan_secs = 600                   # rescan when the index is older than this
ignore = ["playground"]                # extra folder names to skip
example_suffixes = ["example", "sample", "template", "dist"]
```

`node_modules`, `.git`, `target`, `vendor`, `dist`, `build`, `.venv` and similar
folders are always skipped.

| Environment variable | Used for |
| :--- | :--- |
| `DOTVAULT_SERVER` | Server for `register`, `login` and `recover` (same as `--server`). |
| `DOTVAULT_USERNAME`, `DOTVAULT_PASSWORD` | Non-interactive login or registration. |
| `DOTVAULT_NEW_PASSWORD`, `DOTVAULT_RECOVERY_KEY` | Non-interactive `passwd` and `recover`. |
| `DOTVAULT_TOKEN` | A session token for scripts, with `DOTVAULT_SERVER`. It can share and open but has no vault key. |
| `VISUAL`, `EDITOR` | The editor for `edit`. |
| `NO_COLOR` | Disable colors. |

Environment variables are visible to other processes of the same user, so prefer
the interactive prompts for everyday use.

## Self-hosting

```bash
cargo run --release -p dotvault-server        # listens on 127.0.0.1:8080
dotvault register --server http://localhost:8080
```

For real use you need a Linux machine, a domain name and a TLS proxy (the client
refuses plain `http://` except for localhost), plus backups of the database.
[docs/deploy.md](docs/deploy.md) has the requirements and step-by-step setup with
systemd and Caddy, and Docker and Fly.io alternatives.

## Security

- Cloud data is encrypted on your machine (Argon2id, HKDF, XChaCha20-Poly1305).
  The server never receives your password, your vault key or a share key.
- A share's key lives in the link or code, not on the server.
- Text from shares, servers and files is sanitized before it is printed, shell
  output only includes plain variable names, and written files are safe to
  `source`.
- Your recovery key is the only way back in if you forget your password. If you
  lose both, the cloud data cannot be recovered.
- The session is kept in a `0600` file, not the OS keychain, and the design has
  not been independently reviewed.

Details and known limits are in [docs/security-model.md](docs/security-model.md).
Report vulnerabilities as described in [SECURITY.md](SECURITY.md).

## Documentation

- [ARCHITECTURE.md](ARCHITECTURE.md): components, key hierarchy, data flows, server API.
- [docs/security-model.md](docs/security-model.md): what is protected and what is not.
- [docs/deploy.md](docs/deploy.md): running your own server.
- [CONTRIBUTING.md](CONTRIBUTING.md): building, testing and sending changes.
- [SECURITY.md](SECURITY.md) and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise,
any contribution intentionally submitted for inclusion in this work by you, as
defined in the Apache-2.0 license, is dual licensed as above, without any
additional terms or conditions.
