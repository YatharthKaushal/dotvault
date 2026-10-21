# Contributing to dotvault

Thanks for your interest in dotvault. This document covers how to get the project
running locally and what is expected of a pull request.

dotvault is in **early development**. Module boundaries and the on-disk and wire
formats may change without notice.

---

## Prerequisites

| Tool | Version | Notes |
| :--- | :--- | :--- |
| **Rust** | 1.85+ | Install with [rustup](https://rustup.rs/) |
| **A C compiler** | — | The bundled SQLite and the TLS library build C code |

Plus, depending on your platform:

- **Linux** — `build-essential` (Debian / Ubuntu), `gcc` (Fedora), or `base-devel` (Arch).
- **macOS** — Xcode Command Line Tools (`xcode-select --install`).
- **Windows** — [Build Tools for Visual Studio](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "Desktop development with C++" workload.

Optional:

| Tool | For |
| :--- | :--- |
| [`just`](https://github.com/casey/just) | Shortcuts in the `justfile` |
| [`cargo-dist`](https://github.com/axodotdev/cargo-dist) | Changing the release pipeline |
| Docker | Building the server image |

---

## Setup

```bash
git clone https://github.com/YatharthKaushal/dotvault.git
cd dotvault
cargo build --workspace
```

The first build compiles the dependencies and takes a few minutes. The
workspace's dev profile optimizes dependencies, so the password hashing in the
tests is fast.

## Running

```bash
# the CLI — use throwaway directories so you don't touch your real settings
export DOTVAULT_CONFIG_DIR=/tmp/dv-config DOTVAULT_DATA_DIR=/tmp/dv-data
cargo run -p dotvault -- scan ~/code
cargo run -p dotvault -- ls

# the server, on 127.0.0.1:8080
cargo run -p dotvault-server

# then, in another terminal, use it
cargo run -p dotvault -- register --server http://localhost:8080
```

`just cli ls` and `just server` do the same.

## Building

```bash
# optimized binaries land in target/release/
cargo build --release -p dotvault -p dotvault-server
```

## Checks

Run these before opening a pull request — CI runs the same set (the tests on
Linux, macOS and Windows).

```bash
cargo fmt    --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test   --workspace --locked
```

`just lint` and `just test` wrap them.

The CLI's end-to-end tests start a real server on a free port and run the real
binary with several users, so they take a few seconds and need no setup. A few
tests run a real `sh` and only run on Unix.

---

## Project layout

| Path | Contains |
| :--- | :--- |
| `crates/dotvault-core/` | `.env` parser, crypto, share codes, API types — shared by CLI and server |
| `crates/dotvault-cli/src/commands/` | One module per command group |
| `crates/dotvault-cli/src/` | Scanning, index, filters, HTTP client, session, terminal output |
| `crates/dotvault-cli/tests/` | End-to-end tests of the local and cloud commands |
| `crates/dotvault-server/src/` | axum routes, auth, rate limiting, janitor |
| `crates/dotvault-server/migrations/` | SQLite schema |
| `docs/` | Security model, deployment guide |
| `deploy/` | Dockerfile, Fly.io and Litestream templates |

See [ARCHITECTURE.md](ARCHITECTURE.md) for how the pieces fit together.

### Rules that keep the project trustworthy

- **The server must never be able to decrypt anything.** Don't add a column,
  log line or endpoint that carries a password, a vault key, a share key or a
  plaintext variable.
- **Never log secrets.** Not values, tokens, keys or request bodies — the server
  test suite fails if request URIs are logged.
- **Values are masked unless `--reveal` is passed**, in every output format, and
  the local index stores variable *names* only.
- **Treat anything from another person as hostile.** Text from a share, a server
  or a file in a cloned repository goes through `output::sanitize` before it is
  printed; names are validated before they reach a shell or a file.
- **Don't change key derivation casually.** `crypto.rs` pins its outputs in
  known-answer tests, because changing them locks every existing account out.
  A change needs a new format version, not an edited test.
- **Keep the editor lossless.** `DotEnv::parse(s).to_string() == s` must hold for
  every input; the property tests check it.

---

## Pull requests

1. **Fork** the repository.
2. **Create a feature branch** off `main`.
3. **Keep commits focused** and descriptive — one logical change per commit.
4. **Follow the existing project structure.**
5. **Run the checks above** before submitting.
6. **Open a PR** with a clear description of what changed and why.

> [!TIP]
> For larger features or architectural changes, open an issue first so the
> implementation can be discussed before development begins. This avoids
> throwing away work that doesn't fit the direction of the project.

### Commit messages

No strict format enforced. Prefer an imperative subject under ~72 characters,
with a body explaining *why* when the change isn't self-evident.

```
Refuse to open a share without an action

Opening a share uses up one of its allowed opens, so `open` with none of
--reveal, --write or --save would burn a view and show nothing.
```

---

## Code of Conduct

By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Reporting security issues

Please don't open a public issue for a vulnerability — see the
[Security Policy](SECURITY.md).

## Licensing of contributions

dotvault is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE),
at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work by you, as defined in the
Apache-2.0 license, is dual licensed as above, without any additional terms or
conditions.
