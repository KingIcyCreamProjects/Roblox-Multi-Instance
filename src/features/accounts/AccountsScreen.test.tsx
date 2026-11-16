import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useUi } from "@/app/store";
import { Toaster, useToasts } from "@/components/Toasts";
import * as ipc from "@/lib/ipc";
import { mockVault, resetAccountsMock } from "@/lib/mock";
import { lastSite } from "@/lib/mockTools";
import { t } from "@/lib/i18n";
import { AccountSheets, AccountsScreen } from "./AccountsScreen";
import { emptySel } from "./model";

// jsdom has no layout; give the virtual list a viewport.
beforeAll(() => {
  for (const k of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"] as const)
    Object.defineProperty(HTMLElement.prototype, k, { configurable: true, get: () => 800 });
});

beforeEach(() => {
  resetAccountsMock(6);
  useUi.setState({
    sel: emptySel,
    collapsed: new Set(),
    toolsOpen: false,
    add: { open: false, tab: "signin", prefill: "", prefillFor: null, nonce: 0 },
  });
  useToasts.setState({ items: [] });
});

function setup() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(["vault"], { unlocked: true }); // toasts may name groups
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
  within(screen.getByRole("grid")).getByText(name).closest('[role="row"]')!;

test("arrows select, Delete removes with an undo toast, Undo restores", async () => {
  const user = userEvent.setup();
  setup();
  const grid = await screen.findByRole("grid", { name: t("accounts.listLabel") });
  grid.focus();
  await user.keyboard("{ArrowDown}");
  expect(row("Orbit")).toHaveAttribute("aria-selected", "true");
  await user.keyboard("{ArrowDown}");
  expect(row("Orbit")).toHaveAttribute("aria-selected", "false");
  expect(row("Pylon")).toHaveAttribute("aria-selected", "true");
  expect(useUi.getState().sel.ids).toEqual(["acc-3"]);

  await user.keyboard("{Delete}");
  await waitFor(() => expect(screen.queryByText("Pylon")).toBeNull());
  const status = screen.getByRole("status", { name: t("toast.region") });
  expect(await within(status).findByText(t("accounts.removed1"))).toBeInTheDocument();
  await user.click(within(status).getByRole("button", { name: t("accounts.undo") }));
  expect(await screen.findByText("Pylon")).toBeInTheDocument();
});

test("F2 opens More and the alias field keeps focus after the request is consumed", async () => {
  const user = userEvent.setup();
  setup();
  const grid = await screen.findByRole("grid", { name: t("accounts.listLabel") });
  grid.focus();
  await user.keyboard("{ArrowDown}{F2}");
  const alias = await screen.findByLabelText(t("inspector.alias"));
  await waitFor(() => expect(alias).toHaveFocus());
  expect(useUi.getState().focusReq).toBeNull();
  // a later render must not collapse More (it used to depend on focusReq)
  await user.keyboard("x");
  expect(screen.getByLabelText(t("inspector.alias"))).toHaveFocus();
});

test("Ctrl+click on a second group header adds it to the selection", async () => {
  const user = userEvent.setup();
  setup();
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  await user.click(screen.getAllByRole("button", { pressed: false, name: /^Main/ })[0]);
  const main = useUi.getState().sel.ids;
  await user.keyboard("{Control>}");
  await user.click(screen.getAllByRole("button", { pressed: false, name: /^Alts/ })[0]);
  await user.keyboard("{/Control}");
  const both = useUi.getState().sel.ids;
  expect(both.length).toBeGreaterThan(main.length);
  expect(both).toEqual(expect.arrayContaining(main));
});

test("Ctrl+right-click (macOS context menu) on an unselected row replaces the selection", async () => {
  setup();
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  useUi.getState().setSel({ ids: ["acc-1", "acc-2"], anchor: "acc-1", cursor: "acc-1" });
  fireEvent.contextMenu(row("Pylon"), { ctrlKey: true });
  expect(useUi.getState().sel.ids).toEqual(["acc-3"]);
});

test("collapsed groups survive leaving the Accounts screen", async () => {
  const user = userEvent.setup();
  setup();
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  await user.click(
    screen.getAllByRole("button", { name: t("accounts.collapseGroup", { name: "Main" }) })[0],
  );
  cleanup();
  setup();
  expect(
    await screen.findByRole("button", { name: t("accounts.expandGroup", { name: "Main" }) }),
  ).toBeInTheDocument();
});

test("search filters rows and shows a no-match state", async () => {
  const user = userEvent.setup();
  setup();
  const search = await screen.findByRole("searchbox", { name: t("accounts.search") });
  await user.type(search, "kestrel");
  expect(screen.getByText("Kestrel main")).toBeInTheDocument();
  expect(screen.queryByText("Orbit")).toBeNull();
  await user.clear(search);
  await user.type(search, "zzz");
  expect(screen.getByText(t("accounts.noMatch", { q: "zzz" }))).toBeInTheDocument();
});

test("paste cookies shows per-line results and stays open", async () => {
  const user = userEvent.setup();
  setup();
  await user.click(await screen.findByRole("button", { name: t("accounts.add") }));
  await user.click(await screen.findByRole("tab", { name: t("add.tab.cookies") }));
  const box = await screen.findByLabelText(t("add.cookies.body"));
  await user.type(box, "_|WARNING:abc|_XYZ{Enter}{Enter}not a cookie");
  await user.click(
    within(box.closest("[role=tabpanel]") as HTMLElement).getByRole("button", {
      name: t("add.cta"),
    }),
  );
  const results = await screen.findByRole("list", { name: t("add.results") });
  const items = within(results).getAllByRole("listitem");
  expect(items).toHaveLength(2);
  // done lines leave the box (no number); rejected ones are numbered where they now are
  expect(items[0]).not.toHaveTextContent(t("add.res.line", { n: 1 }));
  expect(items[0]).toHaveTextContent(t("add.res.added"));
  expect(items[1]).toHaveTextContent(t("add.res.line", { n: 1 }));
  expect(items[1]).toHaveTextContent("This line is not a Roblox session cookie.");
  expect(screen.getByRole("dialog")).toBeInTheDocument();
  expect(box).toHaveValue("not a cookie"); // rejected lines stay to fix
});

test("sign in passes an opted-in password once, then clears it", async () => {
  const user = userEvent.setup();
  setup();
  await user.click(await screen.findByRole("button", { name: t("accounts.add") }));
  expect(screen.queryByLabelText(t("add.signin.pw"))).toBeNull();
  await user.click(await screen.findByLabelText(t("add.signin.savePw")));
  const pw = screen.getByLabelText(t("add.signin.pw"));
  await user.type(pw, "hunter22");
  await user.click(screen.getByRole("button", { name: t("empty.signIn") }));
  await waitFor(() =>
    expect(mockVault.lastArgs.signin_start).toEqual({
      accountId: null,
      usernameHint: null,
      password: "hunter22",
    }),
  );
  expect(pw).toHaveValue("");
});

test("empty vault shows the empty state", async () => {
  resetAccountsMock(0);
  setup();
  expect(await screen.findByText(t("empty.signIn"))).toBeInTheDocument();
});

test("the kebab menu opens Account tools on its own row, not the stale selection", async () => {
  const user = userEvent.setup();
  setup();
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  await user.click(screen.getByText("Orbit"));
  await user.click(
    within(row("Pylon") as HTMLElement).getByRole("button", { name: t("menu.more") }),
  );
  await user.click(await screen.findByRole("menuitem", { name: t("menu.tools") }));
  const dlg = await screen.findByRole("dialog");
  expect(within(dlg).getByText("@pylon_rbx")).toBeInTheDocument();
  expect(useUi.getState().sel.ids).toEqual(["acc-3"]);
});

test("Open site opens roblox.com home as the selected account", async () => {
  const user = userEvent.setup();
  setup();
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  await user.click(screen.getByText("Orbit"));
  await user.click(await screen.findByRole("button", { name: t("inspector.more") }));
  await user.click(await screen.findByRole("button", { name: t("inspector.openSite") }));
  await waitFor(() => expect(lastSite).toBe("https://www.roblox.com/home"));
});

const openPylonMenu = async (user: ReturnType<typeof userEvent.setup>) => {
  setup();
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  await user.click(
    within(row("Pylon") as HTMLElement).getByRole("button", { name: t("menu.more") }),
  );
  return screen.findByRole("menuitem", { name: t("menu.saveDest") });
};

test("Save current destination is disabled while the Play bar is empty", async () => {
  const { usePlayBar } = await import("@/features/playbar/store");
  usePlayBar.setState({ destination: "", server: "", mode: "any" });
  const item = await openPylonMenu(userEvent.setup());
  expect(item).toHaveAttribute("aria-disabled", "true");
});

test("Save current destination stores the Play bar target on the account", async () => {
  const { usePlayBar } = await import("@/features/playbar/store");
  usePlayBar.setState({
    destination: "https://www.roblox.com/games/606849621/Jailbreak",
    server: "",
    mode: "any",
  });
  const user = userEvent.setup();
  await user.click(await openPylonMenu(user));
  expect(await screen.findByText(t("menu.saveDestDone"))).toBeInTheDocument();
  await waitFor(() =>
    expect(mockVault.lastArgs.accounts_update).toMatchObject({
      patch: { launchTarget: { placeId: 606849621, jobId: null, privateLink: null } },
    }),
  );
});

const LINK = "https://www.roblox.com/games/9?privateServerLinkCode=L";
const destField = () => screen.findByRole("textbox", { name: t("dest.label") });

test("group header selects the group; Save writes the group and all its accounts", async () => {
  const user = userEvent.setup();
  setup();
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  await user.click(screen.getAllByRole("button", { pressed: false, name: /^Main/ })[0]);
  const hint = await screen.findByText(/^Saves to the group and all \d+ accounts\.$/);
  const n = Number(hint.textContent!.match(/\d+/)![0]);
  expect(n).toBeGreaterThan(1);
  expect(useUi.getState().sel.ids).toHaveLength(n);
  await user.type(await destField(), LINK);
  await user.click(screen.getByRole("button", { name: t("dest.save") }));
  await waitFor(() =>
    expect(mockVault.lastArgs.group_target_set).toMatchObject({
      group: "Main",
      target: { privateLink: LINK },
    }),
  );
  await waitFor(() => {
    const u = mockVault.lastArgs.accounts_update as { ids: string[]; patch: unknown };
    expect(u.ids).toHaveLength(n);
    expect(u.patch).toMatchObject({ launchTarget: { privateLink: LINK } });
  });
  expect(await screen.findByText(t("dest.savedGroup", { name: "Main", n }))).toBeInTheDocument();
  // Clear removes it from the group and from every account
  await user.click(await screen.findByRole("button", { name: t("dest.clear") }));
  await waitFor(() =>
    expect(mockVault.lastArgs.group_target_set).toEqual({ group: "Main", target: null }),
  );
  await waitFor(() =>
    expect(mockVault.lastArgs.accounts_update).toMatchObject({ patch: { launchTarget: null } }),
  );
});

test("group save: a failed group write puts the accounts back and says so", async () => {
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "group_target_set" ? Promise.reject(new Error("boom")) : orig(cmd, args as never),
  );
  vi.spyOn(console, "error").mockImplementation(() => {});
  const user = userEvent.setup();
  setup();
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  await user.click(screen.getAllByRole("button", { pressed: false, name: /^Main/ })[0]);
  const targets = async () =>
    (await orig("accounts_list")).accounts
      .filter((a) => a.group === "Main")
      .map((a) => a.launchTarget);
  const before = await targets();
  await user.type(await destField(), LINK);
  await user.click(screen.getByRole("button", { name: t("dest.save") }));
  expect(await screen.findByText(t("dest.saveFailed"))).toBeInTheDocument();
  // accounts were written first, then put back
  expect(mockVault.lastArgs.accounts_update).toBeDefined();
  expect(await targets()).toEqual(before);
  expect(screen.queryByText(/^Saved to Main/)).toBeNull();
  vi.restoreAllMocks();
});

