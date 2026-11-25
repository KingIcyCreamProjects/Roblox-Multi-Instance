import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { useUi } from "@/app/store";
import { useToasts } from "@/components/Toasts";
import { AccountsScreen } from "@/features/accounts/AccountsScreen";
import { emptySel } from "@/features/accounts/model";
import { usePlayBar } from "@/features/playbar/store";
import { useLaunch } from "@/features/playbar/usePlay";
import * as ipc from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { mockRemote, mockVault, resetAccountsMock, resetVaultMock } from "@/lib/mock";
import { queryClient } from "@/lib/queryClient";
import { DevicesPage } from "./DevicesPage";
import { runHere, sendTo, useMoving } from "./elsewhere";

// jsdom has no layout; give the virtual list a viewport.
beforeAll(() => {
  for (const k of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"] as const)
    Object.defineProperty(HTMLElement.prototype, k, { configurable: true, get: () => 800 });
});

beforeEach(() => {
  resetVaultMock();
  resetAccountsMock(6); // acc-i has userId 1000 + i
  queryClient.clear();
  useLaunch.setState({ batch: null, states: {}, dones: {} });
  useMoving.setState({ ids: [] });
  usePlayBar.getState().setDestination("");
  useToasts.setState({ items: [] });
  useUi.setState({ sel: emptySel, collapsed: new Set() });
});
afterEach(() => vi.restoreAllMocks());

const away = (userId: number, device = "Office PC", ip = "100.64.0.11") => ({ userId, device, ip });
const office = { device: "Office PC", ip: "100.64.0.11" };
const laptop = { device: "Laptop", ip: "100.64.0.12" };
const toasts = () => useToasts.getState().items.map((x) => x.message);
const accounts = async () => (await ipc.call("accounts_list")).accounts;

const setup = (ui: ReactNode) =>
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <Tooltip.Provider>{ui}</Tooltip.Provider>
    </QueryClientProvider>,
  );

test("runHere asks the other computer, then plays only the released accounts", async () => {
  mockRemote.elsewhere = [away(1000), away(1001)];
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "remote_run_here" ? Promise.resolve([1000]) : orig(cmd, args as never),
  );
  const [a, b] = await accounts();
  await runHere([a, b]);
  expect(ipc.call).toHaveBeenCalledWith("remote_run_here", { userIds: [1000, 1001] });
  await waitFor(() =>
    expect(ipc.call).toHaveBeenCalledWith(
      "launch_start",
      expect.objectContaining({ accountIds: [a.id] }),
    ),
  );
  expect(toasts()).toContain(t("elsewhere.stayed", { n: 1 }));
  expect(useMoving.getState().ids).toEqual([]);
});

test("runHere refuses while a launch batch is running, without asking the other computer", async () => {
  mockRemote.elsewhere = [away(1000)];
  useLaunch.setState({ batch: { id: "b1", ids: ["acc-5"], done: null } });
  const spy = vi.spyOn(ipc, "call");
  const [a] = await accounts();
  await runHere([a]);
  expect(spy).not.toHaveBeenCalledWith("remote_run_here", expect.anything());
  expect(toasts()).toContain(t("launch.busy"));
});

test("runHere refuses when Roblox is missing, before the other computer closes anything", async () => {
  mockRemote.elsewhere = [away(1000)];
  queryClient.setQueryData(["roblox"], { installed: false });
  const spy = vi.spyOn(ipc, "call");
  const [a] = await accounts();
  await runHere([a]);
  expect(spy).not.toHaveBeenCalledWith("remote_run_here", expect.anything());
  expect(toasts()).toContain(t("play.noRoblox"));
  expect(useMoving.getState().ids).toEqual([]);
});

test("a double click moves the accounts once, even before the list answers", async () => {
  mockRemote.elsewhere = [away(1000)];
  const spy = vi.spyOn(ipc, "call");
  const [a] = await accounts();
  await Promise.all([runHere([a]), runHere([a])]);
  expect(spy.mock.calls.filter(([c]) => c === "remote_run_here")).toHaveLength(1);
  await Promise.all([sendTo([a], laptop), sendTo([a], laptop)]);
  expect(spy.mock.calls.filter(([c]) => c === "remote_send")).toHaveLength(1);
  expect(useMoving.getState().ids).toEqual([]);
});

test("accounts that run nowhere else never show as moving", async () => {
  mockRemote.elsewhere = [];
  const seen: string[] = [];
  const unsub = useMoving.subscribe((s) => seen.push(...s.ids));
  const [a] = await accounts();
  await runHere([a]);
  unsub();
  expect(seen).not.toContain(a.id);
});

test("names hidden: the moved toast says 'an account', never the name", async () => {
  mockRemote.elsewhere = [away(1000)];
  queryClient.setQueryData(["vault"], { unlocked: true });
  queryClient.setQueryData(["settings"], { ui: { showUsernames: false } });
  const [a] = await accounts();
  await runHere([a]);
  const name = t("launch.anAccount").toLowerCase();
  await waitFor(() => expect(toasts()).toContain(t("elsewhere.movedOne", { name })));
});

