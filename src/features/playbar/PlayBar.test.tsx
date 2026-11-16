import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MotionConfig } from "motion/react";
import { useUi } from "@/app/store";
import { Toaster, useToasts } from "@/components/Toasts";
import { emptySel, label } from "@/features/accounts/model";
import * as ipc from "@/lib/ipc";
import {
  type AccountsList,
  FAVORITES_CHANGED,
  LAUNCH_DONE,
  LAUNCH_PROGRESS,
  mockBus,
  RECENTS_CHANGED,
  TRAY_PLAY,
} from "@/lib/ipc";
import { mockVault, resetAccountsMock } from "@/lib/mock";
import { t } from "@/lib/i18n";
import { PlayBar } from "./PlayBar";
import { lastLaunchSet, saveLaunchSet, usePlayBar } from "./store";
import { useLaunch, useLaunchEvents, usePlay } from "./usePlay";

beforeEach(() => {
  resetAccountsMock(6);
  mockVault.lastArgs = {};
  mockVault.launchLive = [];
  useUi.setState({ sel: emptySel });
  usePlayBar.setState({ destination: "", server: "", mode: "any" });
  useLaunch.setState({ batch: null, states: {}, dones: {}, focus: false });
  useToasts.setState({ items: [] });
});
afterEach(() => vi.restoreAllMocks());

/** App mounts the launch listeners; tests mount them beside the Play bar. */
function LaunchEvents() {
  useLaunchEvents();
  return null;
}

function setup(reduced = false) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(["vault"], { unlocked: true });
  render(
    <QueryClientProvider client={qc}>
      <MotionConfig reducedMotion={reduced ? "always" : "never"}>
        <Tooltip.Provider>
          <PlayBar />
          <LaunchEvents />
          <Toaster />
        </Tooltip.Provider>
      </MotionConfig>
    </QueryClientProvider>,
  );
  return qc;
}
const select = (...ids: string[]) =>
  useUi.setState({ sel: { ids, anchor: ids[0] ?? null, cursor: ids[0] ?? null } });
const playBtn = () => screen.getByRole("button", { name: /^(Play|Open Roblox)(?! once)/ });
const ready = () => waitFor(() => expect(playBtn()).toHaveAttribute("aria-disabled", "false"));
const emit = (name: string, detail: unknown) =>
  act(() => void mockBus.dispatchEvent(new CustomEvent(name, { detail })));
const progress = (batchId: string, accountId: string, state: string) =>
  emit(LAUNCH_PROGRESS, { batchId, accountId, state, message: null, gameName: null });
const running = (id: string | null, ids = ["acc-1", "acc-2"]) =>
  act(() => useLaunch.setState({ batch: { id, ids, done: null } }));

test("Grid Arrange opens the arrange popover (no instant arrange)", async () => {
  setup();
  await userEvent.click(await screen.findByRole("button", { name: "Grid Arrange" }));
  expect(await screen.findByRole("dialog")).toBeInTheDocument();
  expect(mockVault.lastArgs.grid_arrange).toBeUndefined();
});

test("tray://play: an account plays with the bar; a recent uses the selection, then the last set", async () => {
  setup();
  await screen.findByText(t("playbar.none"));
  act(() => usePlayBar.getState().setDestination("111"));
  emit(TRAY_PLAY, { accountId: "acc-3" });
  await waitFor(() =>
    expect(mockVault.lastArgs.launch_start).toMatchObject({
      accountIds: ["acc-3"],
      bar: { destination: "111" },
      useBarForAll: true, // a bar destination wins over saved targets
    }),
  );
  act(() => useLaunch.setState({ batch: null }));
  select("acc-1");
  emit(TRAY_PLAY, { placeId: 222 });
  await waitFor(() =>
    expect(mockVault.lastArgs.launch_start).toMatchObject({
      accountIds: ["acc-1"],
      bar: { destination: "222" },
      useBarForAll: true,
    }),
  );
  act(() => useLaunch.setState({ batch: null }));
  select();
  saveLaunchSet(["gone", "acc-2"]);
  emit(TRAY_PLAY, { placeId: 333 });
  await waitFor(() =>
    expect(mockVault.lastArgs.launch_start).toMatchObject({ accountIds: ["acc-2"] }),
  );
});

