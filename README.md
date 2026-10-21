<div align="center">

<pre>
    ┌───────────────────────────────────────────────────────────────────────────┐
    │  ●  ●  ●                                                   dotvault  ─ □ ×
    ├───────────────────────────────────────────────────────────────────────────┤
    │                                                                           │
    │   ██████╗  ██████╗ ████████╗██╗   ██╗ █████╗ ██╗   ██╗██╗     ████████╗   │
    │   ██╔══██╗██╔═══██╗╚══██╔══╝██║   ██║██╔══██╗██║   ██║██║     ╚══██╔══╝   │
    │   ██║  ██║██║   ██║   ██║   ██║   ██║███████║██║   ██║██║        ██║      │
    │   ██║  ██║██║   ██║   ██║   ╚██╗ ██╔╝██╔══██║██║   ██║██║        ██║      │
    │   ██████╔╝╚██████╔╝   ██║    ╚████╔╝ ██║  ██║╚██████╔╝███████╗   ██║      │
    │   ╚═════╝  ╚═════╝    ╚═╝     ╚═══╝  ╚═╝  ╚═╝ ╚═════╝ ╚══════╝   ╚═╝      │
    │                                                                           │
    │   $ every .env, in one vault_                                             │
    │                                                                           │
    └───────────────────────────────────────────────────────────────────────────┘
</pre>

### dotvault

**One place for all your `.env` files**<br/>
find them, edit them, keep an encrypted copy, and share only what you choose.

<br/>

