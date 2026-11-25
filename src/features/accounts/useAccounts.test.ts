import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { useUi } from "@/app/store";
import { useToasts } from "@/components/Toasts";
import { usePlayBar } from "@/features/playbar/store";
import { t } from "@/lib/i18n";
import * as ipc from "@/lib/ipc";
import { mockBus, type AccountsList } from "@/lib/ipc";
import { resetAccountsMock } from "@/lib/mock";
import { afterAdd } from "./AddSheet";
import { useBarTarget } from "./AccountMenu";
import {
  KEY,
  movedOrder,
  namesHidden,
  saving,
  useAccountActions,
  useAccountEvents,
  useAccounts,
} from "./useAccounts";

test("movedOrder appends accounts entering a group, in ids order, like Rust", () => {
  const list = [
    { id: "a", group: "G", order: 0 },
    { id: "b", group: "G", order: 4 },
    { id: "c", group: "U", order: 0 },
    { id: "d", group: "U", order: 1 },
  ];
  const out = movedOrder(list, ["d", "c", "a"], "G");
  expect(out.map((x) => [x.id, x.order])).toEqual([
    ["a", 0], // already in G: unchanged
    ["b", 4],
    ["c", 6],
    ["d", 5],
  ]);
  expect(movedOrder(list, ["c"], undefined)).toBe(list);
});

test("afterAdd keeps rejected lines renumbered and blanks done ones", () => {
  const r = afterAdd("ok1\nbad\n\nok2\nbad2", [
    { line: 1, status: "added", username: "x", message: null },
    { line: 2, status: "invalid", username: null, message: "no" },
    { line: 4, status: "updated", username: "y", message: null },
    { line: 5, status: "invalid", username: null, message: "no" },
  ]);
  expect(r.text).toBe("bad\nbad2");
  expect(r.results.map((x) => x.line)).toEqual([0, 1, 0, 2]);
});

test("names are hidden while locked or with privacy mode on", () => {
  const qc = new QueryClient();
  expect(namesHidden(qc)).toBe(true);
  qc.setQueryData(["vault"], { unlocked: true });
  expect(namesHidden(qc)).toBe(false);
  qc.setQueryData(["settings"], { ui: { showUsernames: false } });
  expect(namesHidden(qc)).toBe(true);
  qc.setQueryData(["settings"], { ui: { showUsernames: true } });
  expect(namesHidden(qc)).toBe(false);
});

const wrap = (qc: QueryClient) =>
  function Wrapper({ children }: { children: ReactNode }) {
    return createElement(QueryClientProvider, { client: qc }, children);
  };

beforeEach(() => {
  useToasts.setState({ items: [] });
});
afterEach(() => vi.restoreAllMocks());

test("updateMany saves in one call and returns true; a refused save reverts and resyncs", async () => {
  resetAccountsMock(3);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const { result } = renderHook(() => ({ list: useAccounts(), act: useAccountActions() }), {
    wrapper: wrap(qc),
  });
  await waitFor(() => expect(result.current.list.data).toBeDefined());
  expect(await result.current.act.updateMany(["acc-0"], { alias: "Bee" })).toBe(true);
  expect(qc.getQueryData<AccountsList>(KEY)!.accounts[0].alias).toBe("Bee");
  vi.spyOn(ipc, "call").mockRejectedValueOnce(new Error("accounts_update: Save failed"));
  expect(await result.current.act.updateMany(["acc-0"], { alias: "Oops" })).toBe(false);
  expect(qc.getQueryData<AccountsList>(KEY)!.accounts[0].alias).toBe("Bee");
});

test("accounts://changed waits for an in-flight save, then refetches once", async () => {
  const qc = new QueryClient();
  const spy = vi.spyOn(qc, "invalidateQueries");
  renderHook(() => useAccountEvents(), { wrapper: wrap(qc) });
  let finish: () => void = () => {};
  const save = saving(
    qc,
    new Promise<void>((resolve) => {
      finish = () => resolve();
    }),
  );
  act(() => void mockBus.dispatchEvent(new CustomEvent("accounts://changed", { detail: null })));
  expect(spy).not.toHaveBeenCalled();
  finish();
  await save;
  expect(spy).toHaveBeenCalledTimes(1);
  expect(spy).toHaveBeenCalledWith({ queryKey: KEY });
});

test("remove drops the rows at once; undo brings them back", async () => {
  resetAccountsMock(3);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const { result } = renderHook(() => ({ list: useAccounts(), act: useAccountActions() }), {
    wrapper: wrap(qc),
  });
  await waitFor(() => expect(result.current.list.data).toBeDefined());
  await act(() => result.current.act.remove(["acc-1"]));
  expect(qc.getQueryData<AccountsList>(KEY)!.accounts.map((a) => a.id)).toEqual(["acc-0", "acc-2"]);
  const undo = useToasts.getState().items.at(-1)?.action;
  expect(undo?.label).toBe(t("accounts.undo"));
  act(() => undo!.run());
  await waitFor(() =>
    expect(
      qc
        .getQueryData<AccountsList>(KEY)!
        .accounts.map((a) => a.id)
        .sort(),
    ).toEqual(["acc-0", "acc-1", "acc-2"]),
  );
});

