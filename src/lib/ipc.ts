// Typed wrappers over the Rust command surface (src-tauri/src/commands.rs).
// Outside Tauri (vite dev in a browser, tests) calls go to the dev mock so screens render.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { t, type MsgKey } from "./i18n";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export type Settings = Record<string, unknown> & {
  theme: "system" | "dark" | "light";
  ui: {
    showUsernames: boolean;
    density: "comfortable" | "compact";
    scale: number;
    reducedMotion: string;
    tabOrder: string[];
  };
  multi: { enabled: boolean };
  fps: { unlock: boolean; value: number };
  graphics: { set: boolean; level: number };
  watcher: { enabled: boolean; reconnect: boolean };
  friend: {
    enabled: boolean;
    port: number;
    publicUrl: string;
    username: string;
    canAdd: boolean;
  };
  remote: { enabled: boolean; port: number; deviceName: string; files: boolean; audio: boolean };
  startup: { launchOnLogin: boolean };
  tray: { enabled: boolean };
  automation: { enabled: boolean };
  dev: { enabled: boolean };
  /** `seen`: last version whose update notes were shown (null on installs before the log) */
  updates: { auto: boolean; seen: string | null };
  onboarding: {
    tosSeen: boolean;
    trayToastSeen: boolean;
    tourDone: boolean;
    /** tour steps already shown (missing in settings from before v0.3) */
    tourSeen?: string[];
  };
};

export type GridRect = { x: number; y: number; w: number; h: number };
/** Physical pixels. `id`: device name, stable across calls. */
export type MonitorInfo = {
  id: string;
  label: string;
  work: GridRect;
  rect: GridRect;
  primary: boolean;
  scale: number;
};
/** Presets: Classic 3-wide = columns 3, Even grid = auto, Wide + stack = wide; Focus + strip /
 * side / corner / rest hidden = focus with others strip / side / corner / minimize. */
export type GridLayout =
  | { type: "auto" }
  | { type: "columns"; count: number }
  | { type: "rows"; counts: number[]; align?: "left" | "center" | "stretch" }
  | { type: "bands" }
  | { type: "pyramid" }
  | { type: "wide" }
  | { type: "cascade" }
  | { type: "single" }
  | {
      type: "focus";
      main: string;
      others: "strip" | "side" | "corner" | "minimize" | "behind";
    };
/** From grid_suggest. Focus layouts come with `main: ""` = first account of the order. */
export type GridSuggestion = { label: string; layout: GridLayout };
/** Saved as setting `gridArrange` via settings_set; always send the whole `layout`. */
export type GridArrangeDefault = { monitor: string | null; order: string[]; layout: GridLayout };

export type Capabilities = {
  os: "windows" | "macos" | "linux";
  launch: boolean;
  launchReason: string | null;
  multiInstance: boolean;
  cookieLock: boolean;
  windowRect: boolean;
  watcher: boolean;
  clientHelpers: boolean;
};

export type WatcherClient = {
  pid: number;
  accountId: string | null;
  username: string | null;
  memoryMb: number;
  title: string | null;
  startedAt: number;
  /** false = Roblox desktop/tray app: listed, never closed */
  gameClient: boolean;
  lastServerEvent: number | null;
};
export type WatcherRule = {
  id: "lowMemory" | "wrongTitle" | "noServer";
  enabled: boolean;
  lastFiredAt: number | null;
};
export type WatcherStatus = { running: boolean; clients: WatcherClient[]; rules: WatcherRule[] };

export type AppInfo = {
  version: string;
  dataDir: string;
  portable: boolean;
  /** Settings file unreadable at startup: changes last until the app quits. */
  settingsReadOnly: boolean;
  /** Windows transparency effects are on, so the acrylic window material shows. */
  acrylic: boolean;
  /** Message when a corrupt settings.json was reset to defaults at startup. */
  settingsRecovered: string | null;
};

export type AutomationStatus = {
  wsRunning: boolean;
  apiRunning: boolean;
  wsPort: number;
  apiPort: number;
  lan: boolean;
};

