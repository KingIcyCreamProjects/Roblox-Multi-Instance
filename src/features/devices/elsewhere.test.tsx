import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { useUi } from "@/app/store";
import { useToasts } from "@/components/Toasts";
import { AccountsScreen } from "@/features/accounts/AccountsScreen";
import { emptySel } from "@/features/accounts/model";
import { useLaunch } from "@/features/playbar/usePlay";
import * as ipc from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { mockRemote, mockVault, resetAccountsMock, resetVaultMock } from "@/lib/mock";
import { queryClient } from "@/lib/queryClient";
import { DevicesPage } from "./DevicesPage";
import { runHere, useMoving } from "./elsewhere";

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
  useToasts.setState({ items: [] });
  useUi.setState({ sel: emptySel, collapsed: new Set() });
});
afterEach(() => vi.restoreAllMocks());

const away = (userId: number, device = "Office PC", ip = "100.64.0.11") => ({ userId, device, ip });
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