test("setGroupTarget: a refused target reverts to the saved ones; null clears", async () => {
  resetAccountsMock(3);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const { result } = renderHook(() => ({ list: useAccounts(), act: useAccountActions() }), {
    wrapper: wrap(qc),
  });
  await waitFor(() => expect(result.current.list.data).toBeDefined());
  const target = { placeId: 5, jobId: null, privateLink: null };
  expect(await result.current.act.setGroupTarget("Main", target)).toBe(true);
  expect(qc.getQueryData<AccountsList>(KEY)!.groupTargets).toEqual({ Main: target });
  expect(await result.current.act.setGroupTarget("Nowhere", target)).toBe(false);
  expect(qc.getQueryData<AccountsList>(KEY)!.groupTargets).toEqual({ Main: target });
  expect(await result.current.act.setGroupTarget("Main", null)).toBe(true);
  expect(qc.getQueryData<AccountsList>(KEY)!.groupTargets).toEqual({});
});

test("a refused rename or reorder puts the list, collapse and group selection back", async () => {
  resetAccountsMock(6); // two accounts per group
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const { result } = renderHook(() => ({ list: useAccounts(), act: useAccountActions() }), {
    wrapper: wrap(qc),
  });
  await waitFor(() => expect(result.current.list.data).toBeDefined());
  const before = qc.getQueryData<AccountsList>(KEY)!;
  useUi.setState({ collapsed: new Set(["Main"]), selGroup: "Main" });
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "group_rename" || cmd === "accounts_reorder"
      ? Promise.reject(new Error(`${cmd}: Save failed`))
      : orig(cmd, args as never),
  );
  await act(() => result.current.act.renameGroup("Main", "Alts"));
  expect(useUi.getState().collapsed).toEqual(new Set(["Main"]));
  expect(useUi.getState().selGroup).toBe("Main");
  await waitFor(() => expect(qc.getQueryData<AccountsList>(KEY)!.groups).toEqual(before.groups));
  const order = before.accounts.map((a) => [a.id, a.order]);
  const main = before.accounts.filter((a) => a.group === "Main").map((a) => a.id);
  await act(() => result.current.act.reorder("Main", main.reverse()));
  await waitFor(() =>
    expect(qc.getQueryData<AccountsList>(KEY)!.accounts.map((a) => [a.id, a.order])).toEqual(order),
  );
  useUi.setState({ collapsed: new Set(), selGroup: null });
});

test("sign-in results: a lock-out is silent; the name shows only while names are visible", () => {
  const qc = new QueryClient();
  qc.setQueryData(["vault"], { unlocked: true });
  renderHook(() => useAccountEvents(), { wrapper: wrap(qc) });
  const emitSignin = (detail: unknown) =>
    act(() => void mockBus.dispatchEvent(new CustomEvent("signin://result", { detail })));
  emitSignin({ ok: false, username: null, error: "VAULT_WAS_LOCKED" });
  expect(useToasts.getState().items).toEqual([]);
  emitSignin({ ok: true, username: "bob", error: null });
  expect(useToasts.getState().items.at(-1)?.message).toBe(t("signin.ok", { name: "bob" }));
  qc.setQueryData(["vault"], { unlocked: false });
  emitSignin({ ok: true, username: "bob", error: null });
  expect(useToasts.getState().items.at(-1)?.message).toBe(t("signin.okAnon"));
});

test("rate-limit toast: at most one per 10 s", () => {
  const qc = new QueryClient();
  renderHook(() => useAccountEvents(), { wrapper: wrap(qc) });
  const clock = vi.spyOn(Date, "now").mockReturnValue(1_000_000);
  const hit = () =>
    act(
      () => void mockBus.dispatchEvent(new CustomEvent("roblox://ratelimited", { detail: null })),
    );
  hit();
  clock.mockReturnValue(1_005_000);
  hit();
  expect(useToasts.getState().items).toHaveLength(1);
  clock.mockReturnValue(1_010_000);
  hit();
  expect(useToasts.getState().items).toHaveLength(2);
});

test("saving a bar destination that is not a place or link says why and yields no target", () => {
  usePlayBar.setState({ destination: "@builderman", server: "", mode: "any" });
  const { result } = renderHook(() => useBarTarget());
  expect(result.current.empty).toBe(false);
  expect(result.current.get()).toBeNull();
  expect(useToasts.getState().items.at(-1)?.message).toBe(t("menu.saveDestBad"));
  usePlayBar.setState({ destination: "", server: "", mode: "any" });
});
