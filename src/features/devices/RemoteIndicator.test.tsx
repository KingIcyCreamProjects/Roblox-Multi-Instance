import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { t } from "@/lib/i18n";
import * as ipc from "@/lib/ipc";
import { mockRemote, resetVaultMock } from "@/lib/mock";
import { RemoteIndicator } from "./RemoteIndicator";

beforeEach(() => resetVaultMock());

const setup = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <RemoteIndicator />
    </QueryClientProvider>,
  );

test.each([
  [false, "rs.liveView"],
  [true, "rs.liveControl"],
] as const)("control %s: says viewing or controlling, never 'PC'", async (control, key) => {
  mockRemote.session = { peer: "Laptop", since: "2026-11-02T18:30:00Z", control };
  setup();
  expect(await screen.findByText(t(key, { peer: "Laptop" }))).toHaveAttribute("role", "status");
  expect(screen.getByRole("status")).not.toHaveTextContent("PC");
});

test("no session yet: the generic line; Stop sends remote_stop", async () => {
  const user = userEvent.setup();
  mockRemote.session = null;
  const spy = vi.spyOn(ipc, "call");
  try {
    setup();
    expect(await screen.findByText(t("ri.title"))).toHaveAttribute("role", "status");
    await user.click(screen.getByRole("button", { name: t("rs.stop") }));
    expect(spy).toHaveBeenCalledWith("remote_stop");
    expect(screen.queryByRole("alert")).toBeNull();
  } finally {
    spy.mockRestore();
  }
});

test("a failed Stop says why and stays pressable for a retry", async () => {
  const user = userEvent.setup();
  mockRemote.session = { peer: "Laptop", since: "2026-11-02T18:30:00Z", control: false };
  const orig = ipc.call;
  const spy = vi
    .spyOn(ipc, "call")
    .mockImplementation((cmd, args) =>
      cmd === "remote_stop"
        ? Promise.reject(new Error("No session to stop."))
        : orig(cmd, args as never),
    );
  try {
    setup();
    const stop = await screen.findByRole("button", { name: t("rs.stop") });
    await user.click(stop);
    expect(await screen.findByRole("alert")).toHaveTextContent("No session to stop.");
    expect(stop).toBeEnabled();
  } finally {
    spy.mockRestore();
  }
});
