# Hosting the server

`dotvault-server` is one small binary with SQLite built in. The `dotvault` CLI
talks to it over HTTPS. This guide sets it up on a Linux machine with systemd and
Caddy. Docker and Fly.io are covered at the end.

## What you need

| | |
| :--- | :--- |
| **A Linux machine** | Any x86_64 or aarch64 VM with systemd, such as Ubuntu 22.04+ or Debian 12+. 1 vCPU, 512 MB of RAM and 1 GB of disk are plenty: the server used about 5 MB of memory when idle (not load-tested). Only one instance can run, because the data is a single SQLite file. |
| **A domain name** | An A (and AAAA) record pointing at the machine. The TLS certificate is issued for it. |
| **Open ports** | 80 and 443 from the internet. The server itself listens on `127.0.0.1:8080` and should not be exposed. |
| **A TLS proxy** | The server speaks plain HTTP, and the CLI refuses plain `http://` except for `localhost`. This guide uses [Caddy](https://caddyserver.com/docs/install), which gets and renews a Let's Encrypt certificate by itself. The CLI trusts only public certificate authorities, so self-signed and private-CA certificates do not work. |
| **To build the binary** | Rust 1.85+ and a C compiler (`build-essential`) on the machine that builds it. There is no prebuilt server download yet. Docker is the alternative. |
| **For backups** | The `sqlite3` command (`sudo apt install sqlite3`) and somewhere else to keep the copies. |

## Steps

**1. Build the server.** On the server itself, or on any machine with the same OS
version (the binary links against the system's glibc). It takes a few minutes.

```bash
sudo apt update && sudo apt install -y build-essential git curl sqlite3
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"

git clone https://github.com/YatharthKaushal/dotvault.git
cd dotvault
cargo build --release --locked -p dotvault-server
```

**2. Install it** and create the user it runs as.

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin dotvault
sudo install -m 0755 target/release/dotvault-server /usr/local/bin/
```

**3. Run it as a service.** [`deploy/dotvault.service`](../deploy/dotvault.service)
keeps the data in `/var/lib/dotvault` (private to the `dotvault` user), restarts
the server if it stops, and restricts what it can touch. Set
`DOTVAULT_PUBLIC_URL` to your address, then start it:

```bash
sudo cp deploy/dotvault.service /etc/systemd/system/
sudo nano /etc/systemd/system/dotvault.service     # edit DOTVAULT_PUBLIC_URL
sudo systemctl daemon-reload
sudo systemctl enable --now dotvault

systemctl status dotvault
curl http://127.0.0.1:8080/healthz                 # prints: ok
```

**4. Put TLS in front.** [Install Caddy](https://caddyserver.com/docs/install),
put your domain in [`deploy/Caddyfile`](../deploy/Caddyfile), and load it. Open
the ports if you use a firewall (keep SSH allowed):

```bash
sudo cp deploy/Caddyfile /etc/caddy/Caddyfile
sudo nano /etc/caddy/Caddyfile                     # replace vault.example.com
sudo systemctl reload caddy

sudo ufw allow 80/tcp && sudo ufw allow 443/tcp    # if you use ufw
```

Caddy adds the client address to `X-Forwarded-For`, which the server uses for rate
limiting because the unit sets `DOTVAULT_TRUST_PROXY=1`. A client cannot dodge the
limit by sending its own `X-Forwarded-For`; the last entry, the one Caddy added,
is the one that counts. With another proxy, make sure it appends the address the
same way, and never set `DOTVAULT_TRUST_PROXY=1` when the server is reachable
directly.

**5. Check it from your own machine.**

```bash
curl https://vault.example.com/healthz              # prints: ok
dotvault register --server https://vault.example.com
dotvault whoami
```

`register` shows your recovery key once. Keep it somewhere safe.

Pass `--server` on that first `register` or `login`. Without it the CLI uses the
default hosted server, not yours. After the first success it saves your address in its
config, or you can set `DOTVAULT_SERVER`.

## Configuration

Settings are environment variables; the service file sets them. The server
refuses to start on a bad value and says which one.

| Variable | Default | Meaning |
| :--- | :--- | :--- |
| `DOTVAULT_BIND` | `127.0.0.1:8080` | Address to listen on (`--bind` overrides it). |
| `DOTVAULT_DB` | `./dotvault.db` | SQLite file. Its folder is created if needed. |
| `DOTVAULT_PUBLIC_URL` | unset | Shown on the share landing page. |
| `DOTVAULT_TRUST_PROXY` | off | Set to `1` behind a proxy that appends `X-Forwarded-For`. |
| `DOTVAULT_SERVER_SECRET` | generated once, kept in the database | At least 16 characters. Keys the fake salts that stop `prelogin` from revealing which usernames exist. Set it only if you want it stable across a restore onto a fresh database. |
| `RUST_LOG` | `info,sqlx=warn` | Log level. Logs never contain request bodies, tokens or real paths. |

Limits that are built in: a share is at most 64 KiB and a vault item 256 KiB, 20
versions are kept per item, sessions expire after 30 days of inactivity, and
shares live from 1 minute to 30 days. Rate limits per address are 20 requests a
minute for register, prelogin, login and recovery, and 60 for opening shares.

## Backups

The database holds every account and all the (encrypted) vault data. If you lose
it, nobody can recover it for you: the server does not have the keys.

The server creates the database with mode `0600`, and the unit keeps its folder
private. Keep backups just as private: they contain usernames and password
verifiers, which would let someone try to guess passwords offline.

Take a consistent copy while the server is running (do not just `cp` the file; the
`-wal` file can hold recent changes):

```bash
sudo install -d -m 0700 -o dotvault -g dotvault /var/backups/dotvault
sudo -u dotvault sqlite3 /var/lib/dotvault/dotvault.db \
  ".backup '/var/backups/dotvault/dotvault-$(date +%F).db'"
```

To do it every night and prune old copies, add `/etc/cron.d/dotvault-backup`
(`%` must be escaped in cron):

```
17 3 * * * dotvault sqlite3 /var/lib/dotvault/dotvault.db ".backup '/var/backups/dotvault/dotvault-$(date +\%F).db'" && find /var/backups/dotvault -name '*.db' -mtime +14 -delete
```

That protects against mistakes but not against losing the machine, so also copy
`/var/backups/dotvault` somewhere else (another host, object storage).
[Litestream](https://litestream.io) can stream the database to S3-compatible
storage continuously; [`deploy/litestream.yml`](../deploy/litestream.yml) is a
template.

**Restore:**

```bash
sudo systemctl stop dotvault
sudo -u dotvault cp /var/backups/dotvault/dotvault-2026-01-01.db /var/lib/dotvault/dotvault.db
sudo -u dotvault chmod 600 /var/lib/dotvault/dotvault.db
sudo rm -f /var/lib/dotvault/dotvault.db-wal /var/lib/dotvault/dotvault.db-shm
sudo systemctl start dotvault
```

Everything comes back as it was when the backup was taken. Anything created after
that is lost: new accounts, pushes and shares, and users who logged in since have to
log in again.

## Running it

- **Logs:** `journalctl -u dotvault -f`.
- **Upgrade:** back up, then `git pull`, rebuild, `sudo install -m 0755
  target/release/dotvault-server /usr/local/bin/` and `sudo systemctl restart
  dotvault`. The database schema is updated automatically on start.
- **Who can sign up:** anyone who can reach the server can create an account, and
  there are no per-user storage limits. If it is for you or a team, restrict who
  can reach it (a firewall allowlist, a VPN, or rules in your proxy) rather than
  leaving it open to the internet.
- **Passwords cannot be reset by you.** A user who forgets their password needs
  their recovery key (`dotvault recover`).
- **One instance only.** SQLite does not support several servers sharing a file.

## Docker

An image is not published yet; build it from the repository (Docker 23 or newer,
because the Dockerfile uses BuildKit cache mounts):

```bash
docker build -f deploy/Dockerfile -t dotvault-server .
docker run -d --name dotvault --restart unless-stopped \
  -p 127.0.0.1:8080:8080 -v dotvault-data:/data \
  -e DOTVAULT_PUBLIC_URL=https://vault.example.com \
  -e DOTVAULT_TRUST_PROXY=1 \
  dotvault-server
```

Then continue from step 4 (Caddy on the host forwards to `127.0.0.1:8080`). The
image is distroless, runs as a non-root user and has a built-in health check
(`dotvault-server --healthcheck`). Back up the `dotvault-data` volume's
`dotvault.db` the same way. CI builds this image and checks it on every change to
the server (`.github/workflows/docker.yml`).

## Fly.io

[`deploy/fly.toml`](../deploy/fly.toml) is a starting point. It needs a Fly.io
account and `flyctl`:

```bash
fly launch --no-deploy --copy-config --config deploy/fly.toml --dockerfile deploy/Dockerfile
fly volumes create dotvault_data --size 1
fly deploy --config deploy/fly.toml --dockerfile deploy/Dockerfile
```

Fly provides the TLS endpoint, so there is no Caddy step. Run exactly one machine.
This path has not been tried; check what `X-Forwarded-For` looks like on Fly
before relying on `DOTVAULT_TRUST_PROXY`.
