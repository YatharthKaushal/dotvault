# Security Policy

## Supported versions

dotvault is in early development and has no released versions. Only the `main`
branch is supported.

| Version | Supported |
| :--- | :--- |
| `main` | Yes |
| Everything else | No |

## Reporting a vulnerability

**Do not open a public issue for security vulnerabilities.**

Report privately through either channel:

- [GitHub Security Advisories](https://github.com/YatharthKaushal/dotvault/security/advisories/new) — preferred.
- Email **[theyatharthk@gmail.com](mailto:theyatharthk@gmail.com)**.

Please include:

- A description of the issue and its impact.
- Steps to reproduce, or a proof of concept.
- Affected commit or branch.
- Your platform and version (Linux / macOS / Windows), and whether it involves
  the CLI, the server, or both.

**Please don't include real secrets** — use throwaway values and revoke any share
links or sessions you created while testing.

## What to expect

- Acknowledgement within **7 days**.
- An assessment and planned fix timeline within **30 days**.
- Credit in the advisory, unless you prefer to remain anonymous.

## What is most valuable

dotvault's whole purpose is handling secrets, so reports in these areas matter
most:

- **Cryptography and key handling** — key derivation, wrapping, the recovery
  key, nonce use, anything that lets the server (or anyone without the password)
  read vault items or shares.
- **Share links** — a way to open a share without the key, to exceed an open
  limit or outlive an expiry or revocation, or to learn who opened one without
  being the owner.
- **Untrusted content** — escape sequences, shell metacharacters or file-system
  tricks in data received through `open` that reach the terminal, a shell or a
  file.
- **Authentication and sessions** — login, recovery, token handling, rate
  limiting, and account enumeration.
- **The server** — injection, cross-user data access, and anything that causes
  secrets to appear in logs or in the database in the clear.
- **Local files** — permissions on the saved session, and any case where `set`,
  `unset`, `pull` or `open --write` could corrupt, truncate or redirect a file.

The [security model](docs/security-model.md) describes what dotvault does and
does not protect, including the known limits — reports that go beyond those are
the most useful.
