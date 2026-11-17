// Settings catalog (spec §12). `path` is the dotted key in settings.json; label and
// description live in the i18n catalog as `set.<path>` and `set.<path>.d`.
import type { Capabilities } from "@/lib/ipc";
import type { MsgKey } from "@/lib/i18n";

export type Item = {
  path: string;
  kind: "bool" | "num" | "port" | "enum" | "text";
  min?: number;
  max?: number;
  step?: number;
  /** stored = shown * scale (launch delay is shown in seconds, stored in ms) */
  scale?: number;
  options?: string[];
  /** empty text saves null */
  nullable?: boolean;
  /** disabled with a reason when the platform lacks this capability */
  cap?: keyof Pick<
    Capabilities,
    | "launch"
    | "multiInstance"
    | "cookieLock"
    | "windowRect"
    | "watcher"
    | "clientHelpers"
    | "fullscreenWait"
    | "audio"
  >;
  /** red note under the row (i18n key) */
  warn?: MsgKey;
  /** bool paths this value depends on: any off = the row shows dimmed */
  dep?: string[];
  /** text fields: example value shown while empty */
  placeholder?: string;
  /** text commits on blur / Enter only: the setting writes at once, no Save step */
  onBlur?: boolean;
};

export type SectionId =
  | "general"
  | "roblox"
  | "accounts"
  | "vault"
  | "privacy"
  | "control"
  | "friend"
  | "remote"
  | "relaunch"
  | "watcher"
  | "appearance"
  | "developer"
  | "homelab"
  | "about";

const b = (path: string, extra: Partial<Item> = {}): Item => ({ path, kind: "bool", ...extra });
const n = (path: string, min: number, max: number, extra: Partial<Item> = {}): Item => ({
  path,
  kind: "num",
  min,
  max,
  ...extra,
});
const port = (path: string): Item => ({ path, kind: "port", min: 1024, max: 65535 });
const W = ["watcher.enabled"];
const A = ["automation.enabled"];