/** Tailscale CLI state; `funnelPort` set = Funnel already proxies the friend server. */
export type TsStatus = {
  installed: boolean;
  running: boolean;
  dnsName: string | null;
  funnelPort: number | null;
};
/** What friend_ts_enable rejects with (on the thrown Error's `cause`). */
export type TsError = {
  code: "not_installed" | "not_running" | "needs_admin" | "failed";
  url: string | null;
  message: string;
};
export type HomeLabRelease = { version: string; asset: string; size: number; page: string };
export type FriendStatus = { running: boolean; port: number; publicUrl: string };

/** Remote access host: this computer. */
export type RemoteStatus = {
  /** `ip:port` while listening on the Tailscale address. */
  listening: string | null;
  name: string;
  session: { peer: string; since: string; control: boolean } | null;
  paired: { id: string; name: string; pairedAt: string }[];
  /** Newest first, at most 20. */
  log: {
    peer: string;
    startedAt: string;
    durationSec: number;
    control: boolean;
    /** Transfers in the session: `sent` from this computer, `received` into Downloads. */
    files: { name: string; size: number; dir: "sent" | "received" }[];
  }[];
  /** A shown pairing code can still be used (false once used, burned or expired). */
  codeLive: boolean;
};
export type RemotePeer = {
  name: string;
  os: "windows" | "macos" | "linux" | "ios" | "android" | "other";
  ip: string;
  online: boolean;
  /** Device name RMI answered with; null: no RMI remote access there. */
  rmi: string | null;
  paired: boolean;
  /** Someone is connected to it right now. */
  busy: boolean;
};
export type RemotePeers = { installed: boolean; running: boolean; peers: RemotePeer[] };
/** `token` is secret: the viewer sends it as the socket's first message. */
export type RemoteConnect = { url: string; token: string; name: string };

export type ConsoleLine = { ts: number; level: string; text: string };

export type AutomationAccount = {
  account: string;
  connected: boolean;
  userId: unknown;
  placeId: unknown;
  jobId: unknown;
  console: ConsoleLine[];
  controls: Record<string, unknown>[];
  /** `relaunch.set` / automation_relaunch_set config; `null` until first set. */
  relaunch: { enabled?: boolean; seconds?: number; placeId?: unknown; jobId?: unknown } | null;
  relaunchStatus?: RelaunchStatus;
};

/** Unix ms; `stopped` = cooldown hit (5 relaunches in 10 min) until toggled off and on. */
export type RelaunchStatus = {
  pendingAt: number | null;
  lastRelaunch: number | null;
  count: number;
  stopped: boolean;
  /** An auto-relaunch of this account is running. */
  launching: boolean;
};
export type RobloxStatus = {
  installed: boolean;
  bootstrapper: string | null;
  versionDir: string | null;
  running: boolean;
  multiActive: boolean;
  cookieLockActive: boolean;
  /** Show the `play.multiBlocked` banner. */
  multiBlockedByRunningRoblox: boolean;
  /** false: Roblox may ignore the FPS cap flag. */
  fpsFlagAllowlisted: boolean;
  /** Multi-instance turned off while clients run: the mutex is released once they close. */
  multiReleasePending: boolean;
  /** Mutex held inside RMI itself: closing RMI closes every client. */
  multiInProcess: boolean;
};

export type VaultStatus = {
  exists: boolean;
  unlocked: boolean;
  mode: "os" | "password" | "portable" | null;
  rememberActive: boolean;
  /** Locked and the file header is unreadable: offer backup recovery. */
  damaged: boolean;
};

// Accounts contract: docs/contracts/accounts.md
export type SessionState = "ok" | "expired" | "unknown" | "pending";
export type PresenceState = "offline" | "online" | "in_game" | "in_studio";
export type Presence = {
  state: PresenceState;
  placeId: number | null;
  universeId: number | null;
  gameName: string | null;
  jobId: string | null;
  updatedAt: string;
};
export type LaunchTarget = {
  placeId: number | null;
  jobId: string | null;
  privateLink: string | null;
  /** Server pick for a place with no jobId; absent = any. */
  mode?: "random" | "smallest" | null;
};
export type AccountView = {
  id: string;
  userId: number | null;
  username: string;
  displayName: string;
  alias: string | null;
  note: string | null;
  group: string;
  color: number | null;
  pinned: boolean;
  order: number;
  createdAt: string;
  lastLaunchedAt: string | null;
  lastValidatedAt: string | null;
  session: SessionState;
  presence: Presence | null;
  launchTarget: LaunchTarget | null;
  fpsCap: number | null;
  graphics: number | null;
  windowRect: { x: number; y: number; w: number; h: number } | null;
  trackerId: string;
  friendAllowed: boolean;
  friendPaused: boolean;
  friendLastPlace: number | null;
  /** The friend signed it in from the friend page. */
  friendAdded: boolean;
  extra: Record<string, string>;
  hasPassword: boolean;
  hasCookie: boolean;
  headshot: string | null;
};
export type AccountsList = {
  accounts: AccountView[];
  groups: string[];
  /** Saved launch target per group (used groups only). */
  groupTargets: Record<string, LaunchTarget>;
};
export type AddResult = {
  line: number;
  status: "added" | "updated" | "invalid" | "skipped";
  username: string | null;
  message: string | null;
};
export type AccountPatch = Partial<
  Pick<
    AccountView,
    | "alias"
    | "note"
    | "group"
    | "color"
    | "pinned"
    | "launchTarget"
    | "fpsCap"
    | "graphics"
    | "friendAllowed"
    | "extra"
  >
