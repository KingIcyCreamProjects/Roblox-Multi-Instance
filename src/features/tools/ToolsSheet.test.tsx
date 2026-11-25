import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useUi } from "@/app/store";
import { emptySel } from "@/features/accounts/model";
import { mockBus, type AccountsList } from "@/lib/ipc";
import * as mock from "@/lib/mock";
import { resetAccountsMock } from "@/lib/mock";
import { t } from "@/lib/i18n";
import { ToolsSheet } from "./ToolsSheet";
import { msgOf } from "./useTool";

afterEach(() => vi.restoreAllMocks());

const callsTo = (spy: { mock: { calls: unknown[][] } }, cmd: string) =>
  spy.mock.calls.filter((c) => c[0] === cmd).map((c) => c[1]);

function setup(ids: string[]) {
  resetAccountsMock(6);
  useUi.setState({
    sel: { ...emptySel, ids },
    toolsOpen: true,
  });
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <Tooltip.Provider>
        <ToolsSheet />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
}

const tab = (k: Parameters<typeof t>[0]) => screen.findByRole("tab", { name: new RegExp(t(k)) });

async function fillEmail(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await tab("tools.email"));
  await user.type(await screen.findByLabelText(t("tools.email.new")), "new@example.com");
  await user.type(screen.getByLabelText(t("tools.email.password")), "hunter2hunter2");
  await user.click(screen.getByRole("button", { name: t("tools.email.cta") }));
  return screen.findByRole("group", { name: t("tools.pin.title") });
}

test("PIN_REQUIRED opens the prompt, unlock retries the same email change once", async () => {
  const spy = vi.spyOn(mock, "mockCall");
  const user = userEvent.setup();
  setup(["acc-0"]);
  await user.click(await tab("tools.email"));
  expect(await screen.findByText("o****@example.com")).toBeInTheDocument();
  const pin = await fillEmail(user);
  await user.type(within(pin).getByLabelText(t("tools.pin.label")), "1234");
  await user.click(within(pin).getByRole("button", { name: t("tools.pin.cta") }));
  expect(await screen.findByText(t("tools.email.ok"))).toBeInTheDocument();
  expect(screen.getByText(/PIN unlocked/)).toBeInTheDocument();
  expect(screen.queryByRole("group", { name: t("tools.pin.title") })).toBeNull();
  const args = { accountId: "acc-0", email: "new@example.com", password: "hunter2hunter2" };
  expect(callsTo(spy, "tool_email_set")).toEqual([args, args]); // rejected once, retried once
  expect(callsTo(spy, "tool_pin_unlock")).toEqual([{ accountId: "acc-0", pin: "1234" }]);
  expect(await screen.findByText("n****@example.com")).toBeInTheDocument(); // masked refreshed
});

test("Esc while an op runs keeps the sheet open and says why; its result still shows", async () => {
  const orig = mock.mockCall;
  let release = () => {};
  vi.spyOn(mock, "mockCall").mockImplementation((cmd, args) =>
    cmd === "tool_email_set"
      ? new Promise((r) => (release = () => r(undefined)))
      : orig(cmd, args as never),
  );
  const user = userEvent.setup();
  setup(["acc-1"]);
  await user.click(await tab("tools.email"));
  await user.type(await screen.findByLabelText(t("tools.email.new")), "new@example.com");
  await user.type(screen.getByLabelText(t("tools.email.password")), "hunter2hunter2");
  await user.click(screen.getByRole("button", { name: t("tools.email.cta") })); // hangs
  await user.keyboard("{Escape}");
  expect(await screen.findByText(t("tools.stillWorking"))).toBeInTheDocument();
  expect(useUi.getState().toolsOpen).toBe(true);
  await act(async () => release());
  expect(await screen.findByText(t("tools.email.ok"))).toBeInTheDocument();
  expect(screen.queryByText(t("tools.stillWorking"))).toBeNull();
});

