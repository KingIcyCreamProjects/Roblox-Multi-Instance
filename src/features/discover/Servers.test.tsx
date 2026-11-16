import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useUi } from "@/app/store";
import { useToasts } from "@/components/Toasts";
import { usePlayBar } from "@/features/playbar/store";
import { launchBusy, useLaunch } from "@/features/playbar/usePlay";
import { t } from "@/lib/i18n";
import * as ipc from "@/lib/ipc";
import { resetDiscoverMock } from "@/lib/mockDiscover";
import { Servers } from "./Servers";
import { useDiscover } from "./shared";

// jsdom has no layout: the virtualizer renders no rows, but the grid's active row
// (aria-activedescendant) and the keyboard model still run.
Element.prototype.scrollTo ??= () => {};

const PLACE = 74260430392611;
const wrap = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tooltip.Provider>
        <Servers />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
const page = () =>
  Number(
    screen.queryByText(/servers, loading page \d+/)?.textContent?.match(/page (\d+)/)?.[1] ?? 0,
  );
const active = () => screen.getByRole("grid").getAttribute("aria-activedescendant");

beforeEach(() => {
  resetDiscoverMock(false);
  useDiscover.setState({ place: String(PLACE), refreshPending: false });
  usePlayBar.setState({ destination: "", server: "", mode: "any" });
});
afterEach(() => useUi.setState({ sel: { ids: [], anchor: null, cursor: null } }));

test("the active JobId survives new pages and a sort", async () => {
  const user = userEvent.setup();
  wrap();
  await user.click(screen.getByRole("button", { name: t("servers.refresh") }));
  await waitFor(() => expect(page()).toBeGreaterThan(0));
  const grid = screen.getByRole("grid");
  act(() => grid.focus());
  fireEvent.keyDown(grid, { key: "ArrowDown" });
  const picked = active();
  expect(picked).toBeTruthy();
  // more pages arrive
  const seen = page();
  await waitFor(() => expect(page()).toBeGreaterThan(seen + 1));
  expect(active()).toBe(picked);
  await user.click(screen.getByRole("button", { name: t("servers.col.ping") }));
  expect(active()).toBe(picked);
  // Enter sends that same JobId
  fireEvent.keyDown(grid, { key: "Enter" });
  const { server } = usePlayBar.getState();
  expect(server).toMatch(/^[0-9a-f-]{36}$/);
  expect(picked!.endsWith(server)).toBe(true);
  expect(usePlayBar.getState().mode).toBe("specific");
  await user.click(screen.getByRole("button", { name: t("servers.stop") }));
});

test("Use this server sends the place the search ran in", async () => {
  useUi.setState({ sel: { ids: ["acc-1"], anchor: "acc-1", cursor: "acc-1" } });
  const user = userEvent.setup();
  wrap();
  await user.type(screen.getByLabelText(t("servers.find.label")), "kestrel");
  await user.click(screen.getByRole("button", { name: t("servers.find.go") }));
  // the place input changes while searching
  act(() => useDiscover.setState({ place: "123" }));
  await user.click(await screen.findByRole("button", { name: t("servers.setServer") }));
  const bar = usePlayBar.getState();
  expect(bar.destination).toBe(String(PLACE));
  expect(bar.server).toMatch(/^[0-9a-f-]{36}$/);
  expect(bar.mode).toBe("specific");
});

test("a region filter typed before loading regions still lets Load regions look rows up", async () => {
  useUi.setState({ sel: { ids: ["acc-1"], anchor: "acc-1", cursor: "acc-1" } });
  await ipc.call("settings_set", { patch: { geo: { enabled: true } } });
  const regionsCall = vi.spyOn(ipc, "call");
  const user = userEvent.setup();
  wrap();
  await user.click(screen.getByRole("button", { name: t("servers.refresh") }));
  await waitFor(() => expect(page()).toBeGreaterThan(0));
  await user.click(screen.getByRole("button", { name: t("servers.stop") }));
  await user.type(screen.getByLabelText(t("servers.regionFilter")), "Frankfurt");
  const load = screen.getByRole("button", { name: t("servers.loadRegions") });
  await waitFor(() => expect(load).toBeEnabled());
  await user.click(load);
  await waitFor(() => {
    const c = regionsCall.mock.calls.find(([cmd]) => cmd === "servers_regions");
    const ids = (c?.[1] as { jobIds: string[] } | undefined)?.jobIds ?? [];
    expect(ids.length).toBeGreaterThan(0);
    expect(ids.length).toBeLessThanOrEqual(50);
  });
  regionsCall.mockRestore();
  await ipc.call("settings_set", { patch: { geo: { enabled: false } } });
});