>;
export type CopyField = "username" | "userId" | "profileLink" | "password" | "cookie";
export type SigninResult = { ok: boolean; username: string | null; error: string | null };

// Discover contract: docs/contracts/discover.md
export type ServerKind = "public" | "private";
export type ServerRow = {
  jobId: string;
  playing: number;
  maxPlayers: number;
  ping: number | null;
  fps: number | null;
  kind: ServerKind;
  name: string | null;
};
export type Region = { label: string; countryCode: string | null } | null;
export type GameTile = {
  universeId: number;
  placeId: number;
  name: string;
  creator: string | null;
  playing: number | null;
  likeRatio: number | null;
  thumbnail: string | null;
  icon: string | null;
};
export type Favorite = {
  id: string;
  placeId: number;
  name: string;
  privateLink: string | null;
  thumbnail: string | null;
  order: number;
};
export type Recent = {
  placeId: number;
  name: string;
  thumbnail: string | null;
  lastPlayedAt: string;
};
export type Outfit = { id: number; name: string; thumbnail: string | null };
export type MissingAsset = { assetId: number; name: string; url: string };
export type UniversePlace = { placeId: number; name: string; isRoot: boolean };
export type ServersPage = {
  streamId: string;
  rows: ServerRow[];
  page: number;
  done: boolean;
  error: string | null;
  /** ended by a cancel (stop, newer stream, lock), not by running out of pages */
  stopped: boolean;
};
export type FindProgress = {
  streamId: string;
  pagesScanned: number;
  found: { jobId: string } | null;
  done: boolean;
  error: string | null;
  stopped: boolean;
};
export type RateLimited = { host: string; retryInMs: number };

// Tools contract: docs/contracts/tools.md
export type BatchResult = { accountId: string; ok: boolean; message: string | null };
export type PrivacyOption = { value: string; label: string };
export type Privacy = {
  follow: string;
  privateServerInvite: string;
  followOptions: PrivacyOption[];
  privateServerInviteOptions: PrivacyOption[];
};
export type BlockedUser = { userId: number; username: string; displayName: string };
export type AccountInfo = {
  userId: number;
  username: string;
  displayName: string;
  created: string;
  ageDays: number;
  profileUrl: string;
};
export type PinState = { enabled: boolean; unlockedUntil: string | null };
export type BatchEvent = { batchId: string; result: BatchResult; done: number; total: number };
export type Backup = { name: string; createdAt: string; size: number };

// Legacy importer (spec §11). Secrets never cross IPC; the preview lives in Rust for 5 min.
export type ImportDetect = {
  accountData: { encoding: "locked" | "dpapi" | "plain" | null; error: string | null } | null;
  settings: boolean;
  recents: boolean;
  favorites: boolean;
};
export type LegacyPrefs = {
  launchDelayMs: number | null;
  fpsUnlock: boolean | null;
  fpsValue: number | null;
  multiClient: boolean | null;
  recentsMax: number | null;
};
export type ImportRow = {
  key: string;
  userId: number | null;
  username: string;
  alias: string | null;
  group: string;
  willUpdate: boolean;
  hasCookie: boolean;
  hasPassword: boolean;
  headshot: string | null;
};
export type ImportPreview = {
  previewId: string;
  rows: ImportRow[];
  prefs: LegacyPrefs | null;
  games: { recents: number; favorites: number };
  /** Per-file problems (unreadable settings or game lists). */
  notes: string[];
};
export type ImportResult = {
  key: string;
  id: string | null;
  username: string;
  status: "added" | "updated" | "skipped" | "failed";
  message: string | null;
};

