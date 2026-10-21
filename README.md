# dotvault

One place for all your `.env` files: find them, read them, edit them, keep an
encrypted copy in the cloud, and share chosen variables with a teammate without
pasting secrets into chat.

* **Local, no account:** list every `.env*` file across your projects, search a
  variable across all of them, edit one safely (comments and layout survive),
  diff two environments.
* **Cloud vault (end-to-end encrypted):** `push` a file, `pull` it on another
  machine. Encryption happens on your computer; the server only stores
  ciphertext.
* **Sharing:** `share` hands out a link or a short code for just the variables
  you pick. The recipient needs an account, and you can see who opened it.
* **Small and fast:** a single native binary (under 5 MB), about 2 ms to
  start, about 12 ms to list hundreds of env files from its index, under 10 MB of
  memory.

An account is needed only for cloud features. Everything in the first group works
offline without one.

## Quick start

```sh
# 1. Tell dotvault where your projects live (once). It remembers the folder.
dotvault scan ~/code

# 2. Look around. Example files (.env.example, ...) stay hidden unless asked for.
dotvault ls
dotvault ls -p api -e production        # filter by project / environment
dotvault ls -k DATABASE_URL             # files that define a variable
dotvault ls --examples                  # include .env.example and friends

# 3. Read. Values are masked unless you say --reveal.
dotvault show acme-api:production
dotvault show acme-api --reveal --keys DATABASE_URL
dotvault find 'AWS_*'                   # every file that defines it, and which values differ
dotvault diff acme-api:production acme-api:example

# 4. Edit. Only the lines you touch change; comments and ordering are kept.
dotvault set API_KEY=new-value -p acme-api -e production
dotvault set SECRET                     # no '=': prompts, so it stays out of shell history
dotvault unset OLD_KEY -f ./.env
dotvault edit acme-api:local            # opens $VISUAL / $EDITOR
```

A target is a path (`./.env`, `~/x/.env.local`), a project (`acme-api`), or
`project:env` (`acme-api:production`). With no target, `./.env` is used.

### The cloud vault and sharing

```sh
dotvault register --server https://vault.example.com   # shows your recovery key ONCE
dotvault login

dotvault push acme-api:production               # stored as "acme-api/production"
dotvault vault ls
dotvault pull acme-api/production --out .env --merge     # keeps your comments and extra variables

# Share two variables, for an hour, openable twice:
dotvault share acme-api:production --keys DATABASE_URL,REDIS_URL --expires 1h --max-views 2
#   prints a link, and a short code:  dv1.<id>.<key>

# The recipient (who also has an account):
dotvault open dv1.AbCd….xyz --write .env      # or --reveal to print, or --save to keep in their vault

# You, afterwards:
dotvault shares ls
dotvault shares log <id>                      # who opened or saved it, and when
dotvault shares revoke <id>
```

`share -i` lets you tick variables from a list, `share --vault NAME` shares a
vault item instead of a file, and `open` refuses to run without `--reveal`,
`--write` or `--save` so it cannot waste a limited share.

Forgot your password? `dotvault recover` uses the recovery key. There is no other
way back in: that is what end-to-end encryption means. Read
[docs/security.md](docs/security.md) before you rely on it.

## Install

Nothing is published yet. From a checkout:

```sh
cargo install --path crates/dotvault-cli      # installs the `dotvault` binary
dotvault completions bash > ~/.local/share/bash-completion/completions/dotvault
```

A release pipeline ([cargo-dist](https://github.com/axodotdev/cargo-dist), see
`dist-workspace.toml` and `.github/workflows/release.yml`) is configured: pushing
a version tag such as `v0.1.0` should build archives for Linux, macOS (Intel and
Apple Silicon) and Windows, plus a shell and a PowerShell installer, and attach
them to a GitHub release. It has not run yet, and before the first release the
`repository` URL in `Cargo.toml` must point at the real GitHub repository. Homebrew,
winget, Scoop, MSI and `.deb`/`.rpm` packages are not set up.

Shell completions are available for bash, zsh, fish, elvish and PowerShell.
Rust 1.85 or newer is required to build.

## Self-hosting the server

```sh
cargo run --release -p dotvault-server        # listens on 127.0.0.1:8080
```

Put it behind TLS for real use. See [docs/deploy.md](docs/deploy.md).

## Repository layout

```
crates/
  dotvault-core/     .env parser, crypto, share codes, API types (shared)
  dotvault-cli/      the `dotvault` command
  dotvault-server/   the sync and sharing server (axum + SQLite)
docs/                security model, deployment
deploy/              Dockerfile, Fly.io and Litestream templates
```

```sh
cargo test --workspace                                   # over 200 tests
cargo clippy --workspace --all-targets -- -D warnings
```

The CLI end-to-end tests start a real server and run the real binary with
several users, including the full share → open → save → audit → revoke story and
a scan of the server's database file to check that no secret appears in it.

## Status

Working and tested on Linux: everything above. Not done yet, or not verified:

* No published packages. The release workflow is configured but has never run.
* macOS and Windows builds are expected to work but have not been run.
* The Docker image, Fly config and Litestream template are untested.
* No OS keychain integration (the session is a `0600` file); no browser viewer
  for share links (the link opens a page that tells you the command to run);
  no second factor; no per-user quotas.
* The cryptographic design has not had an independent review.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in this work
by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