test("an invalid group destination shows an inline error and saves nothing", async () => {
  const user = userEvent.setup();
  setup();
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  await user.click(screen.getAllByRole("button", { pressed: false, name: /^Main/ })[0]);
  delete mockVault.lastArgs.group_target_set;
  await user.type(await destField(), "not a game");
  await user.click(screen.getByRole("button", { name: t("dest.save") }));
  expect(await screen.findByRole("alert")).toHaveTextContent(t("dest.invalid"));
  expect(mockVault.lastArgs.group_target_set).toBeUndefined();
});

test("an account's Save writes that account only; Clear removes it", async () => {
  const user = userEvent.setup();
  setup();
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  await user.click(screen.getByText("Orbit"));
  delete mockVault.lastArgs.group_target_set;
  await user.type(await destField(), "606849621");
  await user.click(screen.getByRole("button", { name: t("dest.save") }));
  await waitFor(() =>
    expect(mockVault.lastArgs.accounts_update).toMatchObject({
      ids: ["acc-0"],
      patch: { launchTarget: { placeId: 606849621, jobId: null, privateLink: null } },
    }),
  );
  expect(mockVault.lastArgs.group_target_set).toBeUndefined();
  await user.click(await screen.findByRole("button", { name: t("dest.clear") }));
  await waitFor(() =>
    expect(mockVault.lastArgs.accounts_update).toMatchObject({
      ids: ["acc-0"],
      patch: { launchTarget: null },
    }),
  );
  await waitFor(async () => expect(await destField()).toHaveValue(""));
});

