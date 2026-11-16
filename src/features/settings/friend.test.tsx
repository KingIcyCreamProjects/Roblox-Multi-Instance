import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AccountsScreen } from "@/features/accounts/AccountsScreen";
import { label } from "@/features/accounts/model";
import { t } from "@/lib/i18n";
import * as ipc from "@/lib/ipc";
import type { AccountsList } from "@/lib/ipc";
import { mockCall, resetAccountsMock, seedAccount } from "@/lib/mock";
import { useDraft } from "./draft";
import { FriendExtras } from "./FriendSection";

// jsdom has no layout; give the virtual list a viewport.
beforeAll(() => {
  for (const k of ["clientHeight", "clientWidth", "offsetHeight", "offsetWidth"] as const)
    Object.defineProperty(HTMLElement.prototype, k, { configurable: true, get: () => 800 });
});
beforeEach(() => resetAccountsMock(3));

const wrap = (ui: React.ReactNode) =>
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <Tooltip.Provider>{ui}</Tooltip.Provider>
    </QueryClientProvider>,
  );
const first = async () => ((await mockCall("accounts_list")) as AccountsList).accounts[0]!;

test("friend section lists accounts with switches", async () => {
  const user = userEvent.setup();
  wrap(<FriendExtras />);
  const a = await first();
  const sw = await screen.findByRole("switch", { name: label(a) });
  expect(sw).not.toBeChecked();
  await user.click(sw);
  await waitFor(async () => expect((await first()).friendAllowed).toBe(true));
});

test("copy friend link writes the plain login URL", async () => {
  const user = userEvent.setup();
  const write = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
  await mockCall("settings_set", {
    patch: { friend: { publicUrl: "https://rmi.example.ts.net" } },
  });
  wrap(<FriendExtras />);
  const copy = screen.getByRole("button", { name: t("friend.copy") });
  await waitFor(() => expect(copy).toBeEnabled());
  await user.click(copy);
  await waitFor(() => expect(write).toHaveBeenCalledWith("https://rmi.example.ts.net"));
  expect(await screen.findByText(t("friend.copied"))).toBeInTheDocument();
  expect(screen.queryByText(/revoke/i)).toBeNull();
});

test("copy friend link waits for an unsaved public address", async () => {
  useDraft.getState().setAll({ "friend.publicUrl": "https://new.ts.net" });
  wrap(<FriendExtras />);
  expect(screen.getByRole("button", { name: t("friend.copy") })).toBeDisabled();
  expect(screen.getByText(t("friend.linkUnsaved"))).toBeInTheDocument();
  useDraft.getState().clear();
});

test("copy friend link is off with a hint until a public address is saved", async () => {
  await mockCall("settings_set", { patch: { friend: { publicUrl: "" } } });
  wrap(<FriendExtras />);
  expect(await screen.findByText(t("friend.linkNoUrl"))).toBeInTheDocument();
  expect(screen.getByRole("button", { name: t("friend.copy") })).toBeDisabled();
});

test("password is masked, reveals, copies and rotates", async () => {
  const user = userEvent.setup();
  const write = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
  const copyCalls = vi.spyOn(ipc, "call");
  wrap(<FriendExtras />);
  const pw = (await mockCall("friend_password")) as string;
  expect(document.body.textContent).not.toContain(pw);
  await user.click(screen.getByRole("button", { name: t("friend.showPw") }));
  expect(await screen.findByText(pw)).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("friend.hidePw") }));
  expect(screen.queryByText(pw)).toBeNull();
  // copy goes through Rust (hidden from clipboard history), never the web clipboard
  await user.click(screen.getByRole("button", { name: t("friend.copyPw") }));
  await waitFor(() => expect(copyCalls).toHaveBeenCalledWith("friend_password_copy"));
  expect(await screen.findByText(t("friend.pwCopied"))).toBeInTheDocument();
  expect(write).not.toHaveBeenCalled();
  // signs the friend out, so it asks first
  await user.click(screen.getByRole("button", { name: t("friend.newPw") }));
  expect(screen.getByText(t("friend.newPwAsk"))).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("friend.newPw") }));
  expect(await screen.findByText(t("friend.pwNew"))).toBeInTheDocument();
  const next = (await mockCall("friend_password")) as string;
  expect(next).not.toBe(pw);
  copyCalls.mockRestore();
});

test("turn off friend link removes the Funnel entry, then clears the address", async () => {
  const user = userEvent.setup();
  const calls = vi.spyOn(ipc, "call");
  await mockCall("settings_set", {
    patch: { friend: { publicUrl: "https://rmi.example.ts.net:8443", enabled: true } },
  });
  wrap(<FriendExtras />);
  await user.click(await screen.findByRole("button", { name: t("friend.tsOff") }));
  await waitFor(() =>
    expect(calls).toHaveBeenCalledWith("settings_set", {
      patch: { friend: { publicUrl: "", enabled: false } },
    }),
  );
  const order = calls.mock.calls.map((c) => c[0]);
  expect(order.indexOf("friend_ts_disable")).toBeGreaterThan(-1);
  expect(order.indexOf("friend_ts_disable")).toBeLessThan(order.lastIndexOf("settings_set"));
  const s = (await mockCall("settings_get")) as ipc.Settings;
  expect(s.friend).toMatchObject({ publicUrl: "", enabled: false });
  // no RMI-made address: no button
  expect(screen.queryByRole("button", { name: t("friend.tsOff") })).toBeNull();
  calls.mockRestore();
});

test("own public address: no turn off friend link button", async () => {
  await mockCall("settings_set", {
    patch: { friend: { publicUrl: "https://friend.example.com" } },
  });
  wrap(<FriendExtras />);
  await screen.findByRole("button", { name: t("friend.copy") });
  await waitFor(() => expect(screen.getByRole("button", { name: t("friend.copy") })).toBeEnabled());
  expect(screen.queryByRole("button", { name: t("friend.tsOff") })).toBeNull();
});

test("accounts the friend added say so; the owner's own do not", async () => {
  const a = await first();
  seedAccount(a.id, { friendAdded: true, friendAllowed: true });
  wrap(<FriendExtras />);
  expect(await screen.findAllByText(t("friend.addedBy"))).toHaveLength(1);
  expect(await screen.findByRole("switch", { name: label(a) })).toBeChecked();
});

test("mock refuses backend-only flags like Rust's PATCHABLE", async () => {
  const a = await first();
  for (const k of ["friendAdded", "friendPaused", "friendLastPlace"])
    await expect(mockCall("accounts_update", { ids: [a.id], patch: { [k]: true } })).rejects.toBe(
      `${k} can't be changed.`,
    );
  await mockCall("accounts_update", { ids: [a.id], patch: { friendAllowed: true } });
});

test("paused tag shows on row", async () => {
  const a = await first();
  seedAccount(a.id, { friendPaused: true });
  wrap(<AccountsScreen />);
  expect(await screen.findByText(t("friend.paused"))).toBeInTheDocument();
});

test("select all allows every account, clear all takes it back", async () => {
  const user = userEvent.setup();
  wrap(<FriendExtras />);
  const all = async () => ((await mockCall("accounts_list")) as AccountsList).accounts;
  await user.click(await screen.findByRole("button", { name: t("friend.all") }));
  await waitFor(async () => expect((await all()).every((a) => a.friendAllowed)).toBe(true));
  await user.click(await screen.findByRole("button", { name: t("friend.none") }));
  await waitFor(async () => expect((await all()).some((a) => a.friendAllowed)).toBe(false));
});
