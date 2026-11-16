import * as Dialog from "@radix-ui/react-dialog";
import clsx from "clsx";
import {
  ChevronDown,
  Copy,
  Download,
  FolderInput,
  LayoutGrid,
  Lock,
  LogIn,
  MonitorSmartphone,
  Play,
  Settings as SettingsIcon,
  ShieldCheck,
  Users,
  X,
} from "lucide-react";
import { useEffect, useState, type CSSProperties, type ReactNode } from "react";
import { useUi } from "@/app/store";
import { TabOrder } from "@/app/Tabs";
import { useSettings, useSetSettings } from "@/app/useSettings";
import { Button } from "@/components/ui/Button";
import { call, errMessage, type Capabilities, type Settings } from "@/lib/ipc";
import { t, type MsgKey } from "@/lib/i18n";
import { Toggle } from "@/features/settings/controls";

type Row = {
  label: MsgKey;
  hint: MsgKey;
  cap?: keyof Pick<Capabilities, "multiInstance" | "watcher">;
  get: (s: Settings) => boolean;
  patch: (on: boolean) => unknown;
};

const ROWS: Row[] = [
  {
    label: "tour.pick.multi",
    hint: "tour.pick.multiD",
    cap: "multiInstance",
    get: (s) => s.multi.enabled,
    patch: (on) => ({ multi: { enabled: on } }),
  },
  {
    label: "tour.pick.reconnect",
    hint: "tour.pick.reconnectD",
    cap: "watcher",
    // reconnect runs inside the watcher scan, so turning it on also turns the watcher on
    get: (s) => s.watcher.reconnect && s.watcher.enabled,
    patch: (on) => ({ watcher: on ? { reconnect: true, enabled: true } : { reconnect: false } }),
  },
  {
    label: "tour.pick.hide",
    hint: "tour.pick.hideD",
    get: (s) => !s.ui.showUsernames,
    patch: (on) => ({ ui: { showUsernames: !on } }),
  },
  {
    label: "tour.pick.startup",
    hint: "tour.pick.startupD",
    get: (s) => s.startup.launchOnLogin,
    patch: (on) => ({ startup: { launchOnLogin: on } }),
  },
  {
    label: "tour.pick.tray",
    hint: "tour.pick.trayD",
    get: (s) => s.tray.enabled,
    patch: (on) => ({ tray: { enabled: on } }),
  },
];

const card = "rounded-control bg-surface-2 shadow-[inset_0_0_0_1px_var(--hairline)]";
const tile = "inline-grid shrink-0 place-items-center rounded-control bg-surface-3 text-text-1";
const rowCls =
  "flex items-center gap-3.5 border-t border-[var(--hairline)] px-4 py-3 first:border-t-0";

function Item({ icon, title, body }: { icon: ReactNode; title: string; body: string }) {
  return (
    <div className={clsx(rowCls, "py-3.5")}>
      <span className={clsx(tile, "size-[38px]")}>{icon}</span>
      <div>
        <b className="block text-14 font-semibold text-text-1">{title}</b>
        <span className="block text-13 text-text-3">{body}</span>
      </div>
    </div>
  );
}

type Def = { state: "idle" | "pending" | "ok" | "err"; err?: string };

/** macOS: unsigned-app note, and Accessibility for Grid Arrange (checked, never assumed). */
function MacHelp() {
  const [ok, setOk] = useState<boolean | null>(null);
  const check = (prompt: boolean) =>
    void call("ax_trusted", { prompt }).then(setOk, () => setOk(false));
  return (
    <>
      <p className="text-13 leading-relaxed text-text-2">{t("tour.mac.why")}</p>
      <div className={clsx(card, "flex flex-col gap-3 p-4")}>
        <p className="text-13 text-text-2">{t("tour.mac.axWhy")}</p>
        <div className="flex gap-2.5">
          <Button
            variant="primary"
            className={clsx("h-10 flex-1 justify-center", white)}
            disabled={ok === true}
            onClick={() => check(true)}
          >
            <ShieldCheck size={16} strokeWidth={1.75} aria-hidden />
            {t("tour.mac.ax")}
          </Button>
          {ok === false && (
            <Button className="h-10" onClick={() => check(false)}>
              {t("tour.mac.check")}
            </Button>
          )}
        </div>
        {ok !== null && (
          <p role="status" className={clsx("text-13", ok ? "text-online" : "text-text-2")}>
            {t(ok ? "tour.mac.axOk" : "tour.mac.axTodo")}
          </p>
        )}
      </div>
      <p className="text-13 text-text-3">{t("tour.mac.update")}</p>
    </>
  );
}

