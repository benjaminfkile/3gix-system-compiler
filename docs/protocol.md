# The hub's compiler protocol, as this compiler speaks it

This is the compiler side of `compiler-pipeline.md` sections 5 to 8 and
`third-party-api.md` section 10 in the hub's architecture docs. Every shape
below was read from the hub's code, which wins over its docs where the two
differ. Paths are relative to the hub repository's `3GIXHub/` project
directory. The implementation is `crates/system-compiler/src/hub.rs`; the
tests are `crates/system-compiler/tests/hub_mock.rs`.

## Configuration

| Variable | Fallback | Meaning |
|---|---|---|
| `GX_HUB_URL` | `HUB_URL` | Base URL of the hub, `http` or `https`. `ws` and `wss` are derived from it. |
| `GX_API_KEY` | `COMPILER_API_KEY` | API key holding `compile:submit`. |
| `GX_COMPILER_ID` | `COMPILER_ID` | The compiler's id (a GUID) as registered with the hub. |
| `GX_COMPILER_SECRET` | `COMPILER_SECRET` | The compiler's raw shared secret. |

The fallback names are the ones the hub's local seed script writes into a
compiler's `.env`. Variables are read from the process environment after a
`.env` file is loaded (`--env-file`, or `.env` in the working directory or a
parent). Loading never overrides a variable that is already set and never
logs a value. Credential values are held in a type whose `Debug` and
`Display` print `<redacted>`, are sent only as request headers, and appear
in no log line and no error message.

## Connect

From `Controllers/CompilerConnectController.cs` and
`Middleware/ApiKeyAuthMiddleware.cs`:

```
GET /compiler/connect            (WebSocket upgrade)
X-API-Key: <key with compile:submit>
X-Compiler-Secret: <raw secret>
X-Compiler-Id: <compiler GUID>
```

What the hub does, in order:

1. `ApiKeyAuthMiddleware` answers `401` with no upgrade when `X-API-Key` is
   missing or unknown.
2. `[RequiresCapability("compile:submit")]` answers `403` with no upgrade
   when the key lacks the capability.
3. A request that is not a WebSocket upgrade gets `400`.
4. The hub accepts the upgrade, then checks `X-Compiler-Id` (must parse as
   a GUID), `X-Compiler-Secret` (non-empty, SHA-256 compared in constant
   time with the stored hash), and that the compiler exists and is not
   retired. Any failure closes the socket with code `4401` and reason
   `Unauthorized`.
5. Otherwise the socket is registered for the compiler id and the hub reads
   from it until it closes. The hub ignores anything the compiler sends
   except a close frame, which it answers with a normal close.

The client treats `401` or `403` on the upgrade and a `4401` close the same
way: the credentials are wrong, so it logs a clear message and exits non-zero
without reconnecting. Retrying the same credentials cannot succeed, and
exiting is the strictest way to meet "no more than once per minute".

## Dispatch frame

From `Adapters/JobDispatch/WebSocketJobDispatchAdapter.cs`,
`DispatchJobAsync`: one WebSocket text frame per job, the UTF-8 JSON of an
anonymous object serialized by `System.Text.Json` with default options, so
the property names are exactly as written in the code and GUIDs are in their
canonical lowercase hyphenated form:

```json
{
  "jobId": "<GUID>",
  "spaceId": "<GUID>",
  "buildId": "<GUID>",
  "chunkKey": "<chunk key>",
  "compilerId": "<GUID>",
  "layerId": "<layer name>"
}
```

There are no other fields. When the compiler's socket is held by another hub
instance, the frame is published on the Redis channel
`compiler:{compilerId}:dispatch` and relayed unchanged by
`Services/CompilerDispatchRelayService.cs`, so a compiler sees the same
bytes either way.

A job can arrive more than once: the relay notes that an undelivered job
stays `Dispatched` and is picked up again by a re-dispatch sweep. The client
ignores a frame whose `jobId` is already being compiled or submitted, and
the hub answers `409` to a second submission of a stored section.

## Submit

From `Controllers/CompilerSubmitController.cs`:

```
POST /space/{spaceId}/build/{buildId}/chunk/{chunkKey}/compiled
X-API-Key: <key with compile:submit>
X-Compiler-Secret: <raw secret>
X-Layer-Section-Type: matter
Content-Type: application/octet-stream

<section bytes, or the frame registry for the key `registry`>
```

`spaceId` and `buildId` are route-constrained to GUIDs. `X-Compiler-Id` is
not sent: the hub finds the compiler by the hash of the secret. Every
response carries `Cache-Control: no-store`.

| Status | Hub's reason | Client action |
|---|---|---|
| `201` | Stored. | Logged at info, counted as `created`. |
| `409` | A section for this layer is already stored, no body. Or, for a legacy snapshot listing the compiler on two layers, a `{"reason": "..."}` body (`Models/ConflictReason.cs`). | Success, counted as `already_stored`; logged at info, or at warn with the reason when there is one. |
| `400` | Section type header missing, or empty body: no body. Validation failure: `{"code": int, "name": string, "reason": string}` (`Models/SectionRejection.cs`). | Logged at error with code, name, and reason; counted as `rejected`. Never retried. |
| `401` | Secret missing or unknown, or API key missing or unknown (middleware). | Logged at error; the daemon stops and exits non-zero. |
| `403` | Key lacks `compile:submit`, compiler not in the build's layer snapshot, or compiler retired. | As `401`. |
| `5xx` | Hub failure. | Retried with backoff, up to 8 attempts, then logged at error and counted as `failed`. |
| other | For example `404` for a malformed route. | Logged at error, counted as `failed`. |

A transport error while submitting is retried like a `5xx`.

## Keepalive

The hub calls `app.UseWebSockets()` in `Program.cs` with no options, so
ASP.NET Core's default keep-alive applies: on .NET 8, the hub's target
framework, the server sends an unsolicited pong frame every 120 s and does
not expect pings from the compiler. The client therefore sends no pings of
its own. It answers any ping it receives (the WebSocket library does this
while reading), and it treats 300 s without any frame, two and a half
keep-alive intervals, as a dropped connection.

## Reconnect

When the socket closes or fails, or a connect attempt fails for any reason
other than credentials, the client waits and connects again. The delays are
1 s doubling to 60 s; each delay is drawn uniformly from the upper half of
its nominal value, `[nominal / 2, nominal]`, by a SplitMix64 generator seeded
with `--backoff-seed` (default 0), so a run's delays and logs are
reproducible. A successful connect starts the doubling again from 1 s.
Submission retries use the same scheme with the seed mixed with the chunk
key.

`serve --once` returns after the socket closes the first time, once every
job received has been compiled and submitted.

## Logs

Every job outcome is one `tracing` line at info, warn, or error level with
the fields `job_id`, `chunk_key`, `bytes`, `status` (the HTTP status, or
`none` when nothing was submitted), and `elapsed_ms`, plus `code`, `name`,
and `reason` for a `400` with a body. On exit the daemon logs the counts:
`connections`, `jobs`, `duplicates`, `created`, `already_stored`,
`rejected`, and `failed`. `RUST_LOG` sets the level; the default is `info`.