test("setDestination resets server and mode", () => {
  usePlayBar.setState({ server: "job", mode: "specific" });
  usePlayBar.getState().setDestination("42");
  expect(usePlayBar.getState()).toMatchObject({ destination: "42", server: "", mode: "any" });
});

test("Set server, then join is busy while the reservations run", async () => {
  useUi.setState({ sel: { ids: ["acc-1"], anchor: "acc-1", cursor: "acc-1" } });
  useLaunch.setState({ batch: null });
  const orig = ipc.call;
  let finish!: (v: { batchId: string }) => void;
  const spy = vi
    .spyOn(ipc, "call")
    .mockImplementation(((cmd: string, args: never) =>
      cmd === "servers_reserve_join"
        ? new Promise((r) => (finish = r))
        : orig(cmd as never, args)) as typeof ipc.call);
  const user = userEvent.setup();
  wrap();
  await user.click(screen.getByRole("button", { name: t("servers.refresh") }));
  await waitFor(() => expect(page()).toBeGreaterThan(0));
  const grid = screen.getByRole("grid");
  act(() => grid.focus());
  fireEvent.keyDown(grid, { key: "ArrowDown" });
  fireEvent.contextMenu(grid);
  await user.click(await screen.findByRole("menuitem", { name: t("servers.reserveJoin") }));
  expect(launchBusy()).toBe(true); // Play / Join are refused until the reserve call returns
  await act(async () => finish({ batchId: "b1" }));
  expect(launchBusy()).toBe(false);
  spy.mockRestore();
  await user.click(screen.getByRole("button", { name: t("servers.stop") }));
});

test("Load regions works again right after a refresh while an older load is still running", async () => {
  useUi.setState({ sel: { ids: ["acc-1"], anchor: "acc-1", cursor: "acc-1" } });
  await ipc.call("settings_set", { patch: { geo: { enabled: true } } });
  const orig = ipc.call;
  const spy = vi
    .spyOn(ipc, "call")
    .mockImplementation(((cmd: string, args: never) =>
      cmd === "servers_regions"
        ? new Promise(() => {})
        : orig(cmd as never, args)) as typeof ipc.call);
  const user = userEvent.setup();
  wrap();
  const refresh = screen.getByRole("button", { name: t("servers.refresh") });
  await user.click(refresh);
  await waitFor(() => expect(page()).toBeGreaterThan(0));
  await user.click(screen.getByRole("button", { name: t("servers.stop") }));
  await user.type(screen.getByLabelText(t("servers.regionFilter")), "Frankfurt");
  const load = screen.getByRole("button", { name: t("servers.loadRegions") });
  await waitFor(() => expect(load).toBeEnabled());
  await user.click(load);
  await user.click(refresh);
  await waitFor(() => expect(page()).toBeGreaterThan(0));
  await user.click(screen.getByRole("button", { name: t("servers.stop") }));
  const again = screen.getByRole("button", { name: t("servers.loadRegions") });
  await waitFor(() => expect(again).toBeEnabled());
  await user.click(again);
  expect(spy.mock.calls.filter(([c]) => c === "servers_regions")).toHaveLength(2);
  spy.mockRestore();
  await ipc.call("settings_set", { patch: { geo: { enabled: false } } });
});

test("Set server, then join starts a reserved batch", async () => {
  useUi.setState({ sel: { ids: ["acc-1"], anchor: "acc-1", cursor: "acc-1" } });
  useToasts.setState({ items: [] });
  const user = userEvent.setup();
  wrap();
  await user.click(screen.getByRole("button", { name: t("servers.refresh") }));
  await waitFor(() => expect(page()).toBeGreaterThan(0));
  const grid = screen.getByRole("grid");
  act(() => grid.focus());
  fireEvent.keyDown(grid, { key: "ArrowDown" });
  fireEvent.contextMenu(grid);
  await user.click(await screen.findByRole("menuitem", { name: t("servers.reserveJoin") }));
  await waitFor(() =>
    expect(useToasts.getState().items.map((x) => x.message)).toContain(t("servers.reserveStarted")),
  );
  await user.click(screen.getByRole("button", { name: t("servers.stop") }));
});