test("saving an account destination with Enter keeps focus in the field", async () => {
  const user = userEvent.setup();
  setup();
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  await user.click(screen.getByText("Orbit"));
  const field = await destField();
  await user.click(field);
  await user.keyboard("606849621{Enter}");
  await waitFor(() =>
    expect(mockVault.lastArgs.accounts_update).toMatchObject({
      patch: { launchTarget: { placeId: 606849621 } },
    }),
  );
  await screen.findByRole("button", { name: t("dest.clear") }); // saved value rendered
  expect(await destField()).toBe(field); // not remounted
  expect(field).toHaveFocus();
});

test("Save current destination shows no success toast when the save fails", async () => {
  const { usePlayBar } = await import("@/features/playbar/store");
  usePlayBar.setState({ destination: "606849621", server: "", mode: "any" });
  const orig = ipc.call;
  const spy = vi
    .spyOn(ipc, "call")
    .mockImplementation((cmd, args) =>
      cmd === "accounts_update" ? Promise.reject(new Error("boom")) : orig(cmd, args as never),
    );
  vi.spyOn(console, "error").mockImplementation(() => {});
  const user = userEvent.setup();
  await user.click(await openPylonMenu(user));
  await waitFor(() => expect(spy).toHaveBeenCalledWith("accounts_update", expect.anything()));
  await new Promise((r) => setTimeout(r, 50));
  expect(screen.queryByText(t("menu.saveDestDone"))).toBeNull();
  vi.restoreAllMocks();
});