type None = Record<string, never>;

type Commands = {
  accounts_list: [None, AccountsList];
  accounts_add_cookies: [{ text: string }, AddResult[]];
  accounts_add_pending: [{ text: string }, AddResult[]];
  accounts_update: [{ ids: string[]; patch: AccountPatch }, void];
  accounts_remove: [{ ids: string[] }, { undoToken: string }];
  accounts_undo_remove: [{ undoToken: string }, void];
  accounts_reorder: [{ group: string; ids: string[] }, void];
  groups_reorder: [{ groups: string[] }, void];
  group_rename: [{ from: string; to: string }, void];
  group_target_set: [{ group: string; target: LaunchTarget | null }, void];
  account_copy: [{ id: string; field: CopyField }, void];
  account_reveal: [{ id: string; field: "cookie" | "password" }, string];
  reauth: [{ password: string | null }, void];
  signin_start: [
    { accountId: string | null; usernameHint: string | null; password: string | null },
    void,
  ];
  signin_kill_stray: [None, number];
  accounts_validate: [{ ids: string[] | null }, boolean];
  avatar_full: [{ userId: number }, string | null];
  import_pick_folder: [None, { path: string } | null];
  import_detect: [{ path: string }, ImportDetect];
  import_preview: [{ path: string; password: string | null; accounts: boolean }, ImportPreview];
  import_commit: [
    {
      previewId: string;
      keys: string[];
      importPrefs: boolean;
      importRecents: boolean;
      importFavorites: boolean;
    },
    { results: ImportResult[]; notes: string[]; retry: boolean },
  ];

  servers_stream: [
    { placeId: number; kind: ServerKind; accountId: string | null },
    { streamId: string },
  ];
  servers_stop: [{ streamId: string }, void];
  servers_find_player: [
    { placeId: number; username: string; accountId: string },
    { streamId: string },
  ];
  servers_regions: [
    { placeId: number; jobIds: string[]; accountId: string },
    Record<string, Region>,
  ];
  servers_reserve_join: [
    { placeId: number; jobId: string; accountIds: string[] },
    { batchId: string },
  ];
  games_discover: [None, GameTile[]];
  games_search: [
    { query: string; pageToken: string | null },
    { games: GameTile[]; nextPageToken: string | null },
  ];
  game_details: [{ placeIds: number[] }, GameTile[]];
  universe_of: [{ placeId: number }, { universeId: number }];
  universe_places: [{ universeId: number }, UniversePlace[]];
  favorites_list: [None, Favorite[]];
  favorites_add: [{ placeId: number; name: string; privateLink: string | null }, Favorite];
  favorites_update: [
    { id: string; patch: { name?: string; privateLink?: string | null } },
    Favorite,
  ];
  favorites_remove: [{ id: string }, void];
  favorites_reorder: [{ ids: string[] }, void];
  recents_list: [None, Recent[]];
  recents_clear: [None, void];
  outfits_list: [{ username: string }, { userId: number; outfits: Outfit[] }];
  outfit_wear: [{ outfitId: number; accountId: string }, { missing: MissingAsset[] }];

  tool_pin_state: [{ accountId: string }, PinState];
  tool_pin_unlock: [{ accountId: string; pin: string }, PinState];
  tool_display_name: [{ batchId: string; accountIds: string[]; name: string }, BatchResult[]];
  tool_password: [{ accountId: string; current: string; next: string }, void];
  tool_email_get: [{ accountId: string }, { masked: string | null; verified: boolean }];
  tool_email_set: [{ accountId: string; email: string; password: string }, void];
  tool_privacy_get: [{ accountId: string }, Privacy];
  tool_privacy_set: [
    { batchId: string; accountIds: string[]; follow?: string; privateServerInvite?: string },
    BatchResult[],
  ];
  tool_signout_others: [{ accountId: string }, void];
  tool_friend_request: [{ batchId: string; accountIds: string[]; username: string }, BatchResult[]];
  tool_block: [
    { batchId: string; accountIds: string[]; username: string; block: boolean },
    BatchResult[],
  ];
  tool_blocked_list: [{ accountId: string }, BlockedUser[]];
  tool_unblock_all: [{ accountId: string }, { count: number; failed: number; truncated: boolean }];
  tool_robux: [{ accountId: string }, { robux: number }];
  tool_info: [{ accountId: string }, AccountInfo];
  tool_export_summary: [{ accountId: string }, { path: string } | null];
  account_open_site: [{ accountId: string; url: string }, void];
  /** Available update version. `force` checks now, else the last check's result. */
  update_check: [{ force: boolean }, string | null];
  update_install: [Record<string, never>, void];

  vault_status: [Record<string, never>, VaultStatus];
  vault_create: [{ mode: "os" | "password"; password: string | null }, void];
  vault_unlock: [{ password: string | null; remember: boolean }, void];
  vault_lock: [Record<string, never>, void];
  vault_touch: [Record<string, never>, void];
  settings_get: [Record<string, never>, Settings];
  settings_set: [{ patch: unknown }, Settings];
  /** Validates the JSON file and stores a copy; null clears. */
  client_settings_set: [{ path: string | null }, Settings];
  settings_defaults: [Record<string, never>, Settings];
  open_folder: [{ kind: "data" | "logs" | "backups" }, void];
  /** Windows only: one UAC prompt adds the RMI folder to Windows Security exclusions; returns the folder. */
  defender_exclude: [Record<string, never>, string];
  defender_exclude_cmd: [Record<string, never>, string];
  ax_trusted: [{ prompt: boolean }, boolean];
  vault_change_password: [{ current: string | null; new: string | null }, void];
  vault_backups: [Record<string, never>, Backup[]];
  vault_restore: [{ name: string; password: string | null }, void];
  vault_export_encrypted: [{ path: string; password: string; current: string | null }, void];
  vault_export_plain: [{ path: string; confirm: string; password: string | null }, void];
  vault_import_file: [{ path: string; password: string | null }, void];
  platform_capabilities: [Record<string, never>, Capabilities];
  /** Missing fields fall back to the saved `gridArrange` setting. Returns windows handled. */
  grid_arrange: [{ monitor?: string; order?: string[]; layout?: GridLayout }, number];
  monitors_list: [Record<string, never>, MonitorInfo[]];
  /** 3 or 4 best layouts for `count` windows (1: one), best first. */
  grid_suggest: [{ count: number }, GridSuggestion[]];
  /** Number badges 1..N (slot order) on the Roblox windows for ~3 s, no focus change.
   * Missing `order`: the saved one. Returns windows marked (0 off Windows). */
  grid_identify: [{ order?: string[] }, number];
  /** Accounts with an open Roblox game window on this PC; `unknown` = windows of no account. */
  grid_windows: [Record<string, never>, { accounts: string[]; unknown: number }];
  app_info: [Record<string, never>, AppInfo];
  watcher_status: [Record<string, never>, WatcherStatus];
  watcher_kill: [{ pid: number }, void];
  automation_status: [Record<string, never>, AutomationStatus];
  automation_start: [Record<string, never>, AutomationStatus];
  automation_stop: [Record<string, never>, AutomationStatus];
  automation_token: [Record<string, never>, string];
  automation_token_regenerate: [Record<string, never>, string];
  /** Rust writes the token, or `template` with `{token}` filled in, to the clipboard
   *  (hidden from history, cleared after 30 s and on lock). */
  automation_token_copy: [{ template: string | null }, void];
  automation_notice_take: [Record<string, never>, string | null];
  friend_status: [Record<string, never>, FriendStatus];
  /** The login page link (public URL, no secret). Errors when the public URL is empty. */
  friend_link: [Record<string, never>, string];
  friend_ts_status: [Record<string, never>, TsStatus];
  /** Turns Tailscale Funnel on, saves the public address, returns it. Rejects with a TsError. */
  friend_ts_enable: [Record<string, never>, string];
  friend_ts_disable: [Record<string, never>, void];
  /** Newest HomeLab release for this OS. Rejects when offline, rate limited or not released yet. */
  homelab_latest: [Record<string, never>, HomeLabRelease];
  /** Downloads the installer to Downloads (Windows then opens it); returns the path. */
  homelab_download: [Record<string, never>, string];
  remote_status: [Record<string, never>, RemoteStatus];
  /** One-time pairing code for this computer (5 min). */
  remote_pair_code: [Record<string, never>, { code: string; expiresInSec: number }];
  remote_unpair: [{ id: string }, void];
  remote_stop: [Record<string, never>, void];
  remote_peers: [Record<string, never>, RemotePeers];
  /** Trades the code shown on `ip` for a token; returns that computer's name. */
  remote_pair: [{ ip: string; code: string }, string];
  remote_connect: [{ ip: string }, RemoteConnect];
  remote_forget: [{ ip: string }, void];
  /** Secret: the friend password. */
  friend_password: [Record<string, never>, string];
  friend_password_copy: [Record<string, never>, void];
  /** New friend password and token (signs the friend out); returns the password. */
  friend_new_password: [Record<string, never>, string];
  automation_snapshot: [Record<string, never>, AutomationAccount[]];
  automation_send: [{ account: string; text: string }, null];
  automation_ui_event: [{ account: string; name: string; value: unknown }, null];
  automation_save_console: [{ account: string }, boolean];
  automation_relaunch_set: [
    {
      account: string;
      enabled: boolean;
      delaySec: number;
      placeId?: number | null;
      jobId?: string | null;
    },
    null,
  ];
  roblox_status: [Record<string, never>, RobloxStatus];
  multi_set: [{ enabled: boolean }, Settings];
  cookie_lock_set: [{ enabled: boolean }, Settings];
  launch_start: [{ accountIds: string[]; bar: Bar; useBarForAll: boolean }, { batchId: string }];
  launch_cancel: [{ batchId: string }, void];
  launch_status: [None, LaunchSnapshot[]];
  destination_resolve: [{ input: string }, Preview];
};

