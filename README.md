# dotvault

Find, read, edit, store and share your `.env` files from one place.

[![CI](https://img.shields.io/github/actions/workflow/status/YatharthKaushal/dotvault/ci.yml?branch=main&label=CI)](https://github.com/YatharthKaushal/dotvault/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

**Contents:** [Features](#features) · [Install](#install)
([Update](#update), [Uninstall](#uninstall), [From source](#from-source),
[Completions](#completions)) · [Usage](#usage)
([Copying to the clipboard](#copying-to-the-clipboard),
[History](#history-of-a-variable), [Secret files](#secret-files),
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
  writes are atomic. Commands that change the same file at the same moment take
  turns, so none of them loses its change.
- Diffs two environments.
- Finds SSH keys, private keys, keystores, service-account and other secret JSON,
  and credential dotfiles, marks them, and stores or shares them like env files.
- Keeps a git-like history of every variable it changes: `log` shows the versions,
  `revert` goes back to one, and values stay masked unless you ask.
- Cloud vault with `push` and `pull`. Encryption happens on your machine; the
  server stores only ciphertext.
- Shares chosen variables through a link or short code, with an expiry, an open
  limit, revocation and a log of who opened it. The recipient needs an account.
- One native binary of about 4 to 5.5 MiB depending on the platform (CI fails the
  build above 6 MiB). On Linux it starts in about 2 ms and lists 600 env files
  across 300 projects in about 11 ms.

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
its SHA-256 checksum, copy the binary into place and put it on your `PATH` if it
is not already there. They need no `sudo` or administrator rights. Read them first: [install.sh](install.sh),
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
| Installs to | `~/.local/bin/dotvault`, added to your `PATH` if it is not on it | `%LOCALAPPDATA%\Programs\dotvault\dotvault.exe`, added to your user `PATH` |
| Options | `DOTVAULT_VERSION`, `DOTVAULT_INSTALL_DIR`, `DOTVAULT_NO_MODIFY_PATH=1` (or `--version`, `--dir`, `--no-modify-path`) | `DOTVAULT_VERSION`, `DOTVAULT_INSTALL_DIR`, `DOTVAULT_NO_MODIFY_PATH=1` |

Set the variables on the `sh` side of the pipe, because that is the process that
reads them:

```bash
curl -fsSL https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.sh | DOTVAULT_VERSION=v0.1.0 sh
curl -fsSL https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.sh | sh -s -- --dir ~/bin
```

In PowerShell, set `$env:DOTVAULT_VERSION = "v0.1.0"` before running the installer.

**Your `PATH`.** If the install folder is not already on your `PATH`, the
installer adds it for you and says what it changed. Nothing is changed when it is
already there.

- **Windows:** the folder is added to your user `PATH` and to the PowerShell
  window you ran the installer in, so `dotvault` works straight away. Every other
  entry is kept exactly as it was, including ones that use variables such as
  `%JAVA_HOME%\bin`. Terminals that were already open need to be reopened.
- **Linux and macOS:** one line is appended to your login shell's startup file and
  the installer names the file: `~/.zshrc` for zsh; `~/.bashrc` and the login file
  bash reads (`~/.bash_profile`, `~/.bash_login` or `~/.profile`, whichever
  exists first) for bash; `~/.profile` for sh, dash and ksh; and a file of its own,
  `~/.config/fish/conf.d/dotvault.fish`, for fish. The line only sources
  `~/.local/share/dotvault/env`, which does the work. Running the installer again
  adds nothing twice. New terminals have `dotvault`; in the current one, run the
  `. "$HOME/.local/share/dotvault/env"` line the installer prints.
- **Opting out:** set `DOTVAULT_NO_MODIFY_PATH=1`, or pass `--no-modify-path` to
  `install.sh`, and add the folder yourself, for example
  `export PATH="$HOME/.local/bin:$PATH"`.
- **Not covered:** shells with their own syntax (csh, tcsh, nushell) are left alone
  and the installer prints what to add. Programs that do not read startup files,
  such as cron jobs, need the folder on their `PATH` themselves.

The macOS binaries are not signed; files fetched by the script are not
quarantined, but one downloaded in a browser may be (clear it with
`xattr -d com.apple.quarantine dotvault`). On Alpine and other musl systems, and on
Linux older than glibc 2.34 (Ubuntu 20.04, Debian 11, RHEL 8), build from source.

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

Delete the binary and the `PATH` setup. Your env files, settings and saved login are
not touched; delete the folders listed under [Configuration](#configuration) to
remove those.

- **Linux and macOS:** run `rm ~/.local/bin/dotvault ~/.local/share/dotvault/env`.
  That already stops the `PATH` line from doing anything, and it is safe to leave in
  place. To remove it too, delete the two lines marked
  `# added by the dotvault installer` from `~/.zshrc`, `~/.bashrc` and `~/.profile`
  (or `~/.bash_profile`), whichever have them. For fish, delete
  `~/.config/fish/conf.d/dotvault.fish`. Do not delete the whole
  `~/.local/share/dotvault` folder: it also holds the index and the history.
- **Windows:** delete the folder `%LOCALAPPDATA%\Programs\dotvault`, then remove its
  entry from your user `PATH`: Start, "Edit environment variables for your
  account", select `Path` under User variables, Edit, and delete the dotvault line.

### From source

| Platform | Needs |
| :--- | :--- |
| Linux | Rust 1.89+, a C compiler (`build-essential`, `gcc` or `base-devel`) |
| macOS | Rust 1.89+, Xcode Command Line Tools (`xcode-select --install`) |
| Windows | Rust 1.89+, [C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) ("Desktop development with C++") |

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

Commands that take a key rather than a target (`get`, `set`, `unset`, `log`,
`revert`, `forget`) pick the file with `-f FILE` (a path) or `-p PROJECT [-e ENV]`
instead. `-f` takes a path and `-p` takes a project name.

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
dotvault log DATABASE_URL -p acme-api -e production   # every version of one variable
dotvault revert DATABASE_URL -p acme-api -e production   # go back to the one before
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

### History of a variable

Every time dotvault changes a variable (`set`, `unset`, `edit`, `pull`), it records
a numbered version, like a commit for one variable. Nothing is recorded until
dotvault first changes a file; looking at files (`ls`, `show`, `get`) records
nothing.

```bash
dotvault log DATABASE_URL -p acme-api -e production
# VERSION  WHEN      ACTION    VALUE
# v3       2d ago    set       ******** 5dffa1
# v2       1mo ago   external  ******** 70e1b5
# v1       1y ago    baseline  ******** c8311b

dotvault log -p acme-api -e production            # every change to the file, newest first
dotvault log DATABASE_URL --reveal ...            # show the real values
dotvault revert DATABASE_URL ...                  # back to the version before (--to 2 for a chosen one)
dotvault forget DATABASE_URL ...                  # delete its recorded history (or --all)
```

- Values are masked, with a short tag so you can tell equal values apart. Add
  `--reveal` to see them. `--json` leaves values out unless you add `--reveal`.
- `baseline` is what a variable held when dotvault first changed its file.
  `external` means the file differed from the last record, so it was edited
  outside dotvault; those changes are noticed the next time dotvault changes the
  file, and stamped with the file's modified time.
- `revert` never rewrites history: going back is a new version, so you can go
  forward again. A variable removed in the version you pick is removed again. History
  stores values, not layout: a variable that `revert` brings back is added at the
  end of the file, and a comment that sat on its old line is not restored.
  After an edit made outside dotvault, plain `revert` goes back to the last
  recorded value.
- One `set A=1 B=2` or one `edit` is one change, shown together in `log`.
- The history lives in `history.db` in the data folder (see `dotvault config`),
  readable only by you. **It keeps old and current values in plain text**, the one
  place besides your env files where dotvault stores them, so a backup or sync of
  that folder copies them. It keeps the newest 50 versions of each variable
  (`history_keep`). Turn it off with `history = false` in `config.toml`. `forget`
  overwrites what it deletes, but copies already made by a backup or by the disk
  itself are out of its reach.
- A file is known by its path. If you move it, its old history stays under the
  old path (`dotvault log -f OLD`), and the new location starts fresh.

### Secret files

Env files are not the only secrets lying around. dotvault also recognises these
by their name **and** a quick look inside, and lists them on request:

```bash
dotvault ls --secrets                      # every secret file it found
dotvault ls --kind ssh-key --kind keystore # only some kinds
dotvault ls --secrets -k private_key       # JSON secrets that have a key of that name
dotvault show ~/.ssh/id_ed25519            # kind, size, fingerprint (never the key)
dotvault mark ./notes/passwords.txt        # treat a file as a secret by hand
dotvault unmark ./fixtures/fake-key.json   # stop listing a file (or a wrong guess)
dotvault push ~/.ssh/id_ed25519            # store it, encrypted, in your vault
dotvault pull NAME --out ./id_ed25519      # get the exact bytes back, readable only by you
dotvault share ~/.ssh/deploy_key           # asks first; --yes in scripts
```

Plain `dotvault ls` stays env files only and ends with a hint such as
`24 secret files (--secrets to list)`.

| Kind | What it is |
| :--- | :--- |
| `ssh-key` | An SSH private key (OpenSSH, PEM or PuTTY), with its algorithm and whether a passphrase protects it. Files with no extension in `.ssh`, `id_*` and `*_ed25519`-style names. |
| `private-key` | Any other private key in a `.pem`, `.key` or `.p8` file. A certificate alone is not a secret and is left out. |
| `keystore` | Java and Android keystores (`.jks`, `.keystore`, `.p12`, `.pfx`, `.bks`), checked by their file header, and `key.properties` files that hold a signing password. |
| `service-account` | A Google service-account JSON, whatever the file is called, and `google-services.json`. |
| `auth-json` | OAuth client or token files, `gcloud` logins, Docker `config.json` with logins, Composer `auth.json`. |
| `secret-json` | Other JSON with a real-looking secret in it, such as `private_key`, `client_secret` or `refresh_token`. |
| `aws-credentials`, `npm-token`, `netrc`, `pypi-token`, `git-credentials`, `kubeconfig` | Credential dotfiles, listed only when they hold an actual credential (`${NPM_TOKEN}` placeholders and empty values do not count). |
| `marked` | A file you marked yourself. |

- **What is read.** To tell a key from a certificate, or a service account from
  ordinary JSON, dotvault reads the start of files with plausible names (a JSON
  file in full, up to 64 KiB), on your machine, during a scan. It keeps only the
  kind, a short note such as `ed25519, passphrase` and, for JSON, the top-level
  key names. Never the contents. A scan of the whole computer takes a little longer
  for it (tens of milliseconds for several thousand JSON files).
- **Where it looks.** Everywhere a scan goes, plus `~/.ssh`, `~/.aws`, `~/.docker`,
  `~/.kube` and `~/.config/gcloud` (on Windows `%APPDATA%\gcloud`) when they exist,
  which a scan would otherwise skip as hidden folders. Dependency and fixture
  folders (`node_modules`, `vendor`, `testdata`, `site-packages`) are skipped.
- **Guessing wrong.** `dotvault unmark FILE` silences a file for good;
  `dotvault mark FILE` adds one dotvault would not notice. Files in a `.ssh` folder
  with an extension other than `.pem`, `.key` or `.ppk` are not guessed at; mark them.
- **Cloud.** `push` stores the whole file, binary included, encrypted on your
  machine like any vault item, as `project/file name`. `pull NAME --out PATH` writes
  exactly those bytes with owner-only permissions (Unix; on Windows the file keeps
  your profile folder's access rules) and will not replace a file without `--force`.
  `vault show NAME` describes an item and never prints it. `--as-file` stores or
  shares any small file as it is, even an env file. The limit is about 180 KiB for
  the vault and about 47 KiB for a share.
- **Sharing.** `share` and `open` work as for variables, with `--write PATH` on
  the receiving side. Because a private key, a keystore or a service-account file
  lets the holder act as you, `share` asks first in a terminal, and a script has to
  pass `--yes`. The sender's file name is shown but never used as a path, and
  `open --reveal` prints text files only.
- **Not covered.** Secret files have no version history (history is per env
  variable), and an older dotvault that finds a file in your vault says it cannot
  read the item. A server upgrade is not needed.

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
- While dotvault chose the list, it also searches the credential folders that exist
  (`~/.ssh`, `~/.aws`, `~/.docker`, `~/.kube`, `~/.config/gcloud`), which are hidden
  folders a scan skips. They are for [secret files](#secret-files).
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
- Only file names, variable names and the key names of JSON secrets are indexed,
  never values or contents.
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
| `ls [FOLDER]` | List env files. Filters: `-p/--project`, `-e/--env`, `--path`, `-k/--has-key`, `--examples`, `--only-examples`. Output: `--paths`, `--json`. `--refresh` rescans first. `--secrets` or `--kind KIND` list secret files instead. |
| `mark FILE...` / `unmark FILE...` | Treat files as secrets by hand, or stop listing them (also silences a wrong guess). |
| `show [TARGET]` | Show a file's variables. `--reveal`, `--keys A,B` (names or globs like `'STRIPE_*'`), `--format table\|dotenv\|json\|shell\|powershell`. A secret file (SSH key, keystore, secret JSON) is described instead, never printed; a JSON one shows its fields masked. |
| `find PATTERN` | Find a variable (name or glob) across all files. Same filters as `ls`, plus `--reveal`, `--json`. Rows with the same variant letter hold the same value. |
| `get KEY...` | Print raw values: one name prints the bare value, several names or a glob like `'STRIPE_*'` print `KEY=value` lines. Pick the file with `-f FILE` or `-p PROJECT [-e ENV]`. `-k`/`--kv` gives `KEY=value` even for one name. `-c`/`--copy` copies to the clipboard instead, and `-ckv` copies the whole pair. |
| `set KEY=VALUE...` | Set variables, keeping comments and layout. A bare `KEY` prompts for the value. Creates the file if needed. |
| `unset KEY...` | Remove variables. |
| `edit [TARGET]` | Open the file in `$VISUAL` / `$EDITOR`. |
| `log [KEY]` (alias `history`) | History of one variable, or of every change to the file. `-n N`/`--limit N`, `--reveal`, `--json`. Pick the file with `-f` or `-p`/`-e`. |
| `revert KEY [--to N]` | Put a variable back to an earlier version (default: the one before). Recorded as a new version. |
| `forget KEY` / `forget --all` | Delete the recorded history of a variable, or of the whole file. |
| `diff A B` | Compare two files. `--reveal` shows values. |
| `register`, `login [--no-cache-key]`, `logout`, `whoami` | Account and session. `--server URL` selects the server (default: the hosted one). `--no-cache-key` keeps the vault key off disk, so vault commands ask for your password. |
| `passwd`, `recover` | Change your password, or reset it with the recovery key. |
| `push [TARGET] [--name N] [--keys A,B] [--as-file]` | Store a file in your vault. The default name is `project/env`. Pushing again adds a version. SSH keys, keystores and secret JSON are stored whole, as files; `--as-file` does that for any small file. |
| `pull NAME --out PATH` | Fetch a vault item. Refuses to overwrite unless you add `--merge` or `--force`. `--stdout` prints variables instead. A file item (an SSH key, a keystore) is written exactly, readable only by you; `--merge`, `--keys` and `--stdout` do not apply to it and the folder must exist. |
| `vault ls\|show\|rm` | List, show or delete vault items. Names can be abbreviated when unambiguous. `show` describes a file item and never prints it. |
| `share [TARGET]` | Create a link and code. `--keys A,B` or `-i` choose the variables, `--vault NAME` shares a vault item, `--expires 30m\|24h\|7d` (1 minute to 30 days, default 24 h), `--max-views N`, `--note TEXT`, `--as-file`, `--yes` (skip the question asked before sharing a private key, keystore or service-account file). |
| `open LINK\|CODE` | Open a share. Requires at least one of `--reveal`, `--write PATH` (with `--merge` or `--force`) or `--save [NAME]`, because opening uses up one of the allowed opens. A shared file is written exactly, readable only by you. |
| `shares ls\|log\|revoke` | Manage the shares you created. A short unique prefix of the id is enough. |
| `config` | Show where dotvault keeps its files. |
| `completions SHELL` | Print a completion script. |

Filters accept globs (`-p 'acme-*'`). `-p` and `--path` match by substring when
there is no wildcard; `-e` and `-k` match whole names.

Exit code `0` is success and `1` is an error. `diff` also returns `1` when the
files differ, `find` when nothing matches, and `unset` / `show --keys` when a
variable was not set. `get` fails, and prints nothing, if any name or glob it was
given is not set.

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

The `locks` folder next to it holds a few small empty files. Commands that change
an env file hold one while they work, so two of them never overwrite each other.
Nothing is added to your project folders.

```toml
# config.toml
server = "https://dotvault.synquic.tech"   # saved on your first register or login
roots = ["/"]                          # the whole computer at first; managed by `scan` and `roots`
roots_auto = true                      # dotvault chose the roots; false once you name one
max_depth = 10
auto_scan_secs = 600                   # rescan when the index is older than this
ignore = ["playground"]                # extra folder names to skip
example_suffixes = ["example", "sample", "template", "dist"]
marked = ["/srv/app/passwords.txt"]   # files you marked as secrets (`dotvault mark`)
unmarked = ["/srv/app/fake-key.json"]  # files you said are not secrets (`dotvault unmark`)
history = true                         # keep a history of variables dotvault changes (plain text, owner-only)
history_keep = 50                      # versions kept per variable
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