test("an op still running for another selection does not freeze these tabs", async () => {
  const orig = mock.mockCall;
  vi.spyOn(mock, "mockCall").mockImplementation((cmd, args) =>
    cmd === "tool_email_set" ? new Promise(() => {}) : orig(cmd, args as never),
  );
  const user = userEvent.setup();
  setup(["acc-0"]);
  await user.click(await tab("tools.email"));
  await user.type(await screen.findByLabelText(t("tools.email.new")), "new@example.com");
  await user.type(screen.getByLabelText(t("tools.email.password")), "hunter2hunter2");
  await user.click(screen.getByRole("button", { name: t("tools.email.cta") })); // hangs
  act(() => useUi.setState({ sel: { ...emptySel, ids: ["acc-1"] } }));
  const pw = await tab("tools.password");
  await waitFor(() => expect(pw).not.toHaveAttribute("aria-disabled", "true"));
  await user.click(pw);
  await waitFor(async () =>
    expect(await tab("tools.password")).toHaveAttribute("aria-selected", "true"),
  );
});

test("a short PIN says what to fix and does not call unlock", async () => {
  const spy = vi.spyOn(mock, "mockCall");
  const user = userEvent.setup();
  setup(["acc-0"]);
  const pin = await fillEmail(user);
  await user.type(within(pin).getByLabelText(t("tools.pin.label")), "12");
  await user.click(within(pin).getByRole("button", { name: t("tools.pin.cta") }));
  expect(await within(pin).findByRole("alert")).toHaveTextContent(t("tools.pin.bad"));
  expect(callsTo(spy, "tool_pin_unlock")).toEqual([]);
});

test("wrong PIN stays open with a fix", async () => {
  const user = userEvent.setup();
  setup(["acc-0"]);
  const pin = await fillEmail(user);
  await user.type(within(pin).getByLabelText(t("tools.pin.label")), "9999");
  await user.click(within(pin).getByRole("button", { name: t("tools.pin.cta") }));
  expect(await within(pin).findByRole("alert")).toHaveTextContent(/Wrong PIN/);
});

test("batch: per-account results, one failure; single-only tools disabled with reason", async () => {
  const user = userEvent.setup();
  setup(["acc-0", "acc-1", "acc-4"]);
  const pw = await tab("tools.password");
  expect(pw).toHaveAttribute("aria-disabled", "true");
  expect(screen.getByText(t("tools.singleOnly"))).toBeInTheDocument();
  await user.type(await screen.findByLabelText(t("tools.dn.label")), "Fresh Name");
  await user.click(screen.getByRole("button", { name: t("tools.dn.cta") }));
  const list = await screen.findByRole("list", { name: t("tools.results") });
  await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(3));
  expect(within(list).getAllByLabelText(t("tools.failed"))).toHaveLength(1);
  expect(within(list).getByText(/changed too recently/)).toBeInTheDocument();
  expect(within(list).getAllByLabelText(t("tools.ok"))).toHaveLength(2);
});

test("password fields clear after success, stay after failure", async () => {
  const user = userEvent.setup();
  setup(["acc-0"]);
  await user.click(await tab("tools.password"));
  const cur = await screen.findByLabelText(t("tools.pw.current"));
  const next = screen.getByLabelText(t("tools.pw.new"));
  const again = screen.getByLabelText(t("tools.pw.confirm"));
  await user.type(cur, "wrong");
  await user.type(next, "brand-new-pass");
  await user.type(again, "brand-new-pass");
  await user.click(screen.getByRole("button", { name: t("tools.pw.cta") }));
  expect(await screen.findByRole("alert")).toHaveTextContent(/incorrect/);
  expect(cur).toHaveValue("wrong");
  await user.clear(cur);
  await user.type(cur, "old-password");
  await user.click(screen.getByRole("button", { name: t("tools.pw.cta") }));
  expect(await screen.findByText(t("tools.pw.ok"))).toBeInTheDocument();
  expect(cur).toHaveValue("");
  expect(next).toHaveValue("");
  expect(again).toHaveValue("");
});

test("batch: PIN_REQUIRED is an inline error, never a hanging prompt", async () => {
  const user = userEvent.setup();
  setup(["acc-0", "acc-1", "acc-4"]);
  await user.click(await tab("tools.friends"));
  await user.type(await screen.findByLabelText(t("tools.user.label")), "pin_needed");
  await user.click(screen.getByRole("button", { name: t("tools.fr.cta") }));
  const list = await screen.findByRole("list", { name: t("tools.results") });
  await waitFor(() => expect(within(list).getAllByText(t("tools.pin.batch"))).toHaveLength(3));
  expect(screen.queryByRole("group", { name: t("tools.pin.title") })).toBeNull();
  expect(screen.getByRole("button", { name: t("tools.fr.cta") })).toBeEnabled();
});

