import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useUi } from "@/app/store";
import { useMoving } from "@/features/devices/elsewhere";
import { t } from "@/lib/i18n";
import { mockRemote, mockVault, resetAccountsMock, resetVaultMock } from "@/lib/mock";
import { queryClient } from "@/lib/queryClient";
import { AccountsScreen } from "./AccountsScreen";
import { emptySel } from "./model";

beforeAll(() => {
  for (const k of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"] as const)
    Object.defineProperty(HTMLElement.prototype, k, { configurable: true, get: () => 800 });
});

beforeEach(() => {
  resetVaultMock();
  resetAccountsMock(6); // acc-i has userId 1000 + i
  queryClient.clear();
  useMoving.setState({ ids: [] });
  // acc-1 runs on the office computer; office and laptop can take accounts
  mockRemote.elsewhere = [{ userId: 1001, device: "Office PC", ip: "100.64.0.11" }];
  mockRemote.targets = [
    { device: "Office PC", ip: "100.64.0.11" },
    { device: "Laptop", ip: "100.64.0.12" },
  ];
});
afterEach(() => {
  mockRemote.elsewhere = [];
  mockRemote.targets = [];
});

async function openRunOn(ids: string[]) {
  const user = userEvent.setup();
  useUi.setState({ sel: { ...emptySel, ids }, selGroup: null });
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <Tooltip.Provider>
        <AccountsScreen />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  if (ids.length === 1)
    await user.click(await screen.findByRole("button", { name: t("inspector.more") }));
  await user.click(await screen.findByRole("button", { name: t("elsewhere.runOn") }));
  return user;
}

test("Esc reverts an alias edit and a bad FPS value reverts on blur, neither saves", async () => {
  const user = userEvent.setup();
  useUi.setState({ sel: { ...emptySel, ids: ["acc-0"] }, selGroup: null });
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <Tooltip.Provider>
        <AccountsScreen />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  await user.click(await screen.findByRole("button", { name: t("inspector.more") }));
  mockVault.lastArgs = {};
  const alias = await screen.findByLabelText(t("inspector.alias"));
  const before = (alias as HTMLInputElement).value;
  await user.type(alias, "zz{Escape}");
  expect(alias).toHaveValue(before);
  const fps = await screen.findByLabelText(t("inspector.fps"));
  await user.type(fps, "999");
  expect(screen.getByRole("alert")).toHaveTextContent(t("inspector.fpsErr"));
  await user.tab();
  expect(fps).toHaveValue("");
  expect(mockVault.lastArgs.accounts_update).toBeUndefined();
});

test("Inspector Run on lists only computers the account is not on, and sends it there", async () => {
  const user = await openRunOn(["acc-1"]);
  expect(screen.queryByRole("menuitem", { name: "Office PC" })).not.toBeInTheDocument();
  await user.click(await screen.findByRole("menuitem", { name: "Laptop" }));
  await waitFor(() =>
    expect(mockVault.lastArgs.remote_send).toEqual({ userIds: [1001], ip: "100.64.0.12" }),
  );
});

test("Inspector Run on in a batch sends every picked account; a move disables both buttons", async () => {
  const user = await openRunOn(["acc-0", "acc-1"]);
  await user.click(await screen.findByRole("menuitem", { name: "Office PC" }));
  // acc-1 already runs there: only acc-0 goes
  await waitFor(() =>
    expect(mockVault.lastArgs.remote_send).toEqual({ userIds: [1000], ip: "100.64.0.11" }),
  );
  act(() => useMoving.setState({ ids: ["acc-0"] }));
  // one "Moving…" (on Run N here), Run on stays labelled but disabled
  expect(await screen.findByRole("button", { name: t("elsewhere.moving") })).toBeDisabled();
  expect(screen.getByRole("button", { name: t("elsewhere.runOn") })).toBeDisabled();
});
