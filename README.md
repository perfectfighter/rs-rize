# rs-rize

Rust port of the existing Rize Local v1.2 Windows desktop application, based on
[`98f56dba6138280b64a7ea57c02b91b73a268b8a`](https://github.com/perfectfighter/rize-local-public/tree/98f56dba6138280b64a7ea57c02b91b73a268b8a).
The React interface is retained. Tauri and Rust replace the Electron desktop
process, Node application logic, companion HTTP service, and MCP process. This is
a migration of the existing feature set; it does not add product features.

The local tracker works without an account. Shared teams, AI, and integrations
still need your own companion service and provider configuration. This independent
project contains no proprietary Rize bundle, uses no private Rize backend, and is
not affiliated with Rize. The original repository remains separate.

## Run on Windows

Target: Windows 10/11 x64 with Microsoft Edge WebView2. To build from source,
install Node.js 22+, npm, Rust 1.99.1, and the MSVC C++ build tools with the Windows
SDK. The Cargo manifests declare Rust 1.90 as the minimum; the repository pins the
tested toolchain. See the official [Tauri Windows prerequisites](https://v2.tauri.app/start/prerequisites/#windows).

```sh
npm ci
npm test
npm run build
cargo test --workspace --exclude rs-rize
npm start
```

`npm start` runs the Tauri development desktop. `npm run dev` is a read-only browser
preview and does not receive native tracking, file, or account capabilities.
Node/npm are build and test tools; the installed desktop, service, and MCP runtime
are Rust binaries with no Node/Electron backend fallback.

Choose **Create my workspace** for empty local data or the labeled sample workspace
for exploration. Automatic tracking remains opt-in and is unavailable in sample
mode. [MIGRATION.md](docs/MIGRATION.md) explains moving existing v1.2 data.

## Existing functionality

- Opt-in Windows foreground tracking, idle and schedule handling, private-window
  redaction, exclusions, and local tagging rules.
- Calendar and manual entries; review, split, merge, bulk editing, labels, rule
  regeneration, and client/project/task assignments.
- Focus, meeting, and break timers; pause/resume, goals, extensions, and active
  interval checkpoints; local ambient audio and selected audio files.
- Timesheets, resource allocations and CSV import, contracts, separate-currency
  profitability, invoice PDF export, and manual payment records.
- Self-hosted accounts, teams, email-bound invitation codes, role/resource
  permissions, versioned sync, conflict review, and opt-in automatic sync.
- Configured AI chat and semantic tagging proposals, editable reports, skills,
  report routines, task/calendar integrations, and read-only MCP.
- Windows NSIS packaging and the existing explicit check/download/install update
  flow, implemented using Tauri's signed updater format.

Invoices still require manual delivery and external payment collection. Provider
credentials, a hosted server, signing certificates, and a published update feed
are not bundled. Existing provider scope and limits are documented in
[PROVIDERS.md](docs/PROVIDERS.md).

## Companion service and MCP

```sh
cargo run --release -p rize-service --bin rize-service
```

The default listener is `http://127.0.0.1:4791`; the database defaults to
`data/rize-service.sqlite`. Configure this address in the desktop **Team** page,
then sign in, create/join a team, and review consent before syncing approved work.
Invitation codes are not sent by email. Local-only operation requires no server.

[SELF-HOSTING.md](docs/SELF-HOSTING.md) covers TLS, durable storage, permissions,
backups, and server upgrades. [PROVIDERS.md](docs/PROVIDERS.md) covers AI and vendor
configuration. For MCP, supply `RIZE_SERVER_URL`, `RIZE_SESSION_TOKEN`, and
`RIZE_TEAM_ID` in the MCP client's protected environment, then run:

```sh
cargo run --release -p rize-service --bin rize-mcp
```

## Build the Windows installer

From a Windows x64 development environment:

```sh
npm ci
npm test
cargo test --workspace --exclude rs-rize
npm run package:windows
```

`package:windows` runs `tauri build --bundles nsis`. The installer is written under
`target/release/bundle/nsis/`. This uses the Tauri/WebView2 runtime rather than
shipping Electron. Production signing and publishing are separate publisher tasks.

The Windows workflow uploads `rs-rize-windows-installer` and
`rs-rize-windows-x64` artifacts after a successful build. The portable ZIP contains
`rs-rize.exe`, `rize-service.exe`, `rize-mcp.exe`, the complete `docs/` tree, notices,
and a source ZIP. Extract the whole folder and run `rs-rize.exe`; WebView2 must be
available. Workflow artifacts are retained for 30 days. This workflow does not
automatically publish a GitHub Release or deploy the service.

Unsigned development builds report self-updates as unsupported. A signed installed
NSIS release needs an HTTPS Tauri update feed, a matching updater public key, signed
update artifacts, and actual Windows Authenticode signing. The old Electron
`latest.yml` feed and installers cannot update this port. See
[the migration's update notes](docs/MIGRATION.md#updates-and-installers).

## Rust modules

| Path | Responsibility |
| --- | --- |
| `src/` | Retained React UI and narrow Tauri command/event bridge |
| `src-tauri/src/app.rs` | Desktop lifecycle, command boundary, dialogs, tray, and native coordination |
| `src-tauri/src/pdf.rs`, `updates.rs` | WebView2 invoice PDF export and signed installed updates |
| `crates/rize-core/src/state.rs`, `timer.rs` | Schema-1 JSON storage, mutations, import/export, and timer accounting |
| `crates/rize-core/src/business.rs` | Contracts, exact minor-unit billing, invoices, allocations, and financial invariants |
| `crates/rize-core/src/tracking.rs` | Sanitized samples, idle/schedule boundaries, and local rules |
| `crates/rize-native/` | Windows foreground/session events and current-user DPAPI |
| `crates/rize-cloud/` | Account transport, protected sessions, reviewed-work sync, pagination, and conflicts |
| `crates/rize-service/` | Axum HTTP API, SQLite, authorization, providers, calendars, routines, and MCP |

## Verification and operating limits

`npm test` checks the retained renderer and native bridge. The Rust suite covers
state/timers, tracking, accounting, persistence, authentication, authorization,
real HTTP synchronization, provider transport, calendar parsing, and MCP. Synthetic
fixtures generated from v1.2 exercise behavior across the language boundary.
Provider tests use injected responses and do not make billable calls.

Automated tests and compilation do not establish live Windows foreground capture,
tray/PDF/audio behavior, actual OAuth credentials, production signing, or a deployed
update feed. Those require verification on a Windows device with the intended
provider and publisher setup. The Linux-testable Rust crates do not add Linux or
macOS desktop support.

Workspace exports preserve local schema version 1. The companion service retains
the v1.2 SQLite tables, password/session formats, and provider credential envelope.
Raw foreground metadata and pending activity remain local; approved descriptions
can still contain sensitive text and should be reviewed before sharing. Server
operators can read shared records. See [MIGRATION.md](docs/MIGRATION.md) for backup,
compatibility, and token migration details.

Dependencies are recorded in `Cargo.lock` and `package-lock.json`. Attribution is
retained in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