test("single account: a PIN_REQUIRED batch row opens the prompt and retries", async () => {
  const spy = vi.spyOn(mock, "mockCall");
  const user = userEvent.setup();
  setup(["acc-0"]);
  await user.click(await tab("tools.friends"));
  await user.type(await screen.findByLabelText(t("tools.user.label")), "pin_needed");
  await user.click(screen.getByRole("button", { name: t("tools.fr.cta") }));
  const pin = await screen.findByRole("group", { name: t("tools.pin.title") });
  await user.type(within(pin).getByLabelText(t("tools.pin.label")), "1234");
  await user.click(within(pin).getByRole("button", { name: t("tools.pin.cta") }));
  const list = await screen.findByRole("list", { name: t("tools.results") });
  expect(await within(list).findByLabelText(t("tools.ok"))).toBeInTheDocument();
  expect(callsTo(spy, "tool_friend_request")).toHaveLength(2);
});

test("Cancel while the unlock is in flight never re-runs the op", async () => {
  const orig = mock.mockCall;
  let release = () => {};
  const spy = vi
    .spyOn(mock, "mockCall")
    .mockImplementation((cmd, args) =>
      cmd === "tool_pin_unlock"
        ? new Promise((res) => (release = () => res(orig(cmd, args))))
        : orig(cmd, args),
    );
  const user = userEvent.setup();
  setup(["acc-0"]);
  const pin = await fillEmail(user);
  await user.type(within(pin).getByLabelText(t("tools.pin.label")), "1234");
  await user.click(within(pin).getByRole("button", { name: t("tools.pin.cta") }));
  await user.click(within(pin).getByRole("button", { name: t("tools.cancel") }));
  expect(await screen.findByText(t("tools.pin.cancelled"))).toBeInTheDocument();
  await act(async () => release());
  await waitFor(() => expect(callsTo(spy, "tool_pin_unlock")).toHaveLength(1));
  await new Promise((r) => setTimeout(r, 50));
  expect(callsTo(spy, "tool_email_set")).toHaveLength(1);
});

test("batch events from another batchId are ignored", async () => {
  const user = userEvent.setup();
  setup(["acc-0", "acc-1", "acc-4"]);
  await user.type(await screen.findByLabelText(t("tools.dn.label")), "Fresh Name");
  await user.click(screen.getByRole("button", { name: t("tools.dn.cta") }));
  act(() => {
    mockBus.dispatchEvent(
      new CustomEvent("tools://batch", {
        detail: {
          batchId: "someone-else",
          result: { accountId: "acc-5", ok: true, message: "intruder" },
          done: 1,
          total: 1,
        },
      }),
    );
  });
  expect(screen.queryByText("intruder")).toBeNull();
  const list = await screen.findByRole("list", { name: t("tools.results") });
  await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(3));
  expect(screen.queryByText("intruder")).toBeNull();
});

test("a SESSION_EXPIRED row offers Sign in again for that account", async () => {
  const spy = vi.spyOn(mock, "mockCall");
  const user = userEvent.setup();
  setup(["acc-0", "acc-1", "acc-3"]);
  await user.click(await tab("tools.friends"));
  await user.type(await screen.findByLabelText(t("tools.user.label")), "some_friend");
  await user.click(screen.getByRole("button", { name: t("tools.fr.cta") }));
  const list = await screen.findByRole("list", { name: t("tools.results") });
  await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(3));
  const btns = within(list).getAllByRole("button", { name: t("tools.signInAgain") });
  expect(btns).toHaveLength(1);
  expect(btns[0].closest("li")).toHaveTextContent("Pylon");
  await user.click(btns[0]);
  expect(callsTo(spy, "signin_start")[0]).toMatchObject({ accountId: "acc-3" });
});

