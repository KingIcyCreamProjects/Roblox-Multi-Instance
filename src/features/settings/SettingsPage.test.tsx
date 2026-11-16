import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import * as ipc from "@/lib/ipc";
import { mockCall, mockCaps, mockRoblox, mockUpdate, mockVault, resetVaultMock } from "@/lib/mock";
import { t, type MsgKey } from "@/lib/i18n";
import { useUi } from "@/app/store";
import { GROUPS, SECTIONS } from "./catalog";
import { useDraft } from "./draft";
import { SettingsPage } from "./SettingsPage";

function setup() {
  return render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <Tooltip.Provider>
        <SettingsPage />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
}
/** Open the group that holds this subsection. */
const tab = (user: ReturnType<typeof userEvent.setup>, id: string) =>
  user.click(
    screen.getByRole("button", {
      name: t(`grp.${GROUPS.find((g) => g.subs.includes(id as never))!.id}` as MsgKey),
    }),
  );

const scrollTo = vi.fn();
const scrollIntoView = vi.fn();
beforeEach(() => {
  resetVaultMock();
  useDraft.getState().clear();
  Element.prototype.scrollTo = scrollTo as never;
  Element.prototype.scrollIntoView = scrollIntoView;
  scrollTo.mockClear();
  scrollIntoView.mockClear();
});

test("every catalog item has a label and description", () => {
  for (const it of SECTIONS.flatMap((s) => s.items)) {
    expect(t(`set.${it.path}` as MsgKey), it.path).toBeTruthy();
    expect(t(`set.${it.path}.d` as MsgKey), it.path).toBeTruthy();
  }
  for (const s of SECTIONS) expect(t(`set.section.${s.id}.d` as MsgKey), s.id).toBeTruthy();
});

const saved = async () =>
  (await mockCall("settings_get")) as {
    launch: { replacePrevious: boolean; delayMs: number };
    tray: { enabled: boolean };
  };

test("a toggle is a draft until Save sends one settings_set", async () => {
  const user = userEvent.setup();
  setup();
  const sw = await screen.findByRole("switch", { name: t("set.launch.replacePrevious") });
  const save = screen.getByRole("button", { name: t("settings.save") });
  expect(save).toBeDisabled();
  expect(screen.getByText(t("settings.saved"))).toBeInTheDocument();
  await user.click(sw);
  expect(sw).not.toBeChecked();
  expect(screen.getByText(t("settings.unsaved1"))).toBeInTheDocument();
  expect((await saved()).launch.replacePrevious).toBe(true);
  await user.click(await screen.findByRole("switch", { name: t("set.tray.enabled") }));
  expect(screen.getByText(t("settings.unsavedN", { n: 2 }))).toBeInTheDocument();
  await user.click(save);
  await vi.waitFor(async () => {
    const s = await saved();
    expect(s.launch.replacePrevious).toBe(false);
    expect(s.tray.enabled).toBe(false);
  });
  await screen.findByText(t("settings.saved"));
});

test("Discard drops the draft; flipping back clears the count", async () => {
  const user = userEvent.setup();
  setup();
  const sw = await screen.findByRole("switch", { name: t("set.launch.replacePrevious") });
  await user.click(sw);
  await user.click(screen.getByRole("button", { name: t("settings.discard") }));
  expect(sw).toBeChecked();
  expect(screen.getByText(t("settings.saved"))).toBeInTheDocument();
  await user.click(sw);
  await user.click(sw);
  expect(screen.getByText(t("settings.saved"))).toBeInTheDocument();
  expect((await saved()).launch.replacePrevious).toBe(true);
});

test("reset to defaults asks first, then drafts the defaults for Save", async () => {
  const user = userEvent.setup();
  await mockCall("settings_set", {
    patch: { tray: { enabled: false }, launch: { delayMs: 9000 } },
  });
  setup();
  await screen.findByRole("switch", { name: t("set.tray.enabled") });
  await user.click(screen.getByRole("button", { name: t("settings.resetAll") }));
  expect(screen.getByText(t("settings.resetAsk"))).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("set.cancel") }));
  expect(screen.getByText(t("settings.saved"))).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("settings.resetAll") }));
  await user.click(screen.getByRole("button", { name: t("settings.resetYes") }));
  expect(screen.getByText(t("settings.unsavedN", { n: 2 }))).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("settings.save") }));
  await vi.waitFor(async () => {
    const s = await saved();
    expect(s.tray.enabled).toBe(true);
    expect(s.launch.delayMs).toBe(2500);
  });
});

