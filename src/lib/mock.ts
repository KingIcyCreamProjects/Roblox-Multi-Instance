// Dev/test-only stand-in for the Rust core, so screens render in a plain browser.
// All data here is synthetic.
import {
  mockBus,
  type AccountView,
  type AddResult,
  type GameTile,
  type LaunchTarget,
  type PresenceState,
} from "./ipc";
import { useUi, type Route } from "@/app/store";
import { useDiscover, type DiscoverTab } from "@/features/discover/shared";
import { lastLaunchSet, saveLaunchSet } from "@/features/playbar/store";
import { discoverCall, pushRecent } from "./mockDiscover";
import { resetToolsMock, toolsCall } from "./mockTools";

// Keep in sync with defaults() in src-tauri/src/settings/mod.rs (a Rust test checks the UI keys exist there).
const DEFAULTS: Record<string, unknown> = {
  version: 1,
  launch: {
    delayMs: 2500,
    replacePrevious: true,
    randomPages: 5,
    restoreWindowRect: false,
    startMinimized: false,
  },
  recents: { max: 20 },
  startup: { launchOnLogin: false, minimized: false },
  tray: { enabled: true },
  updates: { auto: true, seen: null },
  multi: { enabled: true, cookieLock: true },
  fps: { unlock: true, value: 60 },
  graphics: { set: true, level: 1 },
  client: { customSettingsPath: null, bootstrapperPassthrough: true },
  presence: { enabled: true, intervalSec: 30 },
  session: { validateIntervalMin: 30 },
  images: { enabled: true },
  ui: {
    showUsernames: true,
    density: "comfortable",
    scale: 100,
    reducedMotion: "system",
    tabOrder: ["home", "accounts", "discover", "automation", "devices", "settings"],
  },
  vault: { autoLockMin: 15, lockOnSleep: true },
  geo: {
    enabled: false,
    urlTemplate: "http://ip-api.com/json/{ip}",
    format: "{city}, {countryCode}",
  },
  automation: { enabled: false },
  ws: { port: 7810, startOnLaunch: false },
  api: {
    port: 7811,
    enableRead: false,
    enableLaunch: false,
    enableEdit: false,
    enableSocial: false,
    enableSecrets: false,
  },
  net: { allowLan: false },
  friend: { enabled: false, port: 7812, publicUrl: "", username: "friend", canAdd: false },
  remote: { enabled: false, port: 7813, deviceName: "", files: false, audio: true },
  relaunch: { spacingSec: 9, defaultDelaySec: 60, checkInternet: true, usePresence: false },
  clients: { minimizeEverySec: 0, closeAfterMin: 0, closeScope: "instance", maxInstances: 0 },
  homelab: { export: false },
  antiAfk: { enabled: false, everyMin: 10, idleSec: 5 },
  watcher: {
    enabled: false,
    scanSec: 6,
    closeLowMemory: false,
    lowMemoryMb: 200,
    closeWrongTitle: false,
    expectedTitle: "Roblox",
    closeNoServer: false,
    noServerSec: 60,
    ignoreExisting: true,
    saveWindowRects: false,
    reconnect: false,
    waitForGame: false,
  },
  theme: "dark",
  dev: { enabled: false },
  // tourDone true: screen tests should not open under the tour (Tour.test sets it false)
  onboarding: {
    tosSeen: false,
    trayToastSeen: false,
    tourDone: true,
    tourSeen: [
      "welcome",
      "add",
      "pick",
      "perf",
      "tabs",
      "friend",
      "remote",
      "homelab",
      "def",
      "mac",
    ],
  },
  gridArrange: { monitor: null, order: [], layout: { type: "columns", count: 3 } },
};
let settings: Record<string, unknown> = structuredClone(DEFAULTS);

type MockVault = {
  mode: "os" | "password" | null;
  password: string | null;
  unlocked: boolean;
  damaged: boolean;
  remember: boolean;
};
const FRESH: MockVault = {
  mode: null,
  password: null,
  unlocked: false,
  damaged: false,
  remember: false,
};
let vault: MockVault = { ...FRESH };
const BACKUPS = [
  { name: "vault-20260101-120000.rmiv", createdAt: "2026-01-01T12:00:00Z", size: 4096 },
];
/** Updater mock: `version` = available update; install always fails (no restart in tests). */
export const mockUpdate = { version: null as string | null };