test("tray://play recent with no live accounts: toast, no launch", async () => {
  setup();
  await screen.findByText(t("playbar.none"));
  saveLaunchSet(["deleted-id"]);
  emit(TRAY_PLAY, { placeId: 444 });
  expect(await screen.findByText(t("play.noneSelected"), { selector: "p" })).toBeInTheDocument();
  expect(mockVault.lastArgs.launch_start).toBeUndefined();
});

test("Play label and disabled states, reason linked to the button", async () => {
  setup();
  expect(playBtn()).toHaveTextContent(t("playbar.play"));
  expect(playBtn()).toHaveAttribute("aria-disabled", "true");
  expect(playBtn()).toHaveAccessibleDescription(t("play.noneSelected"));
  select("acc-1", "acc-2", "acc-4");
  // no game: Play opens Roblox's home screen
  await ready();
  expect(playBtn()).toHaveTextContent(t("playbar.homeN", { n: 3 }));
  act(() => usePlayBar.getState().setDestination("6516141723"));
  expect(playBtn()).toHaveTextContent(t("playbar.playN", { n: 3 }));
  await ready();
  act(() => usePlayBar.getState().setMode("specific"));
  expect(playBtn()).toHaveAccessibleDescription(t("play.noServer"));
  // a destination that already names a server doesn't need the server field
  act(() =>
    usePlayBar.setState({
      destination: "https://www.roblox.com/games/1/x?privateServerLinkCode=LC",
    }),
  );
  await ready();
});

test("Play is blocked only when no selected account is signed in", async () => {
  const qc = setup();
  await screen.findByText(t("playbar.none"));
  await waitFor(() => expect(qc.getQueryData(["accounts"])).toBeDefined());
  act(() =>
    qc.setQueryData(["accounts"], (l: { accounts: { id: string }[] }) => ({
      ...l,
      accounts: l.accounts.map((a) => (a.id === "acc-1" ? { ...a, session: "pending" } : a)),
    })),
  );
  select("acc-1");
  await waitFor(() => expect(playBtn()).toHaveAccessibleDescription(t("session.pendingLong")));
  select("acc-1", "acc-2");
  await ready();
});

test("a 'Roblox not installed' block refetches status on click, so installing Roblox unblocks Play", async () => {
  const qc = setup();
  select("acc-1");
  await ready();
  await waitFor(() => expect(qc.getQueryData(["roblox"])).toBeDefined());
  const status = qc.getQueryData<ipc.RobloxStatus>(["roblox"])!;
  act(() => qc.setQueryData(["roblox"], { ...status, installed: false }));
  await waitFor(() => expect(playBtn()).toHaveAccessibleDescription(t("play.noRoblox")));
  await userEvent.click(playBtn());
  await ready();
  expect(mockVault.lastArgs.launch_start).toBeUndefined();
});

test("play from any entry point (row, menu, inspector) honours a launch block", async () => {
  const { queryClient } = await import("@/lib/queryClient");
  setup();
  queryClient.setQueryData(["caps"], { launch: false, launchReason: "Install Sober first." });
  try {
    act(() => usePlay().play(["acc-1"]));
    expect(await screen.findByText("Install Sober first.")).toBeInTheDocument();
    expect(useLaunch.getState().batch).toBeNull();
    expect(mockVault.lastArgs.launch_start).toBeUndefined();
  } finally {
    queryClient.clear();
  }
});

test("count shows the selection; a destination sent from Discover shows and clears", async () => {
  const user = userEvent.setup();
  setup();
  expect(await screen.findByText(t("playbar.none"))).toBeInTheDocument();
  select("acc-1", "acc-2");
  expect(await screen.findByText(t("playbar.selected", { n: 2 }))).toBeInTheDocument();
  act(() => usePlayBar.getState().setDestination("6516141723"));
  expect(await screen.findByText(t("playbar.dest", { dest: "6516141723" }))).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("playbar.destClear") }));
  expect(usePlayBar.getState().destination).toBe("");
});

test("no game anywhere opens Roblox home; a saved group destination makes it Play", async () => {
  const user = userEvent.setup();
  const qc = setup();
  select("acc-1");
  await waitFor(() => expect(playBtn()).toHaveTextContent(t("playbar.home")));
  await ready();
  const list = (await waitFor(() => {
    const l = qc.getQueryData<{ accounts: { id: string; group: string }[] }>(["accounts"]);
    if (!l) throw new Error("not loaded");
    return l;
  })) as { accounts: { id: string; group: string }[]; groupTargets?: Record<string, unknown> };
  const group = list.accounts.find((a) => a.id === "acc-1")!.group;
  act(() =>
    qc.setQueryData(["accounts"], {
      ...list,
      groupTargets: { [group]: { placeId: 1, jobId: null, privateLink: null } },
    }),
  );
  await waitFor(() => expect(playBtn()).toHaveTextContent(t("playbar.play")));
  await user.click(playBtn());
  await waitFor(() =>
    expect(mockVault.lastArgs.launch_start).toMatchObject({ accountIds: ["acc-1"] }),
  );
});