// Launch contract (M4). Events: LAUNCH_PROGRESS, LAUNCH_DONE, "recents://changed".
export type LaunchMode = "any" | "specific" | "random" | "smallest";
/** `home`: an empty bar may open Roblox's home screen (UI launches always set it). */
export type Bar = { destination: string; server: string; mode: LaunchMode; home?: boolean };
export type LaunchState =
  "queued" | "validating" | "launching" | "launched" | "failed" | "cancelled";
export type LaunchProgress = {
  batchId: string;
  accountId: string;
  state: LaunchState;
  message: string | null;
  gameName: string | null;
};
export type LaunchDone = { batchId: string; launched: number; failed: number; cancelled: number };
/** A live batch from `launch_status` (per-account latest state). */
export type LaunchSnapshot = {
  launchId: string;
  done?: boolean;
  accounts?: Record<string, { state: LaunchState }>;
};
export const LAUNCH_PROGRESS = "launch://progress";
export const LAUNCH_DONE = "launch://done";
export const RECENTS_CHANGED = "recents://changed";
export const FAVORITES_CHANGED = "favorites://changed";
/** Tray menu: `{ placeId }` = recent with the current accounts, `{ accountId }` = play one. */
export const TRAY_PLAY = "tray://play";
export const TRAY_HINT = "tray://hint";
export type Preview = {
  kind: "place" | "private" | "follow" | "share" | "job" | "query" | "invalid";
  placeId?: number;
  universeId?: number;
  name?: string;
  icon?: string;
  playing?: number;
  privateLinkCode?: string;
  jobId?: string;
  follow?: { userId: number; username: string; presence: Presence | null };
  error?: string;
};