/** Test knobs: simulate a damaged file or an active remember cache. */
export const mockVault = {
  setDamaged: (d: boolean) => (vault.damaged = d),
  setRemember: (r: boolean) => (vault.remember = r),
  /** Last args per command, for asserting what the UI sent. */
  lastArgs: {} as Record<string, unknown>,
  /** What `launch_status` returns (live batches). */
  launchLive: [] as unknown[],
};
const CAPS = {
  os: "windows",
  launch: true,
  launchReason: null as string | null,
  multiInstance: true,
  cookieLock: true,
  windowRect: true,
  watcher: true,
  clientHelpers: true,
};
export const mockCaps = { ...CAPS };
/** Test knob: encoding of the mock old-manager AccountData.json. */
export const mockImport = {
  encoding: "locked" as "locked" | "dpapi" | "plain",
  expire: false,
  /** commit reports a settings/games failure (Rust keeps the preview) */
  retry: false,
};
export const mockRoblox = { bootstrapper: null as string | null };
/** Test knobs (or `?ts=` / `?hl=` in the dev browser): Tailscale state and what Funnel does. */
const TS = { installed: false, running: false, dnsName: null, funnelPort: null };
export const mockTs = {
  status: { ...TS } as {
    installed: boolean;
    running: boolean;
    dnsName: string | null;
    funnelPort: number | null;
  },
  /** what friend_ts_enable does once Tailscale is running */
  enable: "ok" as "ok" | "needs_admin" | "failed",
};
/** HomeLab release lookup: found, offline (rejects) or never answers. */
export const mockHomeLab = {
  mode: "ok" as "ok" | "offline" | "loading" | "nobuild",
  page: "https://github.com/KingIcyCreamProjects/HomeLab/releases/tag/v1.4.0",
};
/** Remote access (or `?rp=missing|signedout|empty` in the dev browser). */
const peer = (name: string, os: string, n: number, rmi: string | null, more = {}) => ({
  ...{ name, os, ip: `100.64.0.${n}`, online: true, rmi, paired: false, busy: false },
  ...more,
});
const PEERS = [
  peer("den-pc", "windows", 11, "Den PC", { paired: true }),
  peer("laptop", "macos", 12, "Laptop"),
  peer("nas", "linux", 13, null),
  peer("phone", "ios", 14, null, { online: false }),
  peer("studio", "windows", 15, "Studio", { paired: true, busy: true }),
  peer("tablet", "android", 16, null),
];
const REMOTE = () => ({
  ts: "ok" as "ok" | "missing" | "signedout",
  peers: structuredClone(PEERS),
  codeLive: false,
  /** what remote_pair does */
  pair: "ok" as "ok" | "bad",
  paired: [{ id: "a1b2c3d4e5f6", name: "Laptop", pairedAt: "2026-11-01T10:00:00Z" }],
  log: [
    {
      peer: "Laptop",
      startedAt: "2026-11-02T18:30:00Z",
      durationSec: 754,
      control: true,
      files: [
        { name: "report.pdf", size: 245_000, dir: "sent" as const },
        { name: "notes.txt", size: 1200, dir: "sent" as const },
        { name: "photo.jpg", size: 3_400_000, dir: "received" as const },
      ],
    },
    {
      peer: "Laptop",
      startedAt: "2026-11-01T09:12:00Z",
      durationSec: 40,
      control: false,
      files: [] as { name: string; size: number; dir: "sent" | "received" }[],
    },
  ],
  session: null as { peer: string; since: string; control: boolean } | null,
});
export const mockRemote = REMOTE();
export const resetVaultMock = () => {
  Object.assign(mockRemote, REMOTE());
  Object.assign(mockCaps, CAPS);
  mockTs.status = { ...TS };
  mockTs.enable = "ok";
  mockHomeLab.mode = "ok";
  mockHomeLab.page = "https://github.com/KingIcyCreamProjects/HomeLab/releases/tag/v1.4.0";
  mockRoblox.bootstrapper = null;
  vault = { ...FRESH };
  mockVault.lastArgs = {};
  settings = structuredClone(DEFAULTS);
};

let automationRunning = false;
const automationStatus = () => ({
  wsRunning: automationRunning,
  apiRunning: automationRunning,
  wsPort: 7810,
  apiPort: 7811,
  lan: false,
});

const now = Date.now();
const MOCK_TOKEN = "deadbeef".repeat(7) + "cafe1234";
const MOCK_PW = "Mock2Friend3Pass4Abc";
let mockFriendPw = MOCK_PW;
const fakeClients: Record<string, unknown>[] = [
  {
    account: "Alice",
    connected: true,
    userId: 1001,
    placeId: 606849621,
    jobId: "a1b2c3d4-0000-0000-0000-000000000001",
    console: [
      { ts: now - 5000, level: "info", text: "script started" },
      { ts: now - 3000, level: "warn", text: "low health" },
      { ts: now - 1000, level: "error", text: "remote call failed" },
    ],
    controls: [
      { kind: "button", name: "Reset", value: null },
      { kind: "text", name: "Target", value: "spawn" },
      { kind: "break", name: "br" },
      { kind: "number", name: "Speed", value: 16, min: 0, max: 100, step: 1 },
      { kind: "label", name: "State", value: "idle" },
    ],
    relaunch: {},
  },
  {
    account: "Bob",
    connected: false,
    userId: 1002,
    placeId: null,
    jobId: null,
    console: [{ ts: now - 60000, level: "info", text: "disconnected" }],
    controls: [],
    relaunch: { enabled: true, seconds: 60 },
    relaunchStatus: {
      pendingAt: now + 45000,
      lastRelaunch: null,
      count: 0,
      stopped: false,
      launching: false,
    },
  },
];

