import * as Tooltip from "@radix-ui/react-tooltip";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { Check, ChevronUp, Grid2X2, Play, TriangleAlert, X } from "lucide-react";
import { motion, useReducedMotionConfig } from "motion/react";
import { createPortal } from "react-dom";
import { toast } from "@/components/Toasts";
import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useUi } from "@/app/store";
import { Button } from "@/components/ui/Button";
import { Avatar } from "@/features/accounts/AccountRow";
import { useDestName } from "@/features/accounts/DestLabel";
import { useAccounts } from "@/features/accounts/useAccounts";
import { GridPopover } from "@/features/grid/GridPopover";
import { call, type AccountView, type LaunchMode, type LaunchTarget } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { Chip } from "./DestChip";
import { ServerPick } from "./ServerPick";
import { targetFromBar, usePlayBar } from "./store";
import { launchBlocked, useBatchStates, useLaunch, usePlay } from "./usePlay";

const EXPO: [number, number, number, number] = [0.16, 1, 0.3, 1];
const MORPH_S = 0.22;
const FLIGHT_S = 0.35;

function Tip({ text, children }: { text: string | null; children: ReactNode }) {
  // Root + Trigger always render so toggling text never remounts (and unfocuses) the child.
  return (
    <Tooltip.Root>
      <Tooltip.Trigger asChild>{children}</Tooltip.Trigger>
      {text && (
        <Tooltip.Portal>
          <Tooltip.Content
            side="top"
            sideOffset={8}
            className="glass z-50 max-w-72 rounded-chip px-2 py-1 text-12 text-text-1"
          >
            {text}
          </Tooltip.Content>
        </Tooltip.Portal>
      )}
    </Tooltip.Root>
  );
}

// ---- Play button / progress capsule ----

const MAX_STACK = 5;

/** One avatar in the capsule. With motion, a fixed clone flies from the row (transform only,
 * in a portal so the capsule's overflow can't clip it); the slot shows once it lands. */
