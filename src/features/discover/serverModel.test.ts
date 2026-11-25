import { act, renderHook, waitFor } from "@testing-library/react";
import * as ipc from "@/lib/ipc";
import { call, mockBus, type ServerRow, type ServersPage } from "@/lib/ipc";
import { resetDiscoverMock } from "@/lib/mockDiscover";
import {
  filterRows,
  MAX_ROWS,
  nextSort,
  noFilters,
  sortRows,
  useFindPlayer,
  useServerStream,
} from "./serverModel";

const row = (jobId: string, playing: number, ping: number | null, max = 10): ServerRow => ({
  jobId,
  playing,
  maxPlayers: max,
  ping,
  fps: 60,
  kind: "public",
  name: null,
});
const rows = [row("a", 10, 80), row("b", 0, null), row("c", 4, 30), row("d", 7, 120)];
const ids = (r: ServerRow[]) => r.map((x) => x.jobId).join("");

test("filters hide full, hide empty, min/max and region", () => {
  expect(ids(filterRows(rows, { ...noFilters, hideFull: true }, {}))).toBe("bcd");
  expect(ids(filterRows(rows, { ...noFilters, hideEmpty: true }, {}))).toBe("acd");
  expect(ids(filterRows(rows, { ...noFilters, min: 4, max: 7 }, {}))).toBe("cd");
  const regions = { c: { label: "Frankfurt, DE", countryCode: "DE" } };
  expect(ids(filterRows(rows, { ...noFilters, region: "frank" }, regions))).toBe("c");
});

test("sort puts unknown values last in both directions", () => {
  expect(ids(sortRows(rows, { key: "ping", dir: "asc" }, {}))).toBe("cadb");
  expect(ids(sortRows(rows, { key: "ping", dir: "desc" }, {}))).toBe("dacb");
  expect(ids(sortRows(rows, { key: "players", dir: "desc" }, {}))).toBe("adcb");
  expect(sortRows(rows, null, {})).toBe(rows);
});

test("sort by region: labels compare, unloaded regions stay last", () => {
  const regions = {
    a: { label: "Tokyo, JP", countryCode: "JP" },
    c: { label: "Berlin, DE", countryCode: "DE" },
  };
  expect(ids(sortRows(rows, { key: "region", dir: "asc" }, regions))).toBe("cabd");
  expect(ids(sortRows(rows, { key: "region", dir: "desc" }, regions))).toBe("acbd");
});

test("find player: running, then done with the server; cancel resets", async () => {
  resetDiscoverMock(false);
  const { result } = renderHook(() => useFindPlayer());
  await act(() => result.current.start(74260430392611, "kestrel", "acc-1"));
  expect(result.current.running).toBe(true);
  await waitFor(() => expect(result.current.done).toBe(true));
  expect(result.current.found).toMatch(/^[0-9a-f-]{36}$/);
  expect(result.current.placeId).toBe(74260430392611);
  act(() => result.current.cancel());
  expect(result.current).toMatchObject({ phase: "idle", found: null, placeId: null, done: false });
});

test("find player: a player not on any page ends done with no server", async () => {
  resetDiscoverMock(false);
  const { result } = renderHook(() => useFindPlayer());
  await act(() => result.current.start(74260430392611, "nobody", "acc-1"));
  await waitFor(() => expect(result.current.done).toBe(true), { timeout: 3000 });
  expect(result.current.found).toBeNull();
  expect(result.current.error).toBeNull();
});

test("header clicks cycle sort", () => {
  let s = nextSort(null, "ping");
  expect(s).toEqual({ key: "ping", dir: "desc" });
  s = nextSort(s, "ping");
  expect(s).toEqual({ key: "ping", dir: "asc" });
  expect(nextSort(s, "ping")).toBeNull();
  expect(nextSort(s, "jobId")).toEqual({ key: "jobId", dir: "asc" });
});