function Defender({ dir }: { dir: string }) {
  const [def, setDef] = useState<Def>({ state: "idle" });
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const [copyFail, setCopyFail] = useState(false);
  const cmd = `Add-MpPreference -ExclusionPath '${dir.replace(/['‘’‚‛]/g, "$&$&")}'`;
  const run = () => {
    setDef({ state: "pending" });
    call("defender_exclude").then(
      () => setDef({ state: "ok" }),
      (e) => setDef({ state: "err", err: errMessage(e) }),
    );
  };
  return (
    <>
      <p className="text-13 leading-relaxed text-text-2">{t("tour.def.why")}</p>
      <div className={clsx(card, "flex flex-col gap-3 p-4")}>
        <Button
          variant="primary"
          className={clsx("h-10 justify-center", white)}
          disabled={def.state === "pending" || def.state === "ok"}
          onClick={run}
        >
          <ShieldCheck size={16} strokeWidth={1.75} aria-hidden />
          {t("tour.def.add")}
        </Button>
        <p
          role="status"
          className={clsx("text-13", def.state === "ok" ? "text-online" : "text-text-3")}
        >
          {def.state === "pending"
            ? t("tour.def.pending")
            : def.state === "ok"
              ? t("tour.def.ok")
              : dir
                ? t("tour.def.fine", { dir })
                : t("tour.def.fineNoDir")}
        </p>
        {def.state === "err" && (
          <p role="alert" className="text-13 text-error">
            {t("tour.def.err", { err: def.err ?? "" })}
          </p>
        )}
        {dir && (
          <button
            type="button"
            aria-expanded={open}
            onClick={() => setOpen(!open)}
            className="inline-flex items-center gap-1 self-start text-13 text-text-2 hover:text-text-1"
          >
            {t("tour.def.show")}
            <ChevronDown size={14} className={clsx(open && "rotate-180")} aria-hidden />
          </button>
        )}
        {dir && open && (
          <div className="flex items-start gap-2 rounded-control bg-surface-1 p-3">
            <pre className="min-w-0 flex-1 font-mono text-12 break-all whitespace-pre-wrap text-text-1 select-text">
              {cmd}
            </pre>
            <Button
              sm
              onClick={() =>
                void navigator.clipboard.writeText(cmd).then(
                  () => {
                    setCopied(true);
                    setCopyFail(false);
                  },
                  () => setCopyFail(true),
                )
              }
            >
              <Copy size={13} aria-hidden />
              {copied ? t("tour.def.copied") : t("tour.def.copy")}
            </Button>
          </div>
        )}
        {copyFail && (
          <p role="alert" className="text-13 text-error">
            {t("tour.def.copyFail")}
          </p>
        )}
      </div>
      <p className="text-13 text-text-3">{t("tour.def.fallback")}</p>
    </>
  );
}

const FPS_CHOICES = [30, 60, 120, 144, 240];

function Chip({
  on,
  onClick,
  children,
}: {
  on: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <Button
      aria-pressed={on}
      onClick={onClick}
      className={clsx(on ? white : "!text-text-2 hover:!text-text-1")}
    >
      {children}
    </Button>
  );
}

