import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { queryClient } from "@/lib/queryClient";
import { useEffect } from "react";
import { create } from "zustand";
import { toast } from "@/components/Toasts";
import { label } from "@/features/accounts/model";
import { KEY, namesHidden } from "@/features/accounts/useAccounts";
import {
  call,
  codeText,
  errMessage,
  FAVORITES_CHANGED,
  inTauri,
  LAUNCH_DONE,
  LAUNCH_PROGRESS,
  onEvent,
  RECENTS_CHANGED,
  TRAY_PLAY,
  type AccountsList,
  type Capabilities,
  type LaunchDone,
  type LaunchMode,
  type LaunchProgress,
  type LaunchSnapshot,
  type LaunchState,
  type RobloxStatus,
  type VaultStatus,
} from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { useUi } from "@/app/store";
import { saveLaunchSet, lastLaunchSet, usePlayBar } from "./store";

/** The batch on the Play button: starting (`id` null), running, then `done` for the settle check.
 * Any source: this UI's Play, or one adopted from events (auto-relaunch, API, reserve-join). */
export type Batch = {
  id: string | null;
  ids: string[];
  done: LaunchDone | null;
  /** Cancel pressed before launch_start returned the id: cancelled as soon as it does. */
  cancel?: boolean;
  /** A reserve-and-join in flight: nothing to cancel until its real batch arrives. */
  reserve?: boolean;
};

type LaunchStore = {
  batch: Batch | null;
  /** batchId → accountId → state, for every live batch. Events can arrive before launch_start
   * returns the id. An entry goes once its done is handled. */
  states: Record<string, Record<string, LaunchState>>;
  dones: Record<string, LaunchDone>;
  /** The last play came from a focused Play button (focus goes to Cancel, then back). */
  focus: boolean;
};

export const useLaunch = create<LaunchStore>(() => ({
  batch: null,
  states: {},
  dones: {},
  focus: false,
}));

/** Per-account states of the current batch. */
export function useBatchStates() {
  return useLaunch((s) => (s.batch?.id ? s.states[s.batch.id] : undefined));
}

export const SETTLE_MS = 1200;

/** Batches whose done was seen: a late launch_status reply must not bring them back.
 * ponytail: grows by one short id per batch for the app's life; prune if that ever matters. */
const ended = new Set<string>();

type S = LaunchStore;

/** Nothing shown: show the oldest live batch (one we didn't start, e.g. an auto-relaunch). */
function adopt(s: S): Partial<S> {
  if (s.batch) return {};
  const id = Object.keys(s.states).find((b) => !ended.has(b));
  return id ? { batch: { id, ids: Object.keys(s.states[id]), done: null }, focus: false } : {};
}

/** Drop finished batches nobody shows (or will: a start in flight may still own one). */
function prune(s: S): Partial<S> {
  if (s.batch && !s.batch.id) return {};
  const keep = (b: string) => !ended.has(b) || b === s.batch?.id;
  const states = Object.fromEntries(Object.entries(s.states).filter(([b]) => keep(b)));
  const dones = Object.fromEntries(Object.entries(s.dones).filter(([b]) => keep(b)));
  return { states, dones };
}

/** Once the shown batch has both its id and its done event: show the check, then clear. */
function settle() {
  const { batch: b, dones } = useLaunch.getState();
  const d = b?.id ? dones[b.id] : undefined;
  if (!b || !d || b.done) return;
  const settled = { ...b, done: d };
  useLaunch.setState({ batch: settled });
  setTimeout(() => {
    useLaunch.setState((s) => {
      if (s.batch !== settled) return {};
      const next = { ...s, batch: null, ...prune({ ...s, batch: null }) };
      return { ...next, ...adopt(next) };
    });
  }, SETTLE_MS);
}

/** Clears the starting placeholder (`id` null) for these ids the way settle clears a batch,
 * whatever happened to the object meanwhile (a cancel replaces it). */