function merge(a: Record<string, unknown>, b: Record<string, unknown>): Record<string, unknown> {
  const out = { ...a };
  for (const [k, v] of Object.entries(b)) {
    out[k] =
      v && typeof v === "object" && !Array.isArray(v) && typeof a[k] === "object"
        ? merge(a[k] as Record<string, unknown>, v as Record<string, unknown>)
        : v;
  }
  return out;
}

// ── accounts (synthetic) ─────────────────────────────────────────
const NAMES = [
  "Orbit",
  "Kestrel",
  "Nimbus",
  "Pylon",
  "Quartz",
  "Sable",
  "Tundra",
  "Vesper",
  "Willow",
  "Zephyr",
  "Cobalt",
  "Ember",
  "Fable",
  "Harbor",
  "Juniper",
  "Lumen",
];
const GAMES = ["Adopt Me!", "Blox Fruits", "Brookhaven RP", "Pet Simulator 99", "DOORS"];
const GROUPS = ["Main", "Alts", "Grinders"];
const iso = (minsAgo: number) => new Date(Date.now() - minsAgo * 60_000).toISOString();

function synth(i: number, group: string, order: number): AccountView {
  const name = `${NAMES[i % NAMES.length]}${i >= NAMES.length ? i : ""}`;
  const kinds: PresenceState[] = [
    "online",
    "in_game",
    "offline",
    "in_game",
    "offline",
    "in_studio",
  ];
  const state = kinds[i % kinds.length];
  return {
    id: `acc-${i}`,
    userId: 1000 + i,
    username: `${name.toLowerCase()}_rbx`,
    displayName: name,
    alias: i % 5 === 1 ? `${name} main` : null,
    note: i % 4 === 0 ? "Daily login streak account." : null,
    group,
    color: i % 3 === 0 ? (i % 8) + 1 : null,
    pinned: false,
    order,
    createdAt: iso(60 * 24 * 30),
    lastLaunchedAt: i % 2 ? iso(i * 37) : null,
    lastValidatedAt: iso(15),
    session: "ok",
    presence:
      state === "offline"
        ? null
        : {
            state,
            placeId: state === "in_game" ? 920587237 + i : null,
            universeId: null,
            gameName: state === "in_game" ? GAMES[i % GAMES.length] : null,
            jobId: null,
            updatedAt: iso(1),
          },
    launchTarget: null,
    fpsCap: null,
    graphics: null,
    windowRect: null,
    trackerId: `t-${i}`,
    friendAllowed: false,
    friendPaused: false,
    friendLastPlace: null,
    friendAdded: false,
    extra: {},
    hasPassword: i % 2 === 0,
    hasCookie: true,
    headshot: null,
  };
}

let accounts: AccountView[] = [];
const PATCHABLE = [
  "graphics",
  "alias",
  "note",
  "group",
  "color",
  "pinned",
  "launchTarget",
  "fpsCap",
  "friendAllowed",
  "extra",
];

/** Tests: set backend-only fields (friendAdded, friendPaused...) the patch command refuses. */
export function seedAccount(id: string, fields: Partial<AccountView>) {
  Object.assign(
    accounts.find((v) => v.id === id)!,
    fields,
  );
}
let groups: string[] = [];
let groupTargets: Record<string, LaunchTarget> = {};
let removed = new Map<string, AccountView[]>();
let reauthAt = 0;

/** Bumped by every reset: a previous test's launch timers go quiet. Declared before the reset that runs at load. */
let launchGen = 0;

