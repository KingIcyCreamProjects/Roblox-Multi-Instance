# Automation

Roblox Multi Instance (RMI) has an opt-in local automation layer: a WebSocket
control server and a token-gated HTTP API. Your own scripts and tools use it to
talk to running clients and to RMI.

RMI does **not** ship or recommend any in-game script runner, executor or
injector. The automation layer is a plain local interface. Whatever connects to
it is your own business. The reference client in this folder
(`automation-client.mjs`) is a plain Node program for testing only.

## Enabling

1. Settings, Automation: turn on **Enable automation**. An **Automation** entry
   appears in the sidebar.
2. Open the Automation page and press **Start**, or turn on **Start on launch**
   in Settings, Automation so the servers start with RMI.
3. Copy the token from Settings, Automation.

Ports: control server (WebSocket) `7810`, local API (HTTP) `7811`. Both are
settings (`ws.port`, `api.port`).

## Security model

- **Loopback by default.** Both servers bind `127.0.0.1`. LAN access is an
  explicit opt-in (`net.allowLan`). With it on, anyone on your network who has
  the token can control your accounts, so the Automation page shows a warning
  while it is on. Leave it off unless you know why you need it.
- **Token always required.** Every HTTP request needs
  `Authorization: Bearer <token>`. Every WebSocket needs `?token=<token>`. There
  is no unauthenticated mode, not even for 404s.
- **The token stays the same across restarts.** It is made once when you turn
  Automation on, then saved in the data folder encrypted by the OS (DPAPI on
  Windows, a device key elsewhere). It is never written to disk in plain text.
  On macOS and Linux the device key lives in the OS keyring; if the keyring
  refuses it, it is saved as a `device.key` file in the data folder instead
  (owner-only permissions), so then anyone who can read the data folder can
  decrypt the token.
  Regenerating it from Settings disconnects every script and invalidates the
  old token. If the saved token does not decode (for example after moving the
  data folder to another PC or user), RMI makes a new one and shows a toast. If
  the file or the keyring cannot be reached (for example a cancelled keyring
  prompt), the saved file is kept and the token is memory-only until RMI restarts.
- **API families are off by default.** Each family has its own toggle in
  Settings, Automation: Read, Launch, Edit, Social, Secrets. A request to a
  family that is off gets `403 family_disabled`.
- **Secrets are separate.** `GET /v1/accounts/{name}/cookie` needs the Secrets
  family. Every call is logged (never the value) and RMI shows a toast each time
  it is used.
- **Origin and Host rules.** A request with any `Origin` header other than
  `null` is rejected with `403 forbidden_origin`, so web pages cannot reach the
  server. Non-browser clients send no Origin. `Origin: null` is accepted
  because some script runtimes send it; sandboxed iframes and `file://` pages
  send it too, so against those the token is the only defence. With LAN off, the `Host` header
  must be `127.0.0.1`, `localhost` or `[::1]` (any port), otherwise
  `403 forbidden_host`. This blocks DNS rebinding.
- Constant-time token comparison; the token is 32 random bytes (64 hex chars).

## Control server (WebSocket)

Connect to:

```
ws://127.0.0.1:7810/control?token=<token>&account=<username>&job=<jobId>
```

`account` names the account this connection controls (it is shown in the
Automation page). `job` is optional. A missing or wrong token: the server
accepts the upgrade and then closes with code **4401**.

Messages are JSON, one per text frame:

```json
{ "t": "<type>", "id": "<optional correlation id>", "data": { } }
```

### Client to RMI

| type | data | effect |
|---|---|---|
| `hello` | `{ account, userId?, placeId?, jobId? }` | registers the client (also implied by the query string) |
| `log` | `{ text, level? }` | appends to that account's console. `level` is `info` (default), `warn` or `error`; other short strings are kept and shown neutral |
| `echo` | `{ text }` | broadcast to all other connected clients as `message` with `from` |
| `ui.create` | `{ kind, name, value?, min?, max?, step?, decimals? }` | adds (or replaces, by `name`) a control in that account's panel. `kind` is `button`, `text`, `label`, `number` or `break` |
| `ui.get` | `{ name }` (send an `id`) | RMI replies `ui.value` with the same `id` |
| `relaunch.set` | `{ enabled?, seconds?, placeId?, jobId? }` | updates that account's auto-relaunch config |
| `ping` | `{}` | RMI replies `pong` with the same `id` |

Examples:

```json
{ "t": "hello", "data": { "account": "Alice", "userId": 1001, "placeId": 606849621 } }
{ "t": "log", "data": { "text": "script started", "level": "info" } }
{ "t": "echo", "data": { "text": "ready" } }
{ "t": "ui.create", "data": { "kind": "button", "name": "Reset" } }
{ "t": "ui.create", "data": { "kind": "text", "name": "Target", "value": "spawn" } }
{ "t": "ui.create", "data": { "kind": "number", "name": "Speed", "value": 16, "min": 0, "max": 100, "step": 1, "decimals": 0 } }
{ "t": "ui.create", "data": { "kind": "label", "name": "State", "value": "idle" } }
{ "t": "ui.create", "data": { "kind": "break", "name": "row2" } }
{ "t": "ui.get", "id": "7", "data": { "name": "Speed" } }
{ "t": "relaunch.set", "data": { "enabled": true, "seconds": 60, "placeId": 606849621 } }
{ "t": "ping", "id": "p1", "data": {} }
```

A `break` control forces the following controls onto a new row. Only the
fields listed above are kept; anything else is dropped.

### RMI to client

| type | data | when |
|---|---|---|
| `message` | `{ text, from? }` | the user pressed "Send message" in the console (no `from`), or another client sent `echo` (`from` is the sender) |
| `ui.event` | `{ name, value }` | a panel button was clicked (`value: true`) or a text/number field was committed |
| `ui.value` | `{ name, value }` | reply to `ui.get`; `value` is `null` for an unknown control |
| `pong` | `{}` | reply to `ping` |

Examples:

```json
{ "t": "message", "data": { "text": "stop now" } }
{ "t": "message", "data": { "text": "ready", "from": "Bob" } }
{ "t": "ui.event", "data": { "name": "Reset", "value": true } }
{ "t": "ui.event", "data": { "name": "Speed", "value": 20 } }
{ "t": "ui.value", "id": "7", "data": { "name": "Speed", "value": 20 } }
{ "t": "pong", "id": "p1", "data": {} }
```

### Close codes

| code | meaning |
|---|---|
| 4401 | missing or wrong token |
| 1013 | server full (64 connections, or 100 accounts already known), or this client was too slow to read and was dropped |
| 1001 | server stopped or token regenerated |

## Local HTTP API

Base URL `http://127.0.0.1:7811`. All requests need the bearer token. JSON
responses use camelCase. Requests with a body need
`Content-Type: application/json`.

Set the token once:

```sh
TOKEN=<token from Settings, Automation>
```

### Read family

```sh
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7811/v1/accounts
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7811/v1/accounts/Alice
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7811/v1/accounts/Alice/presence
```

An account has: `userId` (`null` until known), `username`, `displayName`, `alias`,
`note`, `group`, `session`, `extra`. It never includes the cookie.

Presence returns `{ "presence": null }` before the first presence check, otherwise
`{ "presence": { state, placeId, universeId, gameName, jobId, updatedAt } }`.
`state` is `offline`, `online`, `in_game` or `in_studio`; the other fields can be
`null`.

### Launch family

```sh
curl -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"placeId":606849621,"jobId":"<jobId>"}' \
  http://127.0.0.1:7811/v1/accounts/Alice/launch
```

Body fields (all optional): `placeId`, `jobId`, `privateLink`, `follow`.
Returns `202` with `{ "launchId": "<id>" }`. The launch is async. Ask for the
truth:

```sh
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7811/v1/launches/<launchId>
```

### Edit family

```sh
curl -X PATCH -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"alias":"Main","note":"farm","group":"Alts","extra":{"k":"v"}}' \
  http://127.0.0.1:7811/v1/accounts/Alice

curl -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"cookie":"<.ROBLOSECURITY value>"}' \
  http://127.0.0.1:7811/v1/accounts/import-cookie
```

Both return the account.

### Social family

```sh
curl -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"username":"SomeUser"}' http://127.0.0.1:7811/v1/accounts/Alice/block
curl -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"username":"SomeUser"}' http://127.0.0.1:7811/v1/accounts/Alice/unblock
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7811/v1/accounts/Alice/blocked
```

`block` and `unblock` return `204` with no body. `blocked` returns
`{ "blocked": [{ userId, username, displayName }], "truncated": false }`
(`truncated` = more than 1000 blocked users; the rest are not listed). An account
without a session gets `404`; an expired session gets `409 session_expired`.

### Secrets family

```sh
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7811/v1/accounts/Alice/cookie
```

Returns `{ "cookie": "..." }`. Logged on every call; RMI toasts each use.

### Route summary

| method | path | family | success |
|---|---|---|---|
| GET | `/v1/accounts` | Read | 200 |
| GET | `/v1/accounts/{name}` | Read | 200 |
| GET | `/v1/accounts/{name}/presence` | Read | 200 |
| POST | `/v1/accounts/{name}/launch` | Launch | 202 |
| GET | `/v1/launches/{id}` | Launch | 200 |
| PATCH | `/v1/accounts/{name}` | Edit | 200 |
| POST | `/v1/accounts/import-cookie` | Edit | 200 |
| POST | `/v1/accounts/{name}/block` | Social | 204 |
| POST | `/v1/accounts/{name}/unblock` | Social | 204 |
| GET | `/v1/accounts/{name}/blocked` | Social | 200 |
| GET | `/v1/accounts/{name}/cookie` | Secrets | 200 |

