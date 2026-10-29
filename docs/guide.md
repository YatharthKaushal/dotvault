# dotvault guide

The full reference. The [README](../README.md) has the short version: install,
quick start and the command list.

| Section | What is in it |
| :--- | :--- |
| [Install](#install) | Installer options, `PATH`, verifying a download, [update](#update), [uninstall](#uninstall), [from source](#from-source), [completions](#completions) |
| [Targets](#targets-and-picking-a-file) | How a command is told which file to use |
| [Commands](#command-reference) | Every command with its flags, filters, exit codes |
| [Clipboard](#copying-to-the-clipboard) | `get -c`, platform support |
| [History](#history-of-a-variable) | `log`, `revert`, `forget` |
| [Secret files](#secret-files) | SSH keys, keystores, secret JSON, credential dotfiles |
| [Where it looks](#where-dotvault-looks) | Which folders and drives are searched |
| [Cloud vault](#cloud-vault) | Account, `push`, `pull`, the server |
| [Sharing](#sharing) | `share`, `open`, `shares` |
| [In scripts](#in-scripts) | JSON output, exit codes, recipes |
| [Configuration](#configuration) | Files and folders, `config.toml`, environment variables |
| [Self-hosting](#self-hosting) | Running your own server |
| [Security](#security) | What is protected, what is not |

## Install

Current version: the [latest release](https://github.com/YatharthKaushal/dotvault/releases/latest).

| | Linux and macOS | Windows (PowerShell, x64) |
| :--- | :--- | :--- |
| Command | `curl -fsSL https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.sh \| sh` | `irm https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.ps1 \| iex` |
| Architectures | x86_64 and aarch64; Linux needs glibc 2.34 or newer | x86_64 |
| Installs to | `~/.local/bin/dotvault` | `%LOCALAPPDATA%\Programs\dotvault\dotvault.exe` |
| `PATH` | added if the folder is not on it | added to your user `PATH` |
| Needs | no `sudo` | no administrator rights |
| Options | `DOTVAULT_VERSION`, `DOTVAULT_INSTALL_DIR`, `DOTVAULT_NO_MODIFY_PATH=1` (or `--version`, `--dir`, `--no-modify-path`) | `DOTVAULT_VERSION`, `DOTVAULT_INSTALL_DIR`, `DOTVAULT_NO_MODIFY_PATH=1` |

The scripts download the archive for your platform from GitHub Releases, verify its
SHA-256 checksum, copy the binary into place and put it on your `PATH` if it is not
already there. Read them first: [install.sh](../install.sh), [install.ps1](../install.ps1).
You can also pick an archive from the
[releases page](https://github.com/YatharthKaushal/dotvault/releases), extract it and
put `dotvault` on your `PATH`. The same commands and direct download links are on the
[download page](https://yatharthkaushal.github.io/dotvault/) (source in
[site/](../site/index.html)).

Set the variables on the `sh` side of the pipe, because that is the process that
reads them:

```bash
curl -fsSL https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.sh | DOTVAULT_VERSION=v0.1.0 sh
curl -fsSL https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.sh | sh -s -- --dir ~/bin
```

In PowerShell, set `$env:DOTVAULT_VERSION = "v0.1.0"` before running the installer.

### Your `PATH`

If the install folder is not already on your `PATH`, the installer adds it and says
what it changed. Nothing is changed when it is already there.

| Platform and shell | What the installer changes |
| :--- | :--- |
| Windows | The folder is added to your user `PATH` and to the PowerShell window you ran the installer in, so `dotvault` works straight away. Every other entry is kept exactly as it was, including ones that use variables such as `%JAVA_HOME%\bin`. Terminals that were already open need to be reopened. |
| zsh | One line in `~/.zshrc` |
| bash | One line in `~/.bashrc` and one in the login file bash reads (`~/.bash_profile`, `~/.bash_login` or `~/.profile`, whichever exists first) |
| sh, dash, ksh | One line in `~/.profile` |
| fish | A file of its own: `~/.config/fish/conf.d/dotvault.fish` |
| csh, tcsh, nushell | Nothing. The installer prints what to add. |

On Linux and macOS the line only sources `~/.local/share/dotvault/env`, which does
the work, and running the installer again adds nothing twice. New terminals have
`dotvault`; in the current one, run the `. "$HOME/.local/share/dotvault/env"` line
the installer prints.

To opt out, set `DOTVAULT_NO_MODIFY_PATH=1` (or pass `--no-modify-path` to
`install.sh`) and add the folder yourself, for example
`export PATH="$HOME/.local/bin:$PATH"`. Programs that do not read startup files, such
as cron jobs, need the folder on their `PATH` themselves.

### Other install notes

| Topic | Detail |
| :--- | :--- |
| cargo-dist installers | Each release also carries `dotvault-installer.sh` and `dotvault-installer.ps1` from [cargo-dist](https://github.com/axodotdev/cargo-dist), which the release notes show. They install into `~/.cargo/bin` (`%USERPROFILE%\.cargo\bin` on Windows) and add that folder to your `PATH`. Use either installer, not both, or you will have two copies in different folders. |
| Verify a download | Every archive has a `.sha256` file beside it: `sha256sum -c dotvault-x86_64-unknown-linux-gnu.tar.xz.sha256` on Linux, `shasum -a 256 -c` on macOS, or compare with `Get-FileHash` in PowerShell. |
| macOS | The binaries are not signed. Files fetched by the script are not quarantined; one downloaded in a browser may be (clear it with `xattr -d com.apple.quarantine dotvault`). |
| Alpine, musl, old Linux | Linux older than glibc 2.34 (Ubuntu 20.04, Debian 11, RHEL 8) and musl systems such as Alpine are not supported by the prebuilt binary. [Build from source.](#from-source) |

### Update

dotvault does not update itself or tell you when a new version exists. Run the same
installer you used originally. It fetches the latest release and replaces the binary
in place. Your env files, settings, index and saved login are kept.

| You installed with | Update with |
| :--- | :--- |
| `install.sh` (Linux, macOS) | `curl -fsSL https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.sh \| sh` |
| `install.ps1` (Windows) | `irm https://raw.githubusercontent.com/YatharthKaushal/dotvault/main/install.ps1 \| iex` |
| cargo-dist's `dotvault-installer.sh` | `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/YatharthKaushal/dotvault/releases/latest/download/dotvault-installer.sh \| sh` |
| cargo-dist's `dotvault-installer.ps1` | `powershell -ExecutionPolicy Bypass -c "irm https://github.com/YatharthKaushal/dotvault/releases/latest/download/dotvault-installer.ps1 \| iex"` |
| a downloaded archive | download the new archive and replace the `dotvault` binary with the one inside |
| from source | `git pull`, then `cargo install --path crates/dotvault-cli --locked --force` |

Check the result with `dotvault -v` and compare it with the
[releases page](https://github.com/YatharthKaushal/dotvault/releases). To pin or roll
back to a version, set `DOTVAULT_VERSION` as shown above.

| Watch out for | Why |
| :--- | :--- |
| A running `dotvault` on Windows | Windows will not replace a program that is in use. Close it first. |
| Mixing install methods | You end up with two copies in different folders and the one earlier on your `PATH` wins. `which dotvault` (Linux, macOS) or `Get-Command dotvault` (PowerShell) shows which one runs. |

### Uninstall

Delete the binary and the `PATH` setup. Your env files, settings and saved login are
not touched; delete the folders listed under [Configuration](#configuration) to remove
those.

| Platform | Steps |
| :--- | :--- |
| Linux, macOS | Run `rm ~/.local/bin/dotvault ~/.local/share/dotvault/env`. That already stops the `PATH` line from doing anything, and it is safe to leave in place. To remove it too, delete the two lines marked `# added by the dotvault installer` from `~/.zshrc`, `~/.bashrc` and `~/.profile` (or `~/.bash_profile`), whichever have them. For fish, delete `~/.config/fish/conf.d/dotvault.fish`. Do not delete the whole `~/.local/share/dotvault` folder: it also holds the index and the history. |
| Windows | Delete the folder `%LOCALAPPDATA%\Programs\dotvault`, then remove its entry from your user `PATH`: Start, "Edit environment variables for your account", select `Path` under User variables, Edit, and delete the dotvault line. |
| From source | `cargo uninstall dotvault` |

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

The binary goes to Cargo's bin folder: `~/.cargo/bin/dotvault`, or
`%USERPROFILE%\.cargo\bin\dotvault.exe` on Windows.

### Completions

`dotvault completions <bash|zsh|fish|elvish|powershell>` prints a script, for example
`dotvault completions bash > ~/.local/share/bash-completion/completions/dotvault`.

## Targets and picking a file

A **target** is what a command works on.

| You type | It means |
| :--- | :--- |
| `./.env`, `~/x/.env.local`, `a/b/.env` | a path |
| `acme-api` | a project |
| `acme-api:production` | a project and an environment (the file `.env.production`) |
| nothing | `./.env` |

The environment is the part of the file name after `.env.`, so `.env.production` is
`production` and a bare `.env` is `default`.

| Commands | How the file is chosen |
| :--- | :--- |
| `show`, `edit`, `push`, `share` | a target, or `-f FILE`, or `-p PROJECT [-e ENV]` |
| `get`, `set`, `unset`, `log`, `revert`, `forget` | they take keys, not a target: `-f FILE` (a path) or `-p PROJECT [-e ENV]` |
| `diff` | two targets |
| `pull`, `vault`, `open` | a vault item name, or a link or code |

`dotvault COMMAND --help` (or `dotvault help COMMAND`) lists every option of a
command, and `dotvault -v` prints the version.

## Command reference

| Command | Description |
| :--- | :--- |
| `scan [PATHS] [--system] [--depth N]` | Search folders for env files and remember them. With no paths, rescans the remembered folders (the whole computer, every drive, on first use). `--system` adds the whole computer to folders you named. `--depth` limits how many levels down it goes. |
| `roots add\|rm\|ls` | Manage the remembered folders: `add` remembers folders without scanning them yet, `rm` forgets them, `ls` lists them. |
| `ls [FOLDER]` (alias `list`) | List env files, optionally only those inside `FOLDER`. Filters: `-p/--project`, `-e/--env`, `--path`, `-k/--has-key`, `--examples`, `--only-examples`; `-e`, `-k` and `--kind` can be repeated. Output: `--paths`, `--json`. `--refresh` rescans first. `--secrets` or `--kind KIND` list secret files instead. |
| `mark FILE...` / `unmark FILE...` | Treat files as secrets by hand, or stop listing them (also silences a wrong guess). |
| `show [TARGET]` | Show a file's variables. `--reveal`, `-k/--keys A,B` (names or globs like `'STRIPE_*'`), `--format table\|dotenv\|json\|shell\|powershell` (default `table`). A bare project name never picks an example file; `--examples` allows it (or name it: `acme-api:example`). A secret file (SSH key, keystore, secret JSON) is described instead, never printed; a JSON one shows its fields masked. |
| `find PATTERN` | Find a variable (name or glob) across all files. Filters as for `ls`: `-p`, `-e`, `--path`, `--examples`, `--only-examples`. Plus `--reveal`, `--json`. Rows with the same variant letter hold the same value. |
| `get KEY...` | Print raw values: one name prints the bare value, several names or a glob like `'STRIPE_*'` print `KEY=value` lines. Pick the file with `-f FILE` or `-p PROJECT [-e ENV]`. `-k`/`--kv` gives `KEY=value` even for one name. `-c`/`--copy` copies to the clipboard instead, and `-ckv` copies the whole pair. |
| `set KEY=VALUE...` | Set variables, keeping comments and layout. A bare `KEY` prompts for the value. Creates the file if needed. |
| `unset KEY...` | Remove variables. Exits `1` if one was not set. |
| `edit [TARGET]` | Open the file in `$VISUAL` / `$EDITOR`. |
| `log [KEY]` (alias `history`) | History of one variable, or of every change to the file. `-n N`/`--limit N`, `--reveal`, `--json`. Pick the file with `-f` or `-p`/`-e`. |
| `revert KEY [--to N]` | Put a variable back to an earlier version (default: the one before). Recorded as a new version. |
| `forget KEY` / `forget --all` | Delete the recorded history of a variable, or of the whole file. |
| `diff A B` | Compare two files or projects. `-` marks a variable only in `A`, `+` only in `B`, `~` one whose value differs. `--reveal` shows the values. Exits `1` if they differ. |
| `register`, `login`, `logout`, `whoami` | Account and session. `register`, `login` and `recover` take `--server URL` (default: the hosted one) and `-u/--username NAME`. `login` and `recover` also take `--no-cache-key`, which keeps the vault key off disk, so vault commands ask for your password. |
| `passwd`, `recover` | Change your password, or reset it with the recovery key. |
| `push [TARGET] [-n/--name N] [-k/--keys A,B] [--as-file]` | Store a file in your vault. The default name is `project/env`. Pushing again adds a version. SSH keys, keystores and secret JSON are stored whole, as files; `--as-file` does that for any small file. |
| `pull NAME [-o/--out PATH] [-k/--keys A,B]` | Fetch a vault item. Refuses to overwrite unless you add `--merge` or `--force`. `--stdout` prints variables instead. A file item (an SSH key, a keystore) is written exactly, readable only by you; `--merge`, `--keys` and `--stdout` do not apply to it and the folder must exist. |
| `vault ls\|show\|rm` | List, show or delete vault items. Names can be abbreviated when unambiguous. `ls` takes `--json`. `show` takes `--reveal`, `--format` and `-k/--keys` like the local `show`, and describes a file item without printing it. `rm` asks first unless you add `-y/--yes`. |
| `share [TARGET]` | Create a link and code. `--keys A,B` or `-i/--interactive` choose the variables, `--vault NAME` shares a vault item, `--expires 30m\|24h\|7d` (1 minute to 30 days, default 24 h), `--max-views N`, `--note TEXT`, `--as-file`, `-y/--yes` (skip the question asked before sharing a private key, keystore or service-account file). |
| `open LINK\|CODE` | Open a share. Requires at least one of `--reveal`, `-w/--write PATH` (with `--merge` or `--force`) or `--save [NAME]`, because opening uses up one of the allowed opens. `--format` chooses how `--reveal` prints, as for `show`. A shared file is written exactly, readable only by you. |
| `shares ls\|log\|revoke` | Manage the shares you created. A short unique prefix of the id is enough. `ls` and `log` take `--json`. |
| `config` | Show where dotvault keeps its files. |
| `completions SHELL` | Print a completion script. |
| `help [COMMAND]` | Same as `COMMAND --help`. |

### Filters and exit codes

| Topic | Rule |
| :--- | :--- |
| Globs | Filters accept globs (`-p 'acme-*'`). |
| Example files | `.env.example` and the like are hidden unless you ask: `--examples` includes them, `--only-examples` shows only them. |
| Matching | `-p` and `--path` match by substring when there is no wildcard; `-e` and `-k` match whole names. |
| Exit `0` | Success. |
| Exit `1` | An error. Also: `diff` when the files differ, `find` when nothing matches, `unset` and `show --keys` when a variable was not set. |
| `get` | Fails, and prints nothing, if any name or glob it was given is not set. |

### Load an environment into your shell

```bash
eval "$(dotvault show acme-api:local --format shell --reveal)"                           # bash, zsh
dotvault show acme-api:local --format powershell --reveal | Invoke-Expression            # PowerShell
```

## Copying to the clipboard

`dotvault get KEY -c` puts the value on the clipboard and prints nothing, so it stays
out of your terminal and scrollback.

| Flags | What is copied |
| :--- | :--- |
| `-c` | The bare value of one name |
| `-ckv`, `-ck`, `--copy --kv` | `KEY=value` instead of just the value |
| several names, or a glob | Always `KEY=value` lines |

| Property | Detail |
| :--- | :--- |
| Exact | What is copied is exact, even if it holds control characters that printing would make visible. |
| Cleared | After 30 seconds, but only if the clipboard still holds that value, so anything you copied in the meantime is left alone. |
| Transport | The value goes to a short-lived helper process through a pipe, never on a command line. |
| History | Where the platform has a way to ask, clipboard history is told to skip it (on Windows also cloud sync). |

| Where | Copying |
| :--- | :--- |
| Windows, macOS | Uses the system clipboard. So far only the Linux X11 path has been tried against a real clipboard, so please report problems. |
| Linux with X11 | Works. The helper stays alive to answer paste requests, then clears the value. |
| Linux with Wayland | Goes through XWayland. Not tested here; if it fails you get an error and `get` without `-c` still works. |
| SSH, containers, servers | No clipboard exists. You get an error, and the value is never printed in its place. |

Any program you run can read the clipboard while the value is on it, and a clipboard
manager that ignores the history hint may keep a copy.

## History of a variable

Every time dotvault changes a variable (`set`, `unset`, `edit`, `pull`, `revert`) it
records a numbered version, like a commit for one variable. Nothing is recorded until
dotvault first changes a file; looking at files (`ls`, `show`, `get`) records nothing.

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

| Action | Meaning |
| :--- | :--- |
| `set`, `unset`, `edit`, `pull`, `revert` | dotvault made the change. |
| `baseline` | What a variable held when dotvault first changed its file. |
| `external` | The file differed from the last record, so it was edited outside dotvault. Noticed the next time dotvault changes the file, and stamped with the file's modified time. |

| Topic | Detail |
| :--- | :--- |
| Masking | Values are masked, with a short tag so you can tell equal values apart. `--reveal` shows them. `--json` leaves values out unless you add `--reveal`. |
| `revert` | Never rewrites history: going back is a new version, so you can go forward again. A variable removed in the version you pick is removed again. After an edit made outside dotvault, plain `revert` goes back to the last recorded value. |
| Layout | History stores values, not layout: a variable that `revert` brings back is added at the end of the file, and a comment that sat on its old line is not restored. |
| Grouping | One `set A=1 B=2` or one `edit` is one change, shown together in `log`. |
| Where | `history.db` in the data folder (see `dotvault config`), readable only by you. |
| Plain text | **It keeps old and current values in plain text**, the one place besides your env files where dotvault stores them, so a backup or sync of that folder copies them. |
| Size | The newest 50 versions of each variable (`history_keep`). |
| Off | `history = false` in `config.toml`. |
| `forget` | Overwrites what it deletes, but copies already made by a backup or by the disk itself are out of its reach. |
| Moved files | A file is known by its path. If you move it, its old history stays under the old path (`dotvault log -f OLD`), and the new location starts fresh. |

## Secret files

Env files are not the only secrets lying around. dotvault also recognises these by
their name **and** a quick look inside, and lists them on request:

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

| Topic | Detail |
| :--- | :--- |
| What is read | To tell a key from a certificate, or a service account from ordinary JSON, dotvault reads the start of files with plausible names (a JSON file in full, up to 64 KiB), on your machine, during a scan. It keeps only the kind, a short note such as `ed25519, passphrase` and, for JSON, the top-level key names. Never the contents. A scan of the whole computer takes a little longer for it (tens of milliseconds for several thousand JSON files). |
| Where it looks | Everywhere a scan goes, plus `~/.ssh`, `~/.aws`, `~/.docker`, `~/.kube` and `~/.config/gcloud` (on Windows `%APPDATA%\gcloud`) when they exist, which a scan would otherwise skip as hidden folders. Dependency and fixture folders (`node_modules`, `vendor`, `testdata`, `site-packages`) are skipped. |
| Guessing wrong | `dotvault unmark FILE` silences a file for good; `dotvault mark FILE` adds one dotvault would not notice. Files in a `.ssh` folder with an extension other than `.pem`, `.key` or `.ppk` are not guessed at; mark them. |
| Cloud | `push` stores the whole file, binary included, encrypted on your machine like any vault item, as `project/file name`. `pull NAME --out PATH` writes exactly those bytes with owner-only permissions (Unix; on Windows the file keeps your profile folder's access rules) and will not replace a file without `--force`. `vault show NAME` describes an item and never prints it. `--as-file` stores or shares any small file as it is, even an env file. The limit is about 180 KiB for the vault and about 47 KiB for a share. |
| Sharing | `share` and `open` work as for variables, with `--write PATH` on the receiving side. Because a private key, a keystore or a service-account file lets the holder act as you, `share` asks first in a terminal, and a script has to pass `--yes`. The sender's file name is shown but never used as a path, and `open --reveal` prints text files only. |
| Not covered | Secret files have no version history (history is per env variable), and an older dotvault that finds a file in your vault says it cannot read the item. A server upgrade is not needed. |

## Where dotvault looks

The first command that needs the list of env files searches the whole computer and
remembers that, so there is nothing to set up and no flag to pass. The first run can
take a minute on a large disk. `dotvault roots` shows and changes the remembered
folders.

| Platform | Searched automatically |
| :--- | :--- |
| Windows | Every fixed and removable drive (`C:\`, `D:\`, a USB stick). Network drives are not searched automatically, because a dead share can hang the command; name one yourself with `dotvault scan Z:\`. |
| Linux | `/`, plus local disks mounted under `/mnt`, `/media` and `/run/media`. |
| macOS | `/`, plus disks in `/Volumes`. macOS may ask your terminal for permission to read Documents, Desktop, Downloads and removable volumes. |
| WSL | Windows drives are under `/mnt/c`, which is not searched automatically, so name it: `dotvault scan /mnt/c/Users/you`. |

| Topic | Detail |
| :--- | :--- |
| Following your disks | While dotvault chose the list, a drive you plug in is searched from the next command on, and one you unplug is dropped. |
| Adding a folder | `dotvault scan ~/code` adds a folder, for something dotvault does not look at on its own, such as a network drive. As your very first command it keeps the search to just that folder. |
| Your list | Once you name a folder (or use `roots add` or `roots rm`) the list is yours and dotvault leaves it alone. Removing the last folder starts over with the whole computer. |
| `scan --system` | Adds the whole computer to a list you named. |
| Credential folders | While dotvault chose the list, it also searches the folders that exist among `~/.ssh`, `~/.aws`, `~/.docker`, `~/.kube`, `~/.config/gcloud`, which are hidden folders a scan skips. They are for [secret files](#secret-files). |
| Upgrading from 0.3 or earlier | If your list is just your home folder, which is what those versions saved on their own, the next command widens it once and says so. Anything else you set up is kept. |
| Skipped | Hidden folders directly inside any home folder (`~/.config`, `~/.cache`), `Library`, `AppData` and media folders, `node_modules`, `.git`, `target` and similar, operating system folders (`/proc`, `/usr`, `/etc`, `C:\Windows`, `Program Files`, the recycle bin on every drive), backup snapshots, and network or FUSE mounts. Name a folder yourself to search inside it anyway: `dotvault scan ~/.work`. |
| Home folder | Never treated as a project, even if it holds a `.git` for dotfiles, so its files are named after their own folder. |
| Given a path | A command given a path (`show ./.env`) never starts a search. |
| Indexed | Only file names, variable names and the key names of JSON secrets. Never values or contents. |
| Elsewhere | To search something else instead, for example in a container, set `DOTVAULT_SCAN_ROOTS` to a list of folders (separated like `PATH`). |
| Drive detection | Finding the drives is new on Windows and macOS and has only been run through the automated tests so far. If a drive is missed, `dotvault scan <drive>` adds it, and a report helps. |

## Cloud vault

An account is only needed for `push`, `pull`, `vault`, `share` and `open`.

```bash
dotvault register                         # shows a recovery key once
dotvault push acme-api:production         # stored as "acme-api/production"
dotvault vault ls

# on another machine
dotvault login
dotvault pull acme-api/production --out .env --merge
```

`register`, `login` and `recover` pick the server like this, first match wins (every
other command uses the server of your session):

| Order | Source |
| :--- | :--- |
| 1 | `--server URL` |
| 2 | `DOTVAULT_SERVER` |
| 3 | `server` in your config file |
| 4 | the server of your last session |
| 5 | the hosted server, https://dotvault.synquic.tech (a notice is printed) |

The first successful `register` or `login` saves the server in your config, so a later
release can never move you to a different one.

| About the hosted server | |
| :--- | :--- |
| Stores | Only ciphertext and account metadata (usernames, timing, sizes). |
| Run by | The maintainer, on a best-effort basis with no uptime or durability guarantee. |
| So | Keep your env files and do not treat the cloud copy as the only copy. |
| Alternative | [Run your own server.](#self-hosting) |

## Sharing

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

| Topic | Detail |
| :--- | :--- |
| Choosing variables | `--keys A,B` or `-i/--interactive`. `--vault NAME` shares a vault item instead of a local file. |
| Limits | `--expires` from 1 minute to 30 days (default 24 h), `--max-views N`. |
| Opening | Uses up one of the allowed opens, so `open` needs at least one of `--reveal`, `--write PATH` or `--save [NAME]`. |
| Key | A share's key lives in the link or code, not on the server. |
| Recipient | Needs an account on the same server. |

## In scripts

| Need | How |
| :--- | :--- |
| Parseable output | `ls`, `find`, `log`, `vault ls`, `shares ls` and `shares log` take `--json`. `show` and `vault show` take `--format json`. |
| Values in JSON | `ls --json` lists names, never values. `find --json` and `show` add values only with `--reveal`. |
| File lists | `ls --paths` prints one path per line. |
| One value | `dotvault get KEY` prints the bare value and nothing else, so `$(...)` works. It fails, and prints nothing, if the name is not set. |
| Gating | `dotvault diff A B` and `dotvault find KEY` set the exit codes in [Filters and exit codes](#filters-and-exit-codes). |
| Parallel writes | Several `set`, `unset`, `revert` or `pull` commands may run at the same time on one file: they take turns, and none loses its change. Reading never waits. |
| Accounts without prompts | The `DOTVAULT_*` variables in [Configuration](#configuration). |

```bash
dotvault ls --paths -k 'STRIPE_*'                               # files that define a variable
dotvault ls --json | jq -r '.[] | select(.project == "acme-api") | .path'
dotvault diff acme-api:example acme-api:local                   # example variables your file lacks
for e in local staging production; do                           # one change, every environment
  dotvault set API_URL=https://api.example.com -p acme-api -e "$e"
done
DB=$(dotvault get DATABASE_URL -p acme-api -e production)
```

## Configuration

Override the locations with `DOTVAULT_CONFIG_DIR` and `DOTVAULT_DATA_DIR`.
`dotvault config` prints the exact paths on your machine.

| | Linux | macOS | Windows |
| :--- | :--- | :--- | :--- |
| `config.toml`, `session.json` | `~/.config/dotvault` | `~/Library/Application Support/dotvault` | `%APPDATA%\dotvault\config` |
| `index.db`, `history.db`, `locks/` | `~/.local/share/dotvault` | `~/Library/Application Support/dotvault` | `%APPDATA%\dotvault\data` |

| File | What it is |
| :--- | :--- |
| `index.db` | A cache of file locations and variable names (never values). It refreshes itself; run `dotvault scan` to pick up new folders and `dotvault ls --refresh` to pick up new files. |
| `history.db` | The [history](#history-of-a-variable). Not a cache, never deleted on its own. |
| `locks/` | A few small empty files. Commands that change an env file hold one while they work, so two of them never overwrite each other. Nothing is added to your project folders. |
| `session.json` | Your login, in a `0600` file. |

### config.toml

| Key | Default | Meaning |
| :--- | :--- | :--- |
| `server` | unset | Saved on your first `register` or `login`. |
| `roots` | empty | Folders to search. The first scan saves the whole computer here; managed by `scan` and `roots`. |
| `roots_auto` | unset | Set by the first scan: `true` while dotvault chose the roots, `false` once you name one. |
| `max_depth` | `10` | How many directory levels a scan descends. |
| `auto_scan_secs` | `600` | Rescan when the index is older than this. |
| `ignore` | `[]` | Extra folder names to skip. |
| `example_suffixes` | `["example", "sample", "template", "dist"]` | File-name parts that mark an example file. |
| `marked` | `[]` | Files you marked as secrets (`dotvault mark`). |
| `unmarked` | `[]` | Files you said are not secrets (`dotvault unmark`). |
| `history` | `true` | Keep a history of variables dotvault changes (plain text, owner-only). |
| `history_keep` | `50` | Versions kept per variable. |

`node_modules`, `.git`, `target`, `vendor`, `dist`, `build`, `.venv` and similar
folders are always skipped.

### Environment variables

| Variable | Used for |
| :--- | :--- |
| `DOTVAULT_SERVER` | Server for `register`, `login` and `recover` (same as `--server`). Default: https://dotvault.synquic.tech. |
| `DOTVAULT_USERNAME`, `DOTVAULT_PASSWORD` | Non-interactive login or registration. |
| `DOTVAULT_NEW_PASSWORD`, `DOTVAULT_RECOVERY_KEY` | Non-interactive `passwd` and `recover`. |
| `DOTVAULT_TOKEN` | A session token for scripts, with `DOTVAULT_SERVER`. It can share and open but has no vault key. |
| `DOTVAULT_HOME` | Home folder, used to shorten paths to `~` and to skip hidden folders inside it, for containers and tests. |
| `DOTVAULT_SCAN_ROOTS` | Folders to search when none are saved yet, instead of the whole computer (a `PATH`-style list). |
| `DOTVAULT_CONFIG_DIR`, `DOTVAULT_DATA_DIR` | Where the config and the data (index, history, locks) live. |
| `VISUAL`, `EDITOR` | The editor for `edit`. |
| `NO_COLOR` | Disable colors. |
| `DOTVAULT_VERSION`, `DOTVAULT_INSTALL_DIR`, `DOTVAULT_NO_MODIFY_PATH` | Options of the [installers](#install). |
| `DOTVAULT_BIND`, `DOTVAULT_DB`, `DOTVAULT_PUBLIC_URL`, `DOTVAULT_TRUST_PROXY`, `DOTVAULT_SERVER_SECRET` | Settings of `dotvault-server`, not of the CLI. See [deploy.md](deploy.md). |

Environment variables are visible to other processes of the same user, so prefer the
interactive prompts for everyday use.

## Self-hosting

```bash
cargo run --release -p dotvault-server        # listens on 127.0.0.1:8080
dotvault register --server http://localhost:8080
```

Pass `--server` on the first `register` or `login`: without it the CLI would use the
hosted server. After the first success it remembers the address.

`dotvault-server` takes `--bind ADDR` (default `127.0.0.1:8080`) and `--healthcheck`,
which exits `0` if a running server answers; everything else is set through the
`DOTVAULT_*` variables in [deploy.md](deploy.md).

For real use you need a Linux machine, a domain name and a TLS proxy (the client
refuses plain `http://` except for localhost), plus backups of the database.
[deploy.md](deploy.md) has the requirements and step-by-step setup with systemd and
Caddy, and Docker and Fly.io alternatives.

## Security

| Topic | Detail |
| :--- | :--- |
| Encryption | Cloud data is encrypted on your machine (Argon2id, HKDF, XChaCha20-Poly1305). The server never receives your password, your vault key or a share key. |
| Shares | A share's key lives in the link or code, not on the server. |
| Output | Text from shares, servers and files is sanitized before it is printed, shell output only includes plain variable names, and written files are safe to `source`. |
| Recovery key | The only way back in if you forget your password. If you lose both, the cloud data cannot be recovered. |
| Session | Kept in a `0600` file, not the OS keychain. |
| Review | The design has not been independently reviewed. |
| Default server | Run by the maintainer. It cannot read your data, but it sees account metadata (usernames, timing, sizes). Use `--server` with your own server to avoid that. |

Details and known limits are in [security-model.md](security-model.md). Report
vulnerabilities as described in [SECURITY.md](../SECURITY.md).
