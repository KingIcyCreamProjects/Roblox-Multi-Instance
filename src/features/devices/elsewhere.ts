import { useQuery } from "@tanstack/react-query";
import { create } from "zustand";
import { toast } from "@/components/Toasts";
import { label } from "@/features/accounts/model";
import { namesHidden } from "@/features/accounts/useAccounts";
import { launchBlocked, launchBusy, play } from "@/features/playbar/usePlay";
import { call, errMessage, type AccountView, type Elsewhere } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { queryClient } from "@/lib/queryClient";

const KEY = ["elsewhere"];
const fetchList = () => call("remote_elsewhere");
const toMap = (list: Elsewhere[]) => new Map(list.map((e) => [e.userId, e]));
const NONE = new Map<number, Elsewhere>();

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

/** Move these accounts here: other computer closes them, then normal play() launches them. */
export async function runHere(accounts: AccountView[]): Promise<void> {
  const where = toMap(await queryClient.ensureQueryData({ queryKey: KEY, queryFn: fetchList }));
  const moving = useMoving.getState().ids;
  const go = accounts.filter(
    (a) => a.userId !== null && where.has(a.userId) && !moving.includes(a.id),
  );
  if (!go.length) return;
  // before the other computer closes anything: a refused launch would leave them running nowhere
  const why = launchBlocked(queryClient) ?? (launchBusy() ? t("launch.busy") : null);
  if (why) return void toast(why);
  const ids = go.map((a) => a.id);
  useMoving.setState((s) => ({ ids: [...s.ids, ...ids] }));
  try {
    const released = await call("remote_run_here", { userIds: go.map((a) => a.userId!) });
    const free = go.filter((a) => released.includes(a.userId!));
    if (free.length) {
      // ponytail: a batch started elsewhere during the wait makes play() refuse with launch.busy;
      // queue the launch if that ever happens in practice
      play(free.map((a) => a.id));
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
    useMoving.setState((s) => ({ ids: s.ids.filter((id) => !ids.includes(id)) }));
    void queryClient.invalidateQueries({ queryKey: KEY });
  }
}