Account routes return `423 vault_locked` while the vault is locked.

`GET /v1/launches/{id}` returns `{ launchId, accountId, state, message, gameName, done }`.
`state` is `queued`, `validating`, `launching`, `launched`, `failed` or `cancelled`.

## Auto-relaunch

With `relaunch.set` (or the toggle in the control panel) on, RMI relaunches an account
when its script disconnects or its RMI-launched Roblox client exits, after `seconds`
(default: Settings > Automation > Relaunch delay). Launches are spaced by the global
relaunch spacing. A reconnect before then cancels it. It launches to `placeId`
(and `jobId` if set), else the place the script last reported, else the account's saved
target. It never relaunches while the vault is locked. After 5 relaunches in 10 minutes
it stops for that account until the toggle is turned off and on. With "Use presence"
on, presence (in game or not) replaces the socket as the up/down signal. Notes appear
in the account's console. While a relaunch is running, the account shows "Launching".

Caveats:

- Auto-relaunch runs only while the automation servers are running.
- It only knows accounts a script has connected as since RMI started. The relaunch
  config is kept in memory, so it is lost when RMI restarts and the script must
  send `relaunch.set` again (or you turn the toggle on again).
- Closes RMI makes itself never relaunch: the client helpers (max instances, close
  after N minutes), "Replace previous client" on launch, stopping the servers and
  regenerating the token. For 30 s after an RMI launch of an account ends, its
  exits and disconnects are treated as part of that launch.
- A failed relaunch is retried after the delay and counts toward the 5-in-10-minutes
  stop.

### Errors

Every error has this shape:

```json
{ "error": { "code": "family_disabled", "message": "The Read API family is turned off in Settings > Automation" } }
```

| status | code | meaning |
|---|---|---|
| 400 | `bad_request` | body is not valid JSON or has the wrong fields |
| 401 | `unauthorized` | missing or wrong bearer token (applies to unknown paths too) |
| 403 | `family_disabled` | that API family is off in Settings |
| 403 | `forbidden_origin` | request carried a browser `Origin` header |
| 403 | `forbidden_host` | LAN is off and the `Host` header is not loopback |
| 403 | `loopback_only` | a secret (cookie) was asked for from another computer over LAN; secrets are only served to `127.0.0.1` |
| 404 | `not_found` | no such endpoint, account or launch, or the account has no cookie saved |
| 409 | `session_expired` | the account's Roblox session expired; sign in again in RMI |
| 409 | `friend_paused` | launch refused: a friend disconnected this account through the friend remote; reconnect it first |
| 423 | `vault_locked` | unlock the vault in RMI first |
| 405 | `method_not_allowed` | known path, wrong method |
| 408 | `timeout` | the request took longer than 30 s |
| 413 | `too_large` | request body is over 2 MB |
| 501 | `not_implemented` | not available in this version yet |
| 502 | `roblox_error` | Roblox could not be reached while checking a cookie |
| 503 | `starting` | RMI is still starting; retry |

## Limits

- WebSocket messages and frames: **64 KiB**. A larger one drops the client.
- Strings in `log`, `echo` and `ui.*` are clipped to 2048 characters.
- Control names: 64 characters. Up to 200 controls per account.
- Console: 5000 lines per account (oldest dropped). The UI loads the last 500. "Save to file" writes all stored lines (one per line, newlines escaped); "Clear" only clears the view.
- Up to 64 simultaneous connections and 100 accounts.
- Each client has a queue of 256 outgoing frames; a client that does not read is
  dropped (close code 1013).
- `relaunch.set` keeps only `enabled`, `seconds`, `placeId`, `jobId`.
- HTTP request bodies: **2 MB**. Each request must finish within 30 s; at most 64
  requests are handled at once. The WebSocket and API ports must differ.
- At the account cap, the account disconnected the longest is evicted, skipping
  accounts with auto-relaunch on.

## Reference client

`docs/automation-client.mjs` is a dependency-free Node client (Node 22 or newer,
which has a global `WebSocket`). It connects with the token and an account,
logs a line, creates a button, prints `ui.event` when you click it in the
Automation page, prints `message`, and sends a `ping` every 15 s while connected (RMI answers `pong`).

```sh
node docs/automation-client.mjs <token> Alice
```

For testing only. RMI does not ship or recommend running scripts inside the
Roblox client.
