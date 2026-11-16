import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { t } from "@/lib/i18n";
import type { Settings } from "@/lib/ipc";
import { mockVault, resetVaultMock } from "@/lib/mock";
import { DEFAULT_ORDER, TabOrder, tabOrder } from "./Tabs";

const with_ = (tabOrder: string[]) => ({ ui: { tabOrder } }) as unknown as Settings;

test("default tab order is Home, Accounts, Discover, Automation, Devices, Settings", () => {
  expect(DEFAULT_ORDER).toEqual([
    "home",
    "accounts",
    "discover",
    "automation",
    "devices",
    "settings",
  ]);
  expect(tabOrder(undefined)).toEqual(DEFAULT_ORDER);
});

test("saved order wins; junk and repeats dropped; missing tabs appended", () => {
  expect(tabOrder(with_(["settings", "bogus", "settings", "discover"]))).toEqual([
    "settings",
    "discover",
    "home",
    "accounts",
    "automation",
    "devices",
  ]);
});

test("TabOrder: move down swaps the row with the next and saves the whole order", async () => {
  resetVaultMock();
  render(
    <QueryClientProvider client={new QueryClient()}>
      <TabOrder />
    </QueryClientProvider>,
  );
  fireEvent.click(screen.getByRole("button", { name: t("tabs.down", { name: t("nav.home") }) }));
  await waitFor(() =>
    expect(mockVault.lastArgs.settings_set).toEqual({
      patch: {
        ui: { tabOrder: ["accounts", "home", "discover", "automation", "devices", "settings"] },
      },
    }),
  );
});
