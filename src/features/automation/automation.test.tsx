import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AccountRow, AutomationPage, RelaunchDelay, RelaunchSwitch } from "./AutomationPage";
import { Controls } from "./Controls";
import { TokenRows } from "./TokenSettings";
import { curlCmd, filterLines, fmtTime, maskToken } from "./lib";
import { useSecretToast } from "./useSecretToast";
import { Toaster, useToasts } from "@/components/Toasts";
import { useUi } from "@/app/store";
import * as ipc from "@/lib/ipc";
import * as report from "@/lib/report";
import { queryClient } from "@/lib/queryClient";
import { t } from "@/lib/i18n";

type Cb = (p: { account: string }) => void;
const cbs: Record<string, Cb> = {};
const secretCb: Cb = (p) => cbs["automation://secret"]?.(p);
vi.mock("@/lib/events", () => ({
  onEvent: (n: string, f: Cb) => ((cbs[n] = f), () => {}),
  openExternal: vi.fn(),
}));
function Harness() {
  queryClient.setQueryData(["vault"], { unlocked: true }); // names show only while unlocked
  useSecretToast();
  return <Toaster />;
}

// the token rows only show once Automation is on
beforeEach(async () => {
  await ipc.call("settings_set", { patch: { automation: { enabled: true } } });
});

const wrap = (ui: React.ReactNode) =>
  render(<QueryClientProvider client={new QueryClient()}>{ui}</QueryClientProvider>);

const line = (text: string, level = "info") => ({ ts: 1, level, text });

test("console filter is case-insensitive substring; blank keeps all", () => {
  const ls = [line("Hello World"), line("bye"), line("HELLO again")];
  expect(filterLines(ls, "hello").map((l) => l.text)).toEqual(["Hello World", "HELLO again"]);
  expect(filterLines(ls, "  ")).toHaveLength(3);
  expect(filterLines(ls, "zzz")).toHaveLength(0);
});

test("the curl example names curl.exe on Windows (PowerShell aliases curl)", () => {
  expect(curlCmd(1, "t")).toMatch(/^curl -H/);
  const ua = vi
    .spyOn(navigator, "userAgent", "get")
    .mockReturnValue("Mozilla/5.0 (Windows NT 10.0)");
  expect(curlCmd(1, "t")).toMatch(/^curl\.exe -H/);
  ua.mockRestore();
});

test("dynamic controls render by kind and fire automation_ui_event", async () => {
  const spy = vi.spyOn(ipc, "call").mockResolvedValue(null);
  const u = userEvent.setup();
  render(
    <Controls
      account="Alice"
      controls={[
        { kind: "button", name: "Reset" },
        { kind: "text", name: "Target", value: "spawn" },
        { kind: "break", name: "b" },
        { kind: "number", name: "Speed", value: 16 },
        { kind: "label", name: "State", value: "idle" },
      ]}
    />,
  );
  expect(screen.getByText("idle")).toBeInTheDocument();
  await u.click(screen.getByRole("button", { name: "Reset" }));
  expect(spy).toHaveBeenCalledWith("automation_ui_event", {
    account: "Alice",
    name: "Reset",
    value: true,
  });
  const n = screen.getByLabelText("Speed");
  await u.clear(n);
  await u.type(n, "20{Enter}");
  expect(spy).toHaveBeenCalledWith("automation_ui_event", {
    account: "Alice",
    name: "Speed",
    value: 20,
  });
  spy.mockRestore();
});

test("no token rows while Automation is off: the token then is a throwaway", async () => {
  await ipc.call("settings_set", { patch: { automation: { enabled: false } } });
  const spy = vi.spyOn(ipc, "call");
  wrap(<TokenRows />);
  await new Promise((r) => setTimeout(r, 50));
  expect(screen.queryByText(t("settings.token"))).toBeNull();
  expect(spy.mock.calls.some(([c]) => c === "automation_token")).toBe(false);
  spy.mockRestore();
});