test("runHere shows the other computer's reason when nothing moved", async () => {
  mockRemote.elsewhere = [away(1000)];
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "remote_run_here"
      ? Promise.reject(new Error("remote_run_here: RMI is locked on Office PC."))
      : orig(cmd, args as never),
  );
  const [a] = await accounts();
  await runHere([a]);
  expect(toasts()).toContain("RMI is locked on Office PC.");
  expect(ipc.call).not.toHaveBeenCalledWith("launch_start", expect.anything());
  expect(useMoving.getState().ids).toEqual([]);
});

test("runHere launches into each account's saved destination, not the Play bar", async () => {
  mockRemote.elsewhere = [away(1000)];
  usePlayBar.getState().setDestination("920587237");
  vi.spyOn(ipc, "call");
  const [a] = await accounts();
  await runHere([a]);
  await waitFor(() =>
    expect(ipc.call).toHaveBeenCalledWith("launch_start", {
      accountIds: [a.id],
      bar: { destination: "", server: "", mode: "any", home: true },
      useBarForAll: false,
    }),
  );
});

test("runHere still launches when a batch started while the other computer closed it", async () => {
  mockRemote.elsewhere = [away(1000)];
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) => {
    if (cmd !== "remote_run_here") return orig(cmd, args as never);
    // an auto-relaunch batch shows on the Play button meanwhile
    useLaunch.setState({ batch: { id: "b9", ids: ["acc-5"], done: null } });
    return Promise.resolve([1000]);
  });
  const [a] = await accounts();
  await runHere([a]);
  await waitFor(() =>
    expect(ipc.call).toHaveBeenCalledWith(
      "launch_start",
      expect.objectContaining({ accountIds: [a.id] }),
    ),
  );
  expect(toasts()).not.toContain(t("launch.busy"));
});

test("runHere says the account was closed there but did not start here", async () => {
  mockRemote.elsewhere = [away(1000)];
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "launch_start"
      ? Promise.reject(new Error("launch_start: RMI is locked."))
      : orig(cmd, args as never),
  );
  const [a] = await accounts();
  await runHere([a]);
  expect(toasts()).toEqual([t("elsewhere.notStarted", { n: 1, why: "RMI is locked." })]);
});

test("runHere and sendTo go by where accounts run now, not an old list", async () => {
  // the cached list still has 1000 on the Office PC; it has closed there since
  queryClient.setQueryData(["elsewhere"], [away(1000)]);
  mockRemote.elsewhere = [];
  vi.spyOn(ipc, "call");
  const [a] = await accounts();
  await runHere([a]);
  expect(ipc.call).not.toHaveBeenCalledWith("remote_run_here", expect.anything());
  await sendTo([a], office);
  expect(ipc.call).toHaveBeenCalledWith("remote_send", { userIds: [1000], ip: office.ip });
});

test("runHere and sendTo say why when the list cannot be read", async () => {
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "remote_elsewhere"
      ? Promise.reject(new Error("remote_elsewhere: RMI is still starting."))
      : orig(cmd, args as never),
  );
  const [a] = await accounts();
  await expect(runHere([a])).resolves.toBeUndefined();
  await expect(sendTo([a], office)).resolves.toBeUndefined();
  expect(toasts()).toEqual(["RMI is still starting.", "RMI is still starting."]);
  expect(ipc.call).not.toHaveBeenCalledWith("remote_run_here", expect.anything());
  expect(ipc.call).not.toHaveBeenCalledWith("remote_send", expect.anything());
});

test("runHere leaves accounts that need signing in where they run", async () => {
  // acc-2 (userId 1002) has an expired session
  mockRemote.elsewhere = [away(1002)];
  vi.spyOn(ipc, "call");
  const all = await accounts();
  await runHere([all[2]]);
  expect(ipc.call).not.toHaveBeenCalledWith("remote_run_here", expect.anything());
  expect(toasts()).toContain(t("elsewhere.signIn", { n: 1 }));
});

test("an account running on a paired computer shows where and offers Run here", async () => {
  mockRemote.elsewhere = [away(1001)];
  const user = userEvent.setup();
  setup(<AccountsScreen />);
  const row = (await screen.findByText(t("elsewhere.on", { device: "Office PC" }))).closest(
    '[role="row"]',
  ) as HTMLElement;
  expect(row.id).toBe("row-acc-1");
  await user.click(within(row).getByRole("button", { name: t("elsewhere.runHere") }));
  await waitFor(() => expect(mockVault.lastArgs.remote_run_here).toEqual({ userIds: [1001] }));
});

test("sendTo asks the target, then says how many could not be sent", async () => {
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "remote_send"
      ? Promise.resolve({ started: [1000], still: [] })
      : orig(cmd, args as never),
  );
  const [a, b] = await accounts();
  await sendTo([a, b], office);
  expect(ipc.call).toHaveBeenCalledWith("remote_send", { userIds: [1000, 1001], ip: office.ip });
  expect(toasts()).toContain(t("elsewhere.stayedHere", { n: 1 }));
  expect(useMoving.getState().ids).toEqual([]);
  expect(ipc.call).not.toHaveBeenCalledWith("launch_start", expect.anything());
});

