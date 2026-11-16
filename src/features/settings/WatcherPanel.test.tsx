import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import * as ipc from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { WatcherPanel } from "./WatcherPanel";

function setup(status: ipc.WatcherStatus) {
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "watcher_status" ? Promise.resolve(status as never) : orig(cmd, args as never),
  );
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <WatcherPanel />
    </QueryClientProvider>,
  );
}

afterEach(() => vi.restoreAllMocks());

test("watcher off says so; running with no clients says so", async () => {
  setup({ running: false, clients: [], rules: [] });
  expect(await screen.findByText(t("watcher.off"))).toBeInTheDocument();
});

test("running with no clients says so", async () => {
  setup({ running: true, clients: [], rules: [] });
  expect(await screen.findByText(t("watcher.noClients"))).toBeInTheDocument();
});

test("non-game client is labelled, a game client with no name is Unknown, rules show state", async () => {
  setup({
    running: true,
    clients: [
      {
        pid: 101,
        accountId: null,
        username: null,
        memoryMb: 150,
        title: null,
        startedAt: 0,
        gameClient: true,
        lastServerEvent: null,
      },
      {
        pid: 102,
        accountId: null,
        username: "hidden-name",
        memoryMb: 80,
        title: "Roblox",
        startedAt: 0,
        gameClient: false,
        lastServerEvent: null,
      },
    ],
    rules: [
      { id: "lowMemory", enabled: true, lastFiredAt: null },
      { id: "wrongTitle", enabled: false, lastFiredAt: null },
      { id: "noServer", enabled: true, lastFiredAt: 1_700_000_000 },
    ],
  });
  expect(await screen.findByText(t("watcher.unknown"))).toBeInTheDocument();
  expect(screen.getByText(t("watcher.notGame"))).toBeInTheDocument();
  expect(screen.queryByText("hidden-name")).toBeNull();
  expect(screen.getAllByText(t("watcher.never"))).toHaveLength(2);
  expect(screen.getByText(t("watcher.ruleOff"))).toBeInTheDocument();
  expect(screen.getByText(/Last fired/)).toBeInTheDocument();
});