/** n synthetic accounts across 3 groups (one pinned, one expired, one pending). */
export function resetAccountsMock(n = 0) {
  accounts = Array.from({ length: n }, (_, i) => synth(i, GROUPS[i % 3], Math.floor(i / 3)));
  groups = n ? [...GROUPS] : [];
  // dev/screenshots: "Main" ships a saved destination (Rebirth Champions: Ultimate)
  groupTargets =
    n && import.meta.env.MODE !== "test"
      ? { Main: { placeId: 74260430392611, jobId: null, privateLink: null } }
      : {};
  removed = new Map();
  reauthAt = 0;
  launchGen++; // a previous test's launch timers go quiet
  resetToolsMock();
  if (n > 2) {
    accounts[0].pinned = true;
    accounts[2] = { ...accounts[2], session: "expired" };
    accounts[n - 1] = {
      ...accounts[n - 1],
      userId: null,
      session: "pending",
      presence: null,
      hasCookie: false,
      hasPassword: true,
    };
  }
}
if (import.meta.env.MODE !== "test") {
  const params = new URLSearchParams(location.search);
  // ?vault=open skips the vault gate in browser dev
  if (params.get("vault") === "open") vault = { ...FRESH, mode: "os", unlocked: true };
  // ?vault=locked (password "password") / ?vault=damaged show the unlock and recovery screens
  if (params.get("vault") === "locked")
    vault = { ...FRESH, mode: "password", password: "password" };
  if (params.get("vault") === "damaged")
    vault = { ...FRESH, mode: "password", password: "password", damaged: true };
  const q = Number(params.get("accounts"));
  resetAccountsMock(params.has("accounts") && Number.isFinite(q) && q >= 0 ? q : 14);
  // a last-used set (Orbit, Kestrel) so Home's Play is live in browser dev
  if (!lastLaunchSet().length) saveLaunchSet(["acc-0", "acc-1"]);
  // ?route=discover&tab=servers&place=123 opens a screen directly in browser dev
  const route = params.get("route");
  if (route) useUi.setState({ route: route as Route });
  const tab = params.get("tab");
  if (tab) useDiscover.setState({ tab: tab as DiscoverTab });
  const place = Number(params.get("place"));
  if (place > 0) setTimeout(() => useDiscover.getState().viewServers(place), 500);
}

const emit = (name: string, detail?: unknown) =>
  mockBus.dispatchEvent(new CustomEvent(name, { detail }));

function addLines(
  text: string,
  parse: (l: string) => string | null,
  bad: string,
  pending: boolean,
) {
  const out: AddResult[] = [];
  text.split(/\r?\n/).forEach((raw, i) => {
    const l = raw.trim();
    if (!l) return;
    const username = parse(l);
    if (!username)
      return out.push({ line: i + 1, status: "invalid", username: null, message: bad });
    const dup = accounts.find((a) => a.username === username);
    if (dup) return out.push({ line: i + 1, status: "updated", username, message: null });
    const a = synth(accounts.length + 100 + i, "Unsorted", accounts.length);
    accounts.push({
      ...a,
      id: `acc-new-${Date.now()}-${i}`,
      username,
      displayName: username,
      presence: null,
      ...(pending ? { userId: null, session: "pending", hasCookie: false, hasPassword: true } : {}),
    });
    if (!groups.includes("Unsorted")) groups.push("Unsorted");
    out.push({ line: i + 1, status: "added", username, message: null });
  });
  if (out.some((r) => r.status === "added")) emit("accounts://changed");
  return out;
}

