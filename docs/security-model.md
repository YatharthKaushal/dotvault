# Security model

What dotvault protects, what it does not, and the known limits.

## Summary

Cloud data (your vault and shares) is encrypted **on your machine** before it
is uploaded. The server stores ciphertext and hashes of derived secrets. It
never receives your password, your vault key or a share key, so a stolen
database, a curious operator or a network observer cannot read your variables.
It cannot protect you from malware on the machine where you run `dotvault`: your
`.env` files are plaintext there by nature.

## Keys

```
password ──Argon2id(salt, 64 MiB, t=3, p=4)──▶ master
                                                 │
                         HKDF("dotvault/v1/auth")├──▶ auth_key   sent to the server, stored as SHA-256(domain ‖ auth_key)
                         HKDF("dotvault/v1/enc") └──▶ enc_key    never leaves your machine

vault_key       random 32 bytes; stored on the server only as XChaCha20-Poly1305(enc_key, vault_key)
recovery_key    random 32 bytes, shown once at registration; wraps vault_key a second time
share_key       random 32 bytes per share; lives only inside the share link/code
```

* **Cipher:** XChaCha20-Poly1305 with random 24-byte nonces. Sealed blobs are
  `version ‖ nonce ‖ ciphertext ‖ tag`, and the version byte is authenticated.
* **Context binding (AAD):** a vault item's payload is bound to its id and
  version, and its name to its id. The server therefore cannot swap ciphertexts
  between items or silently serve an older version as a newer one.
* **Password check:** the server verifies `auth_key`, which is a 256-bit KDF
  output. A fast hash is the right way to store a value like that; there is
  nothing left to brute-force except the password itself, and that costs one
  Argon2id evaluation per guess (see *Limits*).
* **KDF parameters come from the server**, so the client bounds them
  (≥ 19 MiB, ≥ 2 passes, ≤ 1 GiB) and refuses anything outside that range. A
  hostile server can neither weaken the derivation nor exhaust your memory.
* The derivations are pinned by known-answer tests that were cross-checked
  against an independent Argon2/HKDF implementation, because changing them
  would lock every existing account out.

## Sharing

`dotvault share` encrypts the chosen variables with a fresh `share_key`, uploads
the ciphertext, and prints a link `https://host/s/<id>#<share_key>` (also as the
code `dv1.<id>.<share_key>`).

* The key is in the URL **fragment**, which browsers never send to a server,
  and in the code form it never touches a URL at all. The server cannot decrypt
  a share.
* The recipient must be logged in to open it. The server records who opened it
  and when; `dotvault shares log` shows the list.
* Shares expire (default 24 h, at most 30 days) and can have an open limit. The
  server counts opens atomically, so concurrent opens cannot exceed it.
* The share is selective: only the variables you pick are in the ciphertext.
  Unselected variables are never uploaded in any form.
* `dotvault open` refuses to run without `--reveal`, `--write` or `--save`,
  because opening consumes one of the allowed opens.
* A link for a **different server** than the one you are logged in to is
  refused, so a crafted link cannot make the CLI send your session token to
  someone else's server.

What sharing cannot do:

* **Revoking does not un-share.** Revoking deletes the ciphertext, so the link
  stops working, but anyone who already opened it holds the plaintext.
* **Anyone with the whole link and an account can open it.** Treat the link like
  the secret it is, and send it over a different channel than the one you would
  have used for the secret itself.
* **"Saved" in the audit log is self-reported.** The server can verify an open
  because it served the ciphertext; it cannot see what a client does afterwards.
* The server enforces expiry and open limits. A malicious server could ignore
  them, but still could not read the content.
* Opening your own share, or opening it with a mistyped key, uses up an open.

## Receiving content from other people

A share is data chosen by someone else, so `dotvault open` treats it as hostile:

* **Terminal escape injection.** Variable names, values, the share's source label
  and note, item names, paths and server messages are all untrusted. Before
  anything is printed, control characters (ESC, BEL, CR, DEL and the C1 range)
  and Unicode direction overrides are replaced by visible escapes such as
  `\u{1b}`, so a value cannot clear the screen, retitle the window, write to the
  clipboard (OSC 52) or reorder what you read. Machine formats (`--format
  dotenv|shell|json`, `pull --stdout`) emit exact data when piped, and the same
  escaping when stdout is a terminal.
* **Shell injection through output.** `--format shell` and `--format powershell`
  only emit variables whose names are plain identifiers (`[A-Za-z_][A-Za-z0-9_]*`)
  and quote every value; anything else is left out with a warning, and raw key
  text is never echoed. `--format dotenv` and `--write` accept only valid
  variable names.