test("a bar destination (Discover) plays every selected account there; cleared, saved ones apply", async () => {
  const user = userEvent.setup();
  setup();
  select("acc-1", "acc-2");
  act(() => usePlayBar.getState().setDestination("6516141723"));
  // visible label (not a tooltip), clearable
  expect(await screen.findByText(t("playbar.dest", { dest: "6516141723" }))).toBeVisible();
  await ready();
  await user.click(playBtn());
  expect(mockVault.lastArgs.launch_start).toMatchObject({
    accountIds: ["acc-1", "acc-2"],
    bar: { destination: "6516141723", server: "", mode: "any" },
    useBarForAll: true,
  });
  act(() => useLaunch.setState({ batch: null }));
  await user.click(screen.getByRole("button", { name: t("playbar.destClear") }));
  act(() => usePlay().play(["acc-1"]));
  await waitFor(() =>
    expect(mockVault.lastArgs.launch_start).toMatchObject({
      bar: { destination: "", home: true },
      useBarForAll: false,
    }),
  );
});

test("Play once: chevron popover plays all selected at the typed destination and server", async () => {
  const user = userEvent.setup();
  setup();
  select("acc-1", "acc-2");
  await user.click(await screen.findByRole("button", { name: t("playbar.once") }));
  const pop = await screen.findByRole("dialog", { name: t("playbar.once") });
  const field = within(pop).getByRole("combobox", { name: t("playbar.once") });
  expect(field).toHaveFocus();
  await user.type(field, "6516141723");
  await user.click(within(pop).getByRole("radio", { name: t("mode.specific") }));
  // Specific without a JobId: says why, no launch
  await user.click(within(pop).getByRole("button", { name: t("playbar.play") }));
  expect(mockVault.lastArgs.launch_start).toBeUndefined();
  const job = "11111111-2222-3333-4444-555555555555";
  await user.type(within(pop).getByRole("textbox", { name: t("server.label") }), job);
  await user.click(within(pop).getByRole("button", { name: t("playbar.play") }));
  await waitFor(() =>
    expect(mockVault.lastArgs.launch_start).toMatchObject({
      accountIds: ["acc-1", "acc-2"],
      bar: { destination: "6516141723", server: job, mode: "specific" },
      useBarForAll: true,
    }),
  );
  expect(screen.queryByRole("dialog", { name: t("playbar.once") })).toBeNull();
  expect(usePlayBar.getState()).toMatchObject({ destination: "6516141723", mode: "specific" });
});

test("Play once: a server with no destination is blocked, not dropped to the home screen", async () => {
  const user = userEvent.setup();
  setup();
  select("acc-1");
  await user.click(await screen.findByRole("button", { name: t("playbar.once") }));
  const pop = await screen.findByRole("dialog", { name: t("playbar.once") });
  await user.click(within(pop).getByRole("radio", { name: t("mode.specific") }));
  await user.type(
    within(pop).getByRole("textbox", { name: t("server.label") }),
    "https://www.roblox.com/share?code=abc&type=Server",
  );
  await user.click(within(pop).getByRole("button", { name: t("playbar.play") }));
  expect(await screen.findByText(t("dest.invalid"), { selector: "p" })).toBeInTheDocument();
  expect(mockVault.lastArgs.launch_start).toBeUndefined();
});

test("Play once: Escape and outside clicks close it", async () => {
  const user = userEvent.setup();
  setup();
  const chevron = await screen.findByRole("button", { name: t("playbar.once") });
  await user.click(chevron);
  await screen.findByRole("dialog", { name: t("playbar.once") });
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog", { name: t("playbar.once") })).toBeNull();
  expect(chevron).toHaveFocus();
  await user.click(chevron);
  await user.click(document.body);
  expect(screen.queryByRole("dialog", { name: t("playbar.once") })).toBeNull();
});