function accountsCall(cmd: string, a: Record<string, unknown>): unknown {
  const find = (id: unknown) => {
    const x = accounts.find((v) => v.id === id);
    if (!x) throw "Account not found.";
    return x;
  };
  switch (cmd) {
    case "accounts_list":
      return {
        accounts: structuredClone(accounts),
        groups: [...groups],
        groupTargets: Object.fromEntries(
          Object.entries(groupTargets).filter(([g]) => accounts.some((x) => x.group === g)),
        ),
      };
    case "accounts_add_cookies":
      return addLines(
        a.text as string,
        (l) => (/_\|WARNING:/.test(l) ? `player_${(l.length * 7919) % 100000}` : null),
        "This line is not a Roblox session cookie.",
        false,
      );
    case "accounts_add_pending":
      return addLines(
        a.text as string,
        (l) => (/^[A-Za-z0-9_]{3,20}:.+/.test(l) ? l.split(":")[0] : null),
        "Use username:password, one per line.",
        true,
      );
    case "accounts_update":
      // Same rule as Rust's PATCHABLE: backend-only flags (friendAdded...) can't be patched.
      for (const k of Object.keys(a.patch as object))
        if (!PATCHABLE.includes(k)) throw `${k} can't be changed.`;
      for (const id of a.ids as string[]) {
        const x = find(id);
        Object.assign(x, a.patch);
        if (!groups.includes(x.group)) groups.push(x.group);
      }
      return;
    case "accounts_remove": {
      const ids = a.ids as string[];
      const token = `undo-${Date.now()}`;
      removed.set(
        token,
        accounts.filter((x) => ids.includes(x.id)),
      );
      accounts = accounts.filter((x) => !ids.includes(x.id));
      setTimeout(() => removed.delete(token), 8000);
      return { undoToken: token };
    }
    case "accounts_undo_remove": {
      const back = removed.get(a.undoToken as string);
      if (!back) throw "Undo window has passed.";
      accounts.push(...back);
      removed.delete(a.undoToken as string);
      return;
    }
    case "accounts_reorder":
      (a.ids as string[]).forEach((id, i) => (find(id).order = i));
      return;
    case "groups_reorder":
      groups = [...(a.groups as string[])];
      return;
    case "group_rename":
      accounts.forEach((x) => x.group === a.from && (x.group = a.to as string));
      groups = groups.map((g) => (g === a.from ? (a.to as string) : g));
      if (groupTargets[a.from as string]) {
        groupTargets[a.to as string] ??= groupTargets[a.from as string];
        delete groupTargets[a.from as string];
      }
      return;
    case "group_target_set":
      if (a.target) {
        if (!accounts.some((x) => x.group === a.group)) throw "That group has no accounts.";
        groupTargets[a.group as string] = a.target as LaunchTarget;
      } else delete groupTargets[a.group as string];
      return;
    case "account_copy":
    case "account_reveal":
      if ((a.field === "cookie" || a.field === "password") && Date.now() - reauthAt > 120_000)
        throw "REAUTH_REQUIRED";
      return cmd === "account_reveal" ? "(mock secret)" : undefined;
    case "reauth":
      if (vault.mode === "password" && a.password !== vault.password) throw "WRONG_PASSWORD";
      reauthAt = Date.now();
      return;
    case "signin_start":
      setTimeout(() => {
        addLines("_|WARNING:mock", () => `signin_${Date.now() % 10000}`, "", false);
        emit("signin://result", { ok: true, username: "signin_user", error: null });
      }, 1200);
      return;
    case "signin_kill_stray":
      return 0;
    case "accounts_validate":
      return true;
    case "avatar_full":
      return null;
    case "import_pick_folder":
      return { path: "C:\\Users\\me\\OldManager" };
    case "import_detect":
      return {
        accountData: { encoding: mockImport.encoding, error: null },
        settings: true,
        recents: true,
        favorites: false,
      };
    case "import_preview": {
      if (a.accounts && mockImport.encoding === "locked" && a.password !== "oldpass")
        throw "Wrong old password, or the file is damaged.";
      const rows = ["imp_one", "imp_two", "imp_three"].map((username, i) => ({
        key: String(i),
        userId: 5000 + i,
        username,
        alias: i === 0 ? "Old main" : null,
        group: i === 2 ? "Alts" : "Unsorted",
        willUpdate: false,
        hasCookie: true,
        hasPassword: i === 1,
        headshot: null,
      }));
      // one row matches an existing account
      if (accounts[0]) rows[0] = { ...rows[0], userId: accounts[0].userId ?? 0, willUpdate: true };
      return {
        previewId: "pv-1",
        rows: a.accounts ? rows : [],
        prefs: {
          launchDelayMs: 3000,
          fpsUnlock: true,
          fpsValue: 240,
          multiClient: true,
          recentsMax: 20,
        },
        games: { recents: 4, favorites: 0 },
        notes: [],
      };
    }
    case "import_commit": {
      if (mockImport.expire) throw "PREVIEW_EXPIRED";
      const results = (a.keys as string[]).map((key) => {
        const username = ["imp_one", "imp_two", "imp_three"][Number(key)];
        const old = key === "0" && accounts[0];
        if (old) return { key, id: old.id, username, status: "updated", message: null };
        const v = synth(accounts.length + 200, "Unsorted", accounts.length);
        const id = `acc-imp-${key}`;
        accounts.push({ ...v, id, username, displayName: username, session: "unknown" });
        if (!groups.includes("Unsorted")) groups.push("Unsorted");
        return { key, id, username, status: "added", message: null };
      });
      emit("accounts://changed");
      // the validation pass: the second import comes back expired
      setTimeout(() => {
        accounts = accounts.map((x) =>
          x.id.startsWith("acc-imp-")
            ? { ...x, session: x.id.endsWith("1") ? "expired" : "ok" }
            : x,
        );
        emit("accounts://changed");
      }, 50);
      return {
        results,
        notes: a.importPrefs ? ["Settings imported."] : [],
        retry: mockImport.retry,
      };
    }
  }
  return NOT_ACCOUNTS;
}
const NOT_ACCOUNTS = Symbol();

// Launch mock: each account steps validating → launching → launched, 600 ms apart.
const cancelled = new Set<string>();
let batchSeq = 0;