test("expired account: every tool is disabled with a reason, but focusable", async () => {
  setup(["acc-2"]);
  const email = await tab("tools.email");
  expect(email).toHaveAttribute("aria-disabled", "true");
  expect(email).toHaveTextContent(t("tools.signFirst"));
  email.focus();
  expect(email).toHaveFocus();
  expect(screen.getByRole("button", { name: t("tools.signInAgain") })).toBeInTheDocument();
});

test("an 'unknown' session (no verdict yet) still gets the tools", async () => {
  const orig = mock.mockCall;
  vi.spyOn(mock, "mockCall").mockImplementation(async (cmd, args) => {
    const r = await orig(cmd, args);
    if (cmd !== "accounts_list") return r;
    const list = r as AccountsList;
    return {
      ...list,
      accounts: list.accounts.map((a) => (a.id === "acc-1" ? { ...a, session: "unknown" } : a)),
    };
  });
  setup(["acc-1"]);
  expect(await tab("tools.email")).not.toHaveAttribute("aria-disabled", "true");
  expect(screen.queryByRole("button", { name: t("tools.signInAgain") })).toBeNull();
});

test("pending account (no user id) is disabled too", async () => {
  setup(["acc-0", "acc-5"]);
  expect(await tab("tools.displayName")).toHaveAttribute("aria-disabled", "true");
  expect(screen.queryByRole("button", { name: t("tools.dn.cta") })).toBeNull();
  expect(screen.queryByLabelText(t("tools.dn.label"))).toBeNull();
  expect(screen.getByRole("button", { name: t("tools.signInAgain") })).toBeInTheDocument();
});

test("tool errors read as catalog text: codes mapped, wrapper cut, blank falls back", () => {
  expect(msgOf("tool_x: SESSION_EXPIRED")).toBe(t("tools.sessionExpired"));
  expect(msgOf(new Error("tool_pin_unlock: PIN_REQUIRED"))).toBe(t("tools.pin.batch"));
  expect(msgOf("tool_email_set: Password is incorrect.")).toBe("Password is incorrect.");
  expect(msgOf("")).toBe(t("tools.unexpected"));
});

test("password form names short, same-as-current and mismatch; submit waits for all clear", async () => {
  const user = userEvent.setup();
  setup(["acc-0"]);
  await user.click(await tab("tools.password"));
  const cur = await screen.findByLabelText(t("tools.pw.current"));
  const next = screen.getByLabelText(t("tools.pw.new"));
  const again = screen.getByLabelText(t("tools.pw.confirm"));
  const go = screen.getByRole("button", { name: t("tools.pw.cta") });
  await user.type(cur, "old-password");
  await user.type(next, "short");
  expect(screen.getByText(t("tools.pw.short"))).toBeInTheDocument();
  await user.clear(next);
  await user.type(next, "old-password");
  expect(screen.getByText(t("tools.pw.same"))).toBeInTheDocument();
  await user.clear(next);
  await user.type(next, "brand-new-pass");
  await user.type(again, "brand-new-pasS");
  expect(screen.getByText(t("tools.pw.mismatch"))).toBeInTheDocument();
  expect(go).toBeDisabled();
  await user.clear(again);
  await user.type(again, "brand-new-pass");
  expect(go).toBeEnabled();
});

test("email form names a malformed address and blocks submit", async () => {
  const user = userEvent.setup();
  setup(["acc-0"]);
  await user.click(await tab("tools.email"));
  await user.type(await screen.findByLabelText(t("tools.email.new")), "name@example");
  await user.type(screen.getByLabelText(t("tools.email.password")), "pw");
  expect(screen.getByText(t("tools.email.bad"))).toBeInTheDocument();
  expect(screen.getByRole("button", { name: t("tools.email.cta") })).toBeDisabled();
});

test("display name must be 3 to 20 characters before submit enables", async () => {
  const user = userEvent.setup();
  setup(["acc-0"]);
  const name = await screen.findByLabelText(t("tools.dn.label"));
  const go = screen.getByRole("button", { name: t("tools.dn.cta") });
  await user.type(name, "ab");
  expect(screen.getByText(t("tools.dn.bad"))).toBeInTheDocument();
  expect(go).toBeDisabled();
  await user.type(name, "cdefghijklmnopqrstuvw"); // 23 characters
  expect(screen.getByText(t("tools.dn.bad"))).toBeInTheDocument();
  expect(go).toBeDisabled();
  await user.clear(name);
  await user.type(name, "Fresh Name");
  expect(go).toBeEnabled();
});

