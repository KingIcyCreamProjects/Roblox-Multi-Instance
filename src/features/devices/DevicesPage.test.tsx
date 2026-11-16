import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useUi } from "@/app/store";
import { t } from "@/lib/i18n";
import { ACTIVITY_EVENT } from "@/lib/ipc";
import { mockRemote, mockVault, resetVaultMock } from "@/lib/mock";
import { DevicesPage } from "./DevicesPage";
import { loadViewer } from "./viewer";

vi.mock("./viewer", () => ({ loadViewer: vi.fn() }));

const open = vi.fn();
const close = vi.fn();

beforeEach(() => {
  resetVaultMock();
  open.mockReset().mockReturnValue({ close });
  close.mockReset();
  vi.mocked(loadViewer).mockResolvedValue({ open });
});

const setup = () =>
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <DevicesPage />
    </QueryClientProvider>,
  );

const row = async (name: string) => (await screen.findByText(name)).closest("li") as HTMLElement;
const connectBtn = (name: string) => ({ name: t("dev.connectNamed", { name }) });
const pairBtn = (name: string) => ({ name: t("dev.pairTitle", { name }) });

type Opened = { url: string; auth: string; title: string; onClose: (why?: string) => void };
const opened = async (n = 1) => {
  await waitFor(() => expect(open).toHaveBeenCalledTimes(n));
  return open.mock.calls[n - 1]![0] as Opened;
};

test("lists tailnet computers with their state and the right buttons", async () => {
  setup();
  const den = await row("Den PC");
  expect(den).toHaveTextContent(t("dev.paired"));
  expect(within(den).getByRole("button", connectBtn("Den PC"))).toBeEnabled();
  const laptop = await row("Laptop");
  expect(laptop).toHaveTextContent(t("dev.unpaired"));
  expect(within(laptop).getByRole("button", pairBtn("Laptop"))).toBeInTheDocument();
  // someone is connected to it: shown, still connectable (the viewer can take over)
  expect(await row("Studio")).toHaveTextContent(t("dev.inUse"));
  // phones and Linux can never be controlled: said so, listed last, nothing to press
  for (const name of ["nas", "tablet"]) {
    const r = await row(name);
    expect(r).toHaveTextContent(t("dev.cantHost"));
    expect(within(r).queryByRole("button")).toBeNull();
  }
  const names = screen.getAllByRole("listitem").map((li) => li.querySelector("p")!.textContent);
  expect(names.slice(-3)).toEqual(["nas", "phone", "tablet"]);
  const phone = await row("phone");
  expect(phone).toHaveTextContent(t("dev.offline"));
  expect(within(phone).queryByRole("button")).toBeNull();
});

test("pairs with a code, then offers to connect right away", async () => {
  const user = userEvent.setup();
  setup();
  await user.click(within(await row("Laptop")).getByRole("button", pairBtn("Laptop")));
  const dialog = await screen.findByRole("dialog", {
    name: t("dev.pairTitle", { name: "Laptop" }),
  });
  const pair = within(dialog).getByRole("button", { name: t("dev.pair") });
  expect(pair).toBeDisabled();
  await user.type(within(dialog).getByRole("textbox", { name: t("dev.code") }), "k7mp 4xq2");
  await user.click(pair);
  const done = await screen.findByRole("dialog", {
    name: t("dev.pairedTitle", { name: "Laptop" }),
  });
  expect(mockVault.lastArgs.remote_pair).toEqual({ ip: "100.64.0.12", code: "k7mp 4xq2" });
  await user.click(within(done).getByRole("button", { name: t("dev.connect") }));
  const o = await opened();
  expect(o.url).toBe("ws://100.64.0.12:7813/v1/remote/ws");
  // the sheet's button is gone: focus comes back to the device's own Connect button
  o.onClose();
  await waitFor(() =>
    expect(
      within(screen.getByText("Laptop").closest("li")!).getByRole("button", connectBtn("Laptop")),
    ).toHaveFocus(),
  );
});

