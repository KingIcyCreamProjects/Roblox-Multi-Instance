# Account tools contract (M6)

Shared by Rust (`src-tauri/src/roblox/api/*`, `src-tauri/src/tools/*`) and UI. camelCase JSON. All calls need a valid session; state-changing calls use the CSRF handshake in Rust. Errors are short user-readable strings; machine codes: `PIN_REQUIRED`, `SESSION_EXPIRED`, `VAULT_LOCKED` / `VAULT_WAS_LOCKED` (a batch stops at a lock and the remaining rows carry the code), and `NAME_NOT_SAVED` (an ok row's note: the name changed on Roblox but the vault, locked meanwhile, did not take it). Tools never produce `REAUTH_REQUIRED` (password/email changes take the account password the user types, not the vault re-auth). `tool_pin_unlock` never returns `PIN_REQUIRED`; it shows Roblox's message (wrong PIN, attempt limit).

## Types

```ts
type BatchResult = { accountId: string; ok: boolean; message: string | null };
type PrivacyValue = string;                  // e.g. "AllUsers" | "Followers" | "Following" | "Friends" | "NoOne"
type PrivacyOption = { value: PrivacyValue; label: string };
type Privacy = {
  follow: PrivacyValue;                      // who can follow me into experiences
  privateServerInvite: PrivacyValue;         // who can invite me to private servers
  followOptions: PrivacyOption[];
  privateServerInviteOptions: PrivacyOption[];
};
type BlockedUser = { userId: number; username: string; displayName: string };
type AccountInfo = { userId: number; username: string; displayName: string; created: string; ageDays: number; profileUrl: string };
type PinState = { enabled: boolean; unlockedUntil: string | null };
```

## Commands

| Command | Args | Returns | Batch | Notes |
|---|---|---|---|---|
| `tool_pin_state` | `{ accountId }` | `PinState` | – | |
| `tool_pin_unlock` | `{ accountId, pin }` | `PinState` | – | Roblox keeps it unlocked a few minutes; UI shows a countdown |
| `tool_display_name` | `{ batchId, accountIds, name }` | `BatchResult[]` | yes | cooldown errors in plain words |
| `tool_password` | `{ accountId, current, next }` | `void` | – | updates the saved password if one is stored |
| `tool_email_get` | `{ accountId }` | `{ masked: string \| null, verified: boolean }` | – | |
| `tool_email_set` | `{ accountId, email, password }` | `void` | – | |
| `tool_privacy_get` | `{ accountId }` | `Privacy` | – | |
| `tool_privacy_set` | `{ batchId, accountIds, follow?, privateServerInvite? }` | `BatchResult[]` | yes | |
| `tool_signout_others` | `{ accountId }` | `void` | – | takes the refreshed cookie if Roblox issues one; with none the session here ended too: account marked expired, error "Signed out everywhere, including here. Sign in again." |
| `tool_friend_request` | `{ batchId, accountIds, username }` | `BatchResult[]` | yes | |
| `tool_block` | `{ batchId, accountIds, username, block }` | `BatchResult[]` | yes | block=false unblocks |
| `tool_blocked_list` | `{ accountId }` | `BlockedUser[]` | – | |
| `tool_unblock_all` | `{ accountId }` | `{ count: number, failed: number, truncated: boolean }` | – | UI confirms first; keeps going past single failures, but stops with an error on `SESSION_EXPIRED`, `PIN_REQUIRED` or a vault lock; `truncated` = list cut at 20 pages |
| `tool_robux` | `{ accountId }` | `{ robux: number }` | – | read-only |
| `tool_info` | `{ accountId }` | `AccountInfo` | – | |
| `tool_export_summary` | `{ accountId }` | `{ path: string } \| null` | – | Rust opens the save dialog itself; `null` = cancelled; non-secret JSON |
| `account_open_site` | `{ accountId, url }` | `void` | – | ephemeral logged-in webview, no IPC (groups, roblox.com as this account) |
| `outfits_list` / `outfit_wear` | see discover.md | | – | shared with Discover → Outfits |

## Events

- `tools://batch` `{ batchId: string, result: BatchResult, done: number, total: number }` — batch commands (those taking `batchId`, a client-generated id) also stream per-account results while running sequentially (rate limited); the UI ignores events whose `batchId` is not its own. Row messages may carry machine codes (`PIN_REQUIRED`, `SESSION_EXPIRED`, `VAULT_LOCKED`, `VAULT_WAS_LOCKED`, `NAME_NOT_SAVED`).
- `state://changed` `["pin"]` — Roblox's PIN lock state changed (it re-locked, or was unlocked).
