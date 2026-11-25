import { useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { Box, Check, Info, Lock, RotateCcw, Search, SlidersHorizontal, Zap } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { TabOrder } from "@/app/Tabs";
import { KEY_SET, useSetSettings, useSettings } from "@/app/useSettings";
import { Button } from "@/components/ui/Button";
import {
  call,
  errMessage,
  hasCode,
  pickOpenPath,
  type Capabilities,
  type RobloxStatus,
} from "@/lib/ipc";
import { t, type MsgKey } from "@/lib/i18n";
import { useUi } from "@/app/store";
import { GROUPS, SECTIONS, get, type Item, type SectionId } from "./catalog";
import { draftPatch, useDraft } from "./draft";
import { Setting } from "./controls";
import { TokenRows } from "@/features/automation/TokenSettings";
import { FriendExtras, FriendGate, ownerNameClash } from "./FriendSection";
import { HomeLabSection } from "./HomeLabSection";
import { RemoteExtras } from "./RemoteSection";
import { VaultExtras } from "./VaultSection";
import { WatcherPanel } from "./WatcherPanel";
import { openUpdateLog } from "@/features/updates/UpdateLog";

/** Client settings a third-party bootstrapper owns (spec §6.5). */
const CLIENT_PATHS = ["client.customSettingsPath"];

function reasonFor(
  item: Item,
  caps: Capabilities | undefined,
  rbx: RobloxStatus | undefined,
): string | null {
  if (rbx?.bootstrapper && CLIENT_PATHS.includes(item.path))
    return t("settings.managedBy", { name: rbx.bootstrapper });
  if (!item.cap || !caps || caps[item.cap]) return null;
  // a fixable launch reason (Linux: "Install Sober") beats the generic OS text
  if (item.cap === "launch" && caps.launchReason) return caps.launchReason;
  if (caps.os === "linux") return t("settings.winOnly");
  if (caps.os === "macos") return t("settings.notMac");
  return caps.launchReason ?? t("settings.unavailable");
}

/** Reduced motion (setting, or system when "system"): jump instead of gliding. */
const scrollBehavior = (): ScrollBehavior => {
  const rm = document.documentElement.dataset.reducedMotion;
  const reduce =
    rm === "on" || (rm !== "off" && matchMedia("(prefers-reduced-motion: reduce)").matches);
  return reduce ? "auto" : "smooth";
};

/** Search entries for the extras blocks (buttons, not catalog keys): `#sub` jumps to the block. */
const EXTRA_HITS: { g: (typeof GROUPS)[number]["id"]; sub: SectionId; key: MsgKey }[] = [
  { g: "security", sub: "vault", key: "vs.change" },
  { g: "security", sub: "vault", key: "vs.backups" },
  { g: "security", sub: "vault", key: "vs.exportEnc" },
  { g: "security", sub: "vault", key: "vs.import" },
  { g: "security", sub: "vault", key: "vs.exportPlain" },
  { g: "automation", sub: "friend", key: "friend.password" },
  { g: "automation", sub: "friend", key: "friend.copy" },
  { g: "automation", sub: "remote", key: "rs.showCode" },
  { g: "automation", sub: "remote", key: "rs.paired" },
  { g: "automation", sub: "remote", key: "rs.log" },
  { g: "app", sub: "about", key: "settings.updates" },
  { g: "app", sub: "about", key: "settings.about.openData" },
  { g: "app", sub: "about", key: "settings.about.openLogs" },
  { g: "app", sub: "about", key: "updates.open" },
  { g: "app", sub: "about", key: "tour.replay" },
];

function Updates() {
  const qc = useQueryClient();
  const { data: version } = useQuery({
    queryKey: ["update"],
    queryFn: () => call("update_check", { force: false }),
    meta: { silent: true },
  });
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const check = () => {
    setBusy(true);
    setNote(null);
    call("update_check", { force: true })
      .then(
        (v) => {
          qc.setQueryData(["update"], v);
          setNote(v ? null : t("settings.upToDate"));
        },
        // Rust's text is user-facing (portable mode says why it can't update)
        (e) => setNote(errMessage(e)),
      )
      .finally(() => setBusy(false));
  };
  const install = () => {
    setBusy(true);
    setNote(t("settings.updateDownloading"));
    // failure keeps the update pending in Rust, so the button stays valid for a retry
    call("update_install").catch((e) => {
      // UPDATE_BUSY: an install started before this section remounted is still running
      setBusy(hasCode(e, "UPDATE_BUSY"));
      setNote(errMessage(e));
      void qc.invalidateQueries({ queryKey: ["update"] });
    });
  };
  return (
    <div className="flex flex-wrap items-center gap-2">
      {version ? (
        <Button disabled={busy} onClick={install} aria-describedby="upd-note">
          {t("settings.updateInstall")}
        </Button>
      ) : (
        <Button disabled={busy} onClick={check} aria-describedby="upd-note">
          {t("settings.updates")}
        </Button>
      )}
      <span id="upd-note" role="status" className="text-13 text-text-3">
        {note ?? (version ? t("settings.updateAvailable", { v: version }) : null)}
      </span>
    </div>
  );
}

function About() {
  const { data: info } = useQuery({ queryKey: ["info"], queryFn: () => call("app_info") });
  const [err, setErr] = useState<string | null>(null);
  const set = useSetSettings();
  const open = (kind: "data" | "logs") =>
    call("open_folder", { kind }).then(
      () => setErr(null),
      () => setErr(t("settings.about.openErr")),
    );
  return (
    <div className="flex flex-col gap-4 py-3.5">
      <p className="text-14 text-text-1">
        {t("settings.about.version", { v: info?.version ?? "…" })}
      </p>
      <div>
        <p className="text-13 text-text-2">{t("settings.about.data")}</p>
        {/* the path holds the OS user name */}
        <p data-pii className="font-mono text-13 break-all text-text-1 select-text">
          {info?.dataDir ?? "…"}
        </p>
      </div>
      <Updates />
      <div className="flex flex-wrap gap-2">
        <Button onClick={() => void open("data")}>{t("settings.about.openData")}</Button>
        <Button onClick={() => void open("logs")}>{t("settings.about.openLogs")}</Button>
        <Button onClick={() => set.mutate({ onboarding: { tourDone: false, tourSeen: [] } })}>
          {t("tour.replay")}
        </Button>
        <Button onClick={() => openUpdateLog()}>{t("updates.open")}</Button>
      </div>
      {err && (
        <p role="alert" className="text-13 text-error">
          {err}
        </p>
      )}
      <p className="text-13 text-text-2">{t("settings.about.unofficial")}</p>
    </div>
  );
}

/** Controls that aren't plain settings keys. */
type Ctx = {
  rbx: RobloxStatus | undefined;
  disabled: boolean;
  csErr: string | null;
  pickClientSettings: () => void;
  ownerClash: boolean;
};

const extras: Partial<Record<string, (c: Ctx) => ReactNode>> = {
  "client.customSettingsPath": ({ disabled, csErr, pickClientSettings }) => (
    <div className="flex items-center gap-3 pb-2">
      <Button disabled={disabled} onClick={pickClientSettings}>
        {t("settings.chooseFile")}
      </Button>
      {csErr && (
        <p role="alert" className="text-13 text-error">
          {csErr}
        </p>
      )}
    </div>
  ),
  "net.allowLan": () => <TokenRows />,
  "friend.ownerUsername": ({ ownerClash }) =>
    ownerClash && (
      <p role="alert" className="pb-2 text-13 text-error">
        {t("friend.ownerSame")}
      </p>
    ),
};

const sectionExtras: Partial<Record<SectionId, () => ReactNode>> = {
  appearance: () => (
    <div className="border-b border-hairline py-3.5">
      <p className="text-14 text-text-1">{t("tabs.order")}</p>
      <p className="mb-2 text-13 text-text-2">{t("tabs.desc")}</p>
      <div className="rounded-control bg-surface-2 shadow-[inset_0_0_0_1px_var(--hairline)]">
        <TabOrder />
      </div>
    </div>
  ),
  vault: () => <VaultExtras />,
  friend: () => <FriendExtras />,
  remote: () => <RemoteExtras />,
  watcher: () => <WatcherPanel />,
  about: () => <About />,
};

/** Saved on change, never drafted: the client settings file is a command (copies a file), and
 * privacy mode is shared with the title-bar eye and Ctrl+Shift+H. */
const INSTANT = ["client.customSettingsPath", "ui.showUsernames"];

/** Port rows that must all differ (Rust refuses a save that puts two on one port). */
const PORTS = ["ws.port", "api.port", "friend.port", "remote.port"];

/** Edits that put every item of the `subs` sections (the ones this OS shows) whose default
 *  differs from the saved value back to default. */
function resetEdits(defaults: unknown, saved: unknown, subs: SectionId[]) {
  const out: Record<string, unknown> = {};
  for (const it of SECTIONS.filter((x) => subs.includes(x.id)).flatMap((x) => x.items)) {
    if (INSTANT.includes(it.path)) continue;
    const d = get(defaults, it.path);
    if (JSON.stringify(d) !== JSON.stringify(get(saved, it.path))) out[it.path] = d;
  }
  return out;
}

const GROUP_ICON = {
  general: SlidersHorizontal,
  roblox: Box,
  security: Lock,
  automation: Zap,
  app: Info,
} as const;

const navItem =
  "relative flex h-[var(--control-h)] w-full items-center gap-3 rounded-panel px-3 text-left text-14 font-medium transition-colors duration-[var(--dur-quick)] focus-visible:shadow-[var(--focus-ring)]";

export function SettingsPage() {
  const [gid, setGid] = useState<(typeof GROUPS)[number]["id"]>("general");
  const [query, setQuery] = useState("");
  const [active, setActive] = useState<SectionId>("general");
  const [jumpTo, setJumpTo] = useState<string | null>(null);
  const [flash, setFlash] = useState<string | null>(null);
  const [asking, setAsking] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const { data: s, isError, refetch } = useSettings();
  const { data: info } = useQuery({ queryKey: ["info"], queryFn: () => call("app_info") });
  const { data: caps } = useQuery({
    queryKey: ["caps"],
    queryFn: () => call("platform_capabilities"),
  });
  const { data: defaults } = useQuery({
    queryKey: ["settingsDefaults"],
    queryFn: () => call("settings_defaults"),
    staleTime: Infinity,
  });
  const { data: rbx } = useQuery({
    queryKey: ["roblox"],
    queryFn: () => call("roblox_status"),
    staleTime: 30_000,
    meta: { silent: true },
  });
  const qc = useQueryClient();
  const [csErr, setCsErr] = useState<string | null>(null);
  // Validated + copied by Rust; errors stay inline next to the field.
  const setClientSettings = (path: string | null) =>
    call("client_settings_set", { path }).then(
      (next) => {
        setCsErr(null);
        // a settings save in flight syncs the cache when it settles; writing now would undo its optimistic value
        if (!qc.isMutating({ mutationKey: KEY_SET })) qc.setQueryData(["settings"], next);
      },
      (e) => setCsErr(errMessage(e)),
    );
  const pickClientSettings = () =>
    void pickOpenPath("json", t("settings.jsonFiles")).then(
      (p) => {
        if (typeof p === "string") void setClientSettings(p);
      },
      (e) => setCsErr(errMessage(e)),
    );
  // Failures reach the banner once, via the shared mutation cache (reportError).
  const set = useSetSettings();
  const { edits, edit, setAll, clear } = useDraft();
  // No leave prompt: tauri-plugin-dialog makes window.confirm async (a Promise, always truthy).
  // The draft outlives the page, so unsaved edits are still here on the next visit.
  const n = Object.keys(edits).length;

  // the friend remote sees and closes clients through the watcher's process tracking: hidden
  // where the watcher capability is off
  // remote access hosting needs screen capture (Windows and macOS only; Linux can still view)
  const shown = (id: SectionId) =>
    id === "remote"
      ? !!caps && caps.os !== "linux"
      : (id !== "watcher" && id !== "friend") || !!caps?.watcher;
  const group = GROUPS.find((g) => g.id === gid)!;
  const subs = group.subs.filter(shown);
  const label = (it: Item) => t(`set.${it.path}` as MsgKey);
  const desc = (it: Item) => t(`set.${it.path}.d` as MsgKey);

  const q = query.trim().toLowerCase();
  const hits = q
    ? GROUPS.map((g) => ({
        g,
        items: [
          ...g.subs
            .filter(shown)
            .flatMap((id) => SECTIONS.find((x) => x.id === id)!.items)
            .map((it) => ({ path: it.path, label: label(it), desc: desc(it) })),
          ...EXTRA_HITS.filter((x) => x.g === g.id && shown(x.sub)).map((x) => ({
            path: `#${x.sub}`,
            label: t(x.key),
            desc: t(`sec.${x.sub}` as MsgKey),
          })),
        ].filter((it) => `${it.label} ${it.desc}`.toLowerCase().includes(q)),
      })).filter((r) => r.items.length)
    : [];

  const openGroup = (id: typeof gid) => {
    setGid(id);
    setActive(GROUPS.find((g) => g.id === id)!.subs[0]!);
    setQuery("");
    scroller.current?.scrollTo({ top: 0 });
  };
  const jump = (id: typeof gid, path: string) => {
    openGroup(id);
    setJumpTo(path);
  };
  // a row asked for from elsewhere (the update log): open its group and jump to it
  const want = useUi((u) => u.settingsJump);
  useEffect(() => {
    if (!want) return;
    useUi.setState({ settingsJump: null });
    const sub = want.startsWith("#")
      ? want.slice(1)
      : SECTIONS.find((x) => x.items.some((it) => it.path === want))?.id;
    const g = GROUPS.find((x) => (x.subs as readonly string[]).includes(sub ?? ""));
    if (!g) return;
    setGid(g.id);
    setActive(g.subs[0]!);
    setQuery("");
    setJumpTo(want);
  }, [want]);
  // after the group renders: scroll the wanted row into view and flash it
  useEffect(() => {
    if (!jumpTo || q) return;
    // wait for the page and the OS (caps decide which sections exist)
    if (!scroller.current || !caps) return;
    const sub = jumpTo.startsWith("#")
      ? jumpTo.slice(1)
      : SECTIONS.find((x) => x.items.some((it) => it.path === jumpTo))?.id;
    // the row can be absent (friend setup gate, hidden section): land on its section, or give up
    const el =
      scroller.current?.querySelector(
        jumpTo.startsWith("#") ? `[data-sub="${sub}"]` : `[data-path="${jumpTo}"]`,
      ) ?? scroller.current?.querySelector(`[data-sub="${sub}"]`);
    if (!el) {
      setJumpTo(null);
      return;
    }
    el.scrollIntoView({ block: "center", behavior: scrollBehavior() });
    setJumpTo(null);
    setFlash(jumpTo);
  }, [jumpTo, q, gid, caps]);
  // own effect: the jump effect re-runs (and would cancel this timer) as soon as jumpTo clears
  useEffect(() => {
    if (!flash) return;
    const id = setTimeout(() => setFlash(null), 1600);
    return () => clearTimeout(id);
  }, [flash]);

  const place = (el: Element) => {
    const sc = scroller.current!;
    return sc.scrollTop + el.getBoundingClientRect().top - sc.getBoundingClientRect().top;
  };
  const goSub = (id: SectionId) => {
    const el = scroller.current?.querySelector(`[data-sub="${id}"]`);
    if (!el) return;
    setActive(id);
    scroller.current!.scrollTo({ top: place(el) - 36, behavior: scrollBehavior() });
  };
  // scroll-spy: the last block whose top passed the line near the top; the bottom counts as the last
  const spy = () => {
    const sc = scroller.current!;
    const blocks = [...sc.querySelectorAll<HTMLElement>("[data-sub]")];
    if (!blocks.length) return;
    const line = sc.getBoundingClientRect().top + 60;
    let k = 0;
    blocks.forEach((b, i) => {
      if (b.getBoundingClientRect().top <= line) k = i;
    });
    if (sc.scrollTop + sc.clientHeight >= sc.scrollHeight - 2) k = blocks.length - 1;
    setActive(blocks[k]!.dataset.sub as SectionId);
  };

  const value = (path: string) => (path in edits ? edits[path] : get(s, path));
  // an owner login named like the friend's would be two logins on one name: never saved.
  // Only an edit of either name blocks: a clash already stored must not block unrelated saves.
  const ownerClash =
    ownerNameClash(value("friend.ownerUsername"), value("friend.username")) &&
    ["friend.ownerUsername", "friend.username"].some((p) => p in edits);
  // the same rule as Rust's save check: two features on one port, created by this draft
  const portClash = (p: string) =>
    value(p) !== undefined &&
    PORTS.some((o) => o !== p && value(o) === value(p) && (p in edits || o in edits));
  const anyPortClash = PORTS.some(portClash);
  const item = (it: Item) => {
    const reason = reasonFor(it, caps, rbx);
    // parent switch off: the value does nothing yet, so the row dims (it stays editable)
    const idle = !reason && it.dep?.some((d) => !value(d));
    return (
      <div
        key={it.path}
        data-path={it.path}
        className={clsx(
          "rounded-control transition-colors duration-700",
          flash === it.path && "bg-accent/15",
          idle && "opacity-60",
        )}
      >
        <Setting
          item={it}
          value={value(it.path)}
          reason={reason}
          onChange={(v) =>
            it.path === "client.customSettingsPath"
              ? void setClientSettings((v as string | null) ?? null)
              : it.path === "ui.showUsernames"
                ? set.mutate({ ui: { showUsernames: !!v } })
                : edit(it.path, v, get(s, it.path))
          }
        />
        {extras[it.path]?.({ rbx, disabled: !!reason, csErr, pickClientSettings, ownerClash })}
        {it.kind === "port" && portClash(it.path) && (
          <p role="alert" className="pb-2 text-13 text-error">
            {t("set.portClash")}
          </p>
        )}
      </div>
    );
  };

  const nav = (
    <nav aria-label={t("settings.nav")} className="flex flex-col gap-0.5">
      {GROUPS.map((g) => {
        const on = g.id === gid && !q;
        const Icon = GROUP_ICON[g.id];
        return (
          <div key={g.id}>
            <button
              type="button"
              aria-expanded={on}
              onClick={() => openGroup(g.id)}
              className={clsx(
                navItem,
                on
                  ? "bg-surface-3 text-text-1 shadow-[inset_0_0_0_1px_var(--hairline),inset_0_1px_0_rgb(255_255_255/0.1)]"
                  : "text-text-2 hover:bg-surface-3/60 hover:text-text-1",
              )}
            >
              <Icon size={18} strokeWidth={1.75} aria-hidden />
              {t(`grp.${g.id}` as MsgKey)}
            </button>
            {on && (
              <div className="mt-0.5 mb-2 ml-[1.375rem] flex flex-col gap-0.5 border-l border-hairline pl-3">
                {subs.map((id) => (
                  <button
                    key={id}
                    type="button"
                    aria-current={id === active ? "location" : undefined}
                    onClick={() => goSub(id)}
                    className={clsx(
                      "h-7 rounded-control px-2.5 text-left text-13 transition-colors duration-[var(--dur-quick)] focus-visible:shadow-[var(--focus-ring)]",
                      id === active
                        ? "bg-surface-3/70 font-semibold text-text-1"
                        : "text-text-2 hover:bg-surface-3/60 hover:text-text-1",
                    )}
                  >
                    {t(`sec.${id}` as MsgKey)}
                  </button>
                ))}
              </div>
            )}
          </div>
        );
      })}
    </nav>
  );

  // per-call promise, not mutate callbacks: those skip superseded calls and unmounted pages
  const save = () => {
    const sent = edits;
    const done = () => useDraft.getState().dropSent(sent);
    set.mutateAsync(draftPatch(sent)).then(done, (e: unknown) => {
      // a failed automation restart comes after the write: the edits are saved
      if (hasCode(e, "SETTINGS_PARTIAL")) done();
      // the patch was saved; only the OS autostart change failed: keep just that edit
      else if (hasCode(e, "AUTOSTART_FAILED")) {
        const rest = { ...sent };
        delete rest["startup.launchOnLogin"];
        useDraft.getState().dropSent(rest);
      }
    });
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex min-h-0 flex-1">
        <aside className="w-56 shrink-0 overflow-y-auto border-r border-hairline px-3 pt-6">
          <h2 className="px-3 pb-3.5 text-16 font-semibold text-text-1">{t("settings.title")}</h2>
          <label className="mb-4 flex h-[var(--control-h)] items-center gap-2 rounded-control bg-field px-2.5 shadow-[inset_0_0_0_1px_var(--hairline)] focus-within:shadow-[var(--focus-ring)]">
            <Search size={16} strokeWidth={1.75} className="shrink-0 text-text-2" aria-hidden />
            <input
              type="search"
              value={query}
              placeholder={t("settings.search")}
              aria-label={t("settings.search")}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                const first = hits[0];
                if (e.key === "Enter" && first) jump(first.g.id, first.items[0]!.path);
                if (e.key === "Escape") setQuery("");
              }}
              className="min-w-0 flex-1 bg-transparent text-13 text-text-1 shadow-none outline-0 placeholder:text-text-3"
            />
          </label>
          {nav}
        </aside>
        <div
          ref={scroller}
          onScroll={spy}
          className="min-h-0 flex-1 overflow-y-auto px-10 pt-9 pb-[360px]"
        >
          <div className="mx-auto w-full max-w-[700px]">
            {info?.settingsReadOnly && (
              <p role="note" className="pb-3 text-13 text-text-2">
                {t("settings.readOnly")}
              </p>
            )}
            {isError && (
              <div role="alert" className="flex items-center gap-3 py-4 text-13 text-error">
                <p>{t("settings.loadErr")}</p>
                <Button onClick={() => void refetch()}>{t("error.retry")}</Button>
              </div>
            )}
            {s && q && (
              <div>
                {hits.length === 0 && <p className="py-10 text-text-2">{t("settings.noMatch")}</p>}
                {hits.map(({ g, items }) => (
                  <section key={g.id} className="mb-8">
                    <h3 className="mb-1 text-12 font-medium text-text-3">
                      {t(`grp.${g.id}` as MsgKey)}
                    </h3>
                    <ul>
                      {items.map((it) => (
                        <li
                          key={it.path + it.label}
                          className="border-b border-hairline last:border-0"
                        >
                          <button
                            type="button"
                            onClick={() => jump(g.id, it.path)}
                            className="block w-full rounded-control px-2 py-2.5 text-left transition-colors duration-[var(--dur-quick)] hover:bg-surface-3/60"
                          >
                            <span className="block text-14 text-text-1">{it.label}</span>
                            <span className="block text-13 text-text-2">{it.desc}</span>
                          </button>
                        </li>
                      ))}
                    </ul>
                  </section>
                ))}
              </div>
            )}
            {s &&
              !q &&
              subs.map((id) => {
                const sec = SECTIONS.find((x) => x.id === id)!;
                return (
                  <section
                    key={id}
                    data-sub={id}
                    aria-labelledby={`sub-${id}`}
                    className="pt-2 [&+&]:mt-14"
                  >
                    <h2 id={`sub-${id}`} className="text-20 font-semibold text-text-1">
                      {t(`sec.${id}` as MsgKey)}
                    </h2>
                    <p className="mt-0.5 mb-2.5 text-13 text-text-2">
                      {t(`set.section.${id}.d` as MsgKey)}
                    </p>
                    {id === "homelab" && <HomeLabSection />}
                    {id === "friend" ? (
                      <FriendGate>
                        {sec.items.map(item)}
                        {sectionExtras[id]?.()}
                      </FriendGate>
                    ) : (
                      <>
                        {sec.items.map(item)}
                        {sectionExtras[id]?.()}
                      </>
                    )}
                  </section>
                );
              })}
          </div>
        </div>
      </div>
      <div className="flex h-16 shrink-0 items-center gap-3 border-t border-hairline bg-[var(--glass-panel)] px-6 backdrop-blur-[28px] backdrop-saturate-[1.2]">
        {asking ? (
          <>
            <span role="status" className="text-13 text-text-2">
              {t("settings.resetAsk")}
            </span>
            <span className="flex-1" />
            <Button onClick={() => setAsking(false)}>{t("set.cancel")}</Button>
            <Button
              variant="danger"
              onClick={() => {
                if (s && defaults)
                  setAll(resetEdits(defaults, s, GROUPS.flatMap((g) => g.subs).filter(shown)));
                setAsking(false);
              }}
            >
              {t("settings.resetYes")}
            </Button>
          </>
        ) : (
          <>
            <span role="status" className="inline-flex items-center gap-2 text-13 text-text-2">
              <i
                aria-hidden
                className={clsx("size-[7px] rounded-full", n ? "bg-warn" : "bg-online")}
              />
              {n === 0
                ? t("settings.saved")
                : n === 1
                  ? t("settings.unsaved1")
                  : t("settings.unsavedN", { n })}
            </span>
            <span className="flex-1" />
            <Button variant="ghost" disabled={!s || !defaults} onClick={() => setAsking(true)}>
              <RotateCcw size={16} strokeWidth={1.75} aria-hidden />
              {t("settings.resetAll")}
            </Button>
            <Button disabled={n === 0 || set.isPending} onClick={clear}>
              {t("settings.discard")}
            </Button>
            <Button
              variant="primary"
              disabled={n === 0 || set.isPending || ownerClash || anyPortClash}
              onClick={save}
            >
              <Check size={16} strokeWidth={1.75} aria-hidden />
              {t("settings.save")}
            </Button>
          </>
        )}
      </div>
    </div>
  );
}