* **Shell injection through the file.** Many people `source .env`. When dotvault
  writes a value, it leaves it unquoted only if every character is inert to a
  shell, prefers single quotes for anything containing `$`, `` ` ``, `\` or `"`,
  escapes `$` and `` ` `` inside double quotes, and never produces backtick
  quoting. A test sources the written files in a real `sh` with hostile values
  (`$(…)`, backticks, quote-breaking, `;`/`&`/`|`) and checks that nothing runs
  and every value reads back exactly.
* **Token exfiltration.** A link that points at a different server than the one
  you are logged in to is refused before any request is made.

What this does not cover: a hostile value is still *a value you asked to
receive*. If you copy `DATABASE_URL` from a stranger into production, that is
on you. And a value written to a file is only inert to shells that follow POSIX
quoting; other tools that read `.env` files have their own rules.

## Recovery

Your recovery key (`DVR1-…`) is the only way back in if you forget your
password, because the server cannot reset what it cannot read. It is shown once
at registration. **Anyone holding it can take over the account**, so store it
like a password-manager emergency sheet, not next to your password.
`dotvault recover` uses it to set a new password, ends every other session, and
keeps your vault readable. Lose both the password and the key, and the cloud
data is unrecoverable by design. Your local files are never affected.

## What is stored on your machine

| File | Contents | Protection |
|---|---|---|
| `config.toml` | server URL, scanned folders, settings | none needed, no secrets |
| `index.db` | paths and **variable names** of env files, never values | disposable cache |
| `session.json` | server, username, session token, wrapped vault key, and (by default) the vault key | mode `0600` on Unix, written atomically; a warning is printed if it is loosened |

* The session token can be revoked: `dotvault logout` revokes it on the server.
* Caching the vault key is a convenience. `dotvault login --no-cache-key` keeps
  it off disk; vault commands then ask for your password and unwrap it locally.
  Sharing and opening never need the vault key.
* **No OS keychain yet.** Storing the session in macOS Keychain / Windows
  Credential Manager / Secret Service is planned. The current design is the same
  one `gh`, `npm` and `aws` use for their tokens. The tool manages plaintext
  `.env` files on the same disk, so someone who can read your home directory
  already has those.
* `DOTVAULT_TOKEN` + `DOTVAULT_SERVER` give scripts a session that can share and
  open but has no vault key. `DOTVAULT_PASSWORD` and friends exist for
  automation; environment variables are visible to other processes of the same
  user, so prefer the interactive prompt for everyday use.

Local edits rewrite only the lines they change, writes are atomic (temp file +
rename), new files are created `0600`, symlinked env files stay symlinks, and
non-UTF-8 files are refused rather than corrupted.

## Server

* Everything stored is ciphertext, a hash of a high-entropy derived secret, or
  metadata (usernames, timestamps, sizes, share ids, who opened what).
* Sessions are 32 random bytes; only their SHA-256 is stored. They expire after
  30 days of inactivity.
* Unknown usernames get a stable fake salt from `prelogin`, and wrong password
  and unknown user produce the same answer, so login does not reveal which
  accounts exist. (Registration necessarily does: a taken name is reported.)
* Rate limits per client address: 20/min for register, prelogin, login and
  recovery; 60/min for opening shares.
* The access log records the method, the route *template*, status and latency,
  never bodies, tokens, query strings or real paths. A test fails if request
  URIs are logged.
* The share landing page is static, makes no network requests, and sets a strict
  CSP, `Referrer-Policy: no-referrer` and `no-store`.
* **The server speaks plain HTTP and must run behind TLS** (a reverse proxy or
  your platform's edge). The CLI will not talk to a non-`https://` server except
  on loopback.

## Limits and known gaps

* **Offline guessing.** If the database leaks, an attacker can test passwords
  against it at the price of one Argon2id evaluation (64 MiB, 3 passes) each.
  Use a long passphrase. This is the standard trade-off of this design.
* **Metadata is visible to the server:** usernames, when you sync, how big your
  items are and how many you have, share creation and opening times, and client
  IP addresses at the network layer.
* **No second factor**, no account deletion endpoint, no key rotation, and no
  per-user storage quota yet. Authenticated users are bounded per request, not
  in total.
* **Trust roots:** the CLI uses the bundled Mozilla root set, so a server with a
  private CA is not supported yet.
* **Not audited.** The crypto uses well-reviewed RustCrypto primitives and
  standard constructions, but the protocol and the code have had no independent
  review. Do not treat it as audited until they have.
* Only the Linux build has been exercised in testing so far; macOS and Windows
  builds are expected to work but are unverified.

## Reporting a vulnerability

Please don't open a public issue for a security problem. Follow the
[Security Policy](../SECURITY.md): report it privately through a GitHub Security
Advisory or by email.
