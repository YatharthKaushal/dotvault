# dotvault

Find, read, edit, store and share your `.env` files from one place.

[![CI](https://img.shields.io/github/actions/workflow/status/YatharthKaushal/dotvault/ci.yml?branch=main&label=CI)](https://github.com/YatharthKaushal/dotvault/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

**Contents:** [Features](#features) · [Install](#install)
([Update](#update), [Uninstall](#uninstall), [From source](#from-source),
[Completions](#completions)) · [Usage](#usage)
([Copying to the clipboard](#copying-to-the-clipboard),
[Where dotvault looks](#where-dotvault-looks), [Commands](#commands)) ·
[Configuration](#configuration) · [Self-hosting](#self-hosting) ·
[Security](#security) · [Documentation](#documentation) · [License](#license)

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
> Early development. The test suite passes in CI on Linux, macOS and
> Windows, but the cryptographic design has not been independently reviewed. See
> [Security](#security).

## Features

- Finds every `.env` and `.env.*` on your whole computer, on every drive, with no
  setup and no flag, grouped by project and environment.
- Filters by project, environment, path or variable. Example files
  (`.env.example` and the like) are hidden unless you ask for them.
- Searches a variable across all files and shows which environments disagree.
- Masks values unless you pass `--reveal`. Prints as a table, dotenv, JSON, shell
  or PowerShell.
- Copies a value, or several, to the clipboard with `get -c` and clears it again
  after 30 seconds.
- Edits in place with `set`, `unset` and `edit`. Comments and order are kept and
  writes are atomic.
- Diffs two environments.
- Cloud vault with `push` and `pull`. Encryption happens on your machine; the
  server stores only ciphertext.
- Shares chosen variables through a link or short code, with an expiry, an open
  limit, revocation and a log of who opened it. The recipient needs an account.
- One native binary of about 4 to 5 MiB depending on the platform. On Linux it starts in
  under 2 ms and lists 600 env files across 300 projects in about 11 ms.

## Install

See the [latest release](https://github.com/YatharthKaushal/dotvault/releases/latest) for the current version.

**Linux and macOS** (x86_64 and aarch64; Linux needs glibc 2.34 or newer):

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
and put `dotvault` on your `PATH`. The same commands and direct download links are
on the [download page](https://yatharthkaushal.github.io/dotvault/) (source in
[site/](site/index.html)).

Each release also carries `dotvault-installer.sh` and `dotvault-installer.ps1`
from [cargo-dist](https://github.com/axodotdev/cargo-dist), which the release
notes show. They install into `~/.cargo/bin` (`%USERPROFILE%\.cargo\bin` on
Windows) and add that folder to your `PATH`. Use either installer, not both, or
you will have two copies in different folders.

To check a download yourself, every archive has a `.sha256` file beside it:
`sha256sum -c dotvault-x86_64-unknown-linux-gnu.tar.xz.sha256` on Linux,
`shasum -a 256 -c` on macOS, or compare it with `Get-FileHash` in PowerShell.

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
systems, and on Linux older than glibc 2.34 (Ubuntu 20.04, Debian 11, RHEL 8),
build from source.

### Update

dotvault does not update itself or tell you when a new version exists. To update,
run the same installer you used originally. It fetches the latest release and
replaces the binary in place. Your env files, settings, index and saved login are
kept.

| You installed with | Update with |
| :--- | :--- |
| `install.sh` (Linux, macOS) | `curl -fsSL https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.sh \| sh` |
| `install.ps1` (Windows) | `irm https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.ps1 \| iex` |
| cargo-dist's `dotvault-installer.sh` | `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/YatharthKaushal/dotvault/releases/latest/download/dotvault-installer.sh \| sh` |
| cargo-dist's `dotvault-installer.ps1` | `powershell -ExecutionPolicy Bypass -c "irm https://github.com/YatharthKaushal/dotvault/releases/latest/download/dotvault-installer.ps1 \| iex"` |
| a downloaded archive | download the new archive and replace the `dotvault` binary with the one inside |
| from source | `git pull`, then `cargo install --path crates/dotvault-cli --locked --force` |

Check the result with `dotvault -v`, and compare it with the version on the
[releases page](https://github.com/YatharthKaushal/dotvault/releases). On Windows,
close any running `dotvault` first, because Windows will not replace a program that
is in use. Update with the method you installed with: mixing methods leaves two
copies in different folders, and the one earlier on your `PATH` wins (`which dotvault`
on Linux and macOS, `Get-Command dotvault` in PowerShell, shows which one runs).

To pin or roll back to a specific version, set `DOTVAULT_VERSION` as shown above.

### Uninstall

Delete the binary (and, on Windows, the folder and its `PATH` entry). Your env files,
settings and saved login are not touched; delete the folders listed under
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
dotvault ls                               # every env file on this computer, all drives, grouped by project
dotvault scan ~/code                      # look only in a folder you name
dotvault scan --system                    # add the whole computer to folders you named
dotvault ls -p acme -e production         # filter
dotvault show acme-api:production         # values are masked
dotvault show acme-api:production --reveal --keys DATABASE_URL,'STRIPE_*'
dotvault get DATABASE_URL -p acme-api -e production        # prints just the value
dotvault get DATABASE_URL STRIPE_KEY -f ./.env             # several: KEY=value lines
dotvault get DATABASE_URL -p acme-api -e production -c     # copies it, clears in 30 s
dotvault get DATABASE_URL -p acme-api -e production -ckv   # copies DATABASE_URL=value
dotvault find STRIPE_KEY                  # where is it defined, and do the copies agree?
dotvault diff acme-api:staging acme-api:production
dotvault set API_KEY=new -p acme-api -e production
dotvault set SECRET                       # prompts, so it stays out of shell history
dotvault unset OLD_KEY -f ./.env
dotvault edit acme-api:local              # opens $VISUAL / $EDITOR
```

### Copying to the clipboard

`dotvault get KEY -c` puts the value on the clipboard and prints nothing, so it
stays out of your terminal and scrollback. `-ckv` (or `-ck`) copies `KEY=value`
instead of just the value, and with several names or a glob it always copies
`KEY=value` lines. What is copied is exact, even if it holds control characters
that printing would make visible. The clipboard is cleared after 30 seconds, but only if it
still holds that value, so anything you copied in the meantime is left alone. The
value goes to a short-lived helper process through a pipe, never on a command
line. Where the platform has a way to ask, clipboard history is told to skip it
(on Windows also cloud sync).

| Where | Copying |
| :--- | :--- |
| Windows, macOS | Uses the system clipboard. So far only the Linux X11 path has been tried against a real clipboard, so please report problems. |
| Linux with X11 | Works. The helper stays alive to answer paste requests, then clears the value. |
| Linux with Wayland | Goes through XWayland. Not tested here; if it fails you get an error and `get` without `-c` still works. |
| SSH, containers, servers | No clipboard exists. You get an error, and the value is never printed in its place. |

Any program you run can read the clipboard while the value is on it, and a
clipboard manager that ignores the history hint may keep a copy.

### Where dotvault looks

The first command that needs the list of env files searches the whole computer
and remembers that, so there is nothing to set up and no flag to pass. The first
run can take a minute on a large disk. `dotvault roots` shows and changes the
remembered folders.

- **Windows:** every fixed and removable drive (`C:\`, `D:\`, a USB stick).
  Network drives are not searched automatically, because a dead share can hang
  the command; name one yourself with `dotvault scan Z:\`.
- **Linux:** `/`, plus local disks mounted under `/mnt`, `/media` and
  `/run/media`. **macOS:** `/`, plus disks in `/Volumes`.
- While dotvault chose the list, it follows your disks: a drive you plug in is
  searched from the next command on, and one you unplug is dropped.
- `dotvault scan ~/code` adds a folder, for something dotvault does not look at
  on its own, such as a network drive. As your very first command it keeps the
  search to just that folder. Once you name a folder (or use `roots add` or
  `roots rm`) the list is yours and dotvault leaves it alone; removing the last
  folder starts over with the whole computer.
- `dotvault scan --system` adds the whole computer to a list you named.
- Upgrading from 0.3 or earlier: if your list is just your home folder, which is
  what those versions saved on their own, the next command widens it once and
  says so. Anything else you set up is kept.
- Skipped: hidden folders directly inside any home folder (`~/.config`,
  `~/.cache`), `Library`, `AppData` and media folders, `node_modules`, `.git`,
  `target` and similar, operating system folders (`/proc`, `/usr`, `/etc`,
  `C:\Windows`, `Program Files`, the recycle bin on every drive), backup
  snapshots, and network or FUSE mounts. Name a folder yourself to search inside
  it anyway: `dotvault scan ~/.work`.
- Your home folder is never treated as a project, even if it holds a `.git` for
  dotfiles, so its files are named after their own folder.
- A command given a path (`show ./.env`) never starts a search.
- macOS may ask your terminal for permission to read Documents, Desktop,
  Downloads and removable volumes. Under WSL, Windows drives are under `/mnt/c`,
  which is not searched automatically, so name it: `dotvault scan /mnt/c/Users/you`.
- Only file names and variable names are indexed, never values.
- Finding the drives is new on Windows and macOS and has only been run through the
  automated tests so far. If a drive is missed, `dotvault scan <drive>` adds it,
  and a report helps.
- To search something else instead, for example in a container, set
  `DOTVAULT_SCAN_ROOTS` to a list of folders (separated like `PATH`).

Cloud vault:

```bash
dotvault register                         # shows a recovery key once
dotvault push acme-api:production         # stored as "acme-api/production"
dotvault vault ls

# on another machine
dotvault login
dotvault pull acme-api/production --out .env --merge
```

Unless you say otherwise, the CLI uses the hosted server at https://dotvault.synquic.tech. To use another
server, pass `--server URL` or set `DOTVAULT_SERVER`; your config file and your last
session count too. The first successful `register` or `login` saves the server in your
config, so a later release can never move you to a different one. The hosted server
stores only ciphertext and account metadata (usernames, timing, sizes). It is run on a
best-effort basis with no uptime or durability guarantee, so keep your env files and
don't treat the cloud copy as the only copy. To run your own server, see
[Self-hosting](#self-hosting).

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
| `scan [PATHS] [--system] [--depth N]` | Search folders for env files and remember them. With no paths, rescans the remembered folders (the whole computer, every drive, on first use). `--system` adds the whole computer to folders you named. |
| `roots add\|rm\|ls` | Manage the remembered folders. |
| `ls [FOLDER]` | List env files. Filters: `-p/--project`, `-e/--env`, `--path`, `-k/--has-key`, `--examples`, `--only-examples`. Output: `--paths`, `--json`. `--refresh` rescans first. |
| `show [TARGET]` | Show a file's variables. `--reveal`, `--keys A,B`, `--format table\|dotenv\|json\|shell\|powershell`. |
| `find PATTERN` | Find a variable (name or glob) across all files. Same filters as `ls`, plus `--reveal`, `--json`. Rows with the same variant letter hold the same value. |
| `get KEY...` | Print raw values: one name prints the bare value, several names or a glob like `'STRIPE_*'` print `KEY=value` lines. Pick the file with `-f FILE` or `-p PROJECT [-e ENV]`. `-k`/`--kv` gives `KEY=value` even for one name. `-c` copies to the clipboard instead, and `-ckv` copies the whole pair. |
| `set KEY=VALUE...` | Set variables, keeping comments and layout. A bare `KEY` prompts for the value. Creates the file if needed. |
| `unset KEY...` | Remove variables. |
| `edit [TARGET]` | Open the file in `$VISUAL` / `$EDITOR`. |
| `diff A B` | Compare two files. `--reveal` shows values. |
| `register`, `login [--no-cache-key]`, `logout`, `whoami` | Account and session. `--server URL` selects the server (default: the hosted one). `--no-cache-key` keeps the vault key off disk, so vault commands ask for your password. |
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
server = "https://dotvault.synquic.tech"   # saved on your first register or login
roots = ["/"]                          # the whole computer at first; managed by `scan` and `roots`
roots_auto = true                      # dotvault chose the roots; false once you name one
max_depth = 10
auto_scan_secs = 600                   # rescan when the index is older than this
ignore = ["playground"]                # extra folder names to skip
example_suffixes = ["example", "sample", "template", "dist"]
```

`node_modules`, `.git`, `target`, `vendor`, `dist`, `build`, `.venv` and similar
folders are always skipped.

| Environment variable | Used for |
| :--- | :--- |
| `DOTVAULT_SERVER` | Server for `register`, `login` and `recover` (same as `--server`). Default: https://dotvault.synquic.tech. |
| `DOTVAULT_USERNAME`, `DOTVAULT_PASSWORD` | Non-interactive login or registration. |
| `DOTVAULT_NEW_PASSWORD`, `DOTVAULT_RECOVERY_KEY` | Non-interactive `passwd` and `recover`. |
| `DOTVAULT_TOKEN` | A session token for scripts, with `DOTVAULT_SERVER`. It can share and open but has no vault key. |
| `DOTVAULT_HOME` | Home folder, used to shorten paths to `~` and to skip hidden folders inside it, for containers and tests. |
| `DOTVAULT_SCAN_ROOTS` | Folders to search when none are saved yet, instead of the whole computer (a `PATH`-style list). |
| `VISUAL`, `EDITOR` | The editor for `edit`. |
| `NO_COLOR` | Disable colors. |

Environment variables are visible to other processes of the same user, so prefer
the interactive prompts for everyday use.

## Self-hosting

```bash
cargo run --release -p dotvault-server        # listens on 127.0.0.1:8080
dotvault register --server http://localhost:8080
```

Pass `--server` on the first `register` or `login`: without it the CLI would use the
hosted server. After the first success it remembers the address.

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
- The CLI's default server is run by the maintainer. It cannot read your data, but
  it sees account metadata (usernames, timing, sizes). Use `--server` with your own
  server to avoid that.

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
