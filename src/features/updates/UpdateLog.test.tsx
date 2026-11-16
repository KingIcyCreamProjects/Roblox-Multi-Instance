import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useUi } from "@/app/store";
import { SECTIONS } from "@/features/settings/catalog";
import * as ipc from "@/lib/ipc";
import { call } from "@/lib/ipc";
import { resetVaultMock } from "@/lib/mock";
import { UPDATES } from "./log";
import { UpdateLog, useUpdateLog, useWhatsNew } from "./UpdateLog";

function Probe() {
  useWhatsNew();
  return null;
}

/** The app running at `version`; settings still go through the mock. */
function mountAt(version: string) {
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "app_info" ? Promise.resolve({ version } as never) : orig(cmd, args as never),
  );
  const qc = new QueryClient();
  render(
    <QueryClientProvider client={qc}>
      <Probe />
    </QueryClientProvider>,
  );
  return qc;
}

const seed = (onboarding: object, seen: string) =>
  call("settings_set", { patch: { onboarding, updates: { seen } } });
const seenNow = async () => (await call("settings_get")).updates.seen;

beforeEach(() => {
  resetVaultMock();
  useUpdateLog.setState({ open: false, pick: null });
});
afterEach(() => vi.restoreAllMocks());

const ALL_STEPS = [
  "welcome",
  "add",
  "pick",
  "perf",
  "tabs",
  "friend",
  "remote",
  "homelab",
  "def",
  "mac",
];

test("after the tour, a release with notes opens its log and is recorded as seen", async () => {
  await seed({ tourDone: true, tourSeen: ALL_STEPS }, "0.2.1");
  mountAt("1.0.0");
  await waitFor(() => expect(useUpdateLog.getState()).toMatchObject({ open: true, pick: "1.0.0" }));
  await waitFor(async () => expect(await seenNow()).toBe("1.0.0"));
});

test("an update that added tour steps waits for that tour before opening the log", async () => {
  await seed({ tourDone: true, tourSeen: [] }, "0.2.1"); // 0.2.1 tour done: perf + tabs still to show
  const qc = mountAt("1.0.0");
  await waitFor(() => expect(qc.getQueryData(["caps"])).toBeDefined());
  await new Promise((r) => setTimeout(r, 20));
  expect(useUpdateLog.getState().open).toBe(false);
  expect(await seenNow()).toBe("0.2.1");
  await call("settings_set", { patch: { onboarding: { tourSeen: ALL_STEPS } } }); // the tour's done()
  void qc.invalidateQueries({ queryKey: ["settings"] });
  await waitFor(() => expect(useUpdateLog.getState()).toMatchObject({ open: true, pick: "1.0.0" }));
});

test("first run records the version but leaves the log closed", async () => {
  await seed({ tourDone: false, tourSeen: [] }, "0.2.1");
  mountAt("1.0.0");
  await waitFor(async () => expect(await seenNow()).toBe("1.0.0"));
  expect(useUpdateLog.getState().open).toBe(false);
});

test("Show me closes the log and opens the tab or the settings row", () => {
  useUi.setState({ route: "accounts", settingsJump: null });
  useUpdateLog.setState({ open: true, pick: "1.0.0" });
  render(<UpdateLog />);
  const btns = () => screen.getAllByRole("button", { name: "Show me" });
  fireEvent.click(btns()[0]!); // Remote access
  expect(useUpdateLog.getState().open).toBe(false);
  expect(useUi.getState().route).toBe("devices");
  act(() => useUpdateLog.setState({ open: true }));
  fireEvent.click(btns()[1]!); // Friend remote
  expect(useUi.getState()).toMatchObject({
    route: "settings",
    settingsJump: "friend.enabled",
  });
});

test("every Show me target is a tab or a settings row that exists", () => {
  const rows = new Set(SECTIONS.flatMap((s) => s.items.map((i) => i.path)));
  const tabs = ["home", "accounts", "discover", "automation", "devices"];
  for (const u of UPDATES)
    for (const line of [...(u.added ?? []), ...(u.improved ?? []), ...(u.fixed ?? [])])
      if (typeof line !== "string")
        expect(tabs.includes(line.to) || rows.has(line.to), line.to).toBe(true);
});

test("a version without notes is recorded but does not open the log", async () => {
  await seed({ tourDone: true, tourSeen: ALL_STEPS }, "0.2.1");
  mountAt("0.1.0-dev");
  await waitFor(async () => expect(await seenNow()).toBe("0.1.0-dev"));
  expect(useUpdateLog.getState().open).toBe(false);
});