function launchCall(cmd: string, a: Record<string, unknown>): unknown {
  switch (cmd) {
    case "launch_start": {
      const ids = a.accountIds as string[];
      if (!ids.length) throw "Pick at least one account.";
      const batchId = `mock-batch-${++batchSeq}`;
      const gen = launchGen;
      const p = (accountId: string, state: string, extra = {}) =>
        gen === launchGen &&
        emit("launch://progress", {
          batchId,
          accountId,
          state,
          message: null,
          gameName: null,
          ...extra,
        });
      ids.forEach((id) => p(id, "queued"));
      const done = { batchId, launched: 0, failed: 0, cancelled: 0 };
      ids.forEach((id, i) => {
        const t = 300 + i * 600;
        setTimeout(() => !cancelled.has(batchId) && p(id, "validating"), t);
        setTimeout(() => !cancelled.has(batchId) && p(id, "launching"), t + 200);
        setTimeout(() => {
          if (cancelled.has(batchId)) {
            done.cancelled++;
            p(id, "cancelled");
          } else {
            done.launched++;
            p(id, "launched", { gameName: "DOORS" });
            pushRecent({
              placeId: 6516141723,
              name: "DOORS",
              thumbnail: (
                discoverCall("game_details", { placeIds: [6516141723] }, emit) as GameTile[]
              )[0].thumbnail,
              lastPlayedAt: iso(0),
            });
            emit("recents://changed");
          }
          if (i === ids.length - 1 && gen === launchGen) emit("launch://done", done);
        }, t + 400);
      });
      return { batchId };
    }
    case "grid_arrange":
    case "grid_identify":
      return 0;
    case "grid_windows":
      return { accounts: ["acc-1", "acc-2"], unknown: 1 };
    case "grid_suggest": {
      const n = Number(a.count);
      if (n < 1) return [];
      if (n === 1) return [{ label: "Full screen", layout: { type: "single" } }];
      return [
        {
          label: n === 2 ? "Side by side" : `${Math.ceil(n / 2)} + ${Math.floor(n / 2)}`,
          layout: { type: "auto" },
        },
        {
          label: `Main big + ${n - 1} small`,
          layout: { type: "focus", main: "", others: "strip" },
        },
        { label: "Cascade", layout: { type: "cascade" } },
      ];
    }
    case "monitors_list":
      return [
        {
          id: "\\\\.\\DISPLAY1",
          label: "Monitor 1",
          work: { x: 0, y: 0, w: 2560, h: 1392 },
          rect: { x: 0, y: 0, w: 2560, h: 1440 },
          primary: true,
          scale: 1.25,
        },
        {
          id: "\\\\.\\DISPLAY2",
          label: "Monitor 2",
          work: { x: 2560, y: 0, w: 1920, h: 1032 },
          rect: { x: 2560, y: 0, w: 1920, h: 1080 },
          primary: false,
          scale: 1,
        },
      ];
    case "launch_cancel":
      cancelled.add(a.batchId as string);
      return undefined;
    case "launch_status":
      return mockVault.launchLive;
    case "destination_resolve": {
      const s = String(a.input ?? "").trim();
      if (!s) return { kind: "invalid", error: "Enter a place id, game link or @username." };
      if (s.startsWith("@"))
        return { kind: "follow", follow: { userId: 1, username: s.slice(1), presence: null } };
      if (/^\d+$/.test(s) || s.includes("roblox.com/games/")) {
        const id = Number(/^\d+$/.test(s) ? s : s.match(/games\/(\d+)/)?.[1]);
        const g = (discoverCall("game_details", { placeIds: [id] }, emit) as GameTile[])[0];
        return {
          kind: "place",
          placeId: id,
          universeId: g.universeId,
          name: g.name,
          icon: g.icon,
          playing: g.playing,
        };
      }
      return { kind: "query" };
    }
  }
  return NOT_ACCOUNTS;
}