test("stream accumulates pages and Stop ends it", async () => {
  resetDiscoverMock(false);
  const { result } = renderHook(() => useServerStream());
  await act(() => result.current.start(74260430392611, "public", null));
  // 5 s: the default 1 s is too tight on a busy machine (CI, parallel builds)
  // pages waits too: rows is the shared buffer, so it can show page 3 one render early
  await waitFor(
    () => {
      expect(result.current.rows.length).toBeGreaterThanOrEqual(300);
      expect(result.current.pages).toBeGreaterThanOrEqual(3);
    },
    { timeout: 5000 },
  );
  expect(result.current.streaming).toBe(true);
  expect(new Set(result.current.rows.map((r) => r.jobId)).size).toBe(result.current.rows.length);
  act(() => result.current.stop());
  const n = result.current.rows.length;
  await new Promise((r) => setTimeout(r, 40));
  expect(result.current.rows.length).toBe(n);
  expect(result.current.streaming).toBe(false);
  expect(result.current.stopped).toBe(true); // partial: never announced as "loaded"
});

test("unmounting (tab gone, vault lock) stops the live stream", async () => {
  resetDiscoverMock(false);
  const spy = vi.spyOn(ipc, "call");
  try {
    const { result, unmount } = renderHook(() => useServerStream());
    await act(() => result.current.start(74260430392611, "public", null));
    unmount();
    expect(spy).toHaveBeenCalledWith("servers_stop", { streamId: expect.any(String) });
  } finally {
    spy.mockRestore();
  }
});

test("a stream id that arrives after Stop is stopped, and its rows never show", async () => {
  resetDiscoverMock(false);
  const spy = vi.spyOn(ipc, "call");
  try {
    const { result } = renderHook(() => useServerStream());
    await act(async () => {
      const p = result.current.start(74260430392611, "public", null);
      result.current.stop(); // before servers_stream answered
      await p;
    });
    expect(spy).toHaveBeenCalledWith("servers_stop", { streamId: expect.any(String) });
    expect(result.current.rows).toHaveLength(0);
  } finally {
    spy.mockRestore();
  }
});

test("sort by fps, kind and jobId; unknown fps last", () => {
  const mixed: ServerRow[] = [
    { ...row("b", 1, 10), fps: 30, kind: "private", name: "Squad" },
    { ...row("a", 1, 10), fps: null, kind: "public" },
    { ...row("c", 1, 10), fps: 59.5, kind: "public" },
  ];
  expect(ids(sortRows(mixed, { key: "fps", dir: "asc" }, {}))).toBe("bca");
  expect(ids(sortRows(mixed, { key: "fps", dir: "desc" }, {}))).toBe("cba");
  expect(ids(sortRows(mixed, { key: "kind", dir: "asc" }, {}))).toBe("bac");
  expect(ids(sortRows(mixed, { key: "jobId", dir: "desc" }, {}))).toBe("cba");
});

test("private stream without an account ends as an error, not streaming", async () => {
  resetDiscoverMock(false);
  const { result } = renderHook(() => useServerStream());
  await act(() => result.current.start(74260430392611, "private", null));
  expect(result.current.error).toMatch(/Private servers need a selected account\./);
  expect(result.current.streaming).toBe(false);
  expect(result.current.rows).toEqual([]);
});

test("stream stops at MAX_ROWS: partial, not streaming", async () => {
  resetDiscoverMock(false);
  let streamId = "";
  const grab = (e: Event) => {
    if (!streamId) streamId = (e as CustomEvent<ServersPage>).detail.streamId;
  };
  mockBus.addEventListener("servers://page", grab);
  const { result } = renderHook(() => useServerStream());
  await act(() => result.current.start(74260430392611, "public", null));
  await waitFor(() => expect(streamId).not.toBe(""));
  mockBus.removeEventListener("servers://page", grab);
  await call("servers_stop", { streamId }); // mock's own pages would race the ones below
  const page = (p: number): ServersPage => ({
    streamId,
    rows: Array.from({ length: 100 }, (_, k) => row(`cap-${p}-${k}`, 1, 10)),
    page: p,
    done: false,
    error: null,
    stopped: false,
  });
  for (let p = 0; p < 200 && result.current.streaming; p++)
    act(() => void mockBus.dispatchEvent(new CustomEvent("servers://page", { detail: page(p) })));
  expect(result.current.rows.length).toBe(MAX_ROWS);
  expect(result.current.streaming).toBe(false);
  expect(result.current.stopped).toBe(true);
});
