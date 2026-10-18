# Run the Rust companion service

Local desktop tracking works without this service. Accounts, shared teams, AI,
integrations, and MCP connect to this independent server. It does not connect to
Rize's backend. Source code does not provision a server or provider account.

## Build and start

Use the pinned Rust toolchain, a C compiler for bundled SQLite, and durable local
storage. Node/npm are unnecessary for running the service binary. From the
repository root:

```sh
cargo test --workspace --exclude rs-rize
cargo run --release -p rize-service --bin rize-service
```

For a deployed process, build once with
`cargo build --release -p rize-service --bin rize-service`, then run the resulting
`target/release/rize-service` binary (`rize-service.exe` on Windows). Keep its
environment and data path stable. Run one service process per SQLite database.

The default listener is `127.0.0.1:4791`. `GET /healthz` returns JSON with `ok:true`.
The default database is `data/rize-service.sqlite`, relative to the process working
directory. It uses WAL journaling and restrictive file permissions where supported.
Use an absolute `RIZE_DB_PATH` for a long-running deployment. Keep databases,
exports, session files, and secrets out of source control and ephemeral storage.

In the desktop **Team** page, set `http://127.0.0.1:4791`, register/sign in, and
create or join a team. Passwords contain 12–256 characters. Invitations produce
single-use codes for a specified email address; the server does not send email.
Review the descriptions and sharing consent before **Sync approved workspace**.
Automatic sync is a separate opt-in while the desktop is running. Conflicts stay
available for explicit review.

## Environment configuration

Set environment variables through the process manager or secret manager that
starts the binary, then restart after configuration changes. The binary does not
automatically read `.env` files.

| Variable | Default / meaning |
| --- | --- |
| `RIZE_DB_PATH` | `data/rize-service.sqlite`; use an absolute persistent path |
| `RIZE_SERVER_HOST` | `127.0.0.1`; non-loopback binding requires an HTTPS public origin |
| `RIZE_SERVER_PORT` | `4791`; `PORT` takes precedence |
| `RIZE_PUBLIC_ORIGIN` | Exact public HTTPS origin for remote use; no path/trailing slash |
| `RIZE_ALLOWED_ORIGINS` | Empty; comma-separated exact browser origins, HTTPS except loopback HTTP |
| `RIZE_DISABLE_REGISTRATION` | `1` rejects new registration |
| `RIZE_SECRET_KEY` | Base64-encoded random 32-byte provider credential encryption key |
| `RIZE_AI_BASE_URL` | Configured HTTPS OpenAI-compatible API base |
| `RIZE_AI_API_KEY` | Server-side AI credential |
| `RIZE_AI_MODEL` | Configured chat model |
| `RIZE_EMBEDDING_MODEL` | Configured model for semantic tag proposals |
| `RIZE_RUN_ROUTINES` | `1` runs enabled, consented, due routines; disabled otherwise |
| `RIZE_GOOGLE_CLIENT_ID` / `RIZE_GOOGLE_CLIENT_SECRET` | Confidential Google Web application credentials |
| `RIZE_MICROSOFT_CLIENT_ID` / `RIZE_MICROSOFT_CLIENT_SECRET` | Confidential Microsoft Web application credentials |
| `RIZE_MICROSOFT_TENANT` | Optional tenant; default `common` |

The OAuth configuration also accepts the retained `RIZE_PUBLIC_URL` alias when
`RIZE_PUBLIC_ORIGIN` is absent. That alias does not satisfy the HTTP server's
non-loopback binding check; use `RIZE_PUBLIC_ORIGIN` for deployment. OAuth accepts
loopback HTTP only for local testing.

Generate an encryption key once, for example with `openssl rand -base64 32`, and
save it in protected server secret configuration. Keep a secure separate backup.
Provider tokens use AES-256-GCM with team/provider-bound authenticated data. The
key is not stored in the database. Losing or changing it makes existing credentials
unreadable; reconnect providers after an intentional rotation. List/status APIs
do not return tokens.

A temporary PowerShell local setup can set a data path before starting:

```powershell
$env:RIZE_DB_PATH = Join-Path $PWD 'data\rize-service.sqlite'
cargo run --release -p rize-service --bin rize-service
```

That environment exists only in the current process/session. It is not durable
deployment or secret provisioning. Keep API keys and session tokens out of shell
transcripts, logs, screenshots, client bundles, and committed MCP configuration.

## Remote service and TLS

Run under a dedicated OS identity with write access to its data directory. The
binary serves HTTP; terminate HTTPS at a correctly configured reverse proxy.
Remote desktop connections require HTTPS. Plain HTTP is accepted only on loopback.

A same-machine proxy can forward to `127.0.0.1:4791` while retaining the default
listener. A container/private-network deployment can set
`RIZE_SERVER_HOST=0.0.0.0` and `RIZE_PUBLIC_ORIGIN=https://your-real-host`, with the
backend port restricted to the proxy. The origin variable does not create TLS,
certificates, DNS, or firewall rules.

