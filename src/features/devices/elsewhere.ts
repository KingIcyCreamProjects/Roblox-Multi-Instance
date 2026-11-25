import { useQuery } from "@tanstack/react-query";
import { create } from "zustand";
import { toast } from "@/components/Toasts";
import { label } from "@/features/accounts/model";
import { KEY as ACCOUNTS, namesHidden } from "@/features/accounts/useAccounts";
import { launchBlocked, launchBusy } from "@/features/playbar/usePlay";
import { call, errMessage, type AccountView, type Elsewhere, type RemoteTarget } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { queryClient } from "@/lib/queryClient";

const KEY = ["elsewhere"];
const fetchList = () => call("remote_elsewhere");
const toMap = (list: Elsewhere[]) => new Map(list.map((e) => [e.userId, e]));
const NONE = new Map<number, Elsewhere>();
/** Where accounts run right now (the cached list can be a move behind); undefined after a toast. */
const fresh = () =>
  queryClient
    .fetchQuery({ queryKey: KEY, queryFn: fetchList, staleTime: 0, meta: { silent: true } })
    .then(toMap, (e) => void toast(errMessage(e)));

/** userId -> where it runs, refetched every 5 s (query key ["elsewhere"]). Empty while locked. */
export function useElsewhere(): Map<number, Elsewhere> {
  const { data } = useQuery({
    queryKey: KEY,
    queryFn: fetchList,
    select: toMap,
    refetchInterval: 5000,
    meta: { silent: true },
  });
  return data ?? NONE;
}

/** Account ids being moved right now (buttons show "Moving..." and are disabled). */
export const useMoving = create<{ ids: string[] }>(() => ({ ids: [] }));

/** Ids taken by a run in progress, from the click on. Not shown: only the accounts that really
 *  move go into `useMoving`, once `fresh()` says which. */
const taken = new Set<string>();

/** Takes the accounts no other run holds, at once (a double click while `fresh()` runs must
 *  not start a second move); returns their ids. `release` gives them back in `finally`. */
function claim(accounts: AccountView[]): string[] {
  const ids = accounts.filter((a) => !taken.has(a.id)).map((a) => a.id);
  ids.forEach((id) => taken.add(id));
  return ids;
}
const mark = (go: AccountView[]) =>
  useMoving.setState((s) => ({ ids: [...s.ids, ...go.map((a) => a.id)] }));
function release(ids: string[]) {
  ids.forEach((id) => taken.delete(id));
  useMoving.setState((s) => ({ ids: s.ids.filter((id) => !ids.includes(id)) }));
}

/** Move these accounts here: other computer closes them, then they launch here into their own
 * saved destination (Roblox's home screen without one), never the Play bar. */
export async function runHere(accounts: AccountView[]): Promise<void> {
  const ids = claim(accounts);
  if (!ids.length) return;
  try {
    const where = await fresh();
    if (!where) return;
    const away = accounts.filter(
      (a) => ids.includes(a.id) && a.userId !== null && where.has(a.userId),
    );
    // a dead session fails only here, after the other computer let go: they stay where they run
    const go = away.filter((a) => a.session !== "expired" && a.session !== "pending");
    if (go.length < away.length) toast(t("elsewhere.signIn", { n: away.length - go.length }));
    if (!go.length) return;
    // before the other computer closes anything: a refused launch would leave them running nowhere
    const why = launchBlocked(queryClient) ?? (launchBusy() ? t("launch.busy") : null);
    if (why) return void toast(why);
    mark(go);
    const released = await call("remote_run_here", { userIds: go.map((a) => a.userId!) });
    const free = go.filter((a) => released.includes(a.userId!));
    // not play(): Rust queues it behind any batch that started meanwhile instead of refusing
    const started =
      free.length > 0 &&
      (await call("launch_start", {
        accountIds: free.map((a) => a.id),
        bar: { destination: "", server: "", mode: "any", home: true },
        useBarForAll: false,
      }).then(
        () => true,
        (e) => void toast(t("elsewhere.notStarted", { n: free.length, why: errMessage(e) })),
      ));
    if (started) {
      const name = namesHidden(queryClient) ? t("launch.anAccount").toLowerCase() : label(free[0]);
      toast(
        free.length === 1
          ? t("elsewhere.movedOne", { name })
          : t("elsewhere.moved", { n: free.length }),
      );
    }
    if (free.length < go.length) toast(t("elsewhere.stayed", { n: go.length - free.length }));
  } catch (e) {
    toast(errMessage(e));
  } finally {
    release(ids);
    void queryClient.invalidateQueries({ queryKey: KEY });
  }
}

const NO_TARGETS: RemoteTarget[] = [];

/** Paired computers that can take accounts, refetched every 5 s (query key ["targets"]). */
export function useTargets(): RemoteTarget[] {
  const { data } = useQuery({
    queryKey: ["targets"],
    queryFn: () => call("remote_targets"),
    refetchInterval: 5000,
    meta: { silent: true },
  });
  return data ?? NO_TARGETS;
}

/** Send these accounts to that computer. Marks them in `useMoving` meanwhile. */
export async function sendTo(accounts: AccountView[], target: RemoteTarget): Promise<void> {
  const ids = claim(accounts);
  if (!ids.length) return;
  try {
    const where = await fresh();
    if (!where) return;
    const go = accounts.filter(
      (a) => ids.includes(a.id) && a.userId !== null && where.get(a.userId)?.ip !== target.ip,
    );
    if (!go.length) return;
    // this computer's batch could still start them here after they were closed: two places
    if (launchBusy()) return void toast(t("launch.busy"));
    mark(go);
    // launches on the target; nothing starts here
    const { started, still, old } = await call("remote_send", {
      userIds: go.map((a) => a.userId!),
      ip: target.ip,
    });
    const ok = go.filter((a) => started.includes(a.userId!));
    const device = target.device;
    if (ok.length) {
      const name = namesHidden(queryClient) ? t("launch.anAccount").toLowerCase() : label(ok[0]);
      toast(
        ok.length === 1
          ? t("elsewhere.sentOne", { name, device })
          : t("elsewhere.sent", { n: ok.length, device }),
      );
    }
    if (ok.length < go.length) toast(t("elsewhere.stayedHere", { n: go.length - ok.length }));
    if (still.length) toast(t("elsewhere.notClosed", { n: still.length }));
    // a computer with an older RMI runs some of them: it has to update before they can move
    if (old) toast(t("elsewhere.updateFirst", { device: old }));
  } catch (e) {
    toast(errMessage(e));
  } finally {
    release(ids);
    void queryClient.invalidateQueries({ queryKey: KEY });
    void queryClient.invalidateQueries({ queryKey: ACCOUNTS });
  }
}