test("token masking shows only the last 4", async () => {
  expect(maskToken("deadbeef".repeat(7) + "cafe1234")).toBe("••••1234");
  wrap(<TokenRows />);
  expect(await screen.findByText("••••1234")).toBeInTheDocument();
  expect(document.body.textContent).not.toContain("deadbeef");
  expect(document.body.textContent).not.toContain("cafe");
});

test("secret event toasts the account name, but not while locked", () => {
  render(<Harness />);
  act(() => secretCb({ account: "Alice" }));
  expect(screen.getByRole("status")).toHaveTextContent(
    "Cookie for Alice was read through the local API",
  );
  queryClient.setQueryData(["vault"], { unlocked: false });
  act(() => secretCb({ account: "Alice" }));
  expect(useToasts.getState().items.at(-1)?.message).toBe(
    "Cookie for an account was read through the local API",
  );
});

const TOKEN = "deadbeef".repeat(7) + "cafe1234";

test("copy puts the real token on the clipboard and shows Copied", async () => {
  const u = userEvent.setup();
  wrap(<TokenRows />);
  await screen.findByText("••••1234");
  await u.click(screen.getByRole("button", { name: /Copy: API token/ }));
  expect(await navigator.clipboard.readText()).toBe(TOKEN);
  expect(await screen.findAllByText("Copied")).not.toHaveLength(0);
  // the examples copy the real token too, while showing it masked
  await u.click(screen.getByRole("button", { name: /Copy: Local API example/ }));
  expect(await navigator.clipboard.readText()).toBe(
    `curl -H "Authorization: Bearer ${TOKEN}" http://127.0.0.1:7811/v1/accounts`,
  );
});

test("regenerate asks first, cancel closes, failure keeps the confirm and reports", async () => {
  const u = userEvent.setup();
  const orig = ipc.call;
  let fail = true;
  const spy = vi.spyOn(ipc, "call").mockImplementation((cmd, args) => {
    if (cmd === "automation_token_regenerate") {
      return fail ? Promise.reject(new Error("boom")) : Promise.resolve("f".repeat(64));
    }
    return orig(cmd, args as never);
  });
  useUi.getState().setError(null);
  wrap(<TokenRows />);
  await screen.findByText("••••1234");
  await u.click(screen.getByRole("button", { name: "Regenerate" }));
  expect(screen.getByText(/invalidate this token/)).toBeInTheDocument();
  await u.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.queryByText(/invalidate this token/)).toBeNull();

  await u.click(screen.getByRole("button", { name: "Regenerate" }));
  await u.click(screen.getByRole("button", { name: "Regenerate token" }));
  await waitFor(() => expect(useUi.getState().lastError).toContain("boom"));
  expect(screen.getByText(/invalidate this token/)).toBeInTheDocument(); // still open

  fail = false;
  await u.click(screen.getByRole("button", { name: "Regenerate token" }));
  expect(await screen.findByText("••••ffff")).toBeInTheDocument();
  expect(screen.queryByText(/invalidate this token/)).toBeNull();
  spy.mockRestore();
});

test("number control fires once for Enter+blur, never for empty, honours decimals", async () => {
  const spy = vi.spyOn(ipc, "call").mockResolvedValue(null);
  const u = userEvent.setup();
  render(
    <Controls account="A" controls={[{ kind: "number", name: "Gain", value: 1, decimals: 1 }]} />,
  );
  const n = screen.getByLabelText("Gain");
  await u.clear(n);
  await u.tab(); // empty: no fire
  expect(spy).not.toHaveBeenCalled();
  await u.type(n, "2.46{Enter}");
  await u.tab(); // blur after Enter: no duplicate
  expect(spy).toHaveBeenCalledTimes(1);
  expect(spy).toHaveBeenCalledWith("automation_ui_event", {
    account: "A",
    name: "Gain",
    value: 2.5,
  });
  spy.mockRestore();
});

