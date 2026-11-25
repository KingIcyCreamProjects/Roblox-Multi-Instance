import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import clsx from "clsx";
import {
  Download,
  Eraser,
  ExternalLink,
  KeyRound,
  Search,
  Send,
  TriangleAlert,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { openExternal } from "@/lib/events";
import { toast } from "@/components/Toasts";
import { call, errMessage, type AutomationAccount } from "@/lib/ipc";
import { reportError } from "@/lib/report";
import { t, type MsgKey } from "@/lib/i18n";
import { Toggle } from "@/features/settings/controls";
import { Controls } from "./Controls";
import { DOCS_URL, filterLines, fmtTime, maskToken, wsUrl } from "./lib";
import { CopyButton } from "./TokenSettings";
import { useAutomationStatus, useSnapshot, useToken } from "./useAutomation";
import { useSettings } from "@/app/useSettings";

// Status tokens on small marks only.
const levelColor: Record<string, string> = {
  error: "bg-error",
  warn: "bg-warn",
  info: "bg-ingame",
};
const levelName: Record<string, MsgKey> = {
  error: "auto.level.error",
  warn: "auto.level.warn",
  info: "auto.level.info",
};

const inputCls =
  "h-[var(--control-h)] rounded-control bg-field px-2.5 text-13 text-text-1 shadow-[inset_0_0_0_1px_var(--hairline)] placeholder:text-text-3 hover:shadow-[inset_0_0_0_1px_var(--hairline-strong)] focus-visible:shadow-[var(--focus-ring)]";

/** Thin server strip: both servers, Start/Stop, LAN warning. */
function StatusStrip() {
  const qc = useQueryClient();
  const { data: s, isError, refetch } = useAutomationStatus();
  const toggle = useMutation({
    mutationFn: (on: boolean) => call(on ? "automation_start" : "automation_stop"),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["autoStatus"] }),
  });
  if (!s) {
    return (
      <div className="flex h-11 items-center gap-3 px-3.5 text-13 text-text-3">
        {isError ? (
          <>
            <p role="alert" className="text-error">
              {t("auto.statusError")}
            </p>
            <Button sm onClick={() => void refetch()}>
              {t("error.retry")}
            </Button>
          </>
        ) : (
          <p role="status">{t("auto.statusLoading")}</p>
        )}
      </div>
    );
  }
  const on = s.wsRunning || s.apiRunning;
  const row = (label: string, running: boolean, port: number) => (
    <span className="flex items-center gap-2 text-13 text-text-2">
      <span
        aria-hidden
        className={clsx("size-[7px] rounded-full", running ? "bg-online" : "bg-text-3")}
      />
      {label}
      <span className="text-text-3">
        {running ? t("auto.running") : t("auto.stopped")} ·{" "}
        <span className="font-mono">{port}</span>
      </span>
    </span>
  );
  return (
    <div className="flex flex-col pb-1">
      <div className="flex min-h-11 flex-wrap items-center gap-x-[18px] px-3.5">
        {row(t("auto.ws"), s.wsRunning, s.wsPort)}
        {row(t("auto.api"), s.apiRunning, s.apiPort)}
        <Button
          sm
          className="ml-auto"
          disabled={toggle.isPending}
          onClick={() => toggle.mutate(!on)}
        >
          {on ? t("auto.stop") : t("auto.start")}
        </Button>
      </div>
      {s.lan && (
        <p role="status" className="flex items-center gap-2 px-3.5 text-13 text-text-2">
          <TriangleAlert size={14} strokeWidth={1.75} className="shrink-0 text-warn" aria-hidden />
          {t("auto.lan")}
        </p>
      )}
    </div>
  );
}

function Empty({ port }: { port: number }) {
  const { data: tok } = useToken();
  return (
    <div className="mx-auto flex max-w-xl flex-col gap-4 p-8">
      <p className="text-14 text-text-1">{t("auto.empty")}</p>
      <div className="flex flex-col gap-1">
        <span className="text-12 text-text-3">{t("auto.connectUrl")}</span>
        <code className="rounded-control bg-field px-3 py-2 font-mono text-12 break-all text-text-2">
          {wsUrl(port, tok ? maskToken(tok) : "")}
        </code>
      </div>
      <Button className="self-start" onClick={() => void openExternal(DOCS_URL).catch(reportError)}>
        <ExternalLink size={14} strokeWidth={1.75} aria-hidden />
        {t("auto.docs")}
      </Button>
    </div>
  );
}