function Slot({
  a,
  launched,
  born,
  fly,
}: {
  a: AccountView;
  launched: boolean;
  born: number;
  fly: boolean;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  const [flight, setFlight] = useState<{
    left: number;
    top: number;
    dx: number;
    dy: number;
    s: number;
  } | null>(null);
  const [landed, setLanded] = useState(!fly);
  // motion turned off mid-flight: just show it
  const shown = landed || !fly;
  useLayoutEffect(() => {
    if (!fly) return;
    // wait for the button-to-capsule morph so the landing spot is stable
    const id = setTimeout(
      () => {
        const from = document
          .getElementById(`row-${a.id}`)
          ?.querySelector("[data-avatar]")
          ?.getBoundingClientRect();
        const to = ref.current?.getBoundingClientRect();
        if (from?.width && to?.width)
          setFlight({
            left: to.left,
            top: to.top,
            dx: from.left - to.left,
            dy: from.top - to.top,
            s: from.width / to.width,
          });
        else setLanded(true); // row scrolled away: just appear
      },
      Math.max(0, born + MORPH_S * 1000 - Date.now()),
    );
    return () => clearTimeout(id);
  }, [a.id, born, fly]);
  return (
    <span className="relative -ml-2 shrink-0 first:ml-0">
      <span
        ref={ref}
        data-testid="slot"
        className={clsx("block rounded-control ring-2 ring-accent", !shown && "opacity-0")}
      >
        <Avatar a={a} size={24} />
      </span>
      {launched && shown && (
        <Check
          size={10}
          strokeWidth={3.5}
          aria-hidden
          data-testid="tick"
          className="absolute -right-1 -bottom-1 z-10 rounded-full bg-accent-ink p-px text-accent"
        />
      )}
      {flight &&
        !shown &&
        createPortal(
          <motion.span
            data-testid="flyer"
            aria-hidden
            className="pointer-events-none fixed z-[60] block rounded-control"
            style={{ left: flight.left, top: flight.top, transformOrigin: "0 0" }}
            initial={{ x: flight.dx, y: flight.dy, scale: flight.s }}
            animate={{ x: 0, y: 0, scale: 1 }}
            transition={{ duration: FLIGHT_S, ease: EXPO }}
            onAnimationComplete={() => setLanded(true)}
          >
            <Avatar a={a} size={24} />
          </motion.span>,
          document.body,
        )}
    </span>
  );
}

function Capsule({ focusCancel }: { focusCancel: boolean }) {
  const accounts = useAccounts().data?.accounts ?? [];
  const batch = useLaunch((s) => s.batch)!;
  const states = useBatchStates() ?? {};
  const { cancel } = usePlay();
  const reduced = !!useReducedMotionConfig();
  const [born] = useState(Date.now);
  const total = batch.ids.length;
  const started = batch.ids.filter((id) => states[id] && states[id] !== "queued");
  const d = batch.done;
  if (d) {
    const bad = d.failed + d.cancelled;
    const Icon = d.launched ? Check : TriangleAlert;
    return (
      <motion.span
        key="done"
        initial={{ opacity: 0, scale: 0.9 }}
        animate={{ opacity: 1, scale: 1 }}
        transition={{ duration: 0.18, ease: EXPO }}
        className="flex min-w-0 items-center gap-2 px-4 text-14 font-semibold text-accent-ink tnum"
      >
        <Icon size={16} strokeWidth={2.5} aria-hidden className="shrink-0" />
        <span className="truncate">
          {bad
            ? t("launch.summary", { ok: d.launched, bad })
            : t("launch.allOk", { n: d.launched })}
        </span>
      </motion.span>
    );
  }
  const byId = new Map(accounts.map((a) => [a.id, a]));
  // batch order, first MAX_STACK only: new avatars append, nothing shifts
  const stack = started.slice(0, MAX_STACK).flatMap((id) => byId.get(id) ?? []);
  const more = started.length - stack.length;
  const progress = { n: Math.max(1, started.length), total };
  return (
    <motion.span
      key="run"
      layout="position"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      transition={{ duration: 0.18, delay: 0.06 }}
      className="flex min-w-0 items-center gap-3 pr-1.5 pl-2 text-13 font-medium text-accent-ink @max-[52rem]:gap-2"
    >
      {/* clipped first when the bar is tight; text and Cancel never are */}
      {stack.length > 0 && (
        <span className="flex min-w-0 shrink items-center overflow-hidden py-1 pr-1 pl-0.5">
          {stack.map((a) => (
            <Slot
              key={a.id}
              a={a}
              launched={states[a.id] === "launched"}
              born={born}
              fly={!reduced}
            />
          ))}
          {more > 0 && <span className="ml-1 shrink-0 tnum">+{more}</span>}
        </span>
      )}
      <span className="shrink-0 whitespace-nowrap tnum">
        <span className="@max-[52rem]:hidden">{t("launch.progress", progress)}</span>
        <span className="hidden @max-[52rem]:inline">{t("launch.progressShort", progress)}</span>
      </span>
      {!batch.reserve && (
        <button
          type="button"
          autoFocus={focusCancel}
          aria-disabled={!!batch.cancel}
          onClick={cancel}
          className="h-7 shrink-0 rounded-full bg-accent-ink/12 px-3 text-13 font-semibold text-accent-ink transition-colors hover:bg-accent-ink/20 aria-disabled:opacity-50"
        >
          {t("launch.cancel")}
        </button>
      )}
    </motion.span>
  );
}

const hasTarget = (x?: LaunchTarget | null) => !!(x?.placeId || x?.privateLink);
/** Saved destinations: the account's own, else its group's. */
const hasSavedTarget = (a: AccountView, groupTargets: Record<string, LaunchTarget> = {}) =>
  hasTarget(a.launchTarget) || hasTarget(groupTargets[a.group]);

/** The destination itself picks a server: private link, game instance or share link. */
const namesServer = (d: string) =>
  /privateServerLinkCode=|gameInstanceId=|\/share(-links)?\b/i.test(d);

function blockReason(
  sel: AccountView[],
  bar: { destination: string; server: string; mode: LaunchMode },
  caps: { launch: boolean; launchReason: string | null } | undefined,
  installed: boolean | undefined,
): string | null {
  if (!sel.length) return t("play.noneSelected");
  if (caps && !caps.launch) return caps.launchReason ?? t("play.unsupported");
  if (installed === false) return t("play.noRoblox");
  // nobody signed in yet: every launch would fail (some signed in: the rest report in the summary)
  if (sel.every((a) => a.session === "pending")) return t("session.pendingLong");
  // no game: accounts with a saved target go there, the rest open Roblox's home screen
  if (!bar.destination.trim()) return null;
  // a bar destination plays every selected account there
  if (bar.mode === "specific" && !bar.server.trim() && !namesServer(bar.destination))
    return t("play.noServer");
  return null;
}

function PlayButton({
  reason,
  home,
  onPlay,
}: {
  reason: string | null;
  /** no game anywhere: Play opens Roblox's home screen */
  home: boolean;
  onPlay: (fromButton: boolean) => void;
}) {
  const ids = useUi((u) => u.sel.ids);
  const running = useLaunch((s) => !!s.batch);
  // the last play came from a focused Play button (not Enter, menus, tray or an adopted batch)
  const hadFocus = useLaunch((s) => s.focus);
  const btn = useRef<HTMLButtonElement>(null);
  const reasonId = useId();
  const wasRunning = useRef(running);
  // back to Play after the settle, unless focus moved somewhere on purpose
  useEffect(() => {
    const idle = !document.activeElement || document.activeElement === document.body;
    if (wasRunning.current && !running && hadFocus && idle) btn.current?.focus();
    wasRunning.current = running;
  }, [running, hadFocus]);
  return (
    <motion.div
      layout
      transition={{ duration: MORPH_S, ease: EXPO }}
      className={clsx(
        "flex h-10 max-w-80 items-center overflow-hidden shadow-[inset_0_1px_0_rgb(255_255_255/0.3)] transition-colors duration-[var(--dur-base)]",
        running ? "min-w-42 shrink" : "shrink-0", // min fits one avatar + "n of N" + Cancel
        // blocked drops the accent, like Button's disabled primary (low-opacity accent reads muddy)
        reason && !running ? "bg-surface-3" : "bg-accent",
      )}
      style={{ borderRadius: 999 }}
    >
      {running ? (
        <Capsule focusCancel={hadFocus} />
      ) : (
        <Tip text={reason}>
          <motion.button
            ref={btn}
            layout="position"
            type="button"
            aria-disabled={!!reason}
            aria-describedby={reason ? reasonId : undefined}
            onClick={(e) => onPlay(document.activeElement === e.currentTarget)}
            className={clsx(
              "inline-flex h-10 min-w-0 items-center gap-2 px-5 text-14 font-semibold whitespace-nowrap text-accent-ink transition-colors hover:bg-accent-hover focus-visible:shadow-[inset_0_0_0_2px_var(--on-image)] active:bg-accent-press",
              reason && "cursor-not-allowed text-text-3 hover:bg-transparent active:bg-transparent",
            )}
          >
            <Play
              size={16}
              strokeWidth={2.25}
              fill="currentColor"
              aria-hidden
              className="shrink-0"
            />
            <span className="truncate">
              {t(`playbar.${home ? "home" : "play"}${ids.length > 1 ? "N" : ""}`, {
                n: ids.length,
              })}
            </span>
          </motion.button>
        </Tip>
      )}
      {reason && !running && (
        <span id={reasonId} className="sr-only">
          {reason}
        </span>
      )}
    </motion.div>
  );
}

/** Mounted only while blocked, so a dismiss lasts until the next time it is blocked. */
function MultiBanner({ msg = t("play.multiBlocked") }: { msg?: string }) {
  const [gone, setGone] = useState(false);
  if (gone) return null;
  return (
    <div
      role="note"
      className="flex items-center gap-3 border-t border-hairline bg-surface-1 px-4 py-2 text-13 text-text-2"
    >
      <TriangleAlert size={14} strokeWidth={1.75} aria-hidden className="shrink-0 text-warn" />
      <p className="flex-1">{msg}</p>
      <button
        type="button"
        aria-label={t("error.dismiss")}
        onClick={() => setGone(true)}
        className="inline-grid size-7 place-items-center rounded-control text-text-3 hover:bg-surface-3 hover:text-text-1"
      >
        <X size={14} strokeWidth={1.75} />
      </button>
    </div>
  );
}

export function PlayBar() {
  const { play } = usePlay();
  const destination = usePlayBar((s) => s.destination);
  const server = usePlayBar((s) => s.server);
  const mode = usePlayBar((s) => s.mode);
  const ids = useUi((u) => u.sel.ids);
  const { data: list } = useAccounts();
  const { data: caps } = useQuery({
    queryKey: ["caps"],
    queryFn: () => call("platform_capabilities"),
  });
  const { data: rbx } = useQuery({
    queryKey: ["roblox"],
    queryFn: () => call("roblox_status"),
    staleTime: 30_000,
    // nothing signals Roblox closing: poll only while a multi-instance banner waits on it
    refetchInterval: (q) =>
      q.state.data?.multiBlockedByRunningRoblox || q.state.data?.multiReleasePending ? 5000 : false,
    meta: { silent: true },
  });
  const qc = useQueryClient();
  const [gridOpen, setGridOpen] = useState(false);
  const sel = (list?.accounts ?? []).filter((a) => ids.includes(a.id));
  const groupTargets = list?.groupTargets ?? {};
  const reason = blockReason(sel, { destination, server, mode }, caps, rbx?.installed);
  const running = useLaunch((s) => !!s.batch);
  const [onceOpen, setOnceOpen] = useState(false);
  // a launch from elsewhere closes Play once: no popover (and autoFocus) popping back later
  if (running && onceOpen) setOnceOpen(false);
  const wrap = useRef<HTMLDivElement>(null);
  const chevron = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!onceOpen) return;
    const away = (e: PointerEvent) => {
      if (!wrap.current?.contains(e.target as Node)) setOnceOpen(false);
    };
    document.addEventListener("pointerdown", away);
    return () => document.removeEventListener("pointerdown", away);
  }, [onceOpen]);
  const destName = useDestName(targetFromBar(destination, "", "any"));
  // Play click: when blocked, say why (toasts are a live region)
  const tryPlay = (fromButton = false) => {
    if (reason) {
      launchBlocked(qc); // refetch caps + status: Roblox may be installed by now
      return void toast(reason);
    }
    play(ids, { focus: fromButton });
  };
  return (
    <>
      {rbx?.multiBlockedByRunningRoblox && <MultiBanner />}
      {rbx?.multiReleasePending && <MultiBanner key="pending" msg={t("play.multiPending")} />}
      {rbx?.multiInProcess && <MultiBanner key="inproc" msg={t("play.multiInProcess")} />}
      <div
        ref={wrap}
        className="relative shrink-0"
        onKeyDown={(e) => {
          if (e.key !== "Escape" || !onceOpen) return;
          setOnceOpen(false);
          chevron.current?.focus();
        }}
      >
        {/* outside the bar: a nested backdrop-filter would blur only the bar */}
        {onceOpen && !running && (
          <PlayOnce
            reasonFor={(bar) =>
              blockReason(sel, bar, caps, rbx?.installed) ??
              // a server with no destination would be dropped (home screen): ask for the game
              (bar.mode === "specific" && bar.server.trim() && !bar.destination.trim()
                ? t("dest.invalid")
                : null)
            }
            onPlay={(bar) => {
              setOnceOpen(false);
              play(ids, { dest: bar.destination, mode: bar.mode, server: bar.server });
            }}
          />
        )}
        <div className="@container glass-panel mx-7 mb-5 flex h-14 shrink-0 items-center gap-2.5 rounded-full pr-2 pl-6">
          <span className="shrink-0 text-13 font-semibold tnum">
            {ids.length ? t("playbar.selected", { n: ids.length }) : t("playbar.none")}
          </span>
          {destination.trim() && (
            // a bar destination (Discover, Play once): every Play goes here until cleared
            <span className="flex min-w-0 items-center gap-1 text-12 text-text-2">
              {/* raw text can be a @username follow: blur unless resolved to a game name */}
              <span className="truncate" data-pii={destName ? undefined : true}>
                {t("playbar.dest", { dest: destName ?? destination.trim() })}
              </span>
              <button
                type="button"
                aria-label={t("playbar.destClear")}
                onClick={() => usePlayBar.getState().setDestination("")}
                className="inline-grid size-6 shrink-0 place-items-center rounded-control text-text-3 hover:bg-surface-3 hover:text-text-1"
              >
                <X size={13} strokeWidth={1.75} />
              </button>
            </span>
          )}
          <span className="flex-1" />
          {caps?.windowRect && (
            <button
              type="button"
              onClick={() => setGridOpen(true)}
              className="inline-flex h-10 shrink-0 items-center gap-2 rounded-full bg-surface-3 px-5 text-13 font-semibold text-text-1 shadow-[inset_0_1px_0_rgb(255_255_255/0.14)] hover:bg-surface-4"
            >
              <Grid2X2 size={16} aria-hidden />
              <span>{t("grid.arrange")}</span>
            </button>
          )}
          <PlayButton
            reason={reason}
            home={
              !!sel.length &&
              !destination.trim() &&
              !sel.some((a) => hasSavedTarget(a, groupTargets))
            }
            onPlay={tryPlay}
          />
          {!running && (
            <button
              ref={chevron}
              type="button"
              aria-label={t("playbar.once")}
              aria-expanded={onceOpen}
              onClick={() => setOnceOpen((o) => !o)}
              className="-ml-1 inline-grid size-10 shrink-0 place-items-center rounded-full bg-surface-3 text-text-1 shadow-[inset_0_1px_0_rgb(255_255_255/0.14)] hover:bg-surface-4"
            >
              <ChevronUp size={16} strokeWidth={2} aria-hidden />
            </button>
          )}
        </div>
      </div>
      <GridPopover open={gridOpen} onOpenChange={setGridOpen} />
    </>
  );
}