test("Save to file calls automation_save_console for the selected account", async () => {
  const u = userEvent.setup();
  const spy = vi.spyOn(ipc, "call");
  wrap(
    <Tooltip.Provider>
      <AutomationPage />
    </Tooltip.Provider>,
  );
  await u.click(await screen.findByRole("button", { name: /Save to file/ }));
  expect(spy).toHaveBeenCalledWith("automation_save_console", { account: "Alice" });
  spy.mockRestore();
});

test("automation notice toasts the key Rust holds, taken on mount and on each event", async () => {
  const keys = ["auto.tokenReset", null, "auto.tokenMemoryOnly"];
  const spy = vi.spyOn(ipc, "call").mockImplementation(async () => keys.shift() ?? null);
  render(<Harness />); // mount takes the one emitted before the listener existed
  expect(await screen.findByText(/saved automation token could not be read/)).toBeInTheDocument();
  const notice = () => cbs["automation://notice"]?.({} as never);
  act(() => notice()); // already taken: nothing new
  act(() => notice());
  expect(await screen.findByText(/could not be saved/)).toBeInTheDocument();
  expect(spy).toHaveBeenCalledWith("automation_notice_take");
  spy.mockRestore();
});

test("identical toasts each get their own entry and the status region stays mounted", () => {
  act(() => useToasts.setState({ items: [] }));
  render(<Harness />);
  expect(screen.getByRole("status")).toBeInTheDocument();
  act(() => secretCb({ account: "Alice" }));
  act(() => secretCb({ account: "Alice" }));
  expect(screen.getAllByText(/Cookie for Alice/)).toHaveLength(2);
});

const acct = (over: Partial<ipc.AutomationAccount> = {}): ipc.AutomationAccount => ({
  account: "Bob",
  connected: false,
  userId: 1,
  placeId: null,
  jobId: null,
  console: [],
  controls: [],
  relaunch: {},
  ...over,
});

test("relaunch toggle and delay call automation_relaunch_set", async () => {
  const spy = vi.spyOn(ipc, "call").mockResolvedValue(null);
  const u = userEvent.setup();
  const a = acct({ relaunch: { seconds: 30, placeId: 5 } });
  wrap(
    <>
      <RelaunchSwitch a={a} />
      <RelaunchDelay a={a} />
    </>,
  );
  await u.click(screen.getByRole("switch", { name: "Auto-relaunch" }));
  expect(spy).toHaveBeenCalledWith("automation_relaunch_set", {
    account: "Bob",
    enabled: true,
    delaySec: 30,
    placeId: 5,
    jobId: null,
  });
  const d = screen.getByLabelText("Delay (s)");
  await u.clear(d);
  await u.type(d, "90{Enter}");
  expect(spy).toHaveBeenCalledWith(
    "automation_relaunch_set",
    expect.objectContaining({ enabled: false, delaySec: 90 }),
  );
  spy.mockRestore();
});