/** A command failed with VAULT_LOCKED / VAULT_WAS_LOCKED: the vault locked under us; App refetches status. */
export const VAULT_LOCKED_EVENT = "rmi:vault-locked";
/** User activity the window listeners can't see (keys kept inside a remote session); App
 *  counts it toward the idle auto-lock like any key press. */
export const ACTIVITY_EVENT = "rmi:activity";

/**
 * Rust error codes: the whole error is the code (`CODE`, or `cmd: CODE` once call() wraps it).
 * Branch on them with hasCode(), never on wording. Codes listed here also get catalog text.
 */
const CODE_TEXT: Record<string, MsgKey> = {
  VAULT_LOCKED: "err.vaultLocked",
  VAULT_WAS_LOCKED: "err.vaultWasLocked",
  WRONG_PASSWORD: "vault.err.wrong",
  VAULT_DAMAGED: "vault.err.damaged",
  PREVIEW_EXPIRED: "imp.expired",
  SIGNIN_BUSY: "signin.busy",
  UPDATE_BUSY: "settings.updateBusy",
  RESERVE_BUSY: "launch.busy",
  LAUNCH_BUSY: "launch.already",
  VAULT_BUSY: "vault.err.busy",
  NAME_NOT_SAVED: "tools.nameNotSaved",
  REAUTH_REQUIRED: "reauth.required",
  SESSION_EXPIRED: "tools.sessionExpired",
  PIN_REQUIRED: "tools.pin.batch",
  HOMELAB_NO_BUILD: "hl.noBuild",
};
const CODE = /^(\w+: )?([A-Z][A-Z0-9_]+)$/;
const raw = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The machine code an error (or batch row message) carries, else "". */
export const errCode = (e: unknown) => CODE.exec(raw(e))?.[2] ?? "";
export const hasCode = (e: unknown, code: string) => errCode(e) === code;