export function clearPlaceholder(ids: string[]) {
  useLaunch.setState((s) => {
    if (!s.batch || s.batch.id || s.batch.ids !== ids) return {};
    const next = { ...s, batch: null, ...prune({ ...s, batch: null }) };
    return { ...next, ...adopt(next) };
  });
}

type PlayOpts = {
  /** Use the Play bar for every account, ignoring saved launch targets (also implied by a
   * non-empty bar destination). */
  here?: boolean;
  /** Put this destination in the Play bar first (only once the play is accepted). */
  dest?: string;
  /** With `dest`: its server pick (Play once). */
  mode?: LaunchMode;
  server?: string;
  /** Started from a focused Play button. */
  focus?: boolean;
};
type Play = (ids: string[], opts?: PlayOpts) => void;

/** True while a batch is on the Play button (one at a time, whatever the entry point). A batch
 * that is done and only showing its check no longer counts; the next play replaces it. */
export const launchBusy = () => {
  const b = useLaunch.getState().batch;
  return !!b && !b.done;
};

/** A server pick without a destination makes Rust skip the home-screen path: drop it. */
export const barOf = (destination: string, server: string, mode: LaunchMode) => {
  const d = destination.trim();
  return { destination: d, server: d ? server.trim() : "", mode, home: true };
};

/** Launch unsupported or Roblox missing, per the cached caps and status: the reason, else null.
 * A block refetches both, so installing Roblox unblocks the next try without a restart. */
export function launchBlocked(qc: QueryClient): string | null {
  const caps = qc.getQueryData<Capabilities>(["caps"]);
  const why =
    caps && !caps.launch
      ? (caps.launchReason ?? t("play.unsupported"))
      : qc.getQueryData<RobloxStatus>(["roblox"])?.installed === false
        ? t("play.noRoblox")
        : null;
  if (why) for (const k of ["caps", "roblox"]) void qc.refetchQueries({ queryKey: [k] });
  return why;
}

const play: Play = (ids, opts) => {
  if (!ids.length) return;
  // every entry point (row, Enter, menus, inspector, hero, tray), not just the Play bar
  const why = launchBlocked(queryClient);
  if (why) return void toast(why);
  // one batch at a time, whatever the entry point (row, keyboard, menus, inspector, Play bar,
  // hero, tray); checked before any side effect
  if (launchBusy()) return void toast(t("launch.busy"));
  saveLaunchSet(ids); // Home's hero Play reads it
  if (opts?.dest !== undefined) usePlayBar.getState().setDestination(opts.dest);
  if (opts?.mode) usePlayBar.setState({ mode: opts.mode, server: opts.server ?? "" });
  const { destination, server, mode } = usePlayBar.getState();
  const mine: Batch = { id: null, ids, done: null };
  useLaunch.setState({ batch: mine, focus: !!opts?.focus });
  const isMine = (b: Batch | null) => !!b && !b.id && b.ids === ids;
  call("launch_start", {
    accountIds: ids,
    bar: barOf(destination, server, mode),
    // a bar destination (Discover, Play once) wins over saved targets until cleared
    useBarForAll: !!opts?.here || !!destination.trim(),
  }).then(
    ({ batchId }) => {
      const b = useLaunch.getState().batch;
      if (!isMine(b)) return;
      if (b!.cancel) cancelId(batchId);
      useLaunch.setState((s) => {
        const next = { ...s, batch: { ...b!, id: batchId } };
        return { ...next, ...prune(next) };
      });
      settle();
    },
    (e) => {
      clearPlaceholder(ids);
      toast(errMessage(e));
    },
  );
};

const cancelId = (batchId: string) =>
  call("launch_cancel", { batchId }).catch((e) => toast(errMessage(e)));

const cancel = () => {
  const b = useLaunch.getState().batch;
  if (!b || b.done || b.reserve) return;
  if (b.id) void cancelId(b.id);
  else useLaunch.setState({ batch: { ...b, cancel: true } });
};

export function usePlay(): { play: Play; cancel: () => void } {
  return { play, cancel };
}