Allow browser origins only when operating a browser client. Native Rust requests
do not require a CORS origin. Supplied unlisted origins are rejected, and wildcard
origins are not accepted. Bearer sessions do not rely on browser cookies.
Request bodies are capped at 1 MiB, individual records at 128 KiB, sync writes at
500 changes, and teams at 50,000 records. Pulls page through 1,000 scanned events,
including hidden records so cursors advance correctly. HTTP bodies, header counts,
provider responses, and authentication concurrency/rates are bounded. Rate limits
are process-local; enforce appropriate connection limits at the proxy too.
Untrusted forwarded headers are not used as verified client IPs.

Use one durable local volume for the SQLite database and its WAL. This is a
single-node application, not a multi-node service with shared network storage or
a managed high-availability deployment. Stop the foreground service with Ctrl+C
(or SIGTERM on Unix), and wait for graceful shutdown and the WAL checkpoint before
copying data.

## Permissions and accounts

| Role / flag | Existing scope |
| --- | --- |
| Owner | Team administration, finance and resourcing; may change another member's role/flags |
| Admin | Invite/manage permitted lower roles and shared work; team personal records are visible |
| Member | Own approved time, skills, routines, reports and context; shared taxonomy is read-only |
| Viewer | Read permitted records; no writes |
| Finance | Finance access for non-viewers; admin alone does not automatically grant it |
| Resourcing | Allocation access for non-viewers; admin alone does not automatically grant it |

The API enforces permissions independently of the UI. Owners/admins can read team
approved work; ordinary members see their own personal records. Self role edits,
admin escalation, and removal/demotion of the last owner are rejected. Invitations
expire after seven days, are single-use/email-bound, and depend on the inviter's
current permitted role.

Passwords use salted scrypt with `N=32768`, `r=8`, and `p=3`. Server storage retains
only SHA-256 hashes of opaque session tokens. Sessions expire after seven days;
logout revokes that session. Desktop persistence uses current-user Windows DPAPI,
with session-only fallback when protection is unavailable. There is no built-in
MFA, SSO, email verification/reset/delivery, or operator recovery service. Set
`RIZE_DISABLE_REGISTRATION=1` after onboarding if self-service registration should
close; reopen it deliberately when required.

## Shared data and providers

Only approved entry descriptions, intervals, and tags synchronize. The entry
schema rejects raw application/window metadata, URLs, paths, device details, and
pending activity. Approved descriptions can still contain sensitive text: review
them before sharing. Finance and allocations have separate permission gates.
Version conflicts retain both versions for review. Tombstones apply to shared IDs
and preserve unrelated private pending work.

The server operator can read shared records and account metadata. Team content is
not end-to-end encrypted; the credential key protects integration secrets rather
than the entire database. Configured AI receives permitted approved context and
the user's prompt/skill only with consent. Selected approved time can be exported
to supported integrations with separate consent. Saved routines require service
enablement and their author's explicit consent, and execute under that author's
permissions. Provider retention, pricing, scopes, and plan requirements still apply.

[PROVIDERS.md](PROVIDERS.md) covers AI, Linear/ClickUp/Asana, calendar import,
Google/Microsoft OAuth callback registration, and provider bounds. Existing
callbacks are `/v1/oauth/google/callback` and `/v1/oauth/microsoft/callback` under
`RIZE_PUBLIC_ORIGIN`. Restarting the service expires unfinished OAuth flows.
Synthetic tests do not verify live vendor approval or credentials.

## Backups, upgrades, and migration

Back up desktop JSON separately. Before a service upgrade, stop it, back up the
complete SQLite data directory including any remaining `-wal`/`-shm` files, and
retain the matching `RIZE_SECRET_KEY` separately. A raw copy of a running WAL
database is not guaranteed consistent; use SQLite's online backup mechanism when
downtime is unacceptable.

Restore only while stopped, with appropriate filesystem permissions and the
matching key. Test restoration on an isolated service. Restoring older data can
invalidate sync cursors; keep local exports and deliberately review a full sync.
Preserve conflict state and uncertain provider-write journals. Repeating an
uncertain time export without checking the vendor can create a duplicate.

For an upgrade, back up, check out the intended revision, run the Rust tests,
rebuild, and restart using the same persistent path. The Rust port retains the
v1.2 SQLite account/sync schema, Node-compatible password/session formats, and
provider encryption envelope. [MIGRATION.md](MIGRATION.md) gives the transition
steps. There are no arbitrary downgrade migrations, encrypted whole-database
backups, multi-node orchestration, or a hosted recovery SLA.

## MCP process

Provide `RIZE_SERVER_URL` (default `http://127.0.0.1:4791`), `RIZE_TEAM_ID`, and
`RIZE_SESSION_TOKEN` in the MCP client's protected environment, then launch:

```sh
cargo run --release -p rize-service --bin rize-mcp
```

For deployment, build the `rize-mcp` binary and configure the MCP client to launch
it directly. Obtain a session by authenticating to your own service; the desktop
does not expose its token. Token revocation/expiry also revokes MCP access. This
stdio MCP process reads approved records and analytics under the account's
permissions; it cannot mutate work or access raw tracker metadata. It is not a
public HTTP MCP endpoint. See [PROVIDERS.md](PROVIDERS.md) for protocol and read
limits.
