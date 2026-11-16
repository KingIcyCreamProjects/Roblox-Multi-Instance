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