/** Relaunch config of one account: optimistic write to the snapshot, then Rust. */
function useRelaunch(a: AutomationAccount) {
  const qc = useQueryClient();
  const { data: settings } = useSettings();
  const fallback = Number(
    (settings?.relaunch as { defaultDelaySec?: number } | undefined)?.defaultDelaySec ?? 60,
  );
  const enabled = a.relaunch?.enabled === true;
  // `stored` stays undefined until the user sets a delay, so the account keeps following the
  // Settings default instead of pinning today's value
  const stored = typeof a.relaunch?.seconds === "number" ? a.relaunch.seconds : undefined;
  const seconds = stored ?? fallback;
  const set = (on: boolean, delaySec?: number) => {
    const placeId = Number(a.relaunch?.placeId);
    const jobId = a.relaunch?.jobId;
    // Optimistic, so a second click before the refetch toggles from the new value; the
    // calls reach Rust in order and the refetch (also on error) settles it.
    const relaunch = { ...a.relaunch, enabled: on, seconds: delaySec };
    // an in-flight fetch started before this write would overwrite it with stale data
    void qc.cancelQueries({ queryKey: ["snap"] });
    qc.setQueryData<AutomationAccount[]>(["snap"], (all) =>
      all?.map((x) => (x.account === a.account ? { ...x, relaunch } : x)),
    );
    void call("automation_relaunch_set", {
      account: a.account,
      enabled: on,
      delaySec,
      placeId: Number.isSafeInteger(placeId) && placeId > 0 ? placeId : null,
      jobId: typeof jobId === "string" ? jobId : null,
    })
      .catch(reportError)
      .finally(() => void qc.invalidateQueries({ queryKey: ["snap"] }));
  };
  return { enabled, seconds, stored, set };
}

/** Row end: relaunch engine status and the auto-relaunch switch. */
export function RelaunchSwitch({ a }: { a: AutomationAccount }) {
  const { enabled, stored, set } = useRelaunch(a);
  const s = a.relaunchStatus;
  const id = `rl-${a.account}`;
  const text = s?.stopped
    ? t("auto.cooldown")
    : s?.pendingAt
      ? t("auto.pending", { time: fmtTime(s.pendingAt) })
      : s?.lastRelaunch
        ? t("auto.lastRelaunch", { time: fmtTime(s.lastRelaunch), count: String(s.count) })
        : null;
  return (
    <span className="flex min-w-0 items-center gap-2.5">
      <span
        id={id}
        role="status"
        title={text ?? undefined}
        className={clsx("max-w-52 truncate text-12", s?.stopped ? "text-warn" : "text-text-3")}
      >
        {/* idle: name the switch on screen too (the switch's own label covers screen readers) */}
        {text ?? <span aria-hidden>{t("auto.relaunch")}</span>}
      </span>
      <Toggle
        id={id}
        checked={enabled}
        label={t("auto.relaunch")}
        describedBy={id}
        onChange={(on) => set(on, stored)}
      />
    </span>
  );
}

/** Side panel: seconds to wait before an auto-relaunch (the switch itself is on the row). */
export function RelaunchDelay({ a }: { a: AutomationAccount }) {
  const { enabled, seconds, set } = useRelaunch(a);
  const [draft, setDraft] = useState<string | null>(null);
  const commit = () => {
    if (draft === null) return;
    const n = Math.round(Number(draft));
    setDraft(null);
    if (draft.trim() !== "" && Number.isFinite(n) && Math.min(86_400, Math.max(0, n)) !== seconds)
      set(enabled, Math.min(86_400, Math.max(0, n)));
  };
  return (
    <label className="flex items-center gap-2 text-13 text-text-2">
      {t("auto.delay")}
      <input
        type="number"
        min={0}
        max={86400}
        className={clsx(inputCls, "ml-auto h-7 w-20")}
        value={draft ?? String(seconds)}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && commit()}
      />
    </label>
  );
}

export function AccountRow({
  a,
  active,
  onPick,
}: {
  a: AutomationAccount;
  active: boolean;
  onPick: () => void;
}) {
  return (
    <li
      className={clsx(
        "flex h-14 items-center gap-3 rounded-control pr-3.5 pl-1 transition-colors duration-[var(--dur-quick)]",
        active
          ? "bg-surface-3 shadow-[inset_0_0_0_1px_var(--hairline-strong)]"
          : "hover:bg-surface-2",
      )}
    >
      <button
        type="button"
        onClick={onPick}
        aria-current={active ? "true" : undefined}
        className="flex h-full min-w-0 flex-1 items-center gap-3 rounded-control px-2.5 text-left"
      >
        <span
          aria-hidden
          className={clsx("size-2 shrink-0 rounded-full", a.connected ? "bg-online" : "bg-text-3")}
        />
        <span className="flex min-w-0 flex-col gap-0.5">
          <span data-pii className="truncate text-14 font-semibold text-text-1">
            {a.account}
          </span>
          <span className="text-12 text-text-3">
            {a.connected
              ? t("auto.connected")
              : a.relaunchStatus?.launching
                ? t("auto.launching")
                : t("auto.disconnected")}
          </span>
        </span>
      </button>
      <RelaunchSwitch a={a} />
    </li>
  );
}