test("server choice: hidden for a private link, Smallest and Specific are saved", async () => {
  const user = userEvent.setup();
  setup();
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  await user.click(screen.getByText("Orbit"));
  const modes = () => screen.queryByRole("group", { name: t("mode.label") });
  await user.type(await destField(), LINK);
  expect(modes()).toBeNull();
  await user.clear(await destField());
  await user.type(await destField(), "606849621");
  await user.click(await screen.findByRole("radio", { name: t("mode.smallest") }));
  await user.click(screen.getByRole("button", { name: t("dest.save") }));
  await waitFor(() =>
    expect(mockVault.lastArgs.accounts_update).toMatchObject({
      patch: { launchTarget: { placeId: 606849621, mode: "smallest" } },
    }),
  );
  const job = "123e4567-e89b-12d3-a456-426614174000";
  await user.click(screen.getByRole("radio", { name: t("mode.specific") }));
  await user.click(screen.getByRole("button", { name: t("dest.save") }));
  expect(await screen.findByRole("alert")).toHaveTextContent(t("play.noServer"));
  await user.type(screen.getByRole("textbox", { name: t("server.label") }), job);
  await user.click(screen.getByRole("button", { name: t("dest.save") }));
  await waitFor(() =>
    expect(mockVault.lastArgs.accounts_update).toMatchObject({
      patch: { launchTarget: { placeId: 606849621, jobId: job } },
    }),
  );
});

test("a saved group destination shows the game name, not Place N", async () => {
  const user = userEvent.setup();
  setup();
  await screen.findByRole("grid", { name: t("accounts.listLabel") });
  await user.click(screen.getAllByRole("button", { pressed: false, name: /^Main/ })[0]);
  await user.type(await destField(), "606849621");
  await user.click(screen.getByRole("button", { name: t("dest.save") }));
  expect((await screen.findAllByText("Jailbreak")).length).toBeGreaterThan(1); // panel + row labels
  expect(screen.queryByText(/^Place \d+/)).toBeNull();
});