/** Mount once (App, so it survives a lock): turns launch events into store state and result
 * toasts, and resyncs live batches from Rust on mount (after a reload). */
export function useLaunchEvents() {
  const qc = useQueryClient();
  useEffect(() => {
    const nameOf = (id: string) => {
      const a = qc.getQueryData<AccountsList>(KEY)?.accounts.find((x) => x.id === id);
      return a && !namesHidden(qc) ? label(a) : t("launch.anAccount");
    };
    const offP = onEvent<LaunchProgress>(LAUNCH_PROGRESS, (p) => {
      useLaunch.setState((s) => {
        const states = {
          ...s.states,
          [p.batchId]: { ...s.states[p.batchId], [p.accountId]: p.state },
        };
        const b = s.batch;
        // the shown batch learns accounts it didn't know (adopted before all were queued)
        if (b?.id === p.batchId && !b.ids.includes(p.accountId))
          return { states, batch: { ...b, ids: [...b.ids, p.accountId] } };
        return { states, ...adopt({ ...s, states }) };
      });
      const name = nameOf(p.accountId);
      if (p.state === "launched")
        toast(
          p.gameName ? t("launch.ok", { name, game: p.gameName }) : t("launch.okNoGame", { name }),
        );
      else if (p.state === "failed")
        toast(
          t("launch.failed", { name, why: p.message ? codeText(p.message) : t("launch.unknown") }),
        );
    });
    const offD = onEvent<LaunchDone>(LAUNCH_DONE, (d) => {
      void qc.invalidateQueries({ queryKey: ["recents"] });
      // accounts refetch comes from accounts://changed (launch_one), which waits for saves in flight
      ended.add(d.batchId);
      useLaunch.setState((s) => {
        const next = { ...s, dones: { ...s.dones, [d.batchId]: d } };
        return { ...next, ...prune(next) };
      });
      settle();
    });
    // After a reload: live batches (any source) come back on the Play button.
    // only after both listeners exist, so a done event in the gap is not lost
    Promise.all([offP.ready, offD.ready])
      .then(() => call("launch_status"))
      .then(
        (list: LaunchSnapshot[]) =>
          useLaunch.setState((s) => {
            const states = { ...s.states };
            for (const b of list) {
              if (b.done || ended.has(b.launchId)) continue;
              const got = Object.fromEntries(
                Object.entries(b.accounts ?? {}).map(([a, v]) => [a, v.state]),
              );
              states[b.launchId] = { ...got, ...states[b.launchId] }; // events seen win
            }
            return { states, ...adopt({ ...s, states }) };
          }),
        () => {}, // ponytail: no resync if this fails; events still arrive
      );
    const offR = onEvent(RECENTS_CHANGED, () => {
      void qc.invalidateQueries({ queryKey: ["recents"] });
    });
    const offF = onEvent(FAVORITES_CHANGED, () => {
      void qc.invalidateQueries({ queryKey: ["favorites"] });
    });
    const offT = onEvent<{ placeId?: number; accountId?: string }>(TRAY_PLAY, (p) => {
      // Locked (listeners outlive the Shell): no launch, no Play bar or last-set change.
      if (!qc.getQueryData<VaultStatus>(["vault"])?.unlocked) return;
      if (p.accountId) return play([p.accountId]);
      if (!p.placeId) return;
      // a recent from the tray: the current selection, else the last launched set
      const sel = useUi.getState().sel.ids;
      const live = new Set(qc.getQueryData<AccountsList>(KEY)?.accounts.map((a) => a.id));
      const ids = sel.length ? sel : lastLaunchSet().filter((id) => live.has(id));
      if (!ids.length) {
        // The window may be hidden in the tray: ask the shell to show it so the toast is seen.
        if (inTauri) void import("@tauri-apps/api/event").then(({ emit }) => emit("tray://show"));
        return void toast(t("play.noneSelected"));
      }
      play(ids, { here: true, dest: String(p.placeId) });
    });
    return () => {
      offP();
      offD();
      offR();
      offF();
      offT();
    };
  }, [qc]);
}