test("two quick relaunch clicks toggle on then off (optimistic, before any refetch)", async () => {
  const spy = vi
    .spyOn(ipc, "call")
    .mockImplementation(((cmd: string) =>
      cmd === "automation_snapshot" ? new Promise(() => {}) : Promise.resolve(null)) as never);
  const qc = new QueryClient();
  qc.setQueryData(["snap"], [acct({ relaunch: { seconds: 30 } })]);
  const u = userEvent.setup();
  render(
    <QueryClientProvider client={qc}>
      <Tooltip.Provider>
        <AutomationPage />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  const box = screen.getByRole("switch", { name: "Auto-relaunch" });
  await u.click(box);
  await u.click(box);
  const sets = spy.mock.calls.filter((c) => c[0] === "automation_relaunch_set");
  expect(sets.map((c) => (c[1] as { enabled: boolean }).enabled)).toEqual([true, false]);
  spy.mockRestore();
});

test("relaunch status shows the cooldown stop", () => {
  vi.spyOn(ipc, "call").mockResolvedValue(null);
  wrap(
    <RelaunchSwitch
      a={acct({
        relaunch: { enabled: true },
        relaunchStatus: {
          pendingAt: null,
          lastRelaunch: 1,
          count: 5,
          stopped: true,
          launching: false,
        },
      })}
    />,
  );
  expect(screen.getByRole("status")).toHaveTextContent("Stopped after 5 relaunches");
  expect(screen.getByRole("switch")).toBeChecked();
});

test("a disconnected account with a relaunch running shows Launching", () => {
  vi.spyOn(ipc, "call").mockResolvedValue(null);
  const st = { pendingAt: null, lastRelaunch: 1, count: 1, stopped: false, launching: true };
  wrap(
    <ul>
      <AccountRow a={acct({ relaunchStatus: st })} active={false} onPick={() => {}} />
    </ul>,
  );
  expect(screen.getByText("Launching")).toBeInTheDocument();
});

test("relaunch delay clamps to 0..86400 before it is sent", async () => {
  const spy = vi.spyOn(ipc, "call").mockResolvedValue(null);
  const u = userEvent.setup();
  wrap(<RelaunchDelay a={acct({ relaunch: { enabled: false, seconds: 30 } })} />);
  const d = screen.getByLabelText("Delay (s)");
  await u.clear(d);
  await u.type(d, "99999{Enter}");
  expect(spy).toHaveBeenLastCalledWith(
    "automation_relaunch_set",
    expect.objectContaining({ delaySec: 86400 }),
  );
  await u.clear(d);
  await u.type(d, "-5{Enter}");
  expect(spy).toHaveBeenLastCalledWith(
    "automation_relaunch_set",
    expect.objectContaining({ delaySec: 0 }),
  );
  spy.mockRestore();
});

test("relaunch delay sends nothing for a blank or unchanged value", async () => {
  const spy = vi.spyOn(ipc, "call").mockResolvedValue(null);
  const u = userEvent.setup();
  wrap(<RelaunchDelay a={acct({ relaunch: { enabled: false, seconds: 30 } })} />);
  const d = screen.getByLabelText("Delay (s)");
  await u.clear(d);
  await u.tab(); // blur on blank
  await u.click(d);
  await u.clear(d);
  await u.type(d, "30{Enter}"); // same as stored
  expect(spy.mock.calls.filter((c) => c[0] === "automation_relaunch_set")).toEqual([]);
  spy.mockRestore();
});

test("relaunch toggle sends no delay (the account keeps the Settings default) and drops a non-positive place or non-string job id", async () => {
  const spy = vi.spyOn(ipc, "call").mockResolvedValue(null);
  const u = userEvent.setup();
  wrap(<RelaunchSwitch a={acct({ relaunch: { placeId: 0, jobId: 7 } })} />);
  await u.click(screen.getByRole("switch", { name: "Auto-relaunch" }));
  expect(spy).toHaveBeenLastCalledWith("automation_relaunch_set", {
    account: "Bob",
    enabled: true,
    delaySec: undefined,
    placeId: null,
    jobId: null,
  });
  spy.mockRestore();
});

test("text control fires its string on blur only when it changed", async () => {
  const spy = vi.spyOn(ipc, "call").mockResolvedValue(null);
  const u = userEvent.setup();
  render(<Controls account="A" controls={[{ kind: "text", name: "Target", value: "spawn" }]} />);
  const f = screen.getByLabelText("Target");
  await u.click(f);
  await u.tab(); // blur, unchanged
  expect(spy).not.toHaveBeenCalled();
  await u.clear(f);
  await u.type(f, "ground{Enter}");
  await u.tab(); // blur after Enter: no duplicate
  expect(spy).toHaveBeenCalledTimes(1);
  expect(spy).toHaveBeenCalledWith("automation_ui_event", {
    account: "A",
    name: "Target",
    value: "ground",
  });
  spy.mockRestore();
});

test("Clear hides the lines on screen and says so when none are left", async () => {
  vi.spyOn(ipc, "call").mockImplementation(((cmd: string) =>
    cmd === "automation_snapshot" ? new Promise(() => {}) : Promise.resolve(null)) as never);
  const u = userEvent.setup();
  // jsdom has no layout: give the console's virtual list a viewport (restored below)
  const dims = ["clientHeight", "offsetHeight"] as const;
  const saved = dims.map(
    (k) => [k, Object.getOwnPropertyDescriptor(HTMLElement.prototype, k)] as const,
  );
  for (const k of dims)
    Object.defineProperty(HTMLElement.prototype, k, { configurable: true, get: () => 320 });
  const qc = new QueryClient();
  qc.setQueryData(
    ["snap"],
    [
      acct({
        account: "Carol",
        connected: true,
        console: [
          { ts: 1, level: "info", text: "first line" },
          { ts: 2, level: "error", text: "second line" },
        ],
      }),
    ],
  );
  render(
    <QueryClientProvider client={qc}>
      <Tooltip.Provider>
        <AutomationPage />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  expect(await screen.findByText("first line")).toBeInTheDocument();
  await u.click(screen.getByRole("button", { name: t("auto.clear") }));
  expect(screen.queryByText("first line")).toBeNull();
  expect(screen.getByText(t("auto.noLines"))).toBeInTheDocument();
  for (const [k, d] of saved) {
    if (d) Object.defineProperty(HTMLElement.prototype, k, d);
    else delete (HTMLElement.prototype as unknown as Record<string, unknown>)[k];
  }
  vi.restoreAllMocks();
});

test("a notice seen while locked waits for unlock, then is taken and shown", async () => {
  const keys = ["auto.tokenReset"];
  const spy = vi
    .spyOn(ipc, "call")
    .mockImplementation(async (cmd) =>
      cmd === "automation_notice_take" ? (keys.shift() ?? null) : null,
    );
  act(() => useToasts.setState({ items: [], locked: true }));
  render(<Harness />);
  act(() => cbs["automation://notice"]?.({} as never));
  expect(spy).not.toHaveBeenCalledWith("automation_notice_take");
  act(() => useToasts.setState({ locked: false }));
  expect(await screen.findByText(/saved automation token could not be read/)).toBeInTheDocument();
  expect(spy).toHaveBeenCalledWith("automation_notice_take");
  spy.mockRestore();
});

test("a failed status read shows an error, and Retry refetches it", async () => {
  let fail = true;
  const spy = vi.spyOn(ipc, "call").mockImplementation(((cmd: string) => {
    if (cmd === "automation_snapshot") return new Promise(() => {});
    if (cmd === "automation_status")
      return fail
        ? Promise.reject(new Error("boom"))
        : Promise.resolve({
            wsRunning: false,
            apiRunning: false,
            wsPort: 7810,
            apiPort: 7811,
            lan: false,
          });
    return Promise.resolve(null);
  }) as never);
  const u = userEvent.setup();
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(["snap"], []);
  render(
    <QueryClientProvider client={qc}>
      <Tooltip.Provider>
        <AutomationPage />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(t("auto.statusError"));
  fail = false;
  await u.click(screen.getByRole("button", { name: t("error.retry") }));
  expect(await screen.findByRole("button", { name: t("auto.start") })).toBeInTheDocument();
  spy.mockRestore();
});

test("number control keeps focus and draft when its own commit echoes back, resets on a real change", async () => {
  const spy = vi.spyOn(ipc, "call").mockResolvedValue(null);
  const u = userEvent.setup();
  const ui = (value: number) => (
    <Controls account="A" controls={[{ kind: "number", name: "Speed", value }]} />
  );
  const { rerender } = render(ui(1));
  const f = screen.getByLabelText("Speed");
  await u.clear(f);
  await u.type(f, "5{Enter}");
  await u.type(f, "0"); // keep typing: draft "50"
  rerender(ui(5)); // snapshot echoes the committed 5
  expect(screen.getByLabelText("Speed")).toBe(f);
  expect(f).toHaveValue(50);
  rerender(ui(9)); // a script changed it
  expect(f).toHaveValue(9);
  spy.mockRestore();
});

test("mask: empty token shows nothing, short token shows all of it after the dots", () => {
  expect(maskToken("")).toBe("");
  expect(maskToken("ab")).toBe("••••ab");
});

test("a script with no controls says so", () => {
  render(<Controls account="A" controls={[]} />);
  expect(screen.getByText(t("auto.noControls"))).toBeInTheDocument();
});

test("a failed automation_notice_take is logged, not thrown", async () => {
  const log = vi.spyOn(report, "logError").mockImplementation(() => {});
  vi.spyOn(ipc, "call").mockRejectedValue(new Error("gone"));
  act(() => useToasts.setState({ locked: false }));
  render(<Harness />);
  await waitFor(() =>
    expect(log).toHaveBeenCalledWith(expect.stringContaining("automation_notice_take failed")),
  );
  vi.restoreAllMocks();
});

test("Stop calls automation_stop, and a LAN notice shows while LAN access is on", async () => {
  const status = { wsRunning: true, apiRunning: false, wsPort: 7810, apiPort: 7811, lan: true };
  const spy = vi
    .spyOn(ipc, "call")
    .mockImplementation(((cmd: string) =>
      cmd === "automation_snapshot"
        ? new Promise(() => {})
        : cmd === "automation_status"
          ? Promise.resolve(status)
          : Promise.resolve(null)) as never);
  const qc = new QueryClient();
  qc.setQueryData(["snap"], [acct({ account: "Alice" })]);
  const u = userEvent.setup();
  render(
    <QueryClientProvider client={qc}>
      <Tooltip.Provider>
        <AutomationPage />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  expect(await screen.findByText(t("auto.lan"))).toBeInTheDocument();
  await u.click(screen.getByRole("button", { name: t("auto.stop") }));
  expect(spy).toHaveBeenCalledWith("automation_stop");
  spy.mockRestore();
});

test("send skips blank text, then sends the typed text and clears the box", async () => {
  const spy = vi
    .spyOn(ipc, "call")
    .mockImplementation(((cmd: string) =>
      cmd === "automation_snapshot" ? new Promise(() => {}) : Promise.resolve(null)) as never);
  const qc = new QueryClient();
  qc.setQueryData(["snap"], [acct({ account: "Alice", connected: true })]);
  const u = userEvent.setup();
  render(
    <QueryClientProvider client={qc}>
      <Tooltip.Provider>
        <AutomationPage />
      </Tooltip.Provider>
    </QueryClientProvider>,
  );
  const box = screen.getByLabelText(t("auto.sendHint"));
  await u.type(box, "   {Enter}");
  expect(spy.mock.calls.filter((c) => c[0] === "automation_send")).toEqual([]);
  await u.clear(box);
  await u.type(box, "spawn{Enter}");
  await waitFor(() =>
    expect(spy).toHaveBeenCalledWith("automation_send", { account: "Alice", text: "spawn" }),
  );
  await waitFor(() => expect(box).toHaveValue(""));
  spy.mockRestore();
});

test("relaunch status shows the pending time before the last relaunch", () => {
  vi.spyOn(ipc, "call").mockResolvedValue(null);
  wrap(
    <RelaunchSwitch
      a={acct({
        relaunch: { enabled: true },
        relaunchStatus: {
          pendingAt: 1000,
          lastRelaunch: 2000,
          count: 2,
          stopped: false,
          launching: false,
        },
      })}
    />,
  );
  expect(screen.getByRole("status")).toHaveTextContent(t("auto.pending", { time: fmtTime(1000) }));
  vi.restoreAllMocks();
});

test("the control-server example copies the real token too", async () => {
  const u = userEvent.setup();
  wrap(<TokenRows />);
  await u.click(await screen.findByRole("button", { name: /Copy: Control server example/ }));
  expect(await navigator.clipboard.readText()).toBe(
    `ws://127.0.0.1:7810/control?token=${TOKEN}&account=<name>`,
  );
});
