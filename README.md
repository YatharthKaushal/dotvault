# dotvault

Find, read, edit, store and share your `.env` files from one place.

[![CI](https://img.shields.io/github/actions/workflow/status/YatharthKaushal/dotvault/ci.yml?branch=main&label=CI)](https://github.com/YatharthKaushal/dotvault/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

**Contents:** [Install](#install) · [Quick start](#quick-start) · [Targets](#targets) ·
[Commands](#commands) · [Settings](#settings) · [Docs](#documentation) · [Security](#security)

Every project keeps its `.env` files in its own folder, so finding one value means
hopping between folders. dotvault finds them all, on every drive, and lets you list,
search, read and edit them from one command line. Optionally it keeps an end-to-end
encrypted copy in the cloud and shares chosen variables through a link. An account is
only needed for the cloud features.

> [!NOTE]
> Early development. The tests pass in CI on Linux, macOS and Windows, but the
> cryptographic design has not been independently reviewed. See [Security](#security).

| | |
| :--- | :--- |
| **Find** | Every `.env`, `.env.*` and secret file (SSH keys, keystores, secret JSON) on your whole computer, with no setup, grouped by project and environment |
| **Read** | Values stay masked until `--reveal`. Output as table, dotenv, JSON, shell or PowerShell. `find` shows where a variable lives and whether the copies agree |
| **Copy** | `get -c` puts a value on the clipboard and clears it after 30 seconds |
| **Edit** | `set`, `unset` and `edit` keep comments and order. Writes are atomic, and commands running at the same moment take turns |
| **History** | Git-like versions of every variable dotvault changes: `log`, `revert` |
| **Cloud** | `push` and `pull` with encryption on your machine. Share with a link, an expiry and an open limit |
| **Small** | One native binary of about 4 to 5.5 MiB. On Linux it starts in about 2 ms and lists 600 files across 300 projects in about 11 ms |

## Install

| Platform | Command |
| :--- | :--- |
| Linux, macOS | `curl -fsSL https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.sh \| sh` |
| Windows (PowerShell) | `irm https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.ps1 \| iex` |

It downloads the release for your platform, checks its SHA-256, installs to
`~/.local/bin` (Windows: `%LOCALAPPDATA%\Programs\dotvault`) and adds that folder to
your `PATH`. No `sudo` or administrator rights. Then check with `dotvault -v`.

| Task | How |
| :--- | :--- |
| Update | Run the same installer again |
| A specific version | `DOTVAULT_VERSION=v0.3.6` on the `sh` side of the pipe (PowerShell: `$env:DOTVAULT_VERSION = "v0.3.6"`) |
| Leave `PATH` alone | `DOTVAULT_NO_MODIFY_PATH=1` (or `--no-modify-path`) |
| Uninstall | Linux, macOS: `rm ~/.local/bin/dotvault ~/.local/share/dotvault/env`. Windows: delete the install folder and its `PATH` entry. [Details](docs/guide.md#uninstall) |
| From source | `cargo install --path crates/dotvault-cli --locked` (Rust 1.89+ and a C compiler) |
| Not supported by the binary | Alpine and other musl systems, glibc older than 2.34: build from source |
| Shell completions | `dotvault completions bash\|zsh\|fish\|elvish\|powershell` |

All install options, `PATH` behaviour per shell, checksums and other installers are in
the [guide](docs/guide.md#install).

## Quick start

There is nothing to set up. The first command that needs the list searches the whole
computer (a minute on a large disk) and remembers it.

```bash
dotvault ls                                               # every env file, grouped by project
dotvault show acme-api:production                         # values masked; add --reveal
dotvault get DATABASE_URL -p acme-api -e production -c    # copy it; cleared in 30 s
dotvault set API_KEY=new -p acme-api -e production
dotvault log API_KEY -p acme-api -e production            # every version of one variable
dotvault revert API_KEY -p acme-api -e production         # back to the one before
dotvault find STRIPE_KEY                                  # where is it defined, do the copies agree?
dotvault diff acme-api:staging acme-api:production
```

Cloud and sharing need an account. They use the hosted server unless you pass
`--server URL`.

```bash
dotvault register                                         # prints a recovery key once
dotvault push acme-api:production                         # stored as "acme-api/production"
dotvault login && dotvault pull acme-api/production --out .env --merge     # on another machine
dotvault share acme-api:production --keys DATABASE_URL --expires 1h        # prints a link and a code
dotvault open <link or code> --write .env                                  # the recipient
```

## Targets

What a command works on.

| You type | It means |
| :--- | :--- |
| `./.env`, `~/x/.env.local` | a path |
| `acme-api` | a project |
| `acme-api:production` | a project and environment: the file `.env.production` (a bare `.env` is `default`) |
| nothing | `./.env` |
| `-f FILE`, `-p PROJECT -e ENV` | how `get`, `set`, `unset`, `log`, `revert` and `forget` pick the file, since they take keys. `show`, `edit`, `push` and `share` accept them too |

## Commands

Every command takes `--help`. Every flag is in the [guide](docs/guide.md#command-reference).

| Command | What it does |
| :--- | :--- |
| **Find** | |
| `ls [FOLDER]` | List env files. Filters `-p`, `-e`, `--path`, `-k`. `--secrets` lists secret files. `--paths`, `--json` |
| `find PATTERN` | Where a variable (name or glob) is defined, and whether the copies agree |
| `scan [PATHS]` | Search folders and remember them (default: the whole computer) |
| `roots add\|rm\|ls` | Manage the remembered folders |
| **Read** | |
| `show [TARGET]` | A file's variables, masked. `--reveal`, `--keys`, `--format table\|dotenv\|json\|shell\|powershell` |
| `get KEY...` | Raw values. `-c` copies, `-k` prints `KEY=value`, `-ckv` copies the pair |
| `diff A B` | Compare two files or projects. Exits `1` if they differ |
| **Edit** | |
| `set KEY=VALUE...` | Set variables. A bare `KEY` prompts, so it stays out of shell history |
| `unset KEY...` | Remove variables |
| `edit [TARGET]` | Open the file in `$VISUAL` / `$EDITOR` |
| **History** | |
| `log [KEY]` | Versions of a variable, or every change to the file. `--reveal`, `--json` |
| `revert KEY [--to N]` | Go back to an earlier version (recorded as a new one) |
| `forget KEY\|--all` | Delete the recorded history |
| **Secret files** | |
| `mark` / `unmark FILE...` | Treat a file as a secret, or stop listing it |
| **Cloud** (account) | |
| `register`, `login`, `logout`, `whoami`, `passwd`, `recover` | Account and session |
| `push [TARGET]` | Store a file in your encrypted vault |
| `pull NAME --out PATH` | Fetch it back. `--merge`, `--force`, `--stdout` |
| `vault ls\|show\|rm` | Browse your vault |
| **Share** (account) | |
| `share [TARGET]` | Make a link and code. `--keys`, `--expires`, `--max-views`, `--note` |
| `open LINK\|CODE` | Open one. `--reveal`, `--write PATH`, `--save` |
| `shares ls\|log\|revoke` | Manage the shares you created |
| **Other** | |
| `config` | Show where dotvault keeps its files |
| `completions SHELL` | Print a completion script |
| `help [COMMAND]`, `-v` | Help, version |

| Exit code | Meaning |
| :--- | :--- |
| `0` | Success |
| `1` | An error. Also: `diff` found differences, `find` found nothing, `unset` or `show --keys` named a variable that is not set |

`get` prints nothing and fails if any name it was given is not set, so `$(dotvault get KEY)`
is safe in scripts. More recipes: [In scripts](docs/guide.md#in-scripts).

## Settings

| What | How |
| :--- | :--- |
| Where files live | `dotvault config` prints the paths. Override with `DOTVAULT_CONFIG_DIR` and `DOTVAULT_DATA_DIR` |
| Which folders are searched | The whole computer by default. `dotvault scan ~/code` to name folders, `DOTVAULT_SCAN_ROOTS` to replace them |
| Server | The hosted `https://dotvault.synquic.tech`, or your own with `--server URL` or `DOTVAULT_SERVER` |
| Login without prompts | `DOTVAULT_USERNAME`, `DOTVAULT_PASSWORD`, `DOTVAULT_TOKEN` |
| Editor, colors | `$VISUAL` / `$EDITOR`, `NO_COLOR` |
| History | On, 50 versions per variable. `history = false` in `config.toml` turns it off |

Every `config.toml` key and environment variable is in the
[guide](docs/guide.md#configuration).

## Documentation

| | |
| :--- | :--- |
| [Guide](docs/guide.md) | The full reference: install options, every flag, clipboard, history, secret files, where dotvault looks, cloud, sharing, scripting, configuration |
| [Self-hosting](docs/deploy.md) | Run your own server (systemd and Caddy, Docker, Fly.io) |
| [Security model](docs/security-model.md) | What is protected and what is not |
| [Architecture](ARCHITECTURE.md) | Components, key hierarchy, data flows, server API |
| [Contributing](CONTRIBUTING.md) | Building, testing and sending changes |
| [Security policy](SECURITY.md), [Code of conduct](CODE_OF_CONDUCT.md) | Reporting vulnerabilities, community rules |
| [Download page](https://yatharthkaushal.github.io/dotvault/) | The install commands and direct download links |

## Security

| | |
| :--- | :--- |
| Encryption | Cloud data is encrypted on your machine. The server stores only ciphertext and never sees your password, vault key or a share key |
| Recovery key | The only way back in if you forget your password. Lose both and the cloud data is gone |
| Hosted server | Run by the maintainer on a best-effort basis with no durability guarantee, so keep your env files. It sees account metadata (usernames, timing, sizes) |
| History file | Keeps old and current values in plain text, readable only by you |
| Review | The design has not been independently reviewed |

Report vulnerabilities as described in [SECURITY.md](SECURITY.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise,
any contribution intentionally submitted for inclusion in this work by you, as
defined in the Apache-2.0 license, is dual licensed as above, without any
additional terms or conditions.
