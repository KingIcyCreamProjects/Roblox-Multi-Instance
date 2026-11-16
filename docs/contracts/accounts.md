# Accounts IPC contract (M3)

Shared by the Rust core (`src-tauri/src/accounts/`, `commands.rs`) and the UI (`src/lib/ipc.ts`). JSON is camelCase. Secrets (cookie, password) never appear in any response except `account_reveal`.

## Types

```ts
type SessionState = "ok" | "expired" | "unknown" | "pending"; // pending = imported user:pass, never signed in
type PresenceState = "offline" | "online" | "in_game" | "in_studio";

type Presence = {
  state: PresenceState;
  placeId: number | null;
  universeId: number | null;
  gameName: string | null;
  jobId: string | null;
  updatedAt: string; // ISO
};

type LaunchTarget = { placeId: number | null; jobId: string | null; privateLink: string | null };

type AccountView = {
  id: string;               // RMI uuid
  userId: number | null;    // null only while session === "pending"
  username: string;
  displayName: string;
  alias: string | null;     // ≤ 50 chars
  note: string | null;      // ≤ 5000 chars
  group: string;            // default "Unsorted"
  color: number | null;     // 1..8 → --tag-1..--tag-8
  pinned: boolean;
  order: number;            // manual order within group
  createdAt: string;
  lastLaunchedAt: string | null;
  lastValidatedAt: string | null;
  session: SessionState;
  presence: Presence | null;
  launchTarget: LaunchTarget | null;
  fpsCap: number | null;    // 1..1000
  windowRect: { x: number; y: number; w: number; h: number } | null;
  trackerId: string;
  extra: Record<string, string>;
  hasPassword: boolean;
  hasCookie: boolean;
  headshot: string | null;  // absolute path of cached 48x48 PNG (UI: convertFileSrc), null = initials
};

type AccountsList = {
  accounts: AccountView[];
  groups: string[];                            // display order, always contains every used group
  groupTargets: Record<string, LaunchTarget>;  // saved target per group name, used groups only
};
```

Launch target resolution (Rust `accounts::target_for`, applied in `launch_one` so Play, tray, API and relaunch all agree): the account's own `launchTarget`, else its group's `groupTargets` entry, else the Play bar. A target with neither `placeId` nor `privateLink` counts as unset. `useBarForAll` / Shift+Click "here" forces the Play bar for all. Vault payload key: `groupTargets`.

```ts

type AddResult = {
  line: number;             // 1-based input line
  status: "added" | "updated" | "invalid" | "skipped";
  username: string | null;
  message: string | null;   // plain-language reason for invalid
};
```

## Commands

| Command | Args | Returns | Notes |
|---|---|---|---|
| `accounts_list` | – | `AccountsList` | |
| `accounts_add_cookies` | `{ text }` | `AddResult[]` | one cookie per line; normalises quotes, `.ROBLOSECURITY=` prefix, trailing `;`; blank lines skipped (not reported) |
| `accounts_add_pending` | `{ text }` | `AddResult[]` | `username:password` per line |
| `accounts_update` | `{ ids, patch }` | `void` | all ids in one save (all or nothing), one `accounts://changed`; a group change moves each to the end of the new group in `ids` order; patch keys: alias, note, group, color, pinned, launchTarget, fpsCap, extra (null clears) |
| `accounts_remove` | `{ ids }` | `{ undoToken }` | undo valid 120 s in Rust (the toast offers it 8 s, paused on hover); restore skips an id or userId re-added meanwhile |
| `accounts_undo_remove` | `{ undoToken }` | `void` | |
| `accounts_reorder` | `{ group, ids }` | `void` | ids = full new order of that group |
| `groups_reorder` | `{ groups }` | `void` | |
| `group_rename` | `{ from, to }` | `void` | the group target moves with it; merging into a group that has its own target keeps that one |
| `group_target_set` | `{ group, target: LaunchTarget \| null }` | `void` | null clears; saving on a group with no accounts errors; targets of emptied groups stay hidden and are pruned on the next set |
| `account_copy` | `{ id, field }` | `void` | field: username, userId, profileLink, password, cookie. Rust writes clipboard; cookie/password need re-auth (error `REAUTH_REQUIRED`); clipboard cleared after 30 s if unchanged |
| `account_reveal` | `{ id, field }` | `string` | field: cookie, password; needs re-auth |
| `reauth` | `{ password }` | `void` | password mode: master password; OS mode: password null → OS prompt. Valid 2 min |
| `signin_start` | `{ accountId, usernameHint, password }` | `void` | opens login window; result via event. `password` (optional, typed in RMI before the window opens) is saved with the account on success |
| `signin_kill_stray` | – | `number` | |
| `accounts_validate` | `{ ids }` | `boolean` | ids null = all; false = a check is already running (single flight) |
| `avatar_full` | `{ userId }` | `string \| null` | cached 420x420 full-body PNG path |

## Events

- `accounts://changed` → refetch `accounts_list` (also after vault unlock, lock, restore and import)
- `signin://result` `{ ok: boolean, username: string | null, error: string | null }`
- `roblox://ratelimited` `{ host: string, retryInMs: number }`

## Error strings

Commands reject with a short user-readable string, or with an error code that is the whole string (`REAUTH_REQUIRED`, `VAULT_LOCKED`, `VAULT_WAS_LOCKED`, `WRONG_PASSWORD`, `VAULT_DAMAGED`, `PREVIEW_EXPIRED`). The UI branches only on codes (`hasCode`, exact match) and shows catalog text for them (`src/lib/ipc.ts` `CODE_TEXT`).

## Import (M7, spec §11)

Old files are only read. Decrypted accounts stay in Rust for 5 min under `previewId` (zeroized on expiry, commit, or vault lock); rows never carry cookies or passwords. The old manager's password is used once by `import_preview` and dropped.

Password buffer: Rust wraps the deserialized password in `Zeroizing` on arrival and drops it after decrypt. Copies outside Rust's control (the webview JS string, the IPC JSON request body) cannot be wiped; the UI clears its field after a successful read (kept for a retry after a wrong password) and on sheet close.

| Command | Args | Returns | Notes |
|---|---|---|---|
| `import_pick_folder` | – | `{ path } \| null` | native folder dialog |
| `import_detect` | `{ path }` | `{ accountData: { encoding: "locked"\|"dpapi"\|"plain"\|null, error } \| null, settings, recents, favorites }` | booleans = file present |
| `import_preview` | `{ path, password, accounts }` | `{ previewId, rows: [{ key, userId, username, alias, group, willUpdate, hasCookie, hasPassword, headshot }], prefs, games: { recents, favorites }, notes }` | vault must be unlocked; `accounts: false` = account file never decrypted; `notes` = unreadable settings/game files |
| `import_commit` | `{ previewId, keys, importPrefs, importRecents, importFavorites }` | `{ results: [{ key, id, username, status: "added"\|"updated"\|"skipped"\|"failed", message }], notes: string[] }` | one vault save (backup required); existing userId keeps RMI metadata, only empty fields filled; a signed-in (session ok) account keeps its cookie ("Kept the current sign-in."), else the old cookie replaces it; the old password only fills an empty one; games: names default to the placeId (max 100 chars), thumbnails kept only for https `*.rbxcdn.com`, emits `recents://changed` and `favorites://changed`; trackerId adopted when new or never launched in RMI; previews from before a vault lock are refused; then `accounts://changed` and a session check |