test("group field takes an id or a groups/communities link, names anything else", async () => {
  const user = userEvent.setup();
  const spy = vi.spyOn(mock, "mockCall");
  setup(["acc-0"]);
  await user.click(await tab("tools.groups"));
  const g = await screen.findByLabelText(t("tools.gr.label"));
  const go = screen.getByRole("button", { name: new RegExp(t("tools.gr.cta")) });
  await user.type(g, "hello");
  expect(screen.getByText(t("tools.gr.bad"))).toBeInTheDocument();
  expect(go).toBeDisabled();
  await user.clear(g);
  await user.type(g, "https://www.roblox.com/communities/4567/Some-Group");
  expect(go).toBeEnabled();
  await user.click(go);
  expect(callsTo(spy, "account_open_site")).toEqual([
    { accountId: "acc-0", url: "https://www.roblox.com/groups/4567" },
  ]);
  spy.mockRestore();
});

test("unblock all reports the failed count and that more remain", async () => {
  const user = userEvent.setup();
  const orig = mock.mockCall;
  const spy = vi
    .spyOn(mock, "mockCall")
    .mockImplementation((cmd, args) =>
      cmd === "tool_unblock_all"
        ? Promise.resolve({ count: 4, failed: 2, truncated: true })
        : orig(cmd, args),
    );
  setup(["acc-0"]);
  await user.click(await tab("tools.blocks"));
  await user.click(await screen.findByRole("button", { name: t("tools.bl.all") }));
  const group = screen.getByRole("group", { name: t("tools.bl.all") });
  await user.click(within(group).getByRole("button", { name: t("tools.bl.all") }));
  expect(await screen.findByText(t("tools.bl.allDone", { n: 4 }))).toBeInTheDocument();
  expect(screen.getByText(t("tools.bl.allFailed", { n: 2 }))).toBeInTheDocument();
  expect(screen.getByText(t("tools.bl.allMore"))).toBeInTheDocument();
  spy.mockRestore();
});

test("a cancelled export save says nothing was saved", async () => {
  const user = userEvent.setup();
  const orig = mock.mockCall;
  const spy = vi
    .spyOn(mock, "mockCall")
    .mockImplementation((cmd, args) =>
      cmd === "tool_export_summary" ? Promise.resolve(null) : orig(cmd, args),
    );
  setup(["acc-0"]);
  await user.click(await tab("tools.info"));
  await user.click(await screen.findByRole("button", { name: t("tools.info.export") }));
  expect(await screen.findByText(t("tools.info.exportCancelled"))).toBeInTheDocument();
  spy.mockRestore();
});

test("privacy with nothing changed keeps Save off and sends no call", async () => {
  const spy = vi.spyOn(mock, "mockCall");
  const user = userEvent.setup();
  setup(["acc-0"]);
  await user.click(await tab("tools.privacy"));
  const save = await screen.findByRole("button", { name: t("tools.priv.cta") });
  expect(save).toBeDisabled();
  await user.click(save);
  expect(callsTo(spy, "tool_privacy_set")).toHaveLength(0);
});

test("sign out everywhere else asks first; Cancel sends nothing, Yes sends once", async () => {
  const spy = vi.spyOn(mock, "mockCall");
  const user = userEvent.setup();
  setup(["acc-0"]);
  await user.click(await tab("tools.sessions"));
  await user.click(screen.getByRole("button", { name: t("tools.sess.cta") }));
  await user.click(screen.getByRole("button", { name: t("tools.cancel") }));
  expect(callsTo(spy, "tool_signout_others")).toHaveLength(0);
  await user.click(screen.getByRole("button", { name: t("tools.sess.cta") }));
  await user.click(screen.getByRole("button", { name: t("tools.sess.yes") }));
  expect(await screen.findByRole("status")).toHaveTextContent(t("tools.sess.ok"));
  expect(callsTo(spy, "tool_signout_others")).toEqual([{ accountId: "acc-0" }]);
});