test("search groups hits by group name; a click jumps to the setting", async () => {
  const user = userEvent.setup();
  setup();
  await screen.findByRole("switch", { name: t("set.tray.enabled") });
  await user.type(screen.getByRole("searchbox", { name: t("settings.search") }), "pictures");
  expect(screen.getByRole("heading", { name: t("grp.security") })).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: new RegExp(t("set.images.enabled")) }));
  expect(await screen.findByRole("switch", { name: t("set.images.enabled") })).toBeInTheDocument();
  expect(screen.getByRole("searchbox")).toHaveValue("");
  expect(scrollIntoView).toHaveBeenCalled();
  await user.type(screen.getByRole("searchbox"), "zzzz");
  expect(screen.getByText(t("settings.noMatch"))).toBeInTheDocument();
});

test("Enter in search jumps to the first hit", async () => {
  const user = userEvent.setup();
  setup();
  await screen.findByRole("switch", { name: t("set.tray.enabled") });
  await user.type(screen.getByRole("searchbox"), "lock on sleep{Enter}");
  expect(
    await screen.findByRole("switch", { name: t("set.vault.lockOnSleep") }),
  ).toBeInTheDocument();
  expect(scrollIntoView).toHaveBeenCalled();
});

test("subsection click scrolls; scroll-spy marks the block in view", async () => {
  const user = userEvent.setup();
  setup();
  await screen.findByRole("switch", { name: t("set.tray.enabled") });
  const sub = (id: string) => screen.getByRole("button", { name: t(`sec.${id}` as MsgKey) });
  expect(sub("general")).toHaveAttribute("aria-current", "location");
  await user.click(sub("appearance"));
  expect(scrollTo).toHaveBeenCalledWith(expect.objectContaining({ behavior: "smooth" }));
  expect(sub("appearance")).toHaveAttribute("aria-current", "location");
  // jsdom has no layout: fake block positions, then scroll
  const blocks = [...document.querySelectorAll<HTMLElement>("[data-sub]")];
  const tops = [-400, 10];
  blocks.forEach((b, i) => (b.getBoundingClientRect = () => ({ top: tops[i] }) as DOMRect));
  const sc = blocks[0]!.parentElement!.parentElement!;
  sc.getBoundingClientRect = () => ({ top: 0 }) as DOMRect;
  Object.defineProperty(sc, "scrollHeight", { value: 5000 });
  fireEvent.scroll(sc);
  expect(sub("appearance")).toHaveAttribute("aria-current", "location");
  tops[1] = 500;
  fireEvent.scroll(sc);
  expect(sub("general")).toHaveAttribute("aria-current", "location");
  expect(sub("appearance")).not.toHaveAttribute("aria-current");
});

// tauri-plugin-dialog makes window.confirm async (always truthy), so no prompt: the draft just stays
test("leaving with unsaved changes keeps the draft for the next visit", async () => {
  const user = userEvent.setup();
  setup();
  await user.click(await screen.findByRole("switch", { name: t("set.tray.enabled") }));
  useUi.setState({ route: "settings" });
  const confirm = vi.spyOn(window, "confirm");
  useUi.getState().go("home");
  expect(useUi.getState().route).toBe("home");
  expect(confirm).not.toHaveBeenCalled();
  expect(Object.keys(useDraft.getState().edits)).toEqual(["tray.enabled"]);
  confirm.mockRestore();
});

test("plaintext export needs the exact phrase", async () => {
  const user = userEvent.setup();
  await mockCall("vault_create", { mode: "os", password: null });
  setup();
  await tab(user, "vault");
  const btn = await screen.findByRole("button", { name: t("vs.exportPlainBtn") });
  expect(btn).toBeDisabled();
  const field = screen.getByLabelText(t("vs.typePhrase"));
  await user.type(field, "export plain");
  expect(btn).toBeDisabled();
  await user.type(field, "text");
  expect(btn).toBeEnabled();
});

test("change password mismatch shows an error", async () => {
  const user = userEvent.setup();
  await mockCall("vault_create", { mode: "password", password: "correct horse" });
  setup();
  await tab(user, "vault");
  await user.type((await screen.findAllByLabelText(t("vs.current")))[0]!, "correct horse");
  await user.type(screen.getByLabelText(t("vs.new")), "another horse");
  await user.type(screen.getByLabelText(t("vs.confirmNew")), "different one");
  await user.click(screen.getByRole("button", { name: t("vs.changeBtn") }));
  const alert = await screen.findByText(t("vault.err.mismatch"));
  expect(within(alert.parentElement!).getByLabelText(t("vs.confirmNew"))).toBeInTheDocument();
});