export async function mockCall(cmd: string, args?: unknown): Promise<unknown> {
  const theme = new URLSearchParams(location.search).get("theme");
  if (theme && settings.theme !== theme) settings = { ...settings, theme };
  mockVault.lastArgs[cmd] = args;
  const reauth = (pw: string | null | undefined) => {
    if (vault.mode === "password" && pw !== vault.password) throw "WRONG_PASSWORD";
  };
  const needOpen = () => {
    if (!vault.unlocked) throw "VAULT_LOCKED";
  };
  const r = accountsCall(cmd, (args ?? {}) as Record<string, unknown>);
  if (r !== NOT_ACCOUNTS) return r;
  const l = launchCall(cmd, (args ?? {}) as Record<string, unknown>);
  if (l !== NOT_ACCOUNTS) return l;
  const d = discoverCall(cmd, (args ?? {}) as Record<string, unknown>, emit);
  if (d !== undefined) return d ?? undefined;
  const tl = toolsCall(cmd, (args ?? {}) as Record<string, unknown>, {
    emit,
    displayNames: accounts,
  });
  if (tl !== undefined) return tl ?? undefined;
  switch (cmd) {
    case "vault_status":
      return {
        exists: vault.mode !== null,
        unlocked: vault.unlocked,
        mode: vault.damaged ? null : vault.mode,
        rememberActive: vault.remember,
        damaged: vault.damaged && !vault.unlocked,
      };
    case "vault_create": {
      const a = args as { mode: "os" | "password"; password: string | null };
      if (a.mode === "password" && (a.password?.length ?? 0) < 8)
        throw "Password must be at least 8 characters.";
      vault = { ...FRESH, mode: a.mode, password: a.password, unlocked: true };
      return;
    }
    case "vault_unlock": {
      const a = args as { password: string | null; remember: boolean };
      if (vault.damaged) throw "VAULT_DAMAGED";
      if (vault.mode === "password" && a.password === null && !vault.remember)
        throw "Password required.";
      if (a.password !== null) reauth(a.password);
      if (a.password !== null) vault.remember = a.remember && vault.mode === "password";
      vault.unlocked = true;
      return;
    }
    case "vault_lock":
      vault.unlocked = false;
      return;
    case "vault_touch":
      return;
    case "vault_change_password": {
      const a = args as { current: string | null; new: string | null };
      needOpen();
      reauth(a.current);
      if (a.new !== null && a.new.length < 8) throw "Password must be at least 8 characters.";
      vault = { ...vault, mode: a.new === null ? "os" : "password", password: a.new };
      return;
    }
    case "vault_backups":
      return BACKUPS;
    case "vault_restore": {
      const a = args as { name: string; password: string | null };
      if (!BACKUPS.some((b) => b.name === a.name)) throw "Unknown backup.";
      if (vault.unlocked) return; // payload swap, stays unlocked
      if (!vault.damaged) throw "The vault opens fine: unlock it, then restore from Settings.";
      reauth(a.password);
      vault.damaged = false; // file replaced; still locked
      return;
    }
    case "vault_export_encrypted": {
      const a = args as { current: string | null };
      needOpen();
      reauth(a.current);
      return;
    }
    case "vault_export_plain": {
      const a = args as { confirm: string; password: string | null };
      needOpen();
      if (a.confirm !== "export plaintext") throw 'Type "export plaintext" to confirm.';
      reauth(a.password);
      return;
    }
    case "vault_import_file":
      needOpen();
      return;
    case "ax_trusted":
      return true;
    case "defender_exclude":
    case "defender_exclude_cmd":
      return String.raw`C:\Users\demo\AppData\Local\Roblox Multi Instance`;
    case "open_folder":
      return;
    case "settings_defaults":
      return DEFAULTS;
    case "settings_get":
      return settings;
    case "settings_set":
      settings = merge(settings, (args as { patch: Record<string, unknown> }).patch);
      return settings;
    case "client_settings_set": {
      const p = (args as { path: string | null }).path;
      if (p && !p.endsWith(".json")) throw "That file isn't valid JSON.";
      settings = merge(settings, { client: { customSettingsPath: p } });
      return settings;
    }
    case "platform_capabilities":
      return mockCaps;
    case "watcher_status":
      return { running: false, clients: [], rules: [] };
    case "watcher_kill":
      return;
    case "update_check":
      return mockUpdate.version;
    case "update_install":
      throw new Error("The update could not be installed: offline");
    case "app_info":
      return {
        version: "0.1.0-dev",
        dataDir: "(browser mock)",
        portable: false,
        settingsReadOnly: false,
        acrylic: true,
        settingsRecovered: null,
      };
    case "automation_notice_take":
      return null;
    case "automation_status":
      return automationStatus();
    case "automation_start":
    case "automation_stop":
      automationRunning = cmd === "automation_start";
      return automationStatus();
    case "automation_token":
    case "automation_token_regenerate":
      return MOCK_TOKEN;
    case "friend_status":
      return { running: false, port: 7812, publicUrl: "" };
    case "friend_link":
      return "https://rmi.example.ts.net";
    case "friend_ts_status": {
      const q = new URLSearchParams(location.search).get("ts");
      if (q === "installed") mockTs.status = { ...TS, installed: true };
      if (q === "ready" || q === "admin" || q === "done")
        mockTs.status = {
          installed: true,
          running: true,
          dnsName: "rmi.example.ts.net",
          funnelPort: null,
        };
      if (q === "admin") mockTs.enable = "needs_admin";
      return { ...mockTs.status };
    }
    case "friend_ts_enable": {
      if (!mockTs.status.running)
        throw {
          code: "not_running",
          url: null,
          message: "Tailscale is not running or not signed in.",
        };
      if (mockTs.enable === "needs_admin")
        throw {
          code: "needs_admin",
          url: "https://login.tailscale.com/f/funnel?node=nEXAMPLE",
          message: "Turn on HTTPS and Funnel for your tailnet, then try again.",
        };
      if (mockTs.enable === "failed")
        throw { code: "failed", url: null, message: "Tailscale could not turn on Funnel." };
      const url = "https://rmi.example.ts.net";
      settings = merge(settings, { friend: { publicUrl: url, enabled: true } });
      mockTs.status.funnelPort = 443;
      return url;
    }
    case "friend_ts_disable":
      mockTs.status.funnelPort = null;
      return;
    case "remote_status": {
      const r = settings.remote as { enabled: boolean; port: number; deviceName: string };
      return {
        listening: r.enabled ? `100.64.0.10:${r.port}` : null,
        name: r.deviceName.trim() || "This PC",
        session: mockRemote.session,
        paired: mockRemote.paired,
        log: mockRemote.log,
        codeLive: mockRemote.codeLive,
      };
    }
    case "remote_pair_code":
      if (!(settings.remote as { enabled: boolean }).enabled)
        throw "Turn on remote access and save first.";
      mockRemote.codeLive = true;
      return { code: "K7MP4XQ2", expiresInSec: 300 };
    case "remote_unpair": {
      const id = (args as { id: string }).id;
      mockRemote.paired = mockRemote.paired.filter((p) => p.id !== id);
      return;
    }
    case "remote_stop":
      mockRemote.session = null;
      return;
    case "remote_peers": {
      const q = new URLSearchParams(location.search).get("rp");
      const ts = q === "missing" || q === "signedout" ? q : mockRemote.ts;
      if (ts === "missing") return { installed: false, running: false, peers: [] };
      if (ts === "signedout") return { installed: true, running: false, peers: [] };
      return { installed: true, running: true, peers: q === "empty" ? [] : mockRemote.peers };
    }
    case "remote_pair": {
      const a = args as { ip: string; code: string };
      if (mockRemote.pair === "bad" || a.code.replace(/[\s-]/g, "").length !== 8)
        throw "Wrong code. Check it and try again.";
      const p = mockRemote.peers.find((x) => x.ip === a.ip);
      if (p) p.paired = true;
      return p?.rmi ?? "Computer";
    }
    case "remote_connect": {
      const ip = (args as { ip: string }).ip;
      const p = mockRemote.peers.find((x) => x.ip === ip);
      if (!p?.paired) throw "Pair with this computer first.";
      return { url: `ws://${ip}:7813/v1/remote/ws`, token: "0".repeat(64), name: p.rmi ?? p.name };
    }
    case "remote_forget": {
      const p = mockRemote.peers.find((x) => x.ip === (args as { ip: string }).ip);
      if (p) p.paired = false;
      return;
    }
    case "homelab_latest": {
      const q = new URLSearchParams(location.search).get("hl");
      const mode = q === "offline" || q === "loading" || q === "nobuild" ? q : mockHomeLab.mode;
      if (mode === "loading") return new Promise(() => {});
      if (mode === "offline") throw "GitHub answered 404.";
      if (mode === "nobuild") throw "HOMELAB_NO_BUILD";
      return {
        version: "1.4.0",
        asset: "HomeLabSetup.exe",
        size: 12_900_000,
        page: mockHomeLab.page,
      };
    }
    case "homelab_download":
      return "C:/Users/you/Downloads/HomeLabSetup.exe";
    case "friend_password":
      return mockFriendPw;
    case "friend_password_copy":
      return undefined;
    case "friend_new_password":
      return (mockFriendPw = mockFriendPw === MOCK_PW ? "Zx8Kq2Wm9Rt4Yp7Hc3Nd" : MOCK_PW);
    case "automation_token_copy": {
      const tpl = (args as { template: string | null }).template ?? "{token}";
      await navigator.clipboard?.writeText(tpl.replace("{token}", MOCK_TOKEN));
      return;
    }
    case "automation_snapshot":
      return structuredClone(fakeClients);
    case "automation_send": {
      const a = args as { account: string; text: string };
      const c = fakeClients.find((x) => x.account === a.account);
      (c?.console as unknown[] | undefined)?.push({
        ts: Date.now(),
        level: "info",
        text: "> " + a.text,
      });
      return null;
    }
    case "automation_save_console":
      return true;
    case "automation_relaunch_set": {
      const a = args as { account: string; enabled: boolean; delaySec: number };
      const c = fakeClients.find((x) => x.account === a.account);
      if (!c) throw new Error(`${a.account} has never connected`);
      c.relaunch = { enabled: a.enabled, seconds: a.delaySec };
      return null;
    }
    case "automation_ui_event":
      return null;
    case "roblox_status":
      return {
        installed: true,
        bootstrapper: mockRoblox.bootstrapper,
        versionDir: mockRoblox.bootstrapper ? null : "(browser mock)",
        running: false,
        multiActive: true,
        cookieLockActive: true,
        multiBlockedByRunningRoblox: false,
        fpsFlagAllowlisted: false,
        multiReleasePending: false,
        multiInProcess: false,
      };
    case "multi_set":
    case "cookie_lock_set": {
      const key = cmd === "multi_set" ? "enabled" : "cookieLock";
      settings = merge(settings, { multi: { [key]: (args as { enabled: boolean }).enabled } });
      return settings;
    }
  }
  throw new Error(`mock: unknown command ${cmd}`);
}
