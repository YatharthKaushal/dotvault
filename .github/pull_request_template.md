## What changed

<!-- A short description of the change and why it's needed. -->

## Related issue

<!-- Closes #123, or "none" for small, self-evident changes. -->

## How it was tested

<!-- Platforms tested on, and what you ran. -->

- [ ] Linux
- [ ] macOS
- [ ] Windows

## Checklist

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` passes
- [ ] `cargo test --workspace --locked` passes
- [ ] No secret, token, key or plaintext variable is logged, stored on the server, or cached in the index
- [ ] Text from shares, servers or files is sanitized before it is printed
- [ ] Docs updated if behavior changed (README, `docs/`, `ARCHITECTURE.md`)