/** Per-account "Clear" mark, kept across account switches and page visits (this session). */
const clearedMarks = new Map<string, number>();

function ConsoleView({ a }: { a: AutomationAccount }) {
  const [q, setQ] = useState("");
  const [clearedAt, setCleared] = useState(() => clearedMarks.get(a.account) ?? 0);
  const setClearedAt = (ts: number) => {
    clearedMarks.set(a.account, ts);
    setCleared(ts);
  };
  const lines = useMemo(
    () =>
      filterLines(
        a.console.filter((l) => l.ts > clearedAt),
        q,
      ),
    [a.console, clearedAt, q],
  );
  const box = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  // lines are text-12 (rem): their px height follows the UI scale
  const lineH = (22 * (useSettings().data?.ui.scale ?? 100)) / 100;
  const v = useVirtualizer({
    count: lines.length,
    getScrollElement: () => box.current,
    estimateSize: () => lineH,
    overscan: 12,
    initialRect: { width: 380, height: 320 },
  });
  useEffect(() => v.measure(), [v, lineH]);
  useEffect(() => {
    if (stick.current && lines.length) v.scrollToIndex(lines.length - 1);
  }, [lines, v]);

  // Rust opens the save dialog and writes the whole console (not the filtered view).
  const save = () =>
    void call("automation_save_console", { account: a.account }).catch(reportError);

  return (
    <section aria-label={t("auto.console")} className="flex min-h-40 flex-1 flex-col gap-3">
      <div className="flex items-center gap-1.5">
        <div className="relative flex-1">
          <Search
            size={14}
            strokeWidth={1.75}
            aria-hidden
            className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-text-3"
          />
          <input
            type="search"
            className={clsx(inputCls, "w-full pl-8 [&::-webkit-search-cancel-button]:hidden")}
            placeholder={t("auto.search")}
            aria-label={t("auto.search")}
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        </div>
        <IconButton
          className="shrink-0"
          label={t("auto.save")}
          onClick={save}
          disabled={!a.console.length}
        >
          <Download size={15} strokeWidth={1.75} aria-hidden />
        </IconButton>
        {/* the tooltip explains; the name stays short */}
        <IconButton
          className="shrink-0"
          label={t("auto.clearTip")}
          aria-label={t("auto.clear")}
          onClick={() => setClearedAt(Date.now())}
          disabled={!a.console.length}
        >
          <Eraser size={15} strokeWidth={1.75} aria-hidden />
        </IconButton>
      </div>
      <div
        ref={box}
        role="log"
        aria-label={t("auto.console")}
        tabIndex={0}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
        className="relative min-h-32 flex-1 overflow-auto rounded-panel bg-field py-2 font-mono text-12 shadow-[inset_0_0_0_1px_var(--hairline)] focus-visible:shadow-[var(--focus-ring)]"
      >
        {lines.length === 0 && (
          <p className="px-3 font-sans text-13 text-text-3">
            {q ? t("auto.noMatch") : t("auto.noLines")}
          </p>
        )}
        <div className="relative" style={{ height: v.getTotalSize() }}>
          {v.getVirtualItems().map((r) => {
            const l = lines[r.index];
            return (
              <div
                key={r.key}
                className="absolute flex w-full items-center gap-2.5 px-3"
                style={{ height: r.size, transform: `translateY(${r.start}px)` }}
              >
                <span className="text-text-3 tabular-nums">{fmtTime(l.ts)}</span>
                <span
                  aria-hidden
                  className={clsx(
                    "size-1.5 shrink-0 rounded-full",
                    levelColor[l.level] ?? "bg-text-3",
                  )}
                />
                <span className="sr-only">
                  {levelName[l.level] ? t(levelName[l.level]!) : l.level}
                </span>
                <span data-pii className="truncate text-text-1">
                  {l.text}
                </span>
              </div>
            );
          })}
        </div>
      </div>
    </section>
  );
}