export const SECTIONS: { id: SectionId; items: Item[] }[] = [
  {
    id: "general",
    items: [
      n("launch.delayMs", 0, 60_000, { scale: 1000, step: 0.5 }),
      b("launch.replacePrevious"),
      n("launch.randomPages", 1, 50),
      b("launch.restoreWindowRect", { cap: "windowRect" }),
      // Minimizing goes through the same window setup as restoring the window rect.
      b("launch.startMinimized", { cap: "windowRect" }),
      n("recents.max", 1, 50),
      b("startup.launchOnLogin"),
      b("startup.minimized"),
      {
        path: "startup.accounts",
        kind: "text",
        onBlur: true,
        dep: ["startup.launchOnLogin"],
        placeholder: "Name1, Name2",
      },
      b("tray.enabled"),
      b("updates.auto"),
    ],
  },
  {
    id: "roblox",
    items: [
      b("multi.enabled", { cap: "multiInstance" }),
      b("multi.cookieLock", { cap: "cookieLock" }),
      // FPS and graphics are written to Roblox's own settings file, which only exists on
      // Windows and macOS (Sober on Linux keeps its own): gated like the other client helpers.
      b("fps.unlock", { cap: "clientHelpers" }),
      n("fps.value", 1, 240, { cap: "clientHelpers", dep: ["fps.unlock"] }),
      b("graphics.set", { cap: "clientHelpers" }),
      n("graphics.level", 1, 10, { cap: "clientHelpers", dep: ["graphics.set"] }),
      {
        path: "client.customSettingsPath",
        kind: "text",
        nullable: true,
        cap: "launch",
        onBlur: true,
      },
      b("antiAfk.enabled", { cap: "clientHelpers" }),
      n("antiAfk.everyMin", 1, 19, { cap: "clientHelpers", dep: ["antiAfk.enabled"] }),
      n("antiAfk.idleSec", 1, 120, { cap: "clientHelpers", dep: ["antiAfk.enabled"] }),
    ],
  },
  {
    id: "accounts",
    items: [
      b("presence.enabled"),
      n("presence.intervalSec", 15, 300, { dep: ["presence.enabled"] }),
      n("session.validateIntervalMin", 5, 1440),
      b("images.enabled"),
      b("ui.showUsernames"),
    ],
  },
  { id: "vault", items: [n("vault.autoLockMin", 0, 1440), b("vault.lockOnSleep")] },
  {
    id: "privacy",
    items: [
      b("geo.enabled"),
      { path: "geo.urlTemplate", kind: "text", dep: ["geo.enabled"] },
      { path: "geo.format", kind: "text", dep: ["geo.enabled"] },
    ],
  },
  {
    id: "control",
    items: [
      b("automation.enabled"),
      { ...port("ws.port"), dep: A },
      b("ws.startOnLaunch", { dep: A }),
      { ...port("api.port"), dep: A },
      b("api.enableRead", { dep: A }),
      b("api.enableLaunch", { dep: A }),
      b("api.enableEdit", { dep: A }),
      b("api.enableSocial", { dep: A }),
      b("api.enableSecrets", { dep: A }),
      b("net.allowLan", { warn: "settings.lanWarn", dep: A }),
    ],
  },
  {
    id: "friend",
    items: [
      b("friend.enabled"),
      { ...port("friend.port"), dep: ["friend.enabled"] },
      {
        path: "friend.publicUrl",
        kind: "text",
        dep: ["friend.enabled"],
        placeholder: "https://your-computer.ts.net",
      },
      {
        path: "friend.username",
        kind: "text",
        max: 32,
        placeholder: "friend",
        dep: ["friend.enabled"],
      },
      {
        path: "friend.ownerUsername",
        kind: "text",
        max: 32,
        placeholder: "owner",
        dep: ["friend.enabled"],
      },
      b("friend.canAdd", { dep: ["friend.enabled"], warn: "set.friend.canAdd.warn" }),
    ],
  },
  {
    id: "remote",
    items: [
      b("remote.enabled", { warn: "set.remote.enabled.warn" }),
      { ...port("remote.port"), dep: ["remote.enabled"] },
      { path: "remote.deviceName", kind: "text", dep: ["remote.enabled"] },
      b("remote.files", { dep: ["remote.enabled"] }),
      b("remote.audio", { cap: "audio", dep: ["remote.enabled"] }),
    ],
  },
  {
    id: "relaunch",
    items: [
      n("relaunch.spacingSec", 0, 3600),
      n("relaunch.defaultDelaySec", 0, 3600),
      b("relaunch.checkInternet"),
      b("relaunch.usePresence"),
      n("clients.minimizeEverySec", 0, 86_400, { cap: "clientHelpers", dep: A }),
      n("clients.closeAfterMin", 0, 10_080, { cap: "clientHelpers", dep: A }),
      {
        path: "clients.closeScope",
        kind: "enum",
        options: ["instance", "global"],
        cap: "clientHelpers",
        dep: A,
      },
      n("clients.maxInstances", 0, 100, { cap: "clientHelpers", dep: A }),
    ],
  },
  {
    id: "watcher",
    items: [
      b("watcher.enabled", { cap: "watcher" }),
      n("watcher.scanSec", 2, 600, { cap: "watcher", dep: W }),
      b("watcher.closeLowMemory", { cap: "watcher", dep: W }),
      n("watcher.lowMemoryMb", 20, 1000, { cap: "watcher", dep: [...W, "watcher.closeLowMemory"] }),
      b("watcher.closeWrongTitle", { cap: "windowRect", dep: W }),
      {
        path: "watcher.expectedTitle",
        kind: "text",
        cap: "windowRect",
        dep: [...W, "watcher.closeWrongTitle"],
      },
      b("watcher.closeNoServer", { cap: "watcher", dep: W }),
      n("watcher.noServerSec", 10, 3600, { cap: "watcher", dep: [...W, "watcher.closeNoServer"] }),
      b("watcher.ignoreExisting", { cap: "watcher", dep: W }),
      b("watcher.saveWindowRects", { cap: "windowRect", dep: W }),
      b("watcher.reconnect", { cap: "watcher", dep: W }),
      b("watcher.waitForGame", { cap: "fullscreenWait", dep: [...W, "watcher.reconnect"] }),
    ],
  },
  {
    id: "appearance",
    items: [
      { path: "theme", kind: "enum", options: ["system", "dark", "light"] },
      { path: "ui.density", kind: "enum", options: ["comfortable", "compact"] },
      n("ui.scale", 80, 150),
      { path: "ui.reducedMotion", kind: "enum", options: ["system", "on", "off"] },
    ],
  },
  { id: "homelab", items: [b("homelab.export")] },
  { id: "developer", items: [b("dev.enabled")] },
  { id: "about", items: [] },
];

/** The five groups the page shows; `subs` are SECTIONS ids, shown as titled blocks in one page. */
export const GROUPS: {
  id: "general" | "roblox" | "security" | "automation" | "app";
  subs: SectionId[];
}[] = [
  { id: "general", subs: ["general", "appearance"] },
  { id: "roblox", subs: ["roblox", "watcher"] },
  { id: "security", subs: ["accounts", "vault", "privacy"] },
  { id: "automation", subs: ["control", "friend", "remote", "relaunch"] },
  { id: "app", subs: ["developer", "about"] },
];

export const get = (o: unknown, path: string): unknown =>
  path.split(".").reduce<unknown>((a, k) => (a as Record<string, unknown> | undefined)?.[k], o);

/** {a:{b:1}} from ("a.b", 1) */
export const nest = (path: string, v: unknown): Record<string, unknown> =>
  path
    .split(".")
    .reverse()
    .reduce<unknown>((acc, k) => ({ [k]: acc }), v) as Record<string, unknown>;