test("capsule: counts, Cancel calls launch_cancel, focus moves and returns", async () => {
  const user = userEvent.setup();
  setup();
  select("acc-1", "acc-2");
  act(() => usePlayBar.getState().setDestination("6516141723"));
  await ready();
  await user.click(playBtn());
  expect(await screen.findByText("Launching 1 of 2")).toBeInTheDocument();
  const cancel = await screen.findByRole("button", { name: t("launch.cancel") });
  expect(cancel).toHaveFocus();
  await waitFor(() => expect(cancel).toHaveAttribute("aria-disabled", "false"));
  await user.click(cancel);
  expect(mockVault.lastArgs.launch_cancel).toEqual({
    batchId: expect.stringMatching(/^mock-batch/),
  });
  await waitFor(() => expect(playBtn()).toHaveFocus(), { timeout: 4000 });
});

test("a second play while one runs is refused", async () => {
  setup();
  running("b1");
  act(() => usePlay().play(["acc-3"]));
  expect(await screen.findByText(t("launch.busy"))).toBeInTheDocument();
  expect(mockVault.lastArgs.launch_start).toBeUndefined();
});

test("events from another batch are ignored; events before the id are kept", async () => {
  setup();
  running(null);
  progress("b1", "acc-1", "validating");
  progress("b1", "acc-2", "validating");
  progress("b2", "acc-1", "launched");
  running("b1");
  expect(await screen.findByText("Launching 2 of 2")).toBeInTheDocument();
  emit(LAUNCH_DONE, { batchId: "b2", launched: 1, failed: 0, cancelled: 0 });
  expect(screen.getByText("Launching 2 of 2")).toBeInTheDocument();
  emit(LAUNCH_DONE, { batchId: "b1", launched: 0, failed: 2, cancelled: 0 });
  expect(await screen.findByText(t("launch.summary", { ok: 0, bad: 2 }))).toBeInTheDocument();
});

test("avatars fly from their row through a portal and land in the capsule", async () => {
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    const inRow = !!this.closest("#row-acc-1");
    return DOMRect.fromRect({ x: inRow ? 400 : 10, y: inRow ? 300 : 10, width: 24, height: 24 });
  });
  const row = document.body.appendChild(document.createElement("div"));
  row.id = "row-acc-1";
  row.innerHTML = "<span data-avatar></span>";
  setup();
  await screen.findByText(t("playbar.none"));
  running("b1");
  progress("b1", "acc-1", "validating");
  const flyer = await screen.findByTestId("flyer");
  expect(flyer.parentElement).toBe(document.body);
  await waitFor(() => expect(screen.queryByTestId("flyer")).toBeNull());
  expect(screen.getByTestId("slot")).not.toHaveClass("opacity-0");
  row.remove();
});

test("reduced motion: static avatars and ticks, no flight", async () => {
  setup(true);
  await screen.findByText(t("playbar.none"));
  running("b1");
  progress("b1", "acc-1", "launched");
  await waitFor(() => expect(screen.getByTestId("tick")).toBeInTheDocument());
  expect(screen.getByTestId("slot")).not.toHaveClass("opacity-0");
  expect(screen.queryByTestId("flyer")).toBeNull();
});

test("recents://changed invalidates recents", () => {
  const qc = setup();
  qc.setQueryData(["recents"], []);
  emit(RECENTS_CHANGED, null);
  expect(qc.getQueryState(["recents"])?.isInvalidated).toBe(true);
});

test("favorites://changed invalidates favorites (import adds some)", () => {
  const qc = setup();
  qc.setQueryData(["favorites"], []);
  emit(FAVORITES_CHANGED, null);
  expect(qc.getQueryState(["favorites"])?.isInvalidated).toBe(true);
});

test("a refused play changes nothing: busy, or tray while locked", async () => {
  const qc = setup();
  await screen.findByText(t("playbar.none"));
  saveLaunchSet(["acc-1"]);
  act(() => usePlayBar.getState().setDestination("111"));
  running("b-busy");
  act(() => usePlay().play(["acc-3"], { here: true, dest: "999" }));
  expect(await screen.findByText(t("launch.busy"))).toBeInTheDocument();
  expect(usePlayBar.getState().destination).toBe("111");
  expect(lastLaunchSet()).toEqual(["acc-1"]);
  act(() => useLaunch.setState({ batch: null }));
  qc.setQueryData(["vault"], { unlocked: false });
  select("acc-2");
  emit(TRAY_PLAY, { placeId: 222 });
  emit(TRAY_PLAY, { accountId: "acc-2" });
  expect(usePlayBar.getState().destination).toBe("111");
  expect(mockVault.lastArgs.launch_start).toBeUndefined();
});