test("successful change password sends new password and clears fields", async () => {
  const user = userEvent.setup();
  await mockCall("vault_create", { mode: "password", password: "correct horse" });
  setup();
  await tab(user, "vault");
  const cur = (await screen.findAllByLabelText(t("vs.current")))[0]!;
  await user.type(cur, "correct horse");
  await user.type(screen.getByLabelText(t("vs.new")), "another horse");
  await user.type(screen.getByLabelText(t("vs.confirmNew")), "another horse");
  await user.click(screen.getByRole("button", { name: t("vs.changeBtn") }));
  expect(await screen.findByText(t("vs.changed"))).toBeInTheDocument();
  expect(cur).toHaveValue("");
  expect(screen.getByLabelText(t("vs.new"))).toHaveValue("");
  await mockCall("vault_lock");
  await expect(mockCall("vault_unlock", { password: "correct horse" })).rejects.toBeTruthy();
  await mockCall("vault_unlock", { password: "another horse" });
});

test("restore asks first; cancel backs out; confirm swaps payload and stays unlocked", async () => {
  const user = userEvent.setup();
  await mockCall("vault_create", { mode: "os", password: null });
  setup();
  await tab(user, "vault");
  await user.click(await screen.findByRole("button", { name: new RegExp(t("vs.restore")) }));
  // an OS-protected vault has no password, so the prompt doesn't mention one
  expect(screen.getByText(t("vs.restore.askOs"))).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("vs.restore.no") }));
  expect(screen.queryByText(t("vs.restore.askOs"))).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: new RegExp(t("vs.restore")) }));
  await user.click(screen.getByRole("button", { name: t("vs.restore.yes") }));
  expect(await screen.findByText(t("vs.restored"))).toBeInTheDocument();
  expect(((await mockCall("vault_status")) as { unlocked: boolean }).unlocked).toBe(true);
  expect(mockVault.lastArgs.vault_restore).toEqual({
    name: "vault-20260101-120000.rmiv",
    password: null,
  });
});

test("change password with a wrong current password shows the catalog error", async () => {
  const user = userEvent.setup();
  await mockCall("vault_create", { mode: "password", password: "correct horse" });
  setup();
  await tab(user, "vault");
  await user.type((await screen.findAllByLabelText(t("vs.current")))[0]!, "not it at all");
  await user.type(screen.getByLabelText(t("vs.new")), "another horse");
  await user.type(screen.getByLabelText(t("vs.confirmNew")), "another horse");
  await user.click(screen.getByRole("button", { name: t("vs.changeBtn") }));
  expect(await screen.findByText(t("vault.err.wrong"))).toBeInTheDocument();
});

test("encrypted export asks for the current password and shows failures", async () => {
  const user = userEvent.setup();
  await mockCall("vault_create", { mode: "password", password: "correct horse" });
  setup();
  await tab(user, "vault");
  const btn = await screen.findByRole("button", { name: t("vs.exportEncBtn") });
  const f = within(btn.closest("form")!);
  await user.type(f.getByLabelText(t("vs.current")), "wrong one");
  await user.type(f.getByLabelText(t("vs.exportPassword")), "export pass");
  await user.type(f.getByLabelText(t("vault.confirm")), "export pass");
  await user.click(btn);
  expect(await f.findByText(t("vault.err.wrong"))).toBeInTheDocument();
  expect(mockVault.lastArgs.vault_export_encrypted).toEqual({
    path: "/mock/rmi-vault.rmiv",
    password: "export pass",
    current: "wrong one",
  });
});

test("import sends the picked file and its password", async () => {
  const user = userEvent.setup();
  await mockCall("vault_create", { mode: "os", password: null });
  setup();
  await tab(user, "vault");
  const btn = await screen.findByRole("button", { name: t("vs.importBtn") });
  const f = within(btn.closest("form")!);
  await user.type(f.getByLabelText(t("vs.exportPassword")), "export pass");
  await user.click(btn);
  expect(await f.findByText(t("vs.imported"))).toBeInTheDocument();
  expect(mockVault.lastArgs.vault_import_file).toEqual({
    path: "/mock/picked.rmiv",
    password: "export pass",
  });
});

