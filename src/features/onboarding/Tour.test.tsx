import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { call } from "@/lib/ipc";
import * as ipc from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { Tour, tourSteps } from "./Tour";

const caps = (os: "windows" | "linux" | "macos") => ({
  os,
  launch: true,
  launchReason: null,
  multiInstance: true,
  cookieLock: true,
  windowRect: true,
  watcher: true,
  clientHelpers: true,
  fullscreenWait: true,
  audio: true,
});

test("tour steps through and Start playing marks it done", async () => {
  await call("settings_set", { patch: { onboarding: { tourDone: false, tourSeen: [] } } });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tour caps={caps("windows")} />
    </QueryClientProvider>,
  );
  expect(await screen.findByText(t("tour.welcome.title"))).toBeInTheDocument();
  expect(screen.getByText(t("tour.step", { n: 1, total: 8 }))).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: t("tour.next") }));
  expect(screen.getByText(t("tour.add.title"))).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: t("tour.next") }));
  fireEvent.click(screen.getByRole("button", { name: t("tour.next") }));
  expect(screen.getByText(t("tour.perf.title"))).toBeInTheDocument();
  fireEvent.click(
    within(screen.getByRole("group", { name: t("tour.perf.fps") })).getByRole("button", {
      name: t("tour.perf.fpsRoblox"),
    }),
  );
  await waitFor(async () => expect((await call("settings_get")).fps.unlock).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: t("tour.next") }));
  expect(screen.getByText(t("tour.tabs.title"))).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: t("tabs.up", { name: t("nav.accounts") }) }));
  await waitFor(async () =>
    expect((await call("settings_get")).ui.tabOrder.slice(0, 2)).toEqual(["accounts", "home"]),
  );
  fireEvent.click(screen.getByRole("button", { name: t("tour.next") }));
  expect(screen.getByText(t("tour.friend.title"))).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: t("tour.next") }));
  expect(screen.getByText(t("tour.remote.title"))).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: t("tour.next") }));
  expect(screen.getByText(t("tour.def.title"))).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: t("tour.start") }));
  await waitFor(async () => expect((await call("settings_get")).onboarding.tourDone).toBe(true));
});

test("macOS ends on the Accessibility step and checks the permission", async () => {
  await call("settings_set", { patch: { onboarding: { tourDone: false, tourSeen: [] } } });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tour caps={caps("macos")} />
    </QueryClientProvider>,
  );
  expect(await screen.findByText(t("tour.step", { n: 1, total: 8 }))).toBeInTheDocument();
  for (let n = 0; n < 7; n++) fireEvent.click(screen.getByRole("button", { name: t("tour.next") }));
  expect(screen.getByText(t("tour.mac.title"))).toBeInTheDocument();
  expect(screen.queryByText(t("tour.def.title"))).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: t("tour.mac.ax") }));
  expect(await screen.findByText(t("tour.mac.axOk"))).toBeInTheDocument();
});

test("after an update only the new steps show", async () => {
  await call("settings_set", { patch: { onboarding: { tourDone: true, tourSeen: [] } } });
  const s = await call("settings_get");
  expect(tourSteps(s, caps("windows"))).toEqual(["perf", "tabs", "friend", "remote"]);
  expect(tourSteps(s, caps("macos"))).toEqual(["perf", "tabs", "friend", "remote", "mac"]);
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tour caps={caps("windows")} />
    </QueryClientProvider>,
  );
  expect(await screen.findByText(t("tour.perf.title"))).toBeInTheDocument();
  expect(screen.getByText(t("tour.step", { n: 1, total: 4 }))).toBeInTheDocument();
  for (let n = 0; n < 3; n++) fireEvent.click(screen.getByRole("button", { name: t("tour.next") }));
  fireEvent.click(screen.getByRole("button", { name: t("tour.start") }));
  await waitFor(async () =>
    expect(tourSteps(await call("settings_get"), caps("windows"))).toEqual([]),
  );
});

test("someone who finished the last tour sees only the Friend and Remote steps", async () => {
  const seen = ["welcome", "add", "pick", "perf", "tabs", "def"];
  await call("settings_set", { patch: { onboarding: { tourDone: true, tourSeen: seen } } });
  const s = await call("settings_get");
  expect(tourSteps(s, caps("windows"))).toEqual(["friend", "remote"]);
  expect(tourSteps(s, { ...caps("linux"), watcher: false })).toEqual(["remote"]);
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tour caps={caps("windows")} />
    </QueryClientProvider>,
  );
  expect(await screen.findByText(t("tour.friend.title"))).toBeInTheDocument();
  expect(screen.getByText(t("tour.step", { n: 1, total: 2 }))).toBeInTheDocument();
  expect(screen.queryByText(t("tour.welcome.title"))).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: t("tour.next") }));
  expect(screen.getByText(t("tour.remote.title"))).toBeInTheDocument();
});

test("on Linux the Remote step says it can control others, not be controlled", async () => {
  const seen = ["welcome", "add", "pick", "perf", "tabs", "def", "friend"];
  await call("settings_set", { patch: { onboarding: { tourDone: true, tourSeen: seen } } });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tour caps={{ ...caps("linux"), watcher: false }} />
    </QueryClientProvider>,
  );
  expect(await screen.findByText(t("tour.remote.safeD.linux"))).toBeInTheDocument();
  expect(screen.queryByText(t("tour.remote.safeD"))).toBeNull();
});