/** FPS cap and graphics level every client starts with; saves on change. */
function Perf({ s, set }: { s: Settings; set: (patch: unknown) => void }) {
  const { fps, graphics: g } = s;
  // dragging stays local; one save on release (each save writes settings.json and reconciles)
  const [drag, setDrag] = useState<number | null>(null);
  const level = drag ?? g.level;
  const commit = () => {
    if (drag === null) return;
    set({ graphics: { set: true, level: drag } });
    setDrag(null);
  };
  return (
    <>
      <div className={clsx(card, "flex flex-col gap-3 p-4")}>
        <b className="text-14 font-semibold text-text-1">{t("tour.perf.fps")}</b>
        <div role="group" aria-label={t("tour.perf.fps")} className="flex flex-wrap gap-2">
          <Chip on={!fps.unlock} onClick={() => set({ fps: { unlock: false } })}>
            {t("tour.perf.fpsRoblox")}
          </Chip>
          {FPS_CHOICES.map((n) => (
            <Chip
              key={n}
              on={fps.unlock && fps.value === n}
              onClick={() => set({ fps: { unlock: true, value: n } })}
            >
              {t("tour.perf.fpsCap", { n })}
            </Chip>
          ))}
        </div>
      </div>
      <div className={clsx(card, "flex flex-col gap-3 p-4")}>
        <div className="flex items-center justify-between">
          <b className="text-14 font-semibold text-text-1">{t("tour.perf.gfx")}</b>
          <span className="text-13 text-text-2 tabular-nums">
            {g.set || drag !== null
              ? t("tour.perf.gfxLevel", { n: level })
              : t("tour.perf.gfxRoblox")}
          </span>
        </div>
        <input
          type="range"
          min={1}
          max={10}
          step={1}
          value={level}
          aria-label={t("tour.perf.gfx")}
          aria-valuetext={t("tour.perf.gfxLevel", { n: level })}
          onChange={(e) => setDrag(Number(e.target.value))}
          onPointerUp={commit}
          onKeyUp={commit}
          onBlur={commit}
          className={clsx("range w-full", !g.set && "opacity-40")}
          style={{ "--fill": `${((level - 1) / 9) * 100}%` } as CSSProperties}
        />
        <div className="flex justify-between text-12 text-text-3">
          <span>{t("tour.perf.low")}</span>
          <span>{t("tour.perf.high")}</span>
        </div>
        <div className="flex">
          <Chip on={!g.set} onClick={() => set({ graphics: { set: false } })}>
            {t("tour.perf.gfxRoblox")}
          </Chip>
        </div>
      </div>
    </>
  );
}

// Look A: white primary like the HomeLab wizard, not the app accent.
const white = "!bg-text-1 !text-bg hover:!opacity-90 disabled:!bg-surface-3 disabled:!text-text-1";

type Step =
  "welcome" | "add" | "pick" | "perf" | "tabs" | "friend" | "remote" | "homelab" | "def" | "mac";

const HEADS: Record<Step, [MsgKey, MsgKey]> = {
  welcome: ["tour.welcome.title", "tour.welcome.sub"],
  add: ["tour.add.title", "tour.add.sub"],
  pick: ["tour.pick.title", "tour.pick.sub"],
  perf: ["tour.perf.title", "tour.perf.sub"],
  tabs: ["tour.tabs.title", "tour.tabs.sub"],
  friend: ["tour.friend.title", "tour.friend.sub"],
  remote: ["tour.remote.title", "tour.remote.sub"],
  homelab: ["tour.homelab.title", "tour.homelab.sub"],
  def: ["tour.def.title", "tour.def.sub"],
  mac: ["tour.mac.title", "tour.mac.sub"],
};

/** The v0.2.1 tour: someone who finished it (tourDone, no tourSeen yet) has seen these. */
const LEGACY_SEEN: Step[] = ["welcome", "add", "pick", "def"];

function allSteps(caps: Capabilities): Step[] {
  return [
    "welcome",
    "add",
    "pick",
    "perf",
    "tabs",
    // the friend remote needs process tracking (none on Linux)
    ...(caps.watcher ? (["friend"] as const) : []),
    "remote",
    ...(caps.os === "windows" ? (["def"] as const) : []),
    ...(caps.os === "macos" ? (["mac"] as const) : []),
  ];
}

/** Steps this user hasn't seen: everything on first run, only new steps after an update. */
export function tourSteps(s: Settings, caps: Capabilities): Step[] {
  const o = s.onboarding;
  const seen: string[] = o.tourSeen?.length ? o.tourSeen : o.tourDone ? LEGACY_SEEN : [];
  return allSteps(caps).filter((x) => !seen.includes(x));
}

/** First-run tour (Radix Dialog: focus trap, Esc), and later just the steps an update added.
 * Every way out marks them all seen. */
