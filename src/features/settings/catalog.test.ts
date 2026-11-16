import { GROUPS, SECTIONS, get, nest } from "./catalog";

test("get walks dotted paths and returns undefined through missing keys", () => {
  const o = { a: { b: { c: 3 } }, t: 0 };
  expect(get(o, "a.b.c")).toBe(3);
  expect(get(o, "t")).toBe(0);
  expect(get(o, "a.x.c")).toBeUndefined();
  expect(get(o, "t.deeper")).toBeUndefined();
});

test("nest builds the nested object for a dotted path", () => {
  expect(nest("a.b.c", 1)).toEqual({ a: { b: { c: 1 } } });
  expect(nest("top", "v")).toEqual({ top: "v" });
});

test("start minimized is a general switch", () => {
  const general = SECTIONS.find((s) => s.id === "general")!.items;
  expect(general.find((i) => i.path === "launch.startMinimized")?.kind).toBe("bool");
});

test("settings that do nothing on Linux are gated off there", () => {
  const items = SECTIONS.flatMap((s) => s.items);
  const cap = (p: string) => items.find((i) => i.path === p)?.cap;
  // Linux reports windowRect and clientHelpers false: no window setup, no Roblox settings file.
  expect(cap("launch.startMinimized")).toBe("windowRect");
  for (const p of ["fps.unlock", "fps.value", "graphics.set", "graphics.level"])
    expect(cap(p), p).toBe("clientHelpers");
  // Nothing ever read it, on any OS.
  expect(items.some((i) => i.path === "client.bootstrapperPassthrough")).toBe(false);
});

test("rows only some OSes can act on carry the matching capability and dependency", () => {
  const items = SECTIONS.flatMap((s) => s.items);
  const it = (p: string) => items.find((i) => i.path === p)!;
  expect(it("watcher.waitForGame")).toMatchObject({ cap: "fullscreenWait" });
  expect(it("watcher.waitForGame").dep).toContain("watcher.reconnect");
  expect(it("remote.audio").cap).toBe("audio");
  for (const p of ["watcher.closeWrongTitle", "watcher.expectedTitle", "watcher.saveWindowRects"])
    expect(it(p).cap, p).toBe("windowRect");
  // the client helpers and the control section only run while Automation is on
  for (const p of ["clients.maxInstances", "clients.closeAfterMin", "ws.port", "net.allowLan"])
    expect(it(p).dep, p).toContain("automation.enabled");
  expect(it("friend.username").max).toBe(32);
});

test("every section is shown in exactly one group", () => {
  const ids = SECTIONS.map((s) => s.id).filter((id) => id !== "homelab"); // hidden for now
  const grouped = GROUPS.flatMap((g) => g.subs);
  expect([...grouped].sort()).toEqual([...ids].sort());
  expect(new Set(grouped).size).toBe(grouped.length);
});

test("the HomeLab section is hidden until HomeLab is released", () => {
  const paths = (id: string) => SECTIONS.find((s) => s.id === id)!.items.map((i) => i.path);
  expect(paths("privacy")).not.toContain("homelab.export");
  expect(GROUPS.some((g) => (g.subs as readonly string[]).includes("homelab"))).toBe(false);
});

test("catalog ranges are sane and enum/cap fields are set where needed", () => {
  const caps = new Set([
    "launch",
    "multiInstance",
    "cookieLock",
    "windowRect",
    "watcher",
    "clientHelpers",
    "fullscreenWait",
    "audio",
  ]);
  for (const it of SECTIONS.flatMap((s) => s.items)) {
    if (it.kind === "num" || it.kind === "port") {
      expect(it.min, it.path).toBeLessThan(it.max!);
    }
    if (it.kind === "enum") expect(it.options?.length, it.path).toBeGreaterThan(1);
    if (it.cap) expect(caps.has(it.cap), it.path).toBe(true);
    if (it.step !== undefined) expect(it.step, it.path).toBeGreaterThan(0);
  }
});