test("plaintext export submits phrase and current password", async () => {
  const user = userEvent.setup();
  await mockCall("vault_create", { mode: "password", password: "correct horse" });
  setup();
  await tab(user, "vault");
  const btn = await screen.findByRole("button", { name: t("vs.exportPlainBtn") });
  const f = within(btn.closest("form")!);
  await user.type(f.getByLabelText(t("vs.typePhrase")), "export plaintext");
  await user.type(f.getByLabelText(t("vs.current")), "correct horse");
  await user.click(btn);
  expect(await f.findByText(t("vs.exported"))).toBeInTheDocument();
  expect(mockVault.lastArgs.vault_export_plain).toEqual({
    path: "/mock/rmi-vault-plain.json",
    confirm: "export plaintext",
    password: "correct horse",
  });
});

test("Roblox shows FPS and graphics rows; client settings file errors inline", async () => {
  const user = userEvent.setup();
  setup();
  await tab(user, "roblox");
  expect(await screen.findByText(t("set.graphics.level"))).toBeInTheDocument();
  expect(screen.getByText(t("set.fps.unlock"))).toBeInTheDocument();
  const input = screen.getByRole("textbox", { name: t("set.client.customSettingsPath") });
  await user.type(input, "C:/x/flags.txt{Enter}");
  expect(await screen.findByRole("alert")).toHaveTextContent("isn't valid JSON");
  await user.clear(input);
  await user.type(input, "C:/x/flags.json{Enter}");
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  const s = (await mockCall("settings_get")) as { client: { customSettingsPath: string } };
  expect(s.client.customSettingsPath).toBe("C:/x/flags.json");
});

test("client settings path writes on Enter/blur only, never per keystroke", async () => {
  const user = userEvent.setup();
  setup();
  await tab(user, "roblox");
  const input = await screen.findByRole("textbox", { name: t("set.client.customSettingsPath") });
  mockVault.lastArgs = {};
  await user.type(input, "C:/x/fl");
  expect(mockVault.lastArgs.client_settings_set).toBeUndefined();
  await user.type(input, "ags.json{Enter}");
  await waitFor(() =>
    expect(mockVault.lastArgs.client_settings_set).toEqual({ path: "C:/x/flags.json" }),
  );
});

test("dropSent keeps edits typed while the save was in flight", () => {
  useDraft.setState({ edits: { "a.x": 1, "a.y": "old" } });
  const sent = useDraft.getState().edits;
  useDraft.setState({ edits: { ...sent, "a.y": "new", "a.z": true } });
  useDraft.getState().dropSent(sent);
  expect(useDraft.getState().edits).toEqual({ "a.y": "new", "a.z": true });
});

test("a bootstrapper manages the client settings file, not FPS or graphics", async () => {
  mockRoblox.bootstrapper = "Bloxstrap";
  const user = userEvent.setup();
  setup();
  await tab(user, "roblox");
  const sw = await screen.findByRole("switch", { name: t("set.fps.unlock") });
  await waitFor(() =>
    expect(screen.getAllByText(t("settings.managedBy", { name: "Bloxstrap" })).length).toBe(1),
  );
  expect(sw).toBeEnabled();
});

test("capability-gated control is disabled and says why", async () => {
  const user = userEvent.setup();
  mockCaps.multiInstance = false;
  mockCaps.launchReason = "Launching arrives in a later update.";
  setup();
  await tab(user, "roblox");
  const sw = await screen.findByRole("switch", { name: t("set.multi.enabled") });
  expect(sw).toBeDisabled();
  expect(screen.getAllByText("Launching arrives in a later update.").length).toBeGreaterThan(0);
  // screen readers hear why it is disabled
  expect(sw).toHaveAccessibleDescription(/Launching arrives in a later update\.$/);
});

test("macOS with the capabilities shows the watcher and enables multi-instance", async () => {
  const user = userEvent.setup();
  Object.assign(mockCaps, { os: "macos", multiInstance: true, watcher: true });
  setup();
  await tab(user, "watcher");
  expect(await screen.findByRole("switch", { name: t("set.watcher.enabled") })).toBeEnabled();
  await tab(user, "roblox");
  expect(await screen.findByRole("switch", { name: t("set.multi.enabled") })).toBeEnabled();
});

