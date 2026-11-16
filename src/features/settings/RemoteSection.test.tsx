import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { t } from "@/lib/i18n";
import * as ipc from "@/lib/ipc";
import { mockCall, mockRemote, resetVaultMock } from "@/lib/mock";
import { dur, filesLine, RemoteExtras, size } from "./RemoteSection";

beforeEach(() => resetVaultMock());

const setup = () =>
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <RemoteExtras />
    </QueryClientProvider>,
  );

test("off: says so, no code until it is switched on and saved", async () => {
  setup();
  expect(await screen.findByText(t("rs.off"))).toBeInTheDocument();
  expect(screen.getByRole("button", { name: t("rs.showCode") })).toBeDisabled();
  expect(screen.getByText(t("rs.saveFirst"))).toBeInTheDocument();
  // paired computers and the session log still show
  expect(await screen.findAllByText("Laptop")).toHaveLength(3); // 1 paired + 2 sessions
  expect(
    screen.getAllByText(t("rs.logLine", { mode: t("rs.control"), dur: t("rs.min", { n: 12 }) })),
  ).toHaveLength(1);
  expect(
    screen.getByText(t("rs.logLine", { mode: t("rs.viewOnly"), dur: t("rs.sec", { n: 40 }) })),
  ).toBeInTheDocument();
});

test("on: shows the listening address and a one-time code with a countdown", async () => {
  const user = userEvent.setup();
  await mockCall("settings_set", { patch: { remote: { enabled: true, deviceName: "Den PC" } } });
  setup();
  expect(
    await screen.findByText(t("rs.listening", { name: "Den PC", addr: "100.64.0.10:7813" })),
  ).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: t("rs.showCode") }));
  expect(await screen.findByLabelText(t("rs.code"))).toHaveTextContent("K7MP 4XQ2");
  expect(screen.getByRole("timer")).toHaveTextContent(t("rs.expires", { m: 5, s: "00" }));
  expect(screen.getByRole("button", { name: t("rs.newCode") })).toBeInTheDocument();
});

test("a code that was used (or burned) disappears; Windows gets the firewall hint", async () => {
  const user = userEvent.setup();
  await mockCall("settings_set", { patch: { remote: { enabled: true } } });
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <RemoteExtras />
    </QueryClientProvider>,
  );
  await user.click(await screen.findByRole("button", { name: t("rs.showCode") }));
  expect(await screen.findByLabelText(t("rs.code"))).toBeInTheDocument();
  expect(await screen.findByText(t("rs.firewall"))).toBeInTheDocument();
  mockRemote.codeLive = false; // another computer paired with it
  await qc.invalidateQueries({ queryKey: ["remoteStatus"] });
  await waitFor(() => expect(screen.queryByLabelText(t("rs.code"))).toBeNull());
  expect(screen.getByRole("button", { name: t("rs.showCode") })).toBeInTheDocument();
});

test("remove forgets a paired computer", async () => {
  const user = userEvent.setup();
  setup();
  await user.click(
    await screen.findByRole("button", { name: t("rs.removeNamed", { name: "Laptop" }) }),
  );
  expect(await screen.findByText(t("rs.noPaired"))).toBeInTheDocument();
});

test("a live session shows who, and Stop ends it", async () => {
  const user = userEvent.setup();
  mockRemote.session = { peer: "Laptop", since: "2026-11-02T18:30:00Z", control: true };
  setup();
  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent(t("rs.liveControl", { peer: "Laptop" }));
  await user.click(within(alert).getByRole("button", { name: t("rs.stop") }));
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
});

test("a session's transfers: one compact line, the files on expand", async () => {
  const user = userEvent.setup();
  setup();
  const line = "2 files sent, 1 file received";
  expect(filesLine(mockRemote.log[0].files)).toBe(line);
  const summary = await screen.findByText(line);
  // only the session with transfers gets the line
  expect(screen.getAllByText(/files? (sent|received)/)).toHaveLength(1);
  const meta = t("rs.fileLine", { size: size(3_400_000), dir: t("rs.received") });
  expect(screen.getByText("photo.jpg")).not.toBeVisible();
  await user.click(summary);
  // the name truncates on its own; size and direction sit beside it
  expect(screen.getByText("photo.jpg")).toHaveClass("truncate");
  expect(screen.getByText(meta)).toBeVisible();
  expect(screen.getByText("photo.jpg").parentElement).toHaveTextContent(`photo.jpg${meta}`);
  expect(
    screen.getByText(t("rs.fileLine", { size: size(245_000), dir: t("rs.sent") })),
  ).toBeVisible();
});

test("transfer summaries and sizes", () => {
  const f = (dir: "sent" | "received") => ({ name: "a", size: 1, dir });
  expect(filesLine([])).toBe("");
  expect(filesLine([f("received")])).toBe(t("rs.fileReceived"));
  expect(filesLine([f("sent"), f("sent"), f("sent")])).toBe(t("rs.filesSent", { n: 3 }));
  expect(size(48)).toBe("48 B");
  expect(size(999)).toBe("999 B");
  expect(size(1500)).toBe("1.5 kB");
  expect(size(3_400_000)).toBe("3.4 MB");
  expect(size(4_000_000_000)).toBe("4 GB");
});

test("durations floor to whole units", () => {
  expect(dur(59)).toBe(t("rs.sec", { n: 59 }));
  expect(dur(119)).toBe(t("rs.min", { n: 1 }));
  expect(dur(3599)).toBe(t("rs.min", { n: 59 }));
  expect(dur(3600)).toBe(t("rs.hours", { h: 1, m: 0 }));
  expect(dur(7199)).toBe(t("rs.hours", { h: 1, m: 59 }));
});

test("loading, then an error with Retry, instead of 'Waiting for Tailscale'", async () => {
  await mockCall("settings_set", { patch: { remote: { enabled: true } } });
  const real = ipc.call;
  let fail = true;
  const spy = vi
    .spyOn(ipc, "call")
    .mockImplementation(((cmd: string, args?: unknown) =>
      cmd === "remote_status" && fail
        ? new Promise((_, no) => setTimeout(() => no("status broke"), 300))
        : real(cmd as never, args as never)) as typeof ipc.call);
  try {
    setup();
    expect(await screen.findByText(t("rs.loading"))).toBeInTheDocument();
    expect(await screen.findByText("status broke")).toBeInTheDocument();
    expect(screen.queryByText(t("rs.waiting"))).toBeNull();
    fail = false;
    await userEvent.setup().click(screen.getByRole("button", { name: t("error.retry") }));
    await waitFor(() => expect(screen.queryByText("status broke")).toBeNull());
    expect(screen.queryByText(t("rs.loading"))).toBeNull();
  } finally {
    spy.mockRestore();
  }
});
