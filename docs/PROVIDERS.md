# AI, integrations, calendars and MCP

This Rust port preserves the published v1.2 provider contracts. It uses public
provider APIs and does not connect to Rize's backend. No credentials are bundled.
Tests inject synthetic responses; they do not make billable AI or live vendor calls.

Run the companion service with `cargo run -p rize-service --bin rize-service`.
Set configuration in its environment, never in the frontend bundle.

## AI

Set `RIZE_AI_BASE_URL` to an HTTPS OpenAI-compatible API base such as
`https://your-provider.example/v1`, `RIZE_AI_API_KEY`, `RIZE_AI_MODEL` and optional
`RIZE_EMBEDDING_MODEL`. Chat uses `/chat/completions`, `messages`, `stream:false`
and `max_completion_tokens:1800`. Semantic tags use float `/embeddings` vectors
and cosine similarity. There is no model hardcoding or simulated AI fallback.

Interactive requests require explicit consent. Context contains approved visible
entry descriptions/times/tag IDs and client/project/task names, plus the user's
prompt and selected skill. Raw window/application metadata, pending activity and
financial records are excluded. AI text cannot execute actions. Tag suggestions
require review. Reports, skills and routines retain their existing versioned CRUD.

Routines require `RIZE_RUN_ROUTINES=1`, explicit saved consent, enablement and a
running service. Each routine executes with its owner's current permissions.
Intervals are at least 15 minutes; missed intervals coalesce into one run. Next
run time and safe errors persist. Generated reports never approve/export work.

## Credential vault

Set `RIZE_SECRET_KEY` to a base64-encoded random 32-byte key, for example the output
of `openssl rand -base64 32`. Back it up securely alongside the server database.
Provider credentials use AES-256-GCM with a fresh nonce and team/provider-bound
additional authenticated data. The JSON envelope remains compatible with v1.2.
The key is never stored in the database. Changing it requires reconnecting providers;
status APIs never return secrets. Remote deployments require HTTPS.

## Work integrations

Owners/admins explicitly connect, pull and disconnect integrations. Tokens are
validated with the vendor before being saved. Imports are paginated and capped at
2,000 tasks; absent remote tasks are never deleted locally.

| Provider | Fields | Existing behavior |
| --- | --- | --- |
| Linear | API key; optional team ID | GraphQL issue/project import, status and URL; native time export remains unsupported. |
| ClickUp | Personal token; list ID; workspace ID for time export | Closed/subtask import; selected approved linked time export in milliseconds. |
| Asana | Personal token; project ID | Task import; selected approved linked time export in whole minutes. |

Export requires separate consent and a visible approved entry linked to an imported
task. Asana partial minutes are rejected; no silent rounding occurs. Vendor plan
and scope requirements still apply. A durable journal prevents duplicate completed
or uncertain writes. If a provider may have accepted a timed-out write, check it
before an operator clears that specific `time-exports` journal key. Local edits do
not change already exported vendor time. No webhooks or automatic provider sync
have been added.

## Google and Microsoft calendars

Register confidential Web application OAuth clients. Set `RIZE_PUBLIC_ORIGIN`,
`RIZE_GOOGLE_CLIENT_ID`, `RIZE_GOOGLE_CLIENT_SECRET`, `RIZE_MICROSOFT_CLIENT_ID`,
`RIZE_MICROSOFT_CLIENT_SECRET`, and optional `RIZE_MICROSOFT_TENANT` (default `common`).
Register the exact `/v1/oauth/google/callback` and `/v1/oauth/microsoft/callback`
paths under the public origin. Loopback HTTP is allowed only for local testing.
Enable Google Calendar API and delegated Microsoft Graph `Calendars.Read` as required
by the provider's app registration/consent policies.

Authorization uses one-use, ten-minute, hashed state bound to account/team and
PKCE S256. Callback rechecks current admin membership. Only fixed HTTPS token
endpoints are called; access and offline refresh credentials use the encrypted
vault. Restarting the service expires unfinished authorization flows.

Explicit pull reads the primary calendar over at most 366 days. Google expands
recurring instances; Microsoft is requested in UTC, and all-day dates use its
recognized original zone. Safe pagination is bounded. Calendar context and pending
time proposals are returned; no work is approved automatically. Attendees, message
bodies and attachments are excluded. Disconnect removes local tokens and pending
flows, without revoking the vendor's global grant. No write-back, secondary-calendar
selection or live two-way synchronization has been added.

## iCalendar files

Import an `.ics` file up to 1 MiB, selecting an IANA zone for floating DATE-TIME.
The Rust parser unfolds lines, retains all-day dates, and expands RRULE,
EXDATE and moved/cancelled recurrence overrides using `rrule` and `chrono-tz`.
Recognized Windows timezone names map through the retained v1.2 mapping. Unknown
zones and explosive subdaily recurrence are rejected. Imports are bounded to 1,000
source events and 1,000 occurrences over at most 366 days. The v1.2 behavior of
ignoring standalone RDATE additions is retained. All-day DATE values use UTC
transport instants and preserve their calendar date, independent of server timezone.
A timed event with no
end remains zero-duration; all-day events never fabricate tracked work.

## Read-only MCP

Set `RIZE_SERVER_URL` (default `http://127.0.0.1:4791`), `RIZE_SESSION_TOKEN` and
`RIZE_TEAM_ID`, then run `cargo run -p rize-service --bin rize-mcp`. Use a server
session for an account with the intended permissions. Remote servers require HTTPS.
The stdio process accepts newline-delimited JSON-RPC and MCP revision `2025-11-25`.
It exposes only `approved_time_entries`, `search_work` and `team_analytics`.
Initialization is required; tools cannot mutate data, and raw activity is redacted.
Backend authorization remains authoritative. The v1.2 single-page read limit is
retained (500 approved entries or 100 name matches from the first sync page).

Provider requests use TLS, exact origin checks, rejected redirects, timeouts and
bounded response bodies. Sanitized errors never include provider response bodies
or credential values. A successful synthetic test does not verify live vendor
credentials, plan access, OAuth app publication or billing eligibility.
