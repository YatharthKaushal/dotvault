# Contributing

dotvault is in early development, so module boundaries and the on-disk and wire
formats may change.

## Requirements

- Rust 1.89 or newer ([rustup](https://rustup.rs/)).
- A C compiler, because the bundled SQLite and the TLS library build C code:
  `build-essential` / `gcc` / `base-devel` on Linux, Xcode Command Line Tools on
  macOS, the Visual Studio C++ Build Tools on Windows.
- Optional: [`just`](https://github.com/casey/just) for the shortcuts in the
  `justfile`, and `cargo-dist` if you change the release pipeline.

## Build, run, test

```bash
git clone https://github.com/YatharthKaushal/dotvault.git
cd dotvault
cargo build --workspace

# run the CLI against throwaway directories, not your real settings
export DOTVAULT_CONFIG_DIR=/tmp/dv-config DOTVAULT_DATA_DIR=/tmp/dv-data
cargo run -p dotvault -- ls
cargo run -p dotvault-server            # server on 127.0.0.1:8080

# checks (CI runs the same; tests run on Linux, macOS and Windows)
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

`just lint` and `just test` wrap the checks. The CLI's end-to-end tests start a
real server on a free port and run the real binary, so they need no setup. A few
tests run a real `sh` and only run on Unix.

`install.sh` and `install.ps1` are tested by CI against a release built on the
spot. To run the shell installer's suite yourself, point it at a folder holding
`dotvault-<target>.tar.xz` and its `.sha256` for your machine:
`bash scripts/test-install-sh.sh DIR`.

## Layout

| Path | Contains |
| :--- | :--- |
| `crates/dotvault-core/` | `.env` parser, crypto, share codes, API types, shared by CLI and server |
| `crates/dotvault-cli/` | The `dotvault` binary; `src/commands/` has one module per command group |
| `crates/dotvault-server/` | axum routes, auth, rate limiting, migrations |
| `install.sh`, `install.ps1`, `scripts/` | The installers and the test for the shell one |
| `site/` | The download page, published to GitHub Pages |
| `docs/`, `deploy/` | Security model and deployment guide; Dockerfile and templates |

See [ARCHITECTURE.md](ARCHITECTURE.md) for how the pieces fit together.

## Rules

- The server must never be able to decrypt anything. Do not add a column, log
  line or endpoint that carries a password, vault key, share key or plaintext
  variable.
- Never log secrets: values, tokens, keys or request bodies. A server test fails
  if request URIs are logged.
- Values are masked unless `--reveal` is passed, in every output format, and the
  local index stores variable names only.
- Treat text from another person as hostile. Anything from a share, a server or a
  file in a cloned repository goes through `output::sanitize` before it is
  printed, and names are validated before they reach a shell or a file.
- Do not change key derivation casually. `crypto.rs` pins its outputs in
  known-answer tests because changing them locks every existing account out. A
  change needs a new format version, not an edited test.
- Keep the `.env` editor lossless: `DotEnv::parse(s).to_string() == s` for every
  input.

## Pull requests

Branch off `main`, keep commits focused, run the checks above, and describe what
changed and why. For larger changes, open an issue first. There is no commit
message format; prefer an imperative subject under about 72 characters.

By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md). Report
vulnerabilities as described in [SECURITY.md](SECURITY.md), not in public issues.

## Licensing

dotvault is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE),
at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work by you, as defined in the
Apache-2.0 license, is dual licensed as above, without any additional terms or
conditions.
