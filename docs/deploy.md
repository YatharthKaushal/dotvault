# Running your own server

The server is one binary (`dotvault-server`) with SQLite embedded. It needs a
data directory and a TLS-terminating proxy in front of it. Nothing else.

> **Status of this page.** The server itself is covered by an automated test
> suite. The container image, `fly.toml`, the Litestream config and the Caddy
> snippet below were written carefully but **have not been built or run in this
> repository's CI yet** (no container runtime was available while writing them).
> Try them on a throwaway instance first.

## Configuration

All settings are environment variables.

| Variable | Default | Meaning |
|---|---|---|
| `DOTVAULT_BIND` | `127.0.0.1:8080` | Address to listen on (`--bind` overrides it). The image sets `0.0.0.0:8080`. |
| `DOTVAULT_DB` | `./dotvault.db` | SQLite file. The image sets `/data/dotvault.db`. |
| `DOTVAULT_SERVER_SECRET` | generated once, kept in the database | At least 16 characters. Keys the fake salts that stop `prelogin` from revealing which usernames exist. Set it only if you want it stable across a database restore. |
| `DOTVAULT_PUBLIC_URL` | unset | Shown on the share landing page. |
| `DOTVAULT_TRUST_PROXY` | off | `1` when behind a proxy that appends `X-Forwarded-For`. The **last** entry is used as the client address for rate limiting. Leave it off when exposed directly, or clients can spoof their address. |
| `RUST_LOG` | `info,sqlx=warn` | Log level. Logs never contain bodies, tokens or real paths. |

Limits (compiled in): share ≤ 64 KiB, vault item ≤ 256 KiB, 20 versions kept per
item, sessions last 30 days of inactivity, shares live 1 minute to 30 days.

## TLS is required

The server speaks plain HTTP. Put it behind TLS, and use `https://` in the
client: the CLI refuses a non-HTTPS server except on `localhost`.

Caddy (automatic certificates):

```
vault.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

With Caddy in front, set `DOTVAULT_TRUST_PROXY=1`, and bind the server to
`127.0.0.1` so it is reachable only through the proxy.

## Docker

```sh
docker build -f deploy/Dockerfile -t dotvault-server .
docker run -d --name dotvault \
  -p 127.0.0.1:8080:8080 \
  -v dotvault-data:/data \
  -e DOTVAULT_PUBLIC_URL=https://vault.example.com \
  -e DOTVAULT_TRUST_PROXY=1 \
  dotvault-server
```

The image is distroless, runs as a non-root user, and has a built-in health
check (`dotvault-server --healthcheck`, which probes `/healthz`).

## Fly.io

`deploy/fly.toml` is a starting point:

```sh
fly launch --no-deploy --copy-config --config deploy/fly.toml --dockerfile deploy/Dockerfile
fly volumes create dotvault_data --size 1
fly deploy --config deploy/fly.toml --dockerfile deploy/Dockerfile
```

SQLite lives on a single volume, so run **exactly one machine**; the config pins
that. Check what `X-Forwarded-For` looks like on your platform before relying on
`DOTVAULT_TRUST_PROXY`.

## Backups

The database holds every account and all ciphertext. Losing it loses everyone's
cloud vault, and **nobody can recover it for you**: the data is encrypted with
keys the server does not have. Back it up.

* **Simple:** copy it consistently with SQLite's online backup, e.g.
  `sqlite3 /data/dotvault.db ".backup '/backups/dotvault-$(date +%F).db'"`.
  Do not `cp` the main file alone while the server is running; the write-ahead
  log (`-wal`) may hold recent commits.
* **Continuous:** [Litestream](https://litestream.io) streams changes to S3-compatible
  storage. `deploy/litestream.yml` is a template. Run it as a sidecar that
  mounts the same volume (the server image has no shell, so it cannot start
  Litestream itself).
* A backup contains no plaintext secrets, but it does contain password
  verifiers: store it with the same care as the live database.

## Operating notes

* **One instance.** SQLite means no horizontal scaling. The server idled at about
  5 MiB of memory in testing, including after a session of registrations, pushes
  and shares; a single small instance should serve a team comfortably, but it has
  not been load-tested.
* **Upgrades:** the schema is migrated automatically on start. Take a backup
  first.
* **Resetting a user's password is not possible.** Users recover with their
  recovery key (`dotvault recover`); without it, their cloud data is gone.
* **Quotas:** there is no per-user storage quota yet. If you open registration
  to strangers, put a limit at the proxy.
