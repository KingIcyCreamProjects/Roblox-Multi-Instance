import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { useUi } from "@/app/store";
import { toast } from "@/components/Toasts";
import {
  call,
  codeText,
  errMessage,
  hasCode,
  onEvent,
  type AccountPatch,
  type AccountsList,
  type CopyField,
  type LaunchTarget,
  type Settings,
  type SigninResult,
  type VaultStatus,
} from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { reportError } from "@/lib/report";
import { requestReauth } from "./Reauth";

export const KEY = ["accounts"];
let lastRateToast = 0;

/** Saves in flight. While any runs, `accounts://changed` refetches wait (a refetch landing
 * mid-save would revert the optimistic list); one refetch follows the last save. */
let inflight = 0;
let missed = false;
const refetch = (qc: QueryClient) => {
  if (inflight) missed = true;
  else void qc.invalidateQueries({ queryKey: KEY });
};
export async function saving<T>(qc: QueryClient, p: Promise<T>): Promise<T> {
  inflight++;
  try {
    return await p;
  } finally {
    if (--inflight === 0 && missed) {
      missed = false;
      void qc.invalidateQueries({ queryKey: KEY });
    }
  }
}

/** Rust's group move: accounts entering `group` go to its end, in `ids` order. */
export function movedOrder<A extends { id: string; group: string; order: number }>(
  list: A[],
  ids: string[],
  group: string | undefined,
): A[] {
  if (group === undefined) return list;
  let next = Math.max(-1, ...list.filter((a) => a.group === group).map((a) => a.order)) + 1;
  const order = new Map<string, number>();
  for (const id of ids) {
    const a = list.find((x) => x.id === id);
    if (a && a.group !== group) order.set(id, next++);
  }
  return list.map((a) => (order.has(a.id) ? { ...a, order: order.get(a.id)! } : a));
}

/** User-facing error text: known codes become catalog text, the `cmd: ` prefix call() adds is dropped. */
export const errText = errMessage;

/** Toasts may name an account only while unlocked and with privacy mode off. */
export const namesHidden = (qc: QueryClient) =>
  !qc.getQueryData<VaultStatus>(["vault"])?.unlocked ||
  qc.getQueryData<Settings>(["settings"])?.ui.showUsernames === false;

export function useAccounts() {
  // silent: the screen shows its own error state with Retry, not the shell banner
  return useQuery({ queryKey: KEY, queryFn: () => call("accounts_list"), meta: { silent: true } });
}

/** Backend events: list changes refetch; sign-in results toast. Mount once, in App (survives a lock). */
export function useAccountEvents() {
  const qc = useQueryClient();
  useEffect(() => {
    const offs = [
      onEvent("accounts://changed", () => refetch(qc)),
      onEvent("roblox://ratelimited", () => {
        // one toast per 10 s, however many backoffs; Discover already shows it in its header
        if (useUi.getState().route === "discover") return;
        const now = Date.now();
        if (now - lastRateToast < 10_000) return;
        lastRateToast = now;
        toast(t("roblox.ratelimited"));
      }),
      onEvent<SigninResult>("signin://result", (r) => {
        useUi.setState({ signinWaiting: false });
        // the user locked on purpose: the closed window needs no toast
        if (!r.ok && hasCode(r.error ?? "", "VAULT_WAS_LOCKED")) return;
        toast(
          r.ok
            ? r.username && !namesHidden(qc)
              ? t("signin.ok", { name: r.username })
              : t("signin.okAnon")
            : r.error
              ? codeText(r.error)
              : t("signin.failed"),
        );
        void qc.invalidateQueries({ queryKey: KEY });
      }),
      onEvent<{ username: string }>("watcher://reconnect", ({ username }) =>
        toast(
          namesHidden(qc)
            ? t("watcher.reconnectingAnon")
            : t("watcher.reconnecting", { name: username }),
        ),
      ),
    ];
    return () => offs.forEach((f) => f());
  }, [qc]);
}

