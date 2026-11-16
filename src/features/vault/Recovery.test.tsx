import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { mockCall, mockVault, resetVaultMock } from "@/lib/mock";
import { t } from "@/lib/i18n";
import { Recovery } from "./Recovery";

const PW = "correct horse";
const PICK = `${t("vault.damaged.pick")} vault-20260101-120000.rmiv`;

function setup(onRestored = () => {}) {
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <Tooltip.Provider>
        <Recovery onRestored={onRestored} />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
}

/** A password vault that is locked and damaged, as the gate sees it. */
async function damagedVault() {
  await mockCall("vault_create", { mode: "password", password: PW });
  await mockCall("vault_lock");
  mockVault.setDamaged(true);
}

beforeEach(resetVaultMock);

test("a wrong password shows the error on the field and does not restore", async () => {
  const user = userEvent.setup();
  const onRestored = vi.fn();
  await damagedVault();
  setup(onRestored);
  await user.click(screen.getByRole("button", { name: t("vault.damaged.restore") }));
  await user.type(await screen.findByLabelText(t("vault.damaged.pw")), "not it at all");
  await user.click(await screen.findByRole("button", { name: PICK }));
  expect(await screen.findByText(t("vault.err.wrong"))).toBeInTheDocument();
  expect(screen.getByLabelText(t("vault.damaged.pw"))).toHaveAttribute("aria-invalid", "true");
  expect(onRestored).not.toHaveBeenCalled();
});

test("the right password restores the picked backup and tells the parent", async () => {
  const user = userEvent.setup();
  const onRestored = vi.fn();
  await damagedVault();
  setup(onRestored);
  await user.click(screen.getByRole("button", { name: t("vault.damaged.restore") }));
  await user.type(await screen.findByLabelText(t("vault.damaged.pw")), PW);
  await user.click(await screen.findByRole("button", { name: PICK }));
  await vi.waitFor(() => expect(onRestored).toHaveBeenCalledTimes(1));
  expect(mockVault.lastArgs.vault_restore).toEqual({
    name: "vault-20260101-120000.rmiv",
    password: PW,
  });
});

test("a restore refused for another reason shows the message as an alert, not on the field", async () => {
  const user = userEvent.setup();
  await mockCall("vault_create", { mode: "password", password: PW });
  await mockCall("vault_lock"); // not damaged, so the mock refuses the restore
  setup();
  await user.click(screen.getByRole("button", { name: t("vault.damaged.restore") }));
  await user.click(await screen.findByRole("button", { name: PICK }));
  expect(await screen.findByRole("alert")).toHaveTextContent("The vault opens fine");
  expect(screen.getByLabelText(t("vault.damaged.pw"))).toHaveAttribute("aria-invalid", "false");
});