[![CI](https://img.shields.io/github/actions/workflow/status/YatharthKaushal/dotvault/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/YatharthKaushal/dotvault/actions/workflows/ci.yml)
[![Status](https://img.shields.io/badge/status-early%20development-orange?style=flat-square)](https://github.com/YatharthKaushal/dotvault)
[![Rust](https://img.shields.io/badge/Rust-000000?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![SQLite](https://img.shields.io/badge/SQLite-003B57?style=flat-square&logo=sqlite&logoColor=white)](https://www.sqlite.org/)
[![Encryption](https://img.shields.io/badge/encryption-end--to--end-2ea44f?style=flat-square)](docs/security-model.md)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square)](#license)

[Overview](#overview) ·
[Features](#features) ·
[Install](#install) ·
[Usage](#usage) ·
[Self-hosting](#self-hosting) ·
[Security](#security-at-a-glance) ·
[Tech Stack](#tech-stack) ·
[Structure](#project-structure) ·
[Docs](#documentation) ·
[Contributing](#contributing) ·
[License](#license)

</div>

---

## Overview

If you work on more than a couple of projects, your secrets end up scattered
across dozens of `.env`, `.env.local` and `.env.production` files. Finding one
means hopping between folders, some get lost, and handing one to a teammate
means pasting it into WhatsApp or Teams.

**dotvault is a fast command-line env manager — think a keyring that
understands `.env` files.** One tool to list, search, read and edit every env
file on your machine, keep an end-to-end encrypted copy in the cloud, and share
a few chosen variables through a link or short code.

An account is only needed for the cloud features. Everything in the first group
works offline with no sign-up.

> [!NOTE]
> **Current status: early development.** It works end to end on Linux and is
> covered by an automated test suite, but macOS and Windows builds are untested,
> there are no published releases yet, and the cryptographic design has not had
> an independent review. See [Security](#security-at-a-glance) before relying on
> it for anything important.

---

## Features

<table>
<tr>
<td valign="top" width="33%">

**Find & manage** *(no account)*

- Finds every `.env` and `.env.*` across your projects
- Groups by project and environment
- Filter by project, env, path or variable
- Example files stay hidden until asked for
- Values masked unless `--reveal`
- Search a variable across all files
- See which environments disagree
- Edit in place — comments and order survive
- Diff two environments
- Output as table, dotenv, JSON, shell or PowerShell

</td>
<td valign="top" width="33%">

**Cloud vault** *(end-to-end encrypted)*

- Username + password, only when you need the cloud
- Encrypted on your machine before upload
- Server stores ciphertext in SQLite
- `push` / `pull` with versions
- Merge a pulled file into an existing one
- Recovery key if you forget your password
- Optionally keep the vault key off disk

</td>
<td valign="top" width="33%">

**Sharing**

- A link or short code for **only** the variables you pick
- Expiry (up to 30 days) and an open limit
- Revoke at any time
- Receiver needs an account
- See who opened or saved it
- Open straight into a file, your vault or the terminal
- The server can't read a share
- Content from others is sanitized before it reaches your terminal, shell or files

</td>
</tr>
</table>

**Built to be small and fast:** a single native binary under 5 MB that starts in
about 2 ms, lists hundreds of env files from its index in about 11 ms, and stays
under 10 MB of memory.

**Platforms:** Linux *(tested)* · macOS · Windows

---

## Install

> [!IMPORTANT]
> There is no published release yet, and the repository is currently private, so
> the prebuilt download links below start working once a release is published.
> **Until then, build from source** — it is one command on every platform.

| Platform | Prebuilt *(after the first release)* | To build from source you need | Binary ends up in |
| :--- | :--- | :--- | :--- |
| **Linux** — x86_64, aarch64 *(glibc)* | Shell installer, or `.tar.xz` | Rust 1.85+ and a C compiler | `~/.cargo/bin/dotvault` |
| **macOS** — Apple Silicon, Intel | Shell installer, or `.tar.xz` | Rust 1.85+ and Xcode Command Line Tools | `~/.cargo/bin/dotvault` |
| **Windows** — x86_64 | PowerShell installer, or `.zip` | Rust 1.85+ and MSVC C++ Build Tools | `%USERPROFILE%\.cargo\bin\dotvault.exe` |

<details open>
<summary><b>Linux</b></summary>

<br/>

**From source**

```bash
# 1. A C compiler (the bundled SQLite and TLS code are built from source)
sudo apt install build-essential          # Debian / Ubuntu
sudo dnf install gcc                      # Fedora
sudo pacman -S base-devel                 # Arch

# 2. Rust 1.85 or newer
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 3. dotvault
git clone https://github.com/YatharthKaushal/dotvault.git
cd dotvault
cargo install --path crates/dotvault-cli --locked
```

**Prebuilt** *(once released)*

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/YatharthKaushal/dotvault/releases/latest/download/dotvault-installer.sh | sh
```

Or download `dotvault-x86_64-unknown-linux-gnu.tar.xz` (or
`dotvault-aarch64-unknown-linux-gnu.tar.xz` for ARM) from the
[releases page](https://github.com/YatharthKaushal/dotvault/releases), extract it
with `tar -xf`, and move the `dotvault` binary to a folder on your `PATH`, for
example `~/.local/bin`.

**Where it goes** — the binary: `~/.cargo/bin/dotvault`. Its settings:
`~/.config/dotvault`; its index: `~/.local/share/dotvault`.

> [!NOTE]
> The prebuilt Linux binaries link against glibc. On Alpine or another
> musl-based distribution, build from source.

</details>

<details open>
<summary><b>macOS</b></summary>

<br/>

**From source**

```bash
# 1. Command Line Tools (provides the C compiler)
xcode-select --install

# 2. Rust 1.85 or newer
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 3. dotvault
git clone https://github.com/YatharthKaushal/dotvault.git
cd dotvault
cargo install --path crates/dotvault-cli --locked
```

**Prebuilt** *(once released)*

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/YatharthKaushal/dotvault/releases/latest/download/dotvault-installer.sh | sh
```

Or download `dotvault-aarch64-apple-darwin.tar.xz` (Apple Silicon) or
`dotvault-x86_64-apple-darwin.tar.xz` (Intel) from the
[releases page](https://github.com/YatharthKaushal/dotvault/releases), extract it,
and move `dotvault` to a folder on your `PATH`, for example `/usr/local/bin`.

**Where it goes** — the binary: `~/.cargo/bin/dotvault`. Its settings and index:
`~/Library/Application Support/dotvault`.

> [!NOTE]
> The binaries are not code-signed or notarized. If macOS refuses to run one you
> downloaded in a browser, clear the quarantine flag with
> `xattr -d com.apple.quarantine dotvault`. Files fetched by the shell installer
> or `curl` are not quarantined.

</details>

<details open>
<summary><b>Windows</b></summary>

<br/>

**From source** *(PowerShell)*

```powershell
# 1. Build tools: install "Build Tools for Visual Studio" and tick
#    "Desktop development with C++"
#    https://visualstudio.microsoft.com/visual-cpp-build-tools/

# 2. Rust 1.85 or newer: download and run rustup-init.exe from https://rustup.rs

# 3. dotvault
git clone https://github.com/YatharthKaushal/dotvault.git
cd dotvault
cargo install --path crates\dotvault-cli --locked
```

**Prebuilt** *(once released)*

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/YatharthKaushal/dotvault/releases/latest/download/dotvault-installer.ps1 | iex"
```

Or download `dotvault-x86_64-pc-windows-msvc.zip` from the
[releases page](https://github.com/YatharthKaushal/dotvault/releases), extract it,
and put `dotvault.exe` in a folder that is on your `PATH`.

**Where it goes** — the binary: `%USERPROFILE%\.cargo\bin\dotvault.exe`. Its
settings: `%APPDATA%\dotvault\config`; its index: `%APPDATA%\dotvault\data`.

> [!NOTE]
> The binary is not code-signed, so Windows SmartScreen may warn the first time
> you run a downloaded copy. On Windows Subsystem for Linux, follow the Linux
> instructions inside WSL.

</details>

### After installing

```bash
dotvault --version        # confirms it runs
dotvault config           # shows where it keeps its files on this machine
```

If your shell says `command not found`, add Cargo's bin folder to your `PATH`:

```bash
export PATH="$HOME/.cargo/bin:$PATH"      # add this line to ~/.bashrc, ~/.zshrc, …
```

On Windows, `rustup` already adds `%USERPROFILE%\.cargo\bin` to `PATH`; open a
new terminal after installing.

<details>
<summary><b>Choose where the installer puts it</b></summary>

<br/>

The shell and PowerShell installers install into Cargo's bin folder
(`$CARGO_HOME/bin`, by default `~/.cargo/bin`) and add it to your `PATH` by
editing your shell profile. Two environment variables change that:

| Variable | Effect |
| :--- | :--- |
| `DOTVAULT_INSTALL_DIR` | Install into this folder instead. |
| `DOTVAULT_NO_MODIFY_PATH=1` | Don't edit your shell profile or `PATH`. |

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/YatharthKaushal/dotvault/releases/latest/download/dotvault-installer.sh \
  | DOTVAULT_INSTALL_DIR="$HOME/.local/bin" DOTVAULT_NO_MODIFY_PATH=1 sh
```

Set the variables on the `sh` side of the pipe — that is the process that reads
them. In PowerShell, set them with `$env:DOTVAULT_INSTALL_DIR = "…"` before
running the installer.

</details>

<details>
<summary><b>Update and uninstall</b></summary>

<br/>

| | Built from source | Installed with the installer / archive |
| :--- | :--- | :--- |
| **Update** | `git pull`, then `cargo install --path crates/dotvault-cli --locked --force` | Run the installer again, or replace the binary with the new archive's. |
| **Uninstall** | `cargo uninstall dotvault` | Delete the `dotvault` binary (and `dotvault-receipt.json` next to your config, if the installer left one). |

Uninstalling does not touch your env files, your settings, the index or your
saved login. To remove those too, delete the folders listed under
[Configuration](#configuration). Your cloud vault stays on the server; there is
no account-deletion command yet.

</details>

<details>
<summary><b>The server</b></summary>

<br/>

The server is a separate binary and is only needed if you host your own. Install
it the same way (`cargo install --path crates/dotvault-server --locked`) or run
it in Docker — see **[docs/deploy.md](docs/deploy.md)**.

</details>

### Shell completions

```bash
dotvault completions bash       > ~/.local/share/bash-completion/completions/dotvault
dotvault completions zsh        > "${fpath[1]}/_dotvault"
dotvault completions fish       > ~/.config/fish/completions/dotvault.fish
dotvault completions powershell >> $PROFILE
```

`elvish` is supported too.

---

## Usage

### Quick start

**1 · Local — find and read your env files** *(no account)*

```bash
# Tell dotvault where your projects live. It remembers the folder.
dotvault scan ~/code

dotvault ls                              # every env file, grouped by project
dotvault show acme-api:production        # values are masked
dotvault show acme-api:production --reveal --keys DATABASE_URL
dotvault find 'AWS_*'                    # every file that defines it
```

**2 · Vault — keep an encrypted copy in the cloud**

```bash
dotvault register --server https://vault.example.com   # shows your recovery key ONCE
dotvault push acme-api:production        # stored as "acme-api/production"
dotvault vault ls

# on another machine
dotvault login --server https://vault.example.com
dotvault pull acme-api/production --out .env
```

**3 · Share — hand someone just what they need**

```bash
# You: share two variables for an hour, openable twice
dotvault share acme-api:production --keys DATABASE_URL,REDIS_URL --expires 1h --max-views 2
#   prints a link, plus a short code:  dv1.<id>.<key>

# Them (they need an account on the same server)
dotvault open dv1.AbCd…xyz --write .env

# You, afterwards
dotvault shares ls
dotvault shares log <id>                 # who opened or saved it, and when
dotvault shares revoke <id>
```

### Concepts

| Concept | What it means |
| :--- | :--- |
| **Target** | What a command works on: a **path** (`./.env`, `~/x/.env.local`), a **project** (`acme-api`) or **`project:env`** (`acme-api:production`). With no target, `./.env` is used. |
| **Project** | The nearest folder with a project marker (`.git`, `package.json`, `Cargo.toml`, `go.mod`, `pyproject.toml`, …), never above the folder you scanned. Otherwise the file's directory name. |
| **Environment** | The part of the file name after `.env.` — `local`, `production`, `production.local`. A bare `.env` is `default`. |
| **Example files** | `.env.example`, `.sample`, `.template`, `.dist` are hidden by default so they don't clutter lists. Ask with `--examples`, or name one explicitly (`acme-api:example`). |
| **Masking** | Values print as `********` unless you pass `--reveal` — including in `--format shell\|json\|dotenv`. |
| **Index** | A small local cache of file locations and variable **names** (never values), so lists are instant. It refreshes on its own; run `dotvault scan` to pick up *new* folders and files. |

### Command reference

<details>
<summary><b>Local — find, read, edit</b> <sub>(no account)</sub></summary>

<br/>

| Command | What it does |
| :--- | :--- |
| `scan [PATHS…] [--depth N]` | Search folders for env files and remember them. With no paths, rescans the remembered folders. |
| `roots add\|rm\|ls` | Manage the remembered folders without scanning. |
| `ls [FOLDER]` | List env files. Filters: `-p/--project`, `-e/--env`, `--path`, `-k/--has-key`, `--examples`, `--only-examples`. Output: `--paths`, `--json`. `--refresh` rescans first. |
| `show [TARGET]` | Show one file's variables. `--reveal`, `--keys A,B`, `--format table\|dotenv\|json\|shell\|powershell`. |
| `find PATTERN` | Find a variable (exact name or glob) across all files, and which values differ (variants `A`, `B`, …). Same filters as `ls`, plus `--reveal`, `--json`. |
| `get KEY` | Print one raw value, for piping. Pick the file with `-f FILE` or `-p PROJECT [-e ENV]`. |
| `set KEY=VALUE…` | Set variables, preserving comments and layout. A bare `KEY` prompts (so the value stays out of shell history); a value can also be piped in. Creates the file if it doesn't exist. |
| `unset KEY…` | Remove variables. |
| `edit [TARGET]` | Open the file in `$VISUAL` / `$EDITOR`. |
| `diff A B` | Compare two files — missing, extra and changed variables. `--reveal` shows the values. |

Filters accept globs (`-p 'acme-*'`). `-p` and `--path` match by substring when
there's no wildcard; `-e` and `-k` match whole names.

</details>

<details>
<summary><b>Account</b></summary>

<br/>

| Command | What it does |
| :--- | :--- |
| `register [--server URL] [--username NAME]` | Create an account. Shows your **recovery key once**. |
| `login [--server URL] [--no-cache-key]` | Log in. `--no-cache-key` keeps the vault key off disk; vault commands then ask for your password. |
| `logout` | Revoke the session on the server and forget it locally. |
| `whoami` | Who you are logged in as, and where the vault key lives. |
| `passwd` | Change your password. Your vault stays readable; other devices are logged out. |
| `recover` | Reset your password with the recovery key. |

</details>

<details>
<summary><b>Vault</b> <sub>(your encrypted cloud copy)</sub></summary>

<br/>

| Command | What it does |
| :--- | :--- |
| `push [TARGET] [--name N] [--keys A,B]` | Store a file (or a subset of it). The default name is `project/env`. Pushing again adds a new version. |
| `pull NAME --out PATH` | Fetch an item into a file. Refuses to overwrite unless you add `--merge` (keeps your comments and other variables) or `--force`. `--stdout` prints instead. `--keys` limits it. |
| `vault ls [--json]` | List your items. |
| `vault show NAME` | Show an item's variables (`--reveal`, `--format`, `--keys`). |
| `vault rm NAME [--yes]` | Delete an item. |

Item names can be abbreviated when unambiguous (`vault show just-port`).

</details>

<details>
<summary><b>Sharing</b></summary>

<br/>

| Command | What it does |
| :--- | :--- |
| `share [TARGET]` | Create a link and a short code. `--keys A,B` or `-i` (pick from a list) choose the variables; `--vault NAME` shares a vault item instead of a file. `--expires 30m\|24h\|7d` (1 minute – 30 days, default 24 h), `--max-views N`, `--note "…"`. |
| `open LINK\|CODE` | Open a share. You must say what to do with it: `--reveal` (print; `--format` as for `show`), `--write PATH` (with `--merge` / `--force`), and/or `--save [NAME]` (keep a copy in your vault). |
| `shares ls [--json]` | The shares you created, with open counts and status. |
| `shares log ID` | Who opened or saved a share. A short unique prefix of the id is enough. |
| `shares revoke ID` | Stop a share from working. |

`open` refuses to run without `--reveal`, `--write` or `--save`, because opening
uses up one of the allowed opens.

</details>

<details>
<summary><b>Utility</b></summary>

<br/>

| Command | What it does |
| :--- | :--- |
| `config` | Show where dotvault keeps its files and your settings. |
| `completions SHELL` | Print a completion script (`bash`, `zsh`, `fish`, `elvish`, `powershell`). |

</details>

### Recipes

<details>
<summary><b>Compare staging and production</b></summary>

<br/>

```bash
dotvault diff acme-api:staging acme-api:production            # names only
dotvault diff acme-api:staging acme-api:production --reveal   # with values
```

Exit code is `1` when they differ, so it works in scripts.

</details>

<details>
<summary><b>Where is a secret defined, and do the copies agree?</b></summary>

<br/>

```bash
dotvault find STRIPE_KEY
```

Each row shows the project, environment and a **variant** letter. Rows with the
same letter hold the same value; different letters mean they differ — without
printing the value.

</details>

<details>
<summary><b>Load an environment into your shell</b></summary>

<br/>

```bash
# bash / zsh
eval "$(dotvault show acme-api:local --format shell --reveal)"

# PowerShell
dotvault show acme-api:local --format powershell --reveal | Invoke-Expression
```

Only variables with plain names (`MY_VAR`) are emitted, and every value is
quoted.

</details>

<details>
<summary><b>Rotate a value everywhere it is defined</b></summary>

<br/>

```bash
dotvault ls -k API_KEY --paths | while IFS= read -r file; do
  dotvault set API_KEY="$NEW_VALUE" -f "$file"
done
```

Example files are skipped automatically. Comments and layout in each file are
untouched.

</details>

<details>
<summary><b>Set up a new machine</b></summary>

<br/>

```bash
dotvault login --server https://vault.example.com
dotvault vault ls
dotvault pull acme-api/production --out .env --merge
```

`--merge` adds the pulled variables to an existing `.env` without losing your
local comments or extra variables.

</details>

<details>
<summary><b>Give a teammate two variables, safely</b></summary>

<br/>

```bash
dotvault share acme-api:production -k DATABASE_URL,REDIS_URL --expires 2h --max-views 1
# send them the link or the short code — ideally on a different channel than
# the one you would have used for the secret itself

# they run:
dotvault open <link-or-code> --write .env --merge
```

Check `dotvault shares log <id>` to see when they opened it, and
`dotvault shares revoke <id>` if you sent it to the wrong place.

</details>

<details>
<summary><b>Use it from scripts and CI</b></summary>

<br/>

```bash
# machine-readable output
dotvault ls --json        # file paths, project, env, key names — never values
dotvault find API_KEY --json

# a token from a logged-in machine can share and open (but has no vault key)
export DOTVAULT_SERVER=https://vault.example.com
export DOTVAULT_TOKEN=…   # the "token" field of session.json on a machine where you logged in
dotvault open "$SHARE_CODE" --write .env
```

</details>

### Configuration

**Where things live** — override with `DOTVAULT_CONFIG_DIR` / `DOTVAULT_DATA_DIR`:

| | Linux | macOS | Windows |
| :--- | :--- | :--- | :--- |
| **Config** (`config.toml`, `session.json`) | `~/.config/dotvault` | `~/Library/Application Support/dotvault` | `%APPDATA%\dotvault\config` |
| **Index** (`index.db`) | `~/.local/share/dotvault` | `~/Library/Application Support/dotvault` | `%APPDATA%\dotvault\data` |

`dotvault config` prints the exact paths on your machine.

<details>
<summary><b><code>config.toml</code></b></summary>

<br/>

```toml
server = "https://vault.example.com"   # filled in on first login
roots = ["/home/you/code"]             # managed by `scan` / `roots`
max_depth = 10                         # how deep to look
auto_scan_secs = 600                   # rescan when the index is older than this
ignore = ["playground"]                # extra folder names to skip
example_suffixes = ["example", "sample", "template", "dist"]
```

Always skipped: `node_modules`, `.git`, `target`, `vendor`, `dist`, `build`,
`.venv`, `__pycache__`, `.next`, `.cache`, and similar dependency and build
folders.

</details>

<details>
<summary><b>Environment variables</b></summary>

<br/>

| Variable | Used for |
| :--- | :--- |
| `DOTVAULT_CONFIG_DIR`, `DOTVAULT_DATA_DIR` | Move the config and index. |
| `DOTVAULT_SERVER` | Server for `register` / `login` / `recover` (same as `--server`). |
| `DOTVAULT_USERNAME`, `DOTVAULT_PASSWORD` | Non-interactive login or registration. |
| `DOTVAULT_NEW_PASSWORD`, `DOTVAULT_RECOVERY_KEY` | Non-interactive `passwd` and `recover`. |
| `DOTVAULT_TOKEN` | A session token for scripts (with `DOTVAULT_SERVER`). Can share and open; has no vault key. |
| `VISUAL`, `EDITOR` | The editor for `edit`. |
| `NO_COLOR` | Disable colors. |

Environment variables are visible to other processes of the same user — prefer
the interactive prompts for everyday use.

</details>

<details>
<summary><b>Exit codes</b></summary>

<br/>

| Code | Meaning |
| :--- | :--- |
| `0` | Success. |
| `1` | An error — or, for `diff`, files that differ; for `find`, no match; for `unset` / `show --keys`, a variable that wasn't set. |

</details>

### Troubleshooting

<details>
<summary><b><code>ls</code> shows nothing</b></summary>

<br/>

dotvault only searches folders it has been told about. Run
`dotvault scan ~/code` (or wherever your projects are). New files in a known
folder need `dotvault ls --refresh`.

</details>

<details>
<summary><b>My <code>.env.example</code> is missing from the list</b></summary>

<br/>

Example files are hidden on purpose. Use `dotvault ls --examples`, or open one
directly: `dotvault show acme-api:example`.

</details>

<details>
<summary><b>I forgot my password</b></summary>

<br/>

Use the recovery key you were shown at registration: `dotvault recover`. If you
lost both, the cloud data cannot be recovered — by design, nobody (including the
server) can reset it. Your local files are never affected.

</details>

<details>
<summary><b>"refusing to talk to … over plain http"</b></summary>

<br/>

The client only talks to `https://` servers (plain `http://` is allowed for
`localhost`, for testing). Put your server behind TLS — see
[Deploying](docs/deploy.md).

</details>

<details>
<summary><b>"this share has expired, was revoked, or reached its open limit"</b></summary>

<br/>

Ask the sender for a new one. Each open counts, including your own and ones made
with a mistyped key.

</details>

---

## Self-hosting

The server is one small binary with SQLite built in.

```bash
cargo run --release -p dotvault-server          # listens on 127.0.0.1:8080

dotvault register --server http://localhost:8080
```

For real use, run it behind TLS and back up the database. See
**[docs/deploy.md](docs/deploy.md)** for Docker, Fly.io, Litestream and the
configuration reference.

---

## Security at a glance

- Cloud data is **encrypted on your machine** before upload: Argon2id →
  HKDF → XChaCha20-Poly1305. The server never receives your password, your vault
  key or a share key.
- A share's key lives in the link, not on the server.
- Anything received from another person is treated as hostile: terminal escape
  sequences are neutralized, shell output only emits safe names, and written
  files are safe to `source`.
- Your **recovery key** is the only way back in if you forget your password.

> [!WARNING]
> The design has not been independently reviewed, and the session is kept in a
> `0600` file rather than the OS keychain. Read the
> **[security model](docs/security-model.md)** for what is and isn't protected.

---

## Tech Stack

| Layer | Built with |
| :--- | :--- |
| **CLI** | Rust · clap · ignore (fast directory walking) · rusqlite (local index) · ureq + rustls |
| **Crypto** | RustCrypto — argon2 · hkdf · chacha20poly1305 |
| **Server** | axum · tokio · sqlx · SQLite |
| **Shared core** | Rust — lossless `.env` parser, crypto, share codes, API types |
| **Release** | GitHub Actions · cargo-dist |
| **Container** | Docker (distroless) |

---

## Project Structure

```text
dotvault/
├── crates/
│   ├── dotvault-core/            # Shared library
│   │   └── src/
│   │       ├── dotenv.rs         #   Lossless .env parser and editor
│   │       ├── classify.rs       #   Which files are env files; labels and examples
│   │       ├── crypto.rs         #   Argon2id, HKDF, XChaCha20-Poly1305, key wrapping
│   │       ├── share_code.rs     #   Share links and codes
│   │       ├── payload.rs        #   What gets encrypted into items and shares
│   │       └── api.rs            #   Wire types shared by CLI and server
│   │
│   ├── dotvault-cli/             # The `dotvault` command
│   │   ├── src/
│   │   │   ├── cli.rs            #   Command-line definition
│   │   │   ├── commands/         #   One module per command group
│   │   │   ├── scan.rs index.rs  #   Finding files; the local index
│   │   │   ├── client.rs         #   HTTP client for the server
│   │   │   ├── session.rs        #   Saved login
│   │   │   └── output.rs         #   Tables, masking, safe terminal output
│   │   └── tests/                #   End-to-end tests (real binary, real server)
│   │
│   └── dotvault-server/          # Sync and sharing server
│       ├── src/                  #   axum routes, auth, rate limiting, janitor
│       ├── migrations/           #   SQLite schema
│       ├── assets/               #   Share landing page
│       └── tests/
│
├── docs/                         # Security model, deployment
├── deploy/                       # Dockerfile, Fly.io and Litestream templates
├── ARCHITECTURE.md
├── CONTRIBUTING.md
├── SECURITY.md
└── CODE_OF_CONDUCT.md
```

<details>
<summary><b>Directory overview</b></summary>

<br/>

### `dotvault-core`

Everything both sides must agree on, with no heavy I/O:

- Reading and editing `.env` files without disturbing them
- Deciding what counts as an env file, and which are examples
- All cryptography
- Share link and code formats
- The request and response types of the API

### `dotvault-cli`

The `dotvault` binary:

- Scanning, the local index, filters and target resolution
- Local commands, and the account, vault and sharing commands
- Safe terminal output and file writing

### `dotvault-server`

The service behind the cloud features:

- Accounts and sessions, stored as hashes
- Encrypted vault items and shares, stored as ciphertext
- Expiry, open limits, revocation and the audit log

</details>

---

## Documentation

<table>
<tr>
<td width="30%">

**[Architecture](./ARCHITECTURE.md)**

</td>
<td>

How it is put together: components, key hierarchy, data flows, the server API,
and the design decisions behind them.

</td>
</tr>
<tr>
<td>

**[Security model](./docs/security-model.md)**

</td>
<td>

What is encrypted, what the server can and can't see, threat model, and known
limits.

</td>
</tr>
<tr>
<td>

**[Deploying](./docs/deploy.md)**

</td>
<td>

Running your own server: configuration, TLS, Docker, Fly.io, backups.

</td>
</tr>
<tr>
<td>

**[Contributing](./CONTRIBUTING.md)**

</td>
<td>

Prerequisites, local setup, project layout, and the checks CI runs.

</td>
</tr>
<tr>
<td>

**[Code of Conduct](./CODE_OF_CONDUCT.md)**

</td>
<td>

Expected behavior in issues, pull requests, and discussions.

</td>
</tr>
<tr>
<td>

**[Security Policy](./SECURITY.md)**

</td>
<td>

How to report a vulnerability privately.

</td>
</tr>
</table>

---

## Contributing

Contributions are welcome. Read **[CONTRIBUTING.md](CONTRIBUTING.md)** for setup,
project layout, and the checks CI runs.

1. **Fork** the repository.
2. **Create** a feature branch.
3. **Keep commits focused** and descriptive.
4. **Follow** the existing project structure.
5. **Run the checks** before submitting.
6. **Open a PR** with a clear description of your changes.

> [!TIP]
> For larger features or architectural changes, open an issue first so the
> implementation can be discussed before development begins.

Also see the [Code of Conduct](CODE_OF_CONDUCT.md) and, for vulnerabilities, the
[Security Policy](SECURITY.md) — please don't report those in public issues.

---

## Acknowledgements

dotvault stands on the work of others:

- **[Rust](https://www.rust-lang.org/)** — the language and toolchain.
- **[RustCrypto](https://github.com/RustCrypto)** — Argon2, HKDF and XChaCha20-Poly1305.
- **[axum](https://github.com/tokio-rs/axum)** and **[tokio](https://tokio.rs/)** — the server.
- **[SQLite](https://www.sqlite.org/)** and **[sqlx](https://github.com/launchbadge/sqlx)** — storage.
- **[clap](https://github.com/clap-rs/clap)** — the command line.
- **[ignore](https://github.com/BurntSushi/ripgrep/tree/master/crates/ignore)** — the parallel directory walker from ripgrep.
- **[ureq](https://github.com/algesten/ureq)** and **[rustls](https://github.com/rustls/rustls)** — the HTTP client.
- **[cargo-dist](https://github.com/axodotdev/cargo-dist)** — release packaging.
- **[Bitwarden](https://bitwarden.com/help/bitwarden-security-white-paper/)** — whose published account design (separate authentication and encryption keys derived from the password) inspired the same split here.

---

## License

Licensed under either of

- **[Apache License, Version 2.0](LICENSE-APACHE)**, or
- **[MIT license](LICENSE-MIT)**

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

---

<div align="center">

<sub>◇ ─────────────── ◇ ─────────────── ◇</sub>

### built with ❤️

<sub><b>dotvault</b> · <a href="https://github.com/YatharthKaushal/dotvault">github.com/YatharthKaushal/dotvault</a></sub>

<br/>

</div>
