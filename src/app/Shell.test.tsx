import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import * as Tooltip from "@radix-ui/react-tooltip";
import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import { Tabs } from "./Tabs";
import { Shell, useApplyAppearance } from "./Shell";
import { t } from "@/lib/i18n";
import { mockVault, resetVaultMock } from "@/lib/mock";

const wrap = (ui: React.ReactNode) =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tooltip.Provider>{ui}</Tooltip.Provider>
    </QueryClientProvider>,
  );

test("tabs hide Automation unless enabled", () => {
  const { rerender } = wrap(<Tabs automation={false} />);
  expect(screen.queryByText("Automation")).toBeNull();
  rerender(
    <QueryClientProvider client={new QueryClient()}>
      <Tooltip.Provider>
        <Tabs automation />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  expect(screen.getByText("Automation")).toBeInTheDocument();
});

test("shell applies theme from settings and shows empty state", async () => {
  wrap(<Shell />);
  await waitFor(() => expect(document.documentElement.dataset.theme).toBe("dark"));
  expect(screen.getByText(t("empty.signIn"))).toBeInTheDocument();
});

test("selection ids of removed accounts are dropped on any route (Play bar counts stay true)", async () => {
  const { useUi } = await import("./store");
  const { resetAccountsMock } = await import("@/lib/mock");
  resetAccountsMock(2);
  useUi.setState({
    route: "home",
    sel: { ids: ["acc-1", "gone"], anchor: "gone", cursor: "gone" },
  });
  wrap(<Shell />);
  await waitFor(() =>
    expect(useUi.getState().sel).toEqual({ ids: ["acc-1"], anchor: null, cursor: null }),
  );
});

test("t() interpolates", () => {
  expect(t("playbar.playN", { n: 3 })).toBe("Play 3 accounts");
});

test("Ctrl+K palette runs the best match on Enter", async () => {
  const { useUi } = await import("./store");
  wrap(<Shell />);
  fireEvent.keyDown(window, { key: "k", ctrlKey: true });
  const input = await screen.findByRole("combobox", { name: t("title.search") });
  fireEvent.change(input, { target: { value: "sett" } });
  expect(screen.getAllByRole("option")[0]).toHaveTextContent(
    t("palette.goTo", { place: t("nav.settings") }),
  );
  fireEvent.keyDown(input, { key: "Enter" });
  await waitFor(() => expect(useUi.getState().route).toBe("settings"));
  expect(screen.queryByRole("combobox", { name: t("title.search") })).toBeNull();
});

test("a running launch shows its capsule (progress + Cancel) on every route", async () => {
  const { useUi } = await import("./store");
  const { useLaunch } = await import("@/features/playbar/usePlay");
  useUi.setState({ route: "discover" });
  wrap(<Shell />);
  expect(screen.queryByRole("button", { name: t("launch.cancel") })).toBeNull();
  act(() => useLaunch.setState({ batch: { id: "b1", ids: ["acc-1"], done: null } }));
  expect(await screen.findByRole("button", { name: t("launch.cancel") })).toBeInTheDocument();
  // the full bar (Play, Grid Arrange) stays on accounts/home
  expect(screen.queryByRole("button", { name: t("playbar.once") })).toBeNull();
  act(() => useLaunch.setState({ batch: null }));
  useUi.setState({ route: "accounts" });
});

test("appearance: privacy is the inverse of showUsernames, scale becomes a factor", () => {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity } } });
  qc.setQueryData(["settings"], {
    theme: "light",
    ui: { density: "compact", showUsernames: false, reducedMotion: "always", scale: 125 },
  });
  const wrapper = ({ children }: { children: React.ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
  renderHook(() => useApplyAppearance(), { wrapper });
  const root = document.documentElement;
  expect(root.dataset).toMatchObject({
    theme: "light",
    density: "compact",
    privacy: "on",
    reducedMotion: "always",
  });
  expect(root.style.getPropertyValue("--ui-scale")).toBe("1.25");
  expect(localStorage.getItem("rmi.theme")).toBe("light");
});

test("appearance: theme 'system' follows prefers-color-scheme", () => {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity } } });
  qc.setQueryData(["settings"], { theme: "system", ui: { showUsernames: true, scale: 100 } });
  const wrapper = ({ children }: { children: React.ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
  const mm = vi
    .spyOn(window, "matchMedia")
    .mockReturnValue({ matches: true, addEventListener() {}, removeEventListener() {} } as never);
  renderHook(() => useApplyAppearance(), { wrapper });
  expect(document.documentElement.dataset.theme).toBe("light");
  expect(document.documentElement.dataset.privacy).toBe("off");
  mm.mockRestore();
});

test("automation page leaves for Accounts once automation is off", async () => {
  resetVaultMock(); // mock defaults: automation off
  const { useUi } = await import("./store");
  useUi.setState({ route: "automation" });
  wrap(<Shell />);
  await waitFor(() => expect(useUi.getState().route).toBe("accounts"));
});

test("Ctrl+Shift+H toggles privacy once settings have loaded", async () => {
  resetVaultMock();
  const { useUi } = await import("./store");
  useUi.setState({ route: "accounts" });
  const qc = new QueryClient();
  render(
    <QueryClientProvider client={qc}>
      <Tooltip.Provider>
        <Shell />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  await waitFor(() => expect(qc.getQueryData(["settings"])).toBeDefined());
  fireEvent.keyDown(window, { key: "H", ctrlKey: true, shiftKey: true });
  await waitFor(() =>
    expect(mockVault.lastArgs.settings_set).toEqual({ patch: { ui: { showUsernames: false } } }),
  );
});