test("welcome skips Grid Arrange where windows can't be placed (Linux)", async () => {
  await call("settings_set", { patch: { onboarding: { tourDone: false, tourSeen: [] } } });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tour caps={{ ...caps("linux"), windowRect: false }} />
    </QueryClientProvider>,
  );
  expect(await screen.findByText(t("tour.welcome.vault"))).toBeInTheDocument();
  expect(screen.queryByText(t("tour.welcome.grid"))).toBeNull();
});

test("dragging the graphics slider saves once, on release", async () => {
  await call("settings_set", { patch: { onboarding: { tourDone: true, tourSeen: [] } } });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tour caps={caps("windows")} />
    </QueryClientProvider>,
  );
  const range = await screen.findByRole("slider", { name: t("tour.perf.gfx") });
  const spy = vi.spyOn(ipc, "call");
  for (const v of ["3", "5", "8"]) fireEvent.change(range, { target: { value: v } });
  expect(spy.mock.calls.filter(([c]) => c === "settings_set")).toHaveLength(0);
  fireEvent.pointerUp(range);
  await waitFor(async () =>
    expect((await call("settings_get")).graphics).toMatchObject({ set: true, level: 8 }),
  );
  expect(spy.mock.calls.filter(([c]) => c === "settings_set")).toHaveLength(1);
  spy.mockRestore();
});

test("tourSteps: first run shows every step for the OS; a seen list beats the legacy flag", () => {
  const ob = (onboarding: object) => ({ onboarding }) as Parameters<typeof tourSteps>[0];
  const first = ob({ tourDone: false, tourSeen: [] });
  expect(tourSteps(first, caps("windows"))).toEqual([
    "welcome",
    "add",
    "pick",
    "perf",
    "tabs",
    "friend",
    "remote",
    "def",
  ]);
  // Linux (flags as in linux.rs): the watcher is on so it gets the friend step, but no client
  // helpers so no perf step
  const linux = { ...caps("linux"), cookieLock: false, windowRect: false, clientHelpers: false };
  expect(tourSteps(first, { ...linux, fullscreenWait: false, audio: false })).toEqual([
    "welcome",
    "add",
    "pick",
    "tabs",
    "friend",
    "remote",
  ]);
  const seen = ["welcome", "add", "pick", "perf", "tabs", "friend", "remote", "homelab"];
  expect(tourSteps(ob({ tourDone: true, tourSeen: seen }), caps("windows"))).toEqual(["def"]);
});

test("Esc closes the tour and marks every step for this OS seen", async () => {
  await call("settings_set", { patch: { onboarding: { tourDone: false, tourSeen: [] } } });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tour caps={caps("windows")} />
    </QueryClientProvider>,
  );
  await screen.findByText(t("tour.welcome.title"));
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  await waitFor(async () => {
    const { onboarding } = await call("settings_get");
    expect(onboarding.tourDone).toBe(true);
    expect(onboarding.tourSeen).toEqual(expect.arrayContaining(["welcome", "def"]));
  });
});

test("Skip the tutorial ends the tour from any step but the last", async () => {
  await call("settings_set", { patch: { onboarding: { tourDone: false, tourSeen: [] } } });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tour caps={caps("windows")} />
    </QueryClientProvider>,
  );
  fireEvent.click(await screen.findByRole("button", { name: t("tour.skip") }));
  await waitFor(async () => expect((await call("settings_get")).onboarding.tourDone).toBe(true));
});

test("turning reconnect on also turns the watcher on", async () => {
  await call("settings_set", {
    patch: {
      onboarding: { tourDone: false, tourSeen: [] },
      watcher: { enabled: false, reconnect: false },
    },
  });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tour caps={caps("windows")} />
    </QueryClientProvider>,
  );
  fireEvent.click(await screen.findByRole("button", { name: t("tour.next") }));
  fireEvent.click(screen.getByRole("button", { name: t("tour.next") }));
  fireEvent.click(screen.getByRole("switch", { name: t("tour.pick.reconnect") }));
  await waitFor(async () =>
    expect((await call("settings_get")).watcher).toMatchObject({ enabled: true, reconnect: true }),
  );
});

test("the Windows Security command doubles apostrophes in the folder path", async () => {
  const orig = ipc.call;
  vi.spyOn(ipc, "call").mockImplementation((cmd, args) =>
    cmd === "defender_exclude_cmd"
      ? Promise.resolve("C:/Users/O'Brien/RMI" as never)
      : orig(cmd, args as never),
  );
  await call("settings_set", {
    patch: {
      onboarding: {
        tourDone: true,
        tourSeen: ["welcome", "add", "pick", "perf", "tabs", "friend", "remote", "homelab"],
      },
    },
  });
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Tour caps={caps("windows")} />
    </QueryClientProvider>,
  );
  fireEvent.click(await screen.findByRole("button", { name: t("tour.def.show") }));
  expect(
    screen.getByText("Add-MpPreference -ExclusionPath 'C:/Users/O''Brien/RMI'"),
  ).toBeInTheDocument();
});
