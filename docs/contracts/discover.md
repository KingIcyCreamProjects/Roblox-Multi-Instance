# Discover / Home / Play bar destination contract (M5)

Shared by Rust (`src-tauri/src/roblox/api/*`, `src-tauri/src/discover/mod.rs`) and UI (`src/lib/ipc.ts`). camelCase JSON. Thumbnails are `https://*.rbxcdn.com` URLs (allowed by CSP `img-src`) or null.

## Types

```ts
type ServerRow = {
  jobId: string;              // UUID
  playing: number;
  maxPlayers: number;
  ping: number | null;        // ms, server-reported
  fps: number | null;
  kind: "public" | "private";
  name: string | null;        // private server name
};

type Region = { label: string; countryCode: string | null } | null; // label per geo.format, e.g. "Frankfurt, DE"

type GameTile = {
  universeId: number;
  placeId: number;            // root place
  name: string;
  creator: string | null;
  playing: number | null;
  likeRatio: number | null;   // 0..1
  thumbnail: string | null;   // 16:9 game thumbnail
  icon: string | null;        // square icon
};

type Favorite = { id: string; placeId: number; name: string; privateLink: string | null; thumbnail: string | null; order: number };
type Recent = { placeId: number; name: string; thumbnail: string | null; lastPlayedAt: string };

type Outfit = { id: number; name: string; thumbnail: string | null };
type MissingAsset = { assetId: number; name: string; url: string };

type UniversePlace = { placeId: number; name: string; isRoot: boolean };
```

## Commands

| Command | Args | Returns | Notes |
|---|---|---|---|
| `servers_stream` | `{ placeId, kind, accountId }` | `{ streamId }` | kind public/private; private needs accountId; pages arrive as events |
| `servers_stop` | `{ streamId }` | `void` | |
| `servers_find_player` | `{ placeId, username, accountId }` | `{ streamId }` | authenticated server pages (playerTokens are empty without a cookie) |
| `servers_regions` | `{ placeId, jobIds, accountId }` | `Record<jobId, Region>` | needs an account; respects `geo.enabled` (returns {} when off) |
| `servers_reserve_join` | `{ placeId, jobId, accountIds }` | `{ batchId }` | sub-place "set server, then join" (M4 launcher); one at a time (`RESERVE_BUSY` while another runs); refused with "That server is full or has closed. Pick another one." when the server is gone |
| `games_discover` | – | `GameTile[]` | popular/discover sort |
| `games_search` | `{ query, pageToken }` | `{ games: GameTile[], nextPageToken: string \| null }` | |
| `game_details` | `{ placeIds }` | `GameTile[]` | 10 min cache |
| `universe_of` | `{ placeId }` | `{ universeId }` | |
| `universe_places` | `{ universeId }` | `UniversePlace[]` | |
| `favorites_list` | – | `Favorite[]` | |
| `favorites_add` | `{ placeId, name, privateLink }` | `Favorite` | dedupe by placeId+privateLink |
| `favorites_update` | `{ id, patch: { name?, privateLink? } }` | `Favorite` | |
| `favorites_remove` | `{ id }` | `void` | |
| `favorites_reorder` | `{ ids }` | `void` | |
| `recents_list` | – | `Recent[]` | newest first, capped `recents.max` |
| `recents_clear` | – | `void` | |
| `outfits_list` | `{ username }` | `{ userId, outfits: Outfit[] }` | |
| `outfit_wear` | `{ outfitId, accountId }` | `{ missing: MissingAsset[] }` | never purchases |

## Events

- `servers://page` `{ streamId, rows: ServerRow[], page: number, done: boolean, stopped: boolean, error: string | null }`
- `servers://find` `{ streamId, pagesScanned: number, found: { jobId: string } | null, done: boolean, stopped: boolean, error: string | null }`
- `stopped` is true when the stream ended early: cancelled, replaced by a newer stream, or the vault locked.
- `favorites://changed` (no payload) → refetch favorites; sent after background thumbnails are saved (and after an import).
- `roblox://ratelimited` `{ host, retryInMs }` → inline "Roblox is rate-limiting us, retrying…"

## Play bar destination store (UI only)

`src/features/playbar/store.ts` (Zustand): `{ destination: string; server: string; mode: "any" | "specific" | "random" | "smallest"; setDestination(s); setServer(s); setMode(m) }`; `setDestination` also resets `server` to "" and `mode` to "any". Discover writes into it ("Set as destination", double-click a server row → server + mode "specific"). M4's Play bar reads it.