/** Full-width capsule: message to the selected account's script. */
function SendBar({ account, connected }: { account: string; connected: boolean }) {
  const [text, setText] = useState("");
  const send = useMutation({
    mutationFn: () => call("automation_send", { account, text }),
    onSuccess: () => setText(""),
    // it disconnected mid-send: say so next to the bar, not in the app banner
    onError: (e) => toast(errMessage(e)),
  });
  return (
    <form
      className="glass-panel mx-7 mb-5 flex h-14 shrink-0 items-center gap-2.5 rounded-full px-2"
      onSubmit={(e) => {
        e.preventDefault();
        if (text.trim()) send.mutate();
      }}
    >
      <span data-pii className="max-w-48 shrink-0 truncate pr-1 pl-4 text-13 text-text-3">
        {t("auto.to", { name: account })}
      </span>
      <input
        className="h-10 min-w-0 flex-1 rounded-full bg-field px-[18px] text-13 text-text-1 shadow-[inset_0_0_0_1px_var(--hairline)] placeholder:text-text-3 focus-visible:shadow-[var(--focus-ring)] disabled:opacity-60"
        // no name here: a placeholder can't be blurred in privacy mode ("To {name}" is beside it)
        placeholder={connected ? t("auto.sendHint") : t("auto.notConnected")}
        aria-label={t("auto.sendHint")}
        disabled={!connected}
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      <Button
        type="submit"
        variant="primary"
        pill
        disabled={!connected || !text.trim() || send.isPending}
      >
        <Send size={15} strokeWidth={1.75} aria-hidden />
        {t("auto.send")}
      </Button>
    </form>
  );
}

/** Faint token line: masked on screen, Copy carries the real value. Regenerate is in Settings. */
function TokenLine() {
  const { data: tok } = useToken();
  if (!tok) return null;
  return (
    <div className="mt-3.5 flex h-10 items-center gap-2.5 px-3.5 text-12 text-text-3">
      <KeyRound size={14} strokeWidth={1.75} aria-hidden />
      {t("settings.token")}
      <code className="font-mono text-text-2">{maskToken(tok)}</code>
      <CopyButton label={t("settings.token")} sm ghost />
    </div>
  );
}

function Panel({ a }: { a: AutomationAccount }) {
  const place = [a.placeId, a.jobId].filter(Boolean).map(String).join(" / ");
  return (
    <aside
      aria-label={a.account}
      className="glass-panel flex w-[clamp(300px,38%,26.25rem)] shrink-0 flex-col gap-3.5 overflow-y-auto rounded-panel p-5"
    >
      <div>
        <h2 data-pii className="truncate text-[1.125rem] font-semibold text-text-1">
          {a.account}
        </h2>
        <p className="mt-0.5 flex items-center gap-1.5 text-12 text-text-3">
          <span
            aria-hidden
            className={clsx("size-[7px] rounded-full", a.connected ? "bg-online" : "bg-text-3")}
          />
          <span className={clsx("truncate", place && "font-mono")}>
            {place || t("auto.noPlace")}
          </span>
        </p>
      </div>
      <ConsoleView key={a.account} a={a} />
      <section aria-label={t("auto.controls")} className="flex flex-col gap-2">
        <h3 className="text-12 font-semibold text-text-2">{t("auto.controls")}</h3>
        <Controls account={a.account} controls={a.controls} />
      </section>
      <div className="h-px bg-hairline" />
      <RelaunchDelay key={`d-${a.account}`} a={a} />
    </aside>
  );
}

export function AutomationPage() {
  const { data: snap = [] } = useSnapshot();
  const { data: st } = useAutomationStatus();
  const [picked, setPicked] = useState<string | null>(null);
  const sel = snap.find((a) => a.account === picked) ?? snap[0];
  // the implicit pick sticks: a new account sorting first must not move the panel
  useEffect(() => {
    if (picked === null && sel) setPicked(sel.account);
  }, [picked, sel]);

  return (
    <div className="flex h-full flex-col">
      {!sel ? (
        <div className="px-7 pt-1">
          <StatusStrip />
          <Empty port={st?.wsPort ?? 7810} />
        </div>
      ) : (
        <>
          <div className="flex min-h-0 flex-1 gap-4 px-7 pt-1 pb-3">
            <section className="flex min-w-0 flex-1 flex-col">
              <StatusStrip />
              <nav aria-label={t("auto.accounts")} className="min-h-0 overflow-y-auto">
                <ul className="flex flex-col">
                  {snap.map((a) => (
                    <AccountRow
                      key={a.account}
                      a={a}
                      active={a.account === sel.account}
                      onPick={() => setPicked(a.account)}
                    />
                  ))}
                </ul>
                <TokenLine />
              </nav>
            </section>
            <Panel a={sel} />
          </div>
          <SendBar key={`s-${sel.account}`} account={sel.account} connected={sel.connected} />
        </>
      )}
    </div>
  );
}