export function Tour({ caps }: { caps: Capabilities }) {
  const { data: s } = useSettings();
  const set = useSetSettings();
  const [i, setI] = useState(0);
  const [dir, setDir] = useState("");
  // fixed for this showing: settings change while the tour is open (its own toggles)
  const [frozen, setFrozen] = useState<Step[] | null>(null);
  useEffect(() => {
    if (s && !frozen) setFrozen(tourSteps(s, caps));
  }, [s, caps, frozen]);
  const steps = frozen ?? (s ? tourSteps(s, caps) : []);
  const windows = steps.includes("def");
  const total = steps.length;
  useEffect(() => {
    if (windows) void call("defender_exclude_cmd").then(setDir, () => {});
  }, [windows]);
  if (!s || !total) return null;

  const done = () =>
    set.mutate({
      onboarding: {
        tourDone: true,
        tourSeen: [...new Set([...(s.onboarding.tourSeen ?? []), ...allSteps(caps)])],
      },
    });
  // The flow opens over the app once the tour is gone.
  const leaveTo = (open: () => void) => {
    done();
    open();
  };
  const last = i === total - 1;
  const step = steps[i]!;
  const [title, sub] = HEADS[step];
  const rows = ROWS.filter((r) => !r.cap || caps[r.cap]);
  const choices = [
    [LogIn, "tour.add.signIn", "tour.add.signInD", () => useUi.getState().openAdd("signin")],
    [
      FolderInput,
      "tour.add.import",
      "tour.add.importD",
      () => useUi.setState({ importOpen: true }),
    ],
  ] as const;

  return (
    <Dialog.Root open onOpenChange={(o) => !o && done()}>
      <Dialog.Portal>
        {/* drag region: the scrim covers the title bar, so the window still moves from here */}
        <Dialog.Overlay
          data-tauri-drag-region
          className="sheet-fade fixed inset-0 z-40 bg-[var(--scrim)] backdrop-blur-[6px]"
        />
        <Dialog.Content
          onInteractOutside={(e) => e.preventDefault()}
          onOpenAutoFocus={(e) => e.preventDefault()}
          className="sheet-pop fixed top-1/2 left-1/2 glass z-50 flex max-h-[calc(100vh-48px)] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-panel"
          style={{ width: "min(36.25rem, calc(100vw - 48px))" }}
        >
          {/* header and footer stay put; only the middle scrolls when the window is short */}
          <div className="px-7 pt-6">
            <div className="flex items-center justify-between">
              <span className="text-13 font-semibold text-text-2">
                {t("tour.step", { n: i + 1, total })}
              </span>
              <Dialog.Close
                aria-label={t("tour.close")}
                className="-mr-2 inline-grid size-8 place-items-center rounded-control text-text-2 hover:bg-surface-3 hover:text-text-1"
              >
                <X size={16} strokeWidth={1.75} />
              </Dialog.Close>
            </div>
            <div className="mt-3 flex gap-1.5" aria-hidden>
              {Array.from({ length: total }, (_, n) => (
                <i
                  key={n}
                  className={clsx("h-1 flex-1 rounded-sm", n <= i ? "bg-text-1" : "bg-surface-3")}
                />
              ))}
            </div>
          </div>
          <div className="flex min-h-0 flex-1 flex-col gap-5 overflow-auto px-7 py-5">
            <div>
              <Dialog.Title className="text-28 font-bold tracking-[-0.02em] text-text-1">
                {t(title)}
              </Dialog.Title>
              <Dialog.Description className="mt-1.5 text-14 text-text-2">
                {t(sub)}
              </Dialog.Description>
            </div>

            {step === "welcome" && (
              <div className={card}>
                <Item
                  icon={<Lock size={18} aria-hidden />}
                  title={t("tour.welcome.vault")}
                  body={t("tour.welcome.vaultD")}
                />
                <Item
                  icon={<Play size={18} aria-hidden />}
                  title={t("tour.welcome.play")}
                  body={t("tour.welcome.playD")}
                />
                {/* Grid Arrange needs window placement (none on Linux) */}
                {caps.windowRect && (
                  <Item
                    icon={<LayoutGrid size={18} aria-hidden />}
                    title={t("tour.welcome.grid")}
                    body={t("tour.welcome.gridD")}
                  />
                )}
              </div>
            )}

            {step === "add" && (
              <>
                <div className="grid grid-cols-2 gap-3">
                  {choices.map(([Icon, label, hint, open]) => (
                    <button
                      key={label}
                      type="button"
                      onClick={() => leaveTo(open)}
                      className="flex min-h-[10.5rem] flex-col items-start gap-1.5 rounded-control bg-surface-2 p-5 text-left shadow-[inset_0_0_0_1px_var(--hairline)] hover:bg-surface-3"
                    >
                      <span className={clsx(tile, "mb-2.5 size-[42px]")}>
                        <Icon size={20} aria-hidden />
                      </span>
                      <b className="text-14 font-semibold text-text-1">{t(label)}</b>
                      <span className="text-13 text-text-3">{t(hint)}</span>
                    </button>
                  ))}
                </div>
                <p className="-mt-1 text-center text-13 text-text-3">{t("tour.add.note")}</p>
              </>
            )}

            {step === "pick" && (
              <div className={card}>
                {rows.map((r) => (
                  <div key={r.label} className={rowCls}>
                    <div className="flex-1">
                      <b id={`${r.label}-l`} className="block text-14 font-semibold text-text-1">
                        {t(r.label)}
                      </b>
                      <span id={`${r.label}-d`} className="block text-13 text-text-3">
                        {t(r.hint)}
                      </span>
                    </div>
                    {/* the same switch as Settings */}
                    <Toggle
                      id={r.label}
                      checked={r.get(s)}
                      onChange={(on) => set.mutate(r.patch(on))}
                    />
                  </div>
                ))}
              </div>
            )}

            {step === "perf" && <Perf s={s} set={(p) => set.mutate(p)} />}

            {step === "tabs" && (
              <div className={card}>
                <TabOrder />
              </div>
            )}

            {step === "friend" && (
              <div className={card}>
                <Item
                  icon={<Users size={18} aria-hidden />}
                  title={t("tour.friend.pick")}
                  body={t("tour.friend.pickD")}
                />
                <Item
                  icon={<SettingsIcon size={18} aria-hidden />}
                  title={t("tour.friend.where")}
                  body={t("tour.friend.whereD")}
                />
              </div>
            )}

            {step === "remote" && (
              <div className={card}>
                <Item
                  icon={<MonitorSmartphone size={18} aria-hidden />}
                  title={t("tour.remote.find")}
                  body={t("tour.remote.findD")}
                />
                <Item
                  icon={<ShieldCheck size={18} aria-hidden />}
                  title={t("tour.remote.safe")}
                  body={t(caps.os === "linux" ? "tour.remote.safeD.linux" : "tour.remote.safeD")}
                />
              </div>
            )}

            {step === "homelab" && (
              <div className={card}>
                <Item
                  icon={<Download size={18} aria-hidden />}
                  title={t("tour.homelab.get")}
                  body={t("tour.homelab.getD")}
                />
                <Item
                  icon={<ShieldCheck size={18} aria-hidden />}
                  title={t("tour.homelab.share")}
                  body={t("tour.homelab.shareD")}
                />
              </div>
            )}

            {step === "def" && <Defender dir={dir} />}

            {step === "mac" && <MacHelp />}
          </div>

          <div className="flex flex-col gap-2.5 px-7 pb-5">
            <div className="flex gap-2.5">
              {i > 0 && (
                <Button className="h-10 flex-1" onClick={() => setI(i - 1)}>
                  {t("tour.back")}
                </Button>
              )}
              <Button
                variant="primary"
                autoFocus
                className={clsx("h-10 flex-[2]", white)}
                onClick={() => (last ? done() : setI(i + 1))}
              >
                {t(last ? "tour.start" : "tour.next")}
              </Button>
            </div>
            {step === "perf" && (
              <p className="text-center text-13 text-text-3">{t("tour.perf.note")}</p>
            )}
            {(step === "pick" || step === "tabs") && (
              <p className="text-center text-13 text-text-3">{t("tour.pick.note")}</p>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