export function useAccountActions() {
  const qc = useQueryClient();
  const get = () => qc.getQueryData<AccountsList>(KEY);
  const put = (f: (l: AccountsList) => AccountsList) => {
    const l = get();
    if (!l) return;
    // a refetch already in flight would land over this write: drop it, refetch after the save
    if (qc.isFetching({ queryKey: KEY })) {
      void qc.cancelQueries({ queryKey: KEY });
      missed = true;
    }
    qc.setQueryData(KEY, f(l));
  };
  const resync = (e?: unknown) => {
    if (e !== undefined) reportError(e);
    return qc.invalidateQueries({ queryKey: KEY });
  };

  /** One atomic save for every id. Resolves true once saved; a failure is reported, resynced and resolves false. */
  const updateMany = async (ids: string[], patch: AccountPatch) => {
    put((l) => ({
      ...l,
      groups:
        patch.group && !l.groups.includes(patch.group) ? [...l.groups, patch.group] : l.groups,
      accounts: movedOrder(l.accounts, ids, patch.group).map((a) =>
        ids.includes(a.id) ? { ...a, ...patch } : a,
      ),
    }));
    try {
      await saving(qc, call("accounts_update", { ids, patch }));
      return true;
    } catch (e) {
      await resync(e);
      return false;
    }
  };

  const remove = async (ids: string[]) => {
    if (!ids.length) return;
    const ui = useUi.getState();
    ui.setSel({ ...ui.sel, ids: ui.sel.ids.filter((x) => !ids.includes(x)), anchor: null });
    put((l) => ({ ...l, accounts: l.accounts.filter((a) => !ids.includes(a.id)) }));
    try {
      const { undoToken } = await saving(qc, call("accounts_remove", { ids }));
      toast(ids.length === 1 ? t("accounts.removed1") : t("accounts.removed", { n: ids.length }), {
        label: t("accounts.undo"),
        run: () =>
          void call("accounts_undo_remove", { undoToken }).then(
            () => resync(),
            (e) => resync(e),
          ),
      });
    } catch (e) {
      await resync(e);
    }
  };

  const copy = async (id: string, field: CopyField) => {
    const attempt = () => call("account_copy", { id, field });
    const failed = (e: unknown) => void toast(t("copy.failed", { reason: errMessage(e) }));
    try {
      await attempt();
    } catch (e) {
      if (!hasCode(e, "REAUTH_REQUIRED")) return failed(e);
      if (!(await requestReauth())) return void toast(t("reauth.cancelled"));
      try {
        await attempt();
      } catch (e2) {
        return failed(e2);
      }
    }
    toast(
      field === "cookie" || field === "password"
        ? t("copy.secretDone")
        : t("copy.done", { what: t(`copy.${field}`) }),
    );
  };

  const reorder = async (group: string, ids: string[]) => {
    put((l) => ({
      ...l,
      accounts: l.accounts.map((a) =>
        ids.includes(a.id) ? { ...a, order: ids.indexOf(a.id) } : a,
      ),
    }));
    await saving(qc, call("accounts_reorder", { group, ids })).catch(resync);
  };

  const reorderGroups = async (groups: string[]) => {
    put((l) => ({ ...l, groups }));
    await saving(qc, call("groups_reorder", { groups })).catch(resync);
  };

  /** Rename a group in place: its accounts, place in the order, saved target, collapse and selection follow. */
  const renameGroup = async (from: string, to: string) => {
    const l = get();
    if (l?.groups.includes(to) || l?.accounts.some((a) => a.group === to))
      return void toast(t("group.exists"));
    const ren = (g: string) => (g === from ? to : g);
    put((l) => {
      const groupTargets = { ...l.groupTargets };
      if (groupTargets[from]) groupTargets[to] = groupTargets[from];
      delete groupTargets[from];
      return {
        ...l,
        groups: l.groups.map(ren),
        groupTargets,
        accounts: l.accounts.map((a) => (a.group === from ? { ...a, group: to } : a)),
      };
    });
    useUi.setState((u) => ({
      collapsed: new Set([...u.collapsed].map(ren)),
      selGroup: u.selGroup === from ? to : u.selGroup,
    }));
    await saving(qc, call("group_rename", { from, to })).catch(resync);
  };

  /** `null` clears. Resolves true once saved. */
  const setGroupTarget = async (group: string, target: LaunchTarget | null) => {
    put((l) => {
      const groupTargets = { ...l.groupTargets };
      if (target) groupTargets[group] = target;
      else delete groupTargets[group];
      return { ...l, groupTargets };
    });
    try {
      await saving(qc, call("group_target_set", { group, target }));
      return true;
    } catch (e) {
      await resync(e);
      return false;
    }
  };

  const validate = (ids: string[] | null) =>
    call("accounts_validate", { ids }).then(
      (started) => void toast(t(started ? "session.validating" : "session.busy")),
      reportError,
    );

  const signIn = (
    accountId: string | null,
    usernameHint: string | null,
    password: string | null = null,
  ) => {
    // Rust is the single-flight guard for every entry point; SIGNIN_BUSY means the open
    // window got focus and its result is still to come.
    useUi.setState({ signinWaiting: true });
    return call("signin_start", { accountId, usernameHint, password }).catch((e) => {
      if (hasCode(e, "SIGNIN_BUSY")) return void toast(errMessage(e));
      useUi.setState({ signinWaiting: false });
      reportError(e);
    });
  };

  return {
    update: (id: string, patch: AccountPatch) => updateMany([id], patch),
    updateMany,
    remove,
    copy,
    reorder,
    reorderGroups,
    renameGroup,
    setGroupTarget,
    validate,
    signIn,
  };
}
