import { waitFor } from "@testing-library/react";
import { useToasts } from "@/components/Toasts";
import { KEY } from "@/features/accounts/useAccounts";
import * as ipc from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { queryClient } from "@/lib/queryClient";
import { lastLaunchSet, saveLaunchSet, usePlayBar } from "./store";
import { barOf, clearPlaceholder, launchBusy, play, useLaunch } from "./usePlay";

test("a start Rust refuses, or only pending accounts, leaves the bar and last set alone", async () => {
  useToasts.setState({ items: [] });
  saveLaunchSet(["old"]);
  usePlayBar.setState({ destination: "111", server: "", mode: "any" });
  const spy = vi.spyOn(ipc, "call").mockRejectedValue(new Error("launch_start: LAUNCH_BUSY"));
  play(["a"], { here: true, dest: "999", mode: "smallest" });
  await waitFor(() => expect(useLaunch.getState().batch).toBeNull());
  expect(usePlayBar.getState()).toMatchObject({ destination: "111", mode: "any" });
  expect(lastLaunchSet()).toEqual(["old"]);
  expect(spy.mock.calls[0][1]).toMatchObject({ bar: { destination: "999", mode: "smallest" } });
  spy.mockClear();
  queryClient.setQueryData(KEY, { accounts: [{ id: "p", session: "pending" }] });
  play(["p"]);
  expect(spy).not.toHaveBeenCalled();
  expect(useToasts.getState().items.at(-1)?.message).toBe(t("session.pendingLong"));
  queryClient.removeQueries({ queryKey: KEY });
  usePlayBar.setState({ destination: "" });
  vi.restoreAllMocks();
});

test("a settled (done) batch does not block the next play", () => {
  const b = { id: "b1", ids: ["a"], done: null };
  useLaunch.setState({ batch: b });
  expect(launchBusy()).toBe(true);
  useLaunch.setState({ batch: { ...b, done: { batchId: "b1" } as never } });
  expect(launchBusy()).toBe(false);
  useLaunch.setState({ batch: null });
});

test("barOf drops a leftover server when the destination is empty", () => {
  expect(barOf("", " job ", "specific")).toEqual({
    destination: "",
    server: "",
    mode: "specific",
    home: true,
  });
  expect(barOf(" 1818 ", " job ", "specific")).toEqual({
    destination: "1818",
    server: "job",
    mode: "specific",
    home: true,
  });
});

test("a placeholder replaced meanwhile (cancel) is still cleared, and a real batch is adopted", () => {
  const ids = ["a"];
  useLaunch.setState({
    batch: { id: null, ids, done: null, cancel: true },
    states: { b9: { a: "launching" } },
  });
  expect(launchBusy()).toBe(true);
  clearPlaceholder(ids);
  expect(useLaunch.getState().batch?.id).toBe("b9"); // adopted like settle does
  useLaunch.setState({ batch: null, states: {} });
});
