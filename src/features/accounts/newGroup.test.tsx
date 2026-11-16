import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useUi } from "@/app/store";
import { Toaster, useToasts } from "@/components/Toasts";
import * as ipc from "@/lib/ipc";
import { resetAccountsMock } from "@/lib/mock";
import { t } from "@/lib/i18n";
import { AccountSheets, AccountsScreen } from "./AccountsScreen";
import { emptySel } from "./model";

beforeAll(() => {
  for (const k of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"] as const)
    Object.defineProperty(HTMLElement.prototype, k, { configurable: true, get: () => 800 });
});

beforeEach(() => {
  resetAccountsMock(6);
  useUi.setState({ sel: emptySel, toolsOpen: false, focusReq: null });
  useToasts.setState({ items: [] });
});
afterEach(() => vi.restoreAllMocks());

function setup() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(["vault"], { unlocked: true });
  render(
    <QueryClientProvider client={qc}>
      <Tooltip.Provider>
        <AccountsScreen />
        <AccountSheets />
        <Toaster />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
}

const row = (name: string) =>
  within(screen.getByRole("grid")).getByText(name).closest('[role="row"]') as HTMLElement;

async function pickNewGroup(user: ReturnType<typeof userEvent.setup>) {
  const sub = (await screen.findByText(t("menu.moveTo"))).closest('[role="menuitem"]')!;
  (sub as HTMLElement).focus();
  fireEvent.keyDown(sub, { key: "ArrowRight" });
  await user.click(await screen.findByText(t("menu.newGroup")));
}

async function expectNameField(user: ReturnType<typeof userEvent.setup>) {
  const input = await screen.findByPlaceholderText(t("inspector.newGroupName"));
  // survive the menu's focus-return to its trigger
  await new Promise((r) => setTimeout(r, 50));
  expect(screen.getByPlaceholderText(t("inspector.newGroupName"))).toHaveFocus();
  await user.keyboard("Crew{Enter}");
  return input;
}

test("row More menu -> New group keeps the name field open and saves", async () => {
  const user = userEvent.setup();
  setup();
  const spy = vi.spyOn(ipc, "call");
  await screen.findByText("Orbit");
  await user.click(within(row("Orbit")).getByRole("button", { name: t("menu.more") }));
  await pickNewGroup(user);
  await expectNameField(user);
  await waitFor(() =>
    expect(spy).toHaveBeenCalledWith(
      "accounts_update",
      expect.objectContaining({
        patch: expect.objectContaining({ group: "Crew" }),
      }),
    ),
  );
});

test("context menu -> New group keeps the name field open and saves", async () => {
  const user = userEvent.setup();
  setup();
  const spy = vi.spyOn(ipc, "call");
  await screen.findByText("Orbit");
  await user.pointer({ keys: "[MouseRight]", target: row("Orbit") });
  await pickNewGroup(user);
  await expectNameField(user);
  await waitFor(() =>
    expect(spy).toHaveBeenCalledWith(
      "accounts_update",
      expect.objectContaining({
        patch: expect.objectContaining({ group: "Crew" }),
      }),
    ),
  );
});

test("toolbar New group with nothing selected toasts a hint", async () => {
  const user = userEvent.setup();
  setup();
  await user.click(await screen.findByRole("button", { name: t("accounts.newGroup") }));
  expect(await screen.findByText(t("accounts.newGroupHint"))).toBeInTheDocument();
});

test("toolbar New group with a selection opens the name field", async () => {
  const user = userEvent.setup();
  setup();
  await user.click(await screen.findByText("Orbit"));
  await user.click(screen.getByRole("button", { name: t("accounts.newGroup") }));
  await expectNameField(user);
});

test("with a group header selected, row More -> New group moves the whole selection", async () => {
  const user = userEvent.setup();
  setup();
  const spy = vi.spyOn(ipc, "call");
  await user.click(await screen.findByRole("button", { name: /Main/, pressed: false }));
  await user.click(within(row("Orbit")).getByRole("button", { name: t("menu.more") }));
  await pickNewGroup(user);
  await expectNameField(user);
  await waitFor(() =>
    expect(spy).toHaveBeenCalledWith(
      "accounts_update",
      expect.objectContaining({
        patch: expect.objectContaining({ group: "Crew" }),
      }),
    ),
  );
});

test("group header More -> New group works on the selection", async () => {
  const user = userEvent.setup();
  setup();
  await user.click(await screen.findByText("Orbit"));
  const head = screen.getByText("Main").closest('[role="row"]') as HTMLElement;
  await user.click(within(head).getByRole("button", { name: t("group.menu", { name: "Main" }) }));
  await user.click(await screen.findByText(t("menu.newGroup")));
  await expectNameField(user);
});

test("group header More -> Rename renames in place, keeping focus in the field", async () => {
  const user = userEvent.setup();
  const spy = vi.spyOn(ipc, "call");
  setup();
  const head = (await screen.findByText("Main")).closest('[role="row"]') as HTMLElement;
  await user.click(within(head).getByRole("button", { name: t("group.menu", { name: "Main" }) }));
  await user.click(await screen.findByText(t("group.rename")));
  const input = await within(head).findByRole("textbox", {
    name: t("group.renameLabel", { name: "Main" }),
  });
  // survive the menu's focus-return to its trigger
  await new Promise((r) => setTimeout(r, 50));
  expect(input).toHaveFocus();
  await user.keyboard("{Control>}a{/Control}Crew{Enter}");
  await waitFor(() =>
    expect(spy).toHaveBeenCalledWith("group_rename", { from: "Main", to: "Crew" }),
  );
  expect(await screen.findByText("Crew")).toBeInTheDocument();
});
