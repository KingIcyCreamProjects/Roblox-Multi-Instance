import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { openExternal } from "@/lib/events";
import { t } from "@/lib/i18n";
import * as ipc from "@/lib/ipc";
import { mockCall, mockCaps, mockTs, resetVaultMock } from "@/lib/mock";
import { FriendGate } from "./FriendSection";

vi.mock("@/lib/events", async (orig) => ({
  ...(await orig<typeof import("@/lib/events")>()),
  openExternal: vi.fn(),
}));

beforeEach(() => {
  resetVaultMock();
  vi.mocked(openExternal).mockClear();
});

const setup = () =>
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <FriendGate>
        <p>normal section</p>
      </FriendGate>
    </QueryClientProvider>,
  );
const ready = () =>
  Object.assign(mockTs.status, { installed: true, running: true, dnsName: "rmi.example.ts.net" });
const step = (title: string) => screen.getByRole("heading", { name: title }).closest("li")!;
const enable = () => screen.getByRole("button", { name: t("fs.enable") });

test("not installed: step 1 is current with the download button, step 3 is disabled", async () => {
  const user = userEvent.setup();
  setup();
  expect(await screen.findByRole("button", { name: t("fs.download") })).toBeInTheDocument();
  expect(step(t("fs.step1"))).toHaveAttribute("data-state", "current");
  expect(step(t("fs.step2"))).toHaveAttribute("data-state", "upcoming");
  expect(step(t("fs.step3"))).toHaveAttribute("data-state", "upcoming");
  expect(enable()).toBeDisabled();
  await user.click(screen.getByRole("button", { name: t("fs.download") }));
  expect(openExternal).toHaveBeenCalledWith("https://tailscale.com/download");
  expect(screen.queryByText("normal section")).toBeNull();
});

test("installed but signed out: step 1 done, step 2 current, enable still disabled", async () => {
  Object.assign(mockTs.status, { installed: true });
  setup();
  await waitFor(() => expect(step(t("fs.step1"))).toHaveAttribute("data-state", "done"));
  expect(step(t("fs.step2"))).toHaveAttribute("data-state", "current");
  expect(screen.queryByRole("button", { name: t("fs.download") })).toBeNull();
  expect(enable()).toBeDisabled();
});

test("the sign-in step names the tray only on Windows", async () => {
  setup();
  expect(await screen.findByText(t("fs.step2.win"))).toBeInTheDocument();
  expect(screen.queryByText(t("fs.step2.mac"))).toBeNull();
});

test("on macOS the sign-in step says menu bar, not tray", async () => {
  mockCaps.os = "macos";
  setup();
  expect(await screen.findByText(t("fs.step2.mac"))).toBeInTheDocument();
  expect(screen.queryByText(t("fs.step2.win"))).toBeNull();
});

test("on Linux the sign-in step says tailscale up", async () => {
  mockCaps.os = "linux";
  setup();
  expect(await screen.findByText(t("fs.step2.linux"))).toBeInTheDocument();
  expect(screen.getByText(t("fs.step2.linux"))).toHaveTextContent("tailscale up");
  mockCaps.os = "windows";
});

test("each step says its state in words and the download is a direct button", async () => {
  Object.assign(mockTs.status, { installed: true });
  setup();
  await waitFor(() => expect(step(t("fs.step1"))).toHaveAttribute("data-state", "done"));
  expect(step(t("fs.step1"))).toHaveTextContent(t("fs.tag.done"));
  expect(step(t("fs.step2"))).toHaveTextContent(t("fs.tag.current"));
  expect(step(t("fs.step2"))).toHaveTextContent(t("fs.step2.stuck"));
  expect(step(t("fs.step3"))).toHaveTextContent(t("fs.tag.upcoming"));
});

test("ready: enabling saves the address, hides setup and shows the section", async () => {
  const user = userEvent.setup();
  ready();
  setup();
  await waitFor(() => expect(enable()).toBeEnabled());
  expect(step(t("fs.step3"))).toHaveAttribute("data-state", "current");
  await user.click(enable());
  expect(await screen.findByText("normal section")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: t("fs.enable") })).toBeNull();
  const s = (await mockCall("settings_get")) as { friend: { publicUrl: string; enabled: boolean } };
  expect(s.friend).toMatchObject({ publicUrl: "https://rmi.example.ts.net", enabled: true });
});

test("needs_admin shows the admin step, opens the link, and Try again retries", async () => {
  const user = userEvent.setup();
  ready();
  mockTs.enable = "needs_admin";
  setup();
  await waitFor(() => expect(enable()).toBeEnabled());
  await user.click(enable());
  expect(await screen.findByText(t("fs.admin"))).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("fs.admin.open") }));
  expect(openExternal).toHaveBeenCalledWith("https://login.tailscale.com/f/funnel?node=nEXAMPLE");
  mockTs.enable = "ok";
  await user.click(screen.getByRole("button", { name: t("fs.retry") }));
  expect(await screen.findByText("normal section")).toBeInTheDocument();
});

test("a failure shows the message from Tailscale, not a raw error", async () => {
  const user = userEvent.setup();
  ready();
  mockTs.enable = "failed";
  setup();
  await waitFor(() => expect(enable()).toBeEnabled());
  await user.click(enable());
  expect(await screen.findByRole("alert")).toHaveTextContent("Tailscale could not turn on Funnel.");
  expect(enable()).toBeEnabled();
});

test("I have my own address reveals the normal section", async () => {
  const user = userEvent.setup();
  setup();
  await user.click(await screen.findByRole("button", { name: t("fs.own") }));
  expect(screen.getByText("normal section")).toBeInTheDocument();
});

test("an install with a saved public address never sees setup", async () => {
  await mockCall("settings_set", {
    patch: { friend: { publicUrl: "https://rmi.example.ts.net" } },
  });
  const spy = vi.spyOn(ipc, "call");
  setup();
  expect(await screen.findByText("normal section")).toBeInTheDocument();
  expect(screen.queryByText(t("fs.intro"))).toBeNull();
  expect(spy.mock.calls.filter(([c]) => c === "friend_ts_status")).toHaveLength(0);
  spy.mockRestore();
});

test("status is polled every 3 s while the setup is shown, and stops after", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  try {
    const spy = vi.spyOn(ipc, "call");
    const count = () => spy.mock.calls.filter(([c]) => c === "friend_ts_status").length;
    const { unmount } = setup();
    await screen.findByText(t("fs.intro"));
    await waitFor(() => expect(count()).toBe(1));
    await act(() => vi.advanceTimersByTimeAsync(3100));
    expect(count()).toBe(2);
    unmount();
    await act(() => vi.advanceTimersByTimeAsync(9000));
    expect(count()).toBe(2);
    spy.mockRestore();
  } finally {
    vi.useRealTimers();
  }
});