test("a batch the UI didn't start shows, settles and is cleaned up", async () => {
  setup();
  await screen.findByText(t("playbar.none"));
  progress("relaunch-1", "acc-1", "queued");
  expect(useLaunch.getState().batch).toMatchObject({ id: "relaunch-1", ids: ["acc-1"] });
  expect(await screen.findByRole("button", { name: t("launch.cancel") })).toBeInTheDocument();
  emit(LAUNCH_DONE, { batchId: "relaunch-1", launched: 1, failed: 0, cancelled: 0 });
  await waitFor(() => expect(useLaunch.getState().batch).toBeNull(), { timeout: 3000 });
  expect(useLaunch.getState().states).toEqual({});
  expect(useLaunch.getState().dones).toEqual({});
});

test("Cancel while starting cancels once the id arrives", async () => {
  setup();
  await screen.findByText(t("playbar.none"));
  act(() => {
    usePlay().play(["acc-1"], { dest: "5" });
    usePlay().cancel();
  });
  await waitFor(() =>
    expect(mockVault.lastArgs.launch_cancel).toEqual({
      batchId: expect.stringMatching(/^mock-batch/),
    }),
  );
});

test("on mount, live batches from launch_status come back on the Play button", async () => {
  mockVault.launchLive = [
    {
      launchId: "live-1",
      accounts: { "acc-1": { state: "launching" }, "acc-2": { state: "queued" } },
    },
  ];
  setup();
  expect(await screen.findByText("Launching 1 of 2")).toBeInTheDocument();
});

test("launch results toast the account name, or 'An account' while names are hidden", async () => {
  const qc = setup();
  await screen.findByText(t("playbar.none"));
  await waitFor(() => expect(qc.getQueryData(["accounts"])).toBeDefined());
  const name = label(
    (qc.getQueryData(["accounts"]) as AccountsList).accounts.find((a) => a.id === "acc-1")!,
  );
  emit(LAUNCH_PROGRESS, {
    batchId: "b1",
    accountId: "acc-1",
    state: "launched",
    message: null,
    gameName: "DOORS",
  });
  expect(await screen.findByText(t("launch.ok", { name, game: "DOORS" }))).toBeInTheDocument();
  emit(LAUNCH_PROGRESS, {
    batchId: "b1",
    accountId: "acc-1",
    state: "failed",
    message: "Roblox closed",
    gameName: null,
  });
  expect(
    await screen.findByText(t("launch.failed", { name, why: "Roblox closed" })),
  ).toBeInTheDocument();
  qc.setQueryData(["settings"], { ui: { showUsernames: false } });
  emit(LAUNCH_PROGRESS, {
    batchId: "b2",
    accountId: "acc-2",
    state: "launched",
    message: null,
    gameName: null,
  });
  expect(
    await screen.findByText(t("launch.okNoGame", { name: t("launch.anAccount") })),
  ).toBeInTheDocument();
});

test("a finished batch refreshes recents; accounts refetch only via accounts://changed", async () => {
  const qc = setup();
  await screen.findByText(t("playbar.none"));
  const spy = vi.spyOn(qc, "invalidateQueries");
  emit(LAUNCH_DONE, { batchId: "b1", launched: 1, failed: 0, cancelled: 0 });
  expect(spy).toHaveBeenCalledWith({ queryKey: ["recents"] });
  expect(spy).not.toHaveBeenCalledWith({ queryKey: ["accounts"] }); // would skip the save guard
});

test("a refused launch_start clears the batch and says why", async () => {
  setup();
  await screen.findByText(t("playbar.none"));
  vi.spyOn(ipc, "call").mockRejectedValueOnce(new Error("launch_start: LAUNCH_BUSY"));
  act(() => usePlay().play(["acc-1"]));
  expect(await screen.findByText(t("launch.already"))).toBeInTheDocument();
  expect(useLaunch.getState().batch).toBeNull();
});

test("a failed launch_cancel toasts the error", async () => {
  setup();
  await screen.findByText(t("playbar.none"));
  running("b1");
  vi.spyOn(ipc, "call").mockRejectedValueOnce(new Error("launch_cancel: Batch not found."));
  act(() => usePlay().cancel());
  expect(await screen.findByText("Batch not found.")).toBeInTheDocument();
});
