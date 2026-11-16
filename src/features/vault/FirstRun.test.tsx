import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { mockCall, mockVault, resetVaultMock } from "@/lib/mock";
import { t } from "@/lib/i18n";
import { FirstRun } from "./FirstRun";

function setup() {
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <Tooltip.Provider>
        <FirstRun />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
}

beforeEach(resetVaultMock);

test("strength hint counts code points, not UTF-16 units", async () => {
  setup();
  const pw = await screen.findByLabelText(t("vault.password"));
  fireEvent.change(pw, { target: { value: "x".repeat(7) } });
  expect(screen.getByText(t("vault.strength.short"))).toBeInTheDocument();
  fireEvent.change(pw, { target: { value: "x".repeat(8) } });
  expect(screen.getByText(t("vault.strength.ok"))).toBeInTheDocument();
  fireEvent.change(pw, { target: { value: "x".repeat(12) } });
  expect(screen.getByText(t("vault.strength.good"))).toBeInTheDocument();
  fireEvent.change(pw, { target: { value: "x".repeat(16) } });
  expect(screen.getByText(t("vault.strength.strong"))).toBeInTheDocument();
  // 8 emoji are 16 UTF-16 units but 8 code points: not too short
  fireEvent.change(pw, { target: { value: "🔑".repeat(8) } });
  expect(screen.getByText(t("vault.strength.ok"))).toBeInTheDocument();
});

test("short password is refused inline, and no mismatch error piles on top", async () => {
  const user = userEvent.setup();
  setup();
  await user.type(await screen.findByLabelText(t("vault.password")), "x".repeat(7));
  await user.type(screen.getByLabelText(t("vault.confirm")), "different");
  await user.click(screen.getByRole("button", { name: t("vault.create") }));
  expect(await screen.findByText(t("vault.err.short"))).toBeInTheDocument();
  expect(screen.queryByText(t("vault.err.mismatch"))).toBeNull();
  expect(mockVault.lastArgs.vault_create).toBeUndefined();
});

test("OS mode creates the vault with no password and hides the password fields", async () => {
  const user = userEvent.setup();
  setup();
  await user.click(await screen.findByRole("radio", { name: new RegExp(t("vault.mode.os")) }));
  expect(screen.queryByLabelText(t("vault.password"))).toBeNull();
  await user.click(screen.getByRole("button", { name: t("vault.create") }));
  await vi.waitFor(() =>
    expect(mockVault.lastArgs.vault_create).toEqual({ mode: "os", password: null }),
  );
  expect(await mockCall("vault_status")).toMatchObject({ exists: true, mode: "os" });
});

test("two passwords of valid length that differ are refused inline", async () => {
  const user = userEvent.setup();
  setup();
  await user.type(await screen.findByLabelText(t("vault.password")), "correct horse");
  await user.type(screen.getByLabelText(t("vault.confirm")), "another horse");
  await user.click(screen.getByRole("button", { name: t("vault.create") }));
  expect(await screen.findByText(t("vault.err.mismatch"))).toBeInTheDocument();
  expect(mockVault.lastArgs.vault_create).toBeUndefined();
});