test("About: check for updates, then install; a failed install stays retryable", async () => {
  const user = userEvent.setup();
  mockUpdate.version = null;
  setup();
  await tab(user, "about");
  await user.click(await screen.findByRole("button", { name: t("settings.updates") }));
  expect(await screen.findByText(t("settings.upToDate"))).toBeInTheDocument();
  mockUpdate.version = "9.9.9";
  await user.click(screen.getByRole("button", { name: t("settings.updates") }));
  expect(
    await screen.findByText(t("settings.updateAvailable", { v: "9.9.9" })),
  ).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("settings.updateInstall") }));
  expect(await screen.findByText(/could not be installed/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: t("settings.updateInstall") })).toBeEnabled();
  mockUpdate.version = null;
});

test("typing 0 moves the slider to 0, not back to the saved value", async () => {
  const user = userEvent.setup();
  setup();
  await tab(user, "vault");
  const [range, num] = await screen.findAllByLabelText(t("set.vault.autoLockMin"));
  await user.clear(num);
  await user.type(num, "0");
  expect(range).toHaveValue("0");
  expect(range.style.getPropertyValue("--fill")).toBe("0%");
});

test("a slider change lands in the draft at once; Discard then unmount keeps it gone", async () => {
  const user = userEvent.setup();
  const { unmount } = setup();
  await tab(user, "vault");
  const [range] = await screen.findAllByLabelText(t("set.vault.autoLockMin"));
  fireEvent.change(range, { target: { value: "42" } });
  expect(useDraft.getState().edits).toEqual({ "vault.autoLockMin": 42 });
  await user.click(screen.getByRole("button", { name: t("settings.discard") }));
  unmount();
  expect(useDraft.getState().edits).toEqual({});
});

test("typed text and numbers reach the draft without a blur, so Save is ready", async () => {
  const user = userEvent.setup();
  setup();
  await tab(user, "privacy");
  const url = await screen.findByLabelText(t("set.geo.urlTemplate"));
  await user.type(url, "x");
  expect(url).toHaveFocus();
  expect(screen.getByRole("button", { name: t("settings.save") })).toBeEnabled();
  expect(Object.keys(useDraft.getState().edits)).toEqual(["geo.urlTemplate"]);
  await tab(user, "vault");
  const [, num] = await screen.findAllByLabelText(t("set.vault.autoLockMin"));
  await user.clear(num);
  await user.type(num, "25");
  expect(num).toHaveValue(25);
  expect(useDraft.getState().edits).toMatchObject({ "vault.autoLockMin": 25 });
});

test("Show usernames saves at once, never drafted", async () => {
  const user = userEvent.setup();
  setup();
  await tab(user, "accounts");
  await user.click(await screen.findByRole("switch", { name: t("set.ui.showUsernames") }));
  expect(useDraft.getState().edits).toEqual({});
  await vi.waitFor(async () =>
    expect(
      ((await mockCall("settings_get")) as { ui: { showUsernames: boolean } }).ui.showUsernames,
    ).toBe(false),
  );
});

test("on Linux a Windows-only control is disabled and says why", async () => {
  const user = userEvent.setup();
  Object.assign(mockCaps, { os: "linux", multiInstance: false });
  setup();
  await tab(user, "roblox");
  expect(await screen.findByRole("switch", { name: t("set.multi.enabled") })).toBeDisabled();
  expect(screen.getAllByText(t("settings.winOnly")).length).toBeGreaterThan(0);
});

test("on macOS a Windows-only control says it is not on macOS, not 'not available yet'", async () => {
  const user = userEvent.setup();
  Object.assign(mockCaps, { os: "macos", cookieLock: false });
  setup();
  await tab(user, "roblox");
  expect(await screen.findByRole("switch", { name: t("set.multi.cookieLock") })).toBeDisabled();
  expect(screen.getByText(t("settings.notMac"))).toBeInTheDocument();
  expect(screen.queryByText(t("settings.unavailable"))).not.toBeInTheDocument();
});

test("on Linux without Sober, launch-gated rows show the Install Sober reason", async () => {
  const user = userEvent.setup();
  const why = "Install Sober (org.vinegarhq.Sober) to launch Roblox on Linux.";
  Object.assign(mockCaps, { os: "linux", launch: false, launchReason: why });
  setup();
  await tab(user, "roblox");
  expect(await screen.findByRole("switch", { name: t("set.fps.unlock") })).toBeDisabled();
  expect(screen.getAllByText(why).length).toBeGreaterThan(0);
});