test("Done closes the paired sheet; the row can connect", async () => {
  const user = userEvent.setup();
  setup();
  await user.click(within(await row("Laptop")).getByRole("button", pairBtn("Laptop")));
  await user.type(await screen.findByRole("textbox", { name: t("dev.code") }), "K7MP4XQ2");
  await user.click(screen.getByRole("button", { name: t("dev.pair") }));
  await user.click(await screen.findByRole("button", { name: t("dev.done") }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(
    await within(await row("Laptop")).findByRole("button", connectBtn("Laptop")),
  ).toBeInTheDocument();
});

test("a wrong code stays in the dialog with the reason", async () => {
  const user = userEvent.setup();
  mockRemote.pair = "bad";
  setup();
  await user.click(within(await row("Laptop")).getByRole("button", pairBtn("Laptop")));
  const dialog = await screen.findByRole("dialog");
  await user.type(within(dialog).getByRole("textbox"), "WRONG234");
  await user.click(within(dialog).getByRole("button", { name: t("dev.pair") }));
  expect(await within(dialog).findByRole("alert")).toHaveTextContent("Wrong code");
});

test("connect mounts the shared viewer; the token is never in the URL", async () => {
  const user = userEvent.setup();
  setup();
  const btn = within(await row("Den PC")).getByRole("button", connectBtn("Den PC"));
  await user.click(btn);
  const o = await opened();
  expect(o.url).toBe("ws://100.64.0.11:7813/v1/remote/ws");
  expect(o.auth).toMatch(/^[0-9a-f]{64}$/);
  expect(o.url).not.toContain(o.auth);
  expect(o.title).toBe("Den PC");
  const dialog = screen.getByRole("dialog", { name: t("dev.sessionTitle", { name: "Den PC" }) });
  // focus left the Connect button (Enter must not open a second session)
  expect(dialog).toHaveFocus();
  // keys for the other computer never reach the app's own shortcuts, but count as activity
  const onWindow = vi.fn();
  const onActivity = vi.fn();
  window.addEventListener("keydown", onWindow);
  window.addEventListener(ACTIVITY_EVENT, onActivity);
  fireEvent.keyDown(dialog, { key: "l", ctrlKey: true });
  window.removeEventListener("keydown", onWindow);
  window.removeEventListener(ACTIVITY_EVENT, onActivity);
  expect(onWindow).not.toHaveBeenCalled();
  expect(onActivity).toHaveBeenCalledTimes(1);
  // aria-modal: Tab stays inside (nothing focusable here: it goes nowhere)
  fireEvent.keyDown(dialog, { key: "Tab" });
  expect(dialog).toHaveFocus();
  // tabs can't switch away (that would silently end the session)
  expect(useUi.getState().remoteOpen).toBe(true);
  const route = useUi.getState().route;
  useUi.getState().go(route === "home" ? "settings" : "home");
  expect(useUi.getState().route).toBe(route);
  o.onClose();
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(close).toHaveBeenCalled();
  expect(btn).toHaveFocus();
  expect(useUi.getState().remoteOpen).toBe(false);
});

test("a double click opens one session", async () => {
  const user = userEvent.setup();
  setup();
  await user.dblClick(within(await row("Den PC")).getByRole("button", connectBtn("Den PC")));
  await opened();
  await new Promise((r) => setTimeout(r, 50));
  expect(open).toHaveBeenCalledTimes(1);
});

test("pairing removed on the other computer: token dropped, Pair again", async () => {
  const user = userEvent.setup();
  setup();
  await user.click(within(await row("Den PC")).getByRole("button", connectBtn("Den PC")));
  (await opened()).onClose("removed");
  expect(await screen.findByRole("alert")).toHaveTextContent(
    t("dev.removedThere", { name: "Den PC" }),
  );
  expect(mockVault.lastArgs.remote_forget).toEqual({ ip: "100.64.0.11" });
  await user.click(await within(await row("Den PC")).findByRole("button", pairBtn("Den PC")));
  // pairing again clears the old "Pair again" alert
  const dialog = await screen.findByRole("dialog");
  expect(screen.queryByText(t("dev.removedThere", { name: "Den PC" }))).toBeNull();
  await user.type(within(dialog).getByRole("textbox"), "K7MP4XQ2");
  await user.click(within(dialog).getByRole("button", { name: t("dev.pair") }));
  await user.click(await screen.findByRole("button", { name: t("dev.done") }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(screen.queryByRole("alert")).toBeNull();
});

test("a watched session counts as activity for the idle auto-lock", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  try {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    setup();
    await user.click(within(await row("Den PC")).getByRole("button", connectBtn("Den PC")));
    const o = await opened();
    const onActivity = vi.fn();
    window.addEventListener(ACTIVITY_EVENT, onActivity);
    vi.advanceTimersByTime(60_000);
    expect(onActivity).toHaveBeenCalledTimes(3);
    o.onClose();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    vi.advanceTimersByTime(60_000);
    window.removeEventListener(ACTIVITY_EVENT, onActivity);
    expect(onActivity).toHaveBeenCalledTimes(3);
  } finally {
    vi.useRealTimers();
  }
});

test("a build without the viewer says so", async () => {
  const user = userEvent.setup();
  vi.mocked(loadViewer).mockRejectedValue(new Error("viewer missing"));
  setup();
  await user.click(within(await row("Den PC")).getByRole("button", connectBtn("Den PC")));
  expect(await screen.findByRole("alert")).toHaveTextContent(t("dev.viewerMissing"));
  await user.click(screen.getByRole("button", { name: t("sheet.close") }));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("remove asks first, says a new code is needed, then forgets the pairing", async () => {
  const user = userEvent.setup();
  setup();
  const remove = () =>
    user.click(
      screen.getByRole("button", {
        name: t("dev.removeNamed", { name: "Den PC" }),
      }),
    );
  await row("Den PC");
  await remove();
  expect(screen.getByText(t("dev.removeAsk", { name: "Den PC" }))).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("set.cancel") }));
  expect(mockVault.lastArgs.remote_forget).toBeUndefined();
  await remove();
  await user.click(screen.getByRole("button", { name: t("dev.remove") }));
  expect(
    await within(await row("Den PC")).findByRole("button", pairBtn("Den PC")),
  ).toBeInTheDocument();
  expect(mockVault.lastArgs.remote_forget).toEqual({ ip: "100.64.0.11" });
});

test("no other computers: explains what to do", async () => {
  mockRemote.peers = [];
  setup();
  expect(await screen.findByText(t("dev.empty"))).toBeInTheDocument();
  expect(screen.getByText(t("dev.empty.d"))).toBeInTheDocument();
});

test("Tailscale missing, then signed out: the setup steps", async () => {
  mockRemote.ts = "missing";
  const { unmount } = setup();
  expect(await screen.findByText(t("dev.tsIntro"))).toBeInTheDocument();
  expect(screen.getByRole("button", { name: t("fs.download") })).toBeInTheDocument();
  unmount();
  mockRemote.ts = "signedout";
  setup();
  const step2 = (await screen.findByText(t("fs.step2"))).closest("li")!;
  expect(step2).toHaveAttribute("aria-current", "step");
  expect(screen.queryByRole("button", { name: t("fs.download") })).toBeNull();
});
