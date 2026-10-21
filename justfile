default:
    @just --list

build:
    cargo build --workspace

test:
    cargo test --workspace

lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

cli *ARGS:
    cargo run -p dotvault -- {{ARGS}}

server:
    cargo run -p dotvault-server

release-build:
    cargo build --release -p dotvault
    ls -lh target/release/dotvault