test("the friend remote section is hidden where clients can't be tracked (Linux)", async () => {
  const user = userEvent.setup();
  Object.assign(mockCaps, { os: "linux", watcher: false });
  setup();
  await tab(user, "control");
  await screen.findByRole("heading", { name: t("sec.control") });
  expect(screen.queryByRole("heading", { name: t("sec.friend") })).toBeNull();
});

test("the friend remote section shows where clients are tracked (Windows)", async () => {
  const user = userEvent.setup();
  setup();
  await tab(user, "control");
  expect(await screen.findByRole("heading", { name: t("sec.friend") })).toBeInTheDocument();
});

test("let your friend add accounts: off by default, saves friend.canAdd", async () => {
  const user = userEvent.setup();
  await mockCall("settings_set", {
    patch: { friend: { publicUrl: "https://rmi.example.ts.net" } },
  });
  setup();
  await tab(user, "control");
  const sw = await screen.findByRole("switch", { name: t("set.friend.canAdd") });
  expect(sw).not.toBeChecked();
  expect(screen.getByText(t("set.friend.canAdd.d"))).toBeInTheDocument();
  await user.click(sw);
  await user.click(screen.getByRole("button", { name: t("settings.save") }));
  await vi.waitFor(async () =>
    expect(((await mockCall("settings_get")) as ipc.Settings).friend.canAdd).toBe(true),
  );
  await mockCall("settings_set", { patch: { friend: { publicUrl: "", canAdd: false } } });
});

test("remote file transfer and sound: file transfer off and sound on by default, each saves its own switch", async () => {
  const user = userEvent.setup();
  await mockCall("settings_set", { patch: { remote: { enabled: true } } });
  setup();
  await tab(user, "remote");
  const files = await screen.findByRole("switch", { name: t("set.remote.files") });
  const sound = screen.getByRole("switch", { name: t("set.remote.audio") });
  expect(files).not.toBeChecked();
  expect(sound).toBeChecked();
  expect(screen.getByText(t("set.remote.files.d"))).toBeInTheDocument();
  expect(screen.getByText(t("set.remote.audio.d"))).toBeInTheDocument();
  await user.click(files);
  await user.click(screen.getByRole("button", { name: t("settings.save") }));
  await vi.waitFor(async () => {
    const r = ((await mockCall("settings_get")) as ipc.Settings).remote;
    expect([r.files, r.audio]).toEqual([true, true]);
  });
  await mockCall("settings_set", { patch: { remote: { enabled: false, files: false } } });
});

test("a failed update check shows Rust's reason (portable mode)", async () => {
  const user = userEvent.setup();
  const why = "Portable mode can't update itself. Download the new version and replace the exe.";
  const orig = ipc.call;
  mockUpdate.version = null;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "update_check" && (args as { force: boolean }).force
      ? Promise.reject(new Error(`update_check: ${why}`))
      : orig(cmd, args as never),
  );
  setup();
  await tab(user, "about");
  await user.click(await screen.findByRole("button", { name: t("settings.updates") }));
  expect(await screen.findByText(why)).toBeInTheDocument();
  vi.restoreAllMocks();
});

test("installing says it is downloading and RMI restarts when ready", async () => {
  const user = userEvent.setup();
  const orig = ipc.call;
  mockUpdate.version = "9.9.9";
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "update_install" ? new Promise(() => {}) : orig(cmd, args as never),
  );
  setup();
  await tab(user, "about");
  await user.click(await screen.findByRole("button", { name: t("settings.updateInstall") }));
  expect(await screen.findByText(t("settings.updateDownloading"))).toBeInTheDocument();
  vi.restoreAllMocks();
  mockUpdate.version = null;
});

test("an install already running keeps the button disabled and says why", async () => {
  const user = userEvent.setup();
  const orig = ipc.call;
  mockUpdate.version = "9.9.9";
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "update_install"
      ? Promise.reject(new Error("update_install: UPDATE_BUSY"))
      : orig(cmd, args as never),
  );
  setup();
  await tab(user, "about");
  await user.click(await screen.findByRole("button", { name: t("settings.updateInstall") }));
  expect(await screen.findByText(t("settings.updateBusy"))).toBeInTheDocument();
  expect(screen.getByRole("button", { name: t("settings.updateInstall") })).toBeDisabled();
  vi.restoreAllMocks();
  mockUpdate.version = null;
});
