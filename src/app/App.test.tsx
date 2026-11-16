import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { call, VAULT_LOCKED_EVENT } from "@/lib/ipc";
import { mockCall, mockVault, resetVaultMock } from "@/lib/mock";
import { t } from "@/lib/i18n";
import { App } from "./App";

const PW = "correct horse";

function setup() {
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <App />
    </QueryClientProvider>,
  );
}

beforeEach(resetVaultMock);

async function create(user: ReturnType<typeof userEvent.setup>) {
  await user.type(await screen.findByLabelText(t("vault.password")), PW);
  await user.type(screen.getByLabelText(t("vault.confirm")), PW);
  await user.click(screen.getByRole("button", { name: t("vault.create") }));
}

test("no vault shows first run", async () => {
  setup();
  expect(await screen.findByRole("heading", { name: t("vault.first.title") })).toBeInTheDocument();
});

test("create goes to shell, lock goes to unlock, wrong password errors, right one opens", async () => {
  const user = userEvent.setup();
  setup();
  await create(user);
  await user.click(await screen.findByRole("button", { name: t("title.lock") }));
  const field = await screen.findByLabelText(t("vault.password"));
  await user.type(field, "wrong password{Enter}");
  expect(await screen.findByText(t("vault.err.wrong"))).toBeInTheDocument();
  await user.clear(field);
  await user.type(field, `${PW}{Enter}`);
  expect(await screen.findByText(t("empty.signIn"))).toBeInTheDocument();
});

test("mismatched confirm blocks create", async () => {
  const user = userEvent.setup();
  setup();
  await user.type(await screen.findByLabelText(t("vault.password")), PW);
  await user.type(screen.getByLabelText(t("vault.confirm")), "different one{Enter}");
  expect(await screen.findByText(t("vault.err.mismatch"))).toBeInTheDocument();
});

async function osLocked() {
  await mockCall("vault_create", { mode: "os", password: null });
  await mockCall("vault_lock");
}

test("OS mode auto-unlocks at startup but not after lock; manual Unlock works", async () => {
  const user = userEvent.setup();
  await osLocked();
  setup();
  expect(await screen.findByText(t("empty.signIn"))).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("title.lock") }));
  const btn = await screen.findByRole("button", { name: t("vault.unlock") });
  expect(screen.queryByLabelText(t("vault.password"))).toBeNull();
  await new Promise((r) => setTimeout(r, 100));
  expect(screen.getByRole("button", { name: t("vault.unlock") })).toBeInTheDocument();
  await user.click(btn);
  expect(await screen.findByText(t("empty.signIn"))).toBeInTheDocument();
});

test("Ctrl+L locks from the shell", async () => {
  const user = userEvent.setup();
  setup();
  await create(user);
  expect(await screen.findByText(t("empty.signIn"))).toBeInTheDocument();
  await user.keyboard("{Control>}l{/Control}");
  expect(await screen.findByLabelText(t("vault.password"))).toBeInTheDocument();
});

test("a command failing with the locked error swaps the gate to Unlock", async () => {
  const user = userEvent.setup();
  setup();
  await create(user);
  expect(await screen.findByText(t("empty.signIn"))).toBeInTheDocument();
  await mockCall("vault_lock"); // locked behind the UI's back (auto-lock)
  await expect(
    call("vault_export_plain", { path: "/x", confirm: "export plaintext", password: null }),
  ).rejects.toThrow("vault_export_plain: VAULT_LOCKED");
  expect(await screen.findByLabelText(t("vault.password"))).toBeInTheDocument();
});

test("the locked event refetches status", async () => {
  const user = userEvent.setup();
  setup();
  await create(user);
  expect(await screen.findByText(t("empty.signIn"))).toBeInTheDocument();
  await mockCall("vault_lock");
  window.dispatchEvent(new Event(VAULT_LOCKED_EVENT));
  expect(await screen.findByLabelText(t("vault.password"))).toBeInTheDocument();
});

test("remembered vault auto-unlocks at startup again and again", async () => {
  await mockCall("vault_create", { mode: "password", password: PW });
  await mockCall("vault_lock");
  await mockCall("vault_unlock", { password: PW, remember: true });
  await mockCall("vault_lock");
  for (let i = 0; i < 2; i++) {
    const { unmount } = render(
      <QueryClientProvider
        client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
      >
        <App />
      </QueryClientProvider>,
    );
    expect(await screen.findByText(t("empty.signIn"))).toBeInTheDocument();
    expect(mockVault.lastArgs.vault_unlock).toEqual({ password: null, remember: false });
    unmount();
    await mockCall("vault_lock");
  }
});

test("damaged vault: restore a backup, then the normal Unlock screen", async () => {
  const user = userEvent.setup();
  await mockCall("vault_create", { mode: "password", password: PW });
  await mockCall("vault_lock");
  mockVault.setDamaged(true);
  setup();
  expect(
    await screen.findByRole("heading", { name: t("vault.damaged.title") }),
  ).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: t("vault.unlock") })).toBeNull();
  await user.click(screen.getByRole("button", { name: t("vault.damaged.restore") }));
  await user.type(screen.getByLabelText(t("vault.damaged.pw")), "wrong pass");
  const pick = await screen.findByRole("button", {
    name: `${t("vault.damaged.pick")} vault-20260101-120000.rmiv`,
  });
  await user.click(pick);
  expect(await screen.findByText(t("vault.err.wrong"))).toBeInTheDocument();
  await user.clear(screen.getByLabelText(t("vault.damaged.pw")));
  await user.type(screen.getByLabelText(t("vault.damaged.pw")), PW);
  await user.click(pick);
  expect(await screen.findByRole("heading", { name: t("vault.unlock.title") })).toBeInTheDocument();
  await user.type(screen.getByLabelText(t("vault.password")), `${PW}{Enter}`);
  expect(await screen.findByText(t("empty.signIn"))).toBeInTheDocument();
});

test("activity sends vault_touch only once 30 s have passed since the last one", async () => {
  await mockCall("vault_create", { mode: "password", password: PW });
  const now = vi.spyOn(Date, "now").mockReturnValue(1_000_000);
  try {
    setup();
    await screen.findByText(t("empty.signIn"));
    now.mockReturnValue(1_010_000);
    window.dispatchEvent(new Event("pointerdown"));
    expect("vault_touch" in mockVault.lastArgs).toBe(false);
    now.mockReturnValue(1_031_000);
    window.dispatchEvent(new Event("pointerdown"));
    await waitFor(() => expect("vault_touch" in mockVault.lastArgs).toBe(true));
  } finally {
    now.mockRestore();
  }
});