/** Swap a known code for its catalog text, keeping any `cmd: ` prefix. Other text passes through. */
export const codeText = (s: string) =>
  s.replace(CODE, (m, cmd: string | undefined, c: string) =>
    c in CODE_TEXT ? (cmd ?? "") + t(CODE_TEXT[c]) : m,
  );

/** The user-facing part of an error: catalog text for codes, minus the `cmd: ` prefix call() adds. */
export function errMessage(e: unknown): string {
  return codeText(raw(e)).replace(/^[a-z][a-z0-9_]*: /, "");
}

export async function call<K extends keyof Commands>(
  cmd: K,
  args?: Commands[K][0],
): Promise<Commands[K][1]> {
  let run: () => Promise<unknown>;
  if (inTauri) run = () => invoke(cmd, args);
  else if (import.meta.env.DEV || import.meta.env.MODE === "test")
    run = async () => (await import("./mock")).mockCall(cmd, args);
  else throw new Error(`${cmd}: not running inside Tauri`);
  try {
    return (await run()) as Commands[K][1];
  } catch (e) {
    const msg = raw(e);
    if (hasCode(msg, "VAULT_LOCKED") || hasCode(msg, "VAULT_WAS_LOCKED"))
      window.dispatchEvent(new Event(VAULT_LOCKED_EVENT));
    throw new Error(`${cmd}: ${msg}`, { cause: e });
  }
}

/** Save dialog; null when cancelled. Outside Tauri returns a fake path so flows stay testable. */
export async function pickSavePath(defaultPath: string, ext: string, label: string) {
  if (!inTauri) return `/mock/${defaultPath}`;
  const { save } = await import("@tauri-apps/plugin-dialog");
  return save({ defaultPath, filters: [{ name: label, extensions: [ext] }] });
}

/** Open dialog for one file; null when cancelled. Outside Tauri returns a fake path. */
export async function pickOpenPath(ext: string, label: string) {
  if (!inTauri) return `/mock/picked.${ext}`;
  const { open } = await import("@tauri-apps/plugin-dialog");
  return open({ multiple: false, directory: false, filters: [{ name: label, extensions: [ext] }] });
}

/** Dev/test event bus the mock emits on; Tauri events otherwise. */
export const mockBus = new EventTarget();

/** Subscribe to a backend event; returns an unsubscribe that is safe to call early. */
export function onEvent<T>(
  name: string,
  cb: (payload: T) => void,
): (() => void) & { ready: Promise<unknown> } {
  if (!inTauri) {
    const h = (e: Event) => cb((e as CustomEvent<T>).detail);
    mockBus.addEventListener(name, h);
    return Object.assign(() => mockBus.removeEventListener(name, h), { ready: Promise.resolve() });
  }
  let off: (() => void) | undefined;
  let dead = false;
  const ready = import("@tauri-apps/api/event")
    .then(({ listen }) =>
      listen<T>(name, (e) => cb(e.payload)).then((f) => (dead ? f() : (off = f))),
    )
    .catch((e) => console.error(`listen ${name} failed: ${String(e)}`));
  return Object.assign(
    () => {
      dead = true;
      off?.();
    },
    { ready },
  );
}

/** Local file path → webview URL (Tauri asset protocol); null outside Tauri. */
export const fileSrc = (path: string | null) => (path && inTauri ? convertFileSrc(path) : null);
