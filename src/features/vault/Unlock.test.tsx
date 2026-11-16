import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import * as ipc from "@/lib/ipc";
import { mockCall, mockVault, resetVaultMock } from "@/lib/mock";
import { t } from "@/lib/i18n";
import { Unlock } from "./Unlock";

const status = {
  exists: true,
  unlocked: false,
  mode: "password",
  rememberActive: false,
  damaged: false,
} as const;

function setup() {
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <Tooltip.Provider>
        <Unlock status={status} auto={false} onAutoDone={() => {}} />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
}

beforeEach(resetVaultMock);
afterEach(() => vi.restoreAllMocks());

test("an empty password is refused inline without calling the vault", async () => {
  const user = userEvent.setup();
  setup();
  await user.click(await screen.findByRole("button", { name: t("vault.unlock") }));
  expect(await screen.findByText(t("vault.err.empty"))).toBeInTheDocument();
  expect(mockVault.lastArgs.vault_unlock).toBeUndefined();
});

test("a Lock that cancels the unlock is not shown as an error", async () => {
  const user = userEvent.setup();
  const spy = vi.spyOn(ipc, "call").mockRejectedValue(new Error("vault_unlock: VAULT_WAS_LOCKED"));
  setup();
  await user.type(await screen.findByLabelText(t("vault.password")), "anything");
  await user.click(screen.getByRole("button", { name: t("vault.unlock") }));
  expect(spy).toHaveBeenCalledWith("vault_unlock", { password: "anything", remember: false });
  await vi.waitFor(() =>
    expect(screen.getByRole("button", { name: t("vault.unlock") })).toBeEnabled(),
  );
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.queryByText(t("vault.err.wrong"))).toBeNull();
});

const PW = "correct horse";
const lockedPasswordVault = async () => {
  await mockCall("vault_create", { mode: "password", password: PW });
  await mockCall("vault_lock");
};
function mount(el: ReactElement) {
  return render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <Tooltip.Provider>{el}</Tooltip.Provider>
    </QueryClientProvider>,
  );
}

test("a remembered-password auto unlock that fails shows the form, not an error", async () => {
  await lockedPasswordVault();
  const onAutoDone = vi.fn();
  mount(<Unlock status={status} auto onAutoDone={onAutoDone} />);
  expect(onAutoDone).toHaveBeenCalledTimes(1);
  await vi.waitFor(() =>
    expect(mockVault.lastArgs.vault_unlock).toEqual({ password: null, remember: false }),
  );
  await vi.waitFor(() =>
    expect(screen.getByRole("button", { name: t("vault.unlock") })).toBeEnabled(),
  );
  expect(screen.queryByRole("alert")).toBeNull();
});

test("an unlock that finds the file damaged opens the recovery panel", async () => {
  const user = userEvent.setup();
  await lockedPasswordVault();
  mockVault.setDamaged(true);
  mount(<Unlock status={status} auto={false} onAutoDone={() => {}} />);
  await user.type(await screen.findByLabelText(t("vault.password")), "anything");
  await user.click(screen.getByRole("button", { name: t("vault.unlock") }));
  expect(
    await screen.findByRole("button", { name: t("vault.damaged.restore") }),
  ).toBeInTheDocument();
});

test("a damaged vault at startup shows the recovery screen, not the password form", () => {
  mount(<Unlock status={{ ...status, damaged: true }} auto={false} onAutoDone={() => {}} />);
  expect(screen.getByText(t("vault.damaged.title"))).toBeInTheDocument();
  expect(screen.getByRole("button", { name: t("vault.damaged.restore") })).toBeInTheDocument();
  expect(screen.queryByLabelText(t("vault.password"))).toBeNull();
});