test("sendTo says which accounts still run where they were", async () => {
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "remote_send"
      ? Promise.resolve({ started: [1000, 1001], still: [1001] })
      : orig(cmd, args as never),
  );
  const [a, b] = await accounts();
  await sendTo([a, b], office);
  expect(toasts()).toContain(t("elsewhere.notClosed", { n: 1 }));
});

test("sendTo says which computer must update RMI first", async () => {
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "remote_send"
      ? Promise.resolve({ started: [], still: [], old: "Den PC" })
      : orig(cmd, args as never),
  );
  const [a] = await accounts();
  await sendTo([a], office);
  expect(toasts()).toContain(t("elsewhere.updateFirst", { device: "Den PC" }));
});

test("a vault locked meanwhile shows the catalog text, not the code", async () => {
  mockRemote.elsewhere = [away(1000)];
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "remote_run_here"
      ? Promise.reject(new Error("remote_run_here: VAULT_WAS_LOCKED"))
      : orig(cmd, args as never),
  );
  const [a] = await accounts();
  await runHere([a]);
  expect(toasts()).toContain(t("err.vaultWasLocked"));
});

test("sendTo refuses while a launch batch is running, without asking the target", async () => {
  useLaunch.setState({ batch: { id: "b1", ids: ["acc-5"], done: null } });
  const spy = vi.spyOn(ipc, "call");
  const [a] = await accounts();
  await sendTo([a], office);
  expect(spy).not.toHaveBeenCalledWith("remote_send", expect.anything());
  expect(toasts()).toContain(t("launch.busy"));
});

test("sendTo skips accounts already running on that computer", async () => {
  mockRemote.elsewhere = [away(1000)];
  vi.spyOn(ipc, "call");
  const [a, b] = await accounts();
  await sendTo([a, b], office);
  expect(ipc.call).toHaveBeenCalledWith("remote_send", { userIds: [1001], ip: office.ip });
});

test("sendTo shows the reason when nothing was sent", async () => {
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "remote_send"
      ? Promise.reject(new Error("remote_send: RMI is locked on Office PC."))
      : orig(cmd, args as never),
  );
  const [a] = await accounts();
  await sendTo([a], office);
  expect(toasts()).toContain("RMI is locked on Office PC.");
  expect(useMoving.getState().ids).toEqual([]);
});

const openRowMenu = async (id: string) => {
  const user = userEvent.setup();
  setup(<AccountsScreen />);
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  const row = document.getElementById(id) as HTMLElement;
  await user.click(within(row).getByRole("button", { name: t("menu.more") }));
  return user;
};

test("the account menu offers Run on with this computer and each paired computer", async () => {
  mockRemote.elsewhere = [away(1001)];
  mockRemote.targets = [office, laptop];
  const user = await openRowMenu("row-acc-1");
  await user.click(await screen.findByRole("menuitem", { name: t("elsewhere.runOn") }));
  expect(
    await screen.findByRole("menuitem", { name: t("elsewhere.thisComputer") }),
  ).toBeInTheDocument();
  expect(screen.getByRole("menuitem", { name: "Laptop" })).toBeInTheDocument();
  expect(screen.queryByRole("menuitem", { name: "Office PC" })).not.toBeInTheDocument();
});

test("an account that runs here can be sent to a paired computer", async () => {
  mockRemote.targets = [office];
  const user = await openRowMenu("row-acc-1");
  await user.click(
    await screen.findByRole("menuitem", {
      name: t("elsewhere.runOnDevice", { device: "Office PC" }),
    }),
  );
  await waitFor(() =>
    expect(mockVault.lastArgs.remote_send).toEqual({ userIds: [1001], ip: office.ip }),
  );
});

test("the Devices card of a paired computer offers Run all here", async () => {
  // Den PC is paired at 100.64.0.11; one more account runs on another computer
  mockRemote.elsewhere = [
    away(1000, "Den PC"),
    away(1001, "Den PC"),
    away(1002, "Studio", "100.64.0.15"),
  ];
  const user = userEvent.setup();
  setup(<DevicesPage />);
  const den = (await screen.findByText(t("elsewhere.count", { n: 2 }), { exact: false })).closest(
    "li",
  ) as HTMLElement;
  expect(den).toHaveTextContent("Den PC");
  await user.click(within(den).getByRole("button", { name: t("elsewhere.runAllHere") }));
  await waitFor(() =>
    expect(mockVault.lastArgs.remote_run_here).toEqual({ userIds: [1000, 1001] }),
  );
});

test("a computer that holds the pairing to this one also offers Run all here", async () => {
  // Laptop (100.64.0.12) is not paired from this side; it reports to this one.
  mockRemote.elsewhere = [away(1000, "Laptop", "100.64.0.12")];
  setup(<DevicesPage />);
  const card = (await screen.findByText(t("elsewhere.countOne"), { exact: false })).closest(
    "li",
  ) as HTMLElement;
  expect(card).toHaveTextContent("Laptop");
  expect(within(card).getByRole("button", { name: t("elsewhere.runAllHere") })).toBeEnabled();
});