type Bar = { destination: string; server: string; mode: LaunchMode };

/** Chevron popover: one play of every selected account at a destination typed here. */
function PlayOnce({
  reasonFor,
  onPlay,
}: {
  reasonFor: (bar: Bar) => string | null;
  onPlay: (bar: Bar) => void;
}) {
  const id = useId();
  const [bar, setBar] = useState<Bar>(() => {
    const { destination, server, mode } = usePlayBar.getState();
    return { destination, server, mode };
  });
  const { data: recents = [] } = useQuery({
    queryKey: ["recents"],
    queryFn: () => call("recents_list"),
    meta: { silent: true },
  });
  const reason = reasonFor(bar);
  const go = () => (reason ? void toast(reason) : onPlay(bar));
  return (
    <div
      role="dialog"
      aria-label={t("playbar.once")}
      className="glass absolute right-7 bottom-[calc(100%+8px)] z-40 flex w-90 flex-col gap-3 rounded-panel p-4"
    >
      <label htmlFor={id} className="text-12 font-medium text-text-2">
        {t("playbar.once")}
      </label>
      <input
        id={id}
        autoFocus
        list={`${id}-recents`}
        value={bar.destination}
        spellCheck={false}
        placeholder={t("playbar.placeholder")}
        onChange={(e) => setBar({ ...bar, destination: e.target.value })}
        onKeyDown={(e) => e.key === "Enter" && go()}
        className="h-9 rounded-control bg-field px-3 text-13 text-text-1 shadow-[inset_0_0_0_1px_var(--hairline-strong)] placeholder:text-text-3 focus:shadow-[var(--focus-ring)] focus:outline-none"
      />
      <datalist id={`${id}-recents`}>
        {recents.map((r) => (
          <option key={r.placeId} value={String(r.placeId)} label={r.name} />
        ))}
      </datalist>
      <div className="min-h-5">
        <Chip id={`${id}-pv`} input={bar.destination} />
      </div>
      <ServerPick
        mode={bar.mode}
        job={bar.server}
        onMode={(mode) => setBar({ ...bar, mode })}
        onJob={(server) => setBar({ ...bar, server })}
        onEnter={go}
      />
      <Button
        variant="primary"
        className="aria-disabled:bg-surface-3 aria-disabled:text-text-3 aria-disabled:shadow-none"
        aria-disabled={!!reason}
        title={reason ?? undefined}
        onClick={go}
      >
        <Play size={15} strokeWidth={2} fill="currentColor" aria-hidden />
        {t("playbar.play")}
      </Button>
    </div>
  );
}

/** Other routes: just the running capsule (progress + Cancel) while a batch is on. */
export function RunningBar() {
  return (
    <div className="@container glass-panel mx-7 mb-5 flex h-14 shrink-0 items-center justify-end rounded-full px-2">
      <PlayButton reason={null} home={false} onPlay={() => {}} />
    </div>
  );
}
