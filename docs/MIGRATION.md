# Migrate v1.2 to Rust

This port starts from the published Rize Local v1.2 revision
`98f56dba6138280b64a7ea57c02b91b73a268b8a`. It keeps the React screens and existing
feature contracts while replacing the desktop/backend runtime with Rust and
Tauri. The native bridge and titlebar drag handling adapt those screens to Tauri.
The original repository and its installed application are separate from this one.

## Before moving data

1. In the existing app, stop/pause any active timer and turn off automatic tracking.
2. Export a complete JSON backup in **Settings**. CSV is useful for reports but is
   not a full workspace backup.
3. Quit the old application using its tray menu so it stops collecting activity.
4. Keep the original export and app-data files until the Rust app has been checked.

Do not run both trackers against the same live workspace file. No migration step
requires deleting the old application or its data.

## Desktop workspace

The existing schema version remains **1**. The Rust store validates JSON before
replacement, retaining entries, local raw/pending activity, clients, projects,
tasks, rules, timer/session history, labels, contracts, allocations, invoices, and
shared IDs. Invoice identity, frozen snapshots, and payment history retain their
accounting rules. Older or invalid files are not silently treated as empty data.

Install/start the Rust desktop, then use **Settings → Import** with the JSON export.
Review dates, totals, client/project links, invoice balances, and timer state before
resuming work. Running timers must be paused/checkpointed; closed-app time is not
added to them. Review tracking and sync settings before explicitly enabling either.
Import replaces the selected local workspace after confirmation.

The port uses Tauri's per-user application data directory for identifier
`io.github.perfectfighter.rsrize`, with `workspace.json` and `cloud-session.json`.
Export/import is the supported way to move a workspace; installing the new
executable does not discover or overwrite the old Electron profile automatically.

## Accounts and desktop sessions

The service API still uses `/v1` routes and opaque seven-day bearer sessions.
Desktop credentials remain outside the renderer. The Rust client uses Windows
current-user DPAPI to protect persisted session tokens; where protection is
unavailable, a token remains session-only.

An old Electron `cloud-session.json` token envelope may not decrypt through direct
DPAPI, even on the same machine. Sign in again in the Rust app rather than trying
to extract or convert a token to plaintext. Signing in again does not erase your
server account or team. Review the server URL and selected team before syncing.

Sync metadata includes versions, baselines, conflicts, and a workspace/account/team
binding. Preserve it when backing up an existing Rust profile. A fresh/imported
profile may require an explicit full sync and conflict review. Never clear version
state or choose a conflict winner merely to make an error disappear.

## Existing companion database

The Rust service retains the v1.2 SQLite schema: `users`, `sessions`, `teams`,
`members`, `invites`, `records`, `events`, and `kv`, including event cursors and
tombstones. Existing salted `scrypt:32768:8:3` password hashes and SHA-256 session
token hashes retain their format. A fixed Node-generated scrypt fixture is tested
against the Rust derivation.

1. Back up the desktop separately, then stop the old companion service.
2. Back up its SQLite database directory, including any remaining WAL/SHM files.
3. Keep the matching `RIZE_SECRET_KEY` in protected secret storage.
4. Start one Rust service process with `RIZE_DB_PATH` pointing at the preserved
   database and the same environment/provider configuration.
5. Check `/healthz`, sign in, inspect team membership and approved records, and
   review synchronization before enabling automatic sync or routines.

Do not run the Node and Rust service simultaneously against that database. Test on
a copy first if the database is important. A raw copy of a running WAL database
does not guarantee a consistent backup; stop the service or use SQLite's online
backup mechanism.

Provider credentials retain the v1.2 AES-256-GCM JSON envelope and authenticated
team/provider scope. The same database **and** matching `RIZE_SECRET_KEY` are
required to read them. Losing/changing the key requires reconnection. Saved
reports, routines, calendar context, and completed/uncertain time-export journals
remain database records. Do not discard an uncertain export journal and retry
without checking the vendor, since the earlier write may already have succeeded.

Unfinished OAuth authorization state is in memory and expires during a service
restart. Start those connection flows again. Registered callback URLs, provider
permissions, and actual credentials still need to match the running service.

See [SELF-HOSTING.md](SELF-HOSTING.md) and [PROVIDERS.md](PROVIDERS.md) for service
configuration and backup details. The repository does not provide arbitrary
database downgrade migrations or a hosted recovery service.

## Updates and installers

The Rust desktop uses a Tauri NSIS installer and WebView2. Install this port
explicitly; the old Electron application cannot receive it through its existing
`latest.yml`/blockmap update mechanism. Old Electron portable packaging scripts and
release commands are not part of this runtime.

The UI retains separate **Check**, **Download**, and **Restart and install** actions.
Development, portable, and unsigned/unconfigured builds explain why installation
is unavailable. To distribute working updates, the publisher must:

- Sign the Windows installer using its actual Authenticode identity.
- Generate and protect a Tauri updater signing key; embed its public key using
  `RIZE_UPDATER_PUBLIC_KEY` when compiling.
- Embed `RIZE_UPDATER_ENDPOINT` as the intended HTTPS Tauri update JSON endpoint.
- Generate signed updater artifacts through Tauri's `createUpdaterArtifacts`
  configuration and signing environment, and publish matching version/platform
  metadata and signature with the artifact.
- Set the build marker `RIZE_AUTHENTICODE_SIGNED=1` only for a genuinely signed
  build; that marker itself does not sign an executable.

Tauri uses `TAURI_SIGNING_PRIVATE_KEY` and optional
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` during the publisher's build. Keep the private
key out of the repository and installer. Tauri artifact signing and Windows
Authenticode signing are separate requirements. See the official
[Tauri updater guide](https://v2.tauri.app/plugin/updater/) for artifact/feed format.

No production signing certificate, private updater key, public update deployment,
or live vendor account is included in source. Ordinary build/test commands do not
publish a release or deploy a server.

## Check the port

```sh
npm ci
npm test
npm run build
cargo test --workspace --exclude rs-rize
```

Tests include retained renderer behavior, actual HTTP accounts/sync, money and
invoice invariants, native metadata sanitization, and fixtures generated from the
published v1.2 implementation. Test fixtures contain synthetic data and do not call
paid providers. Windows runtime checks should cover tracking/idle/session changes,
tray quit/restore, pause/resume, export/import, invoice PDF, and audio. Check real
OAuth, signing, and updates only with the intended operator configuration.
