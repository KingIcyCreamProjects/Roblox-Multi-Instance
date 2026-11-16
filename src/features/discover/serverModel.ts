// Servers tab logic: pure filter/sort plus the streaming hooks (servers://page, servers://find).
import { useCallback, useEffect, useRef, useState } from "react";
import {
  call,
  errMessage,
  onEvent,
  type FindProgress,
  type Region,
  type ServerKind,
  type ServerRow,
  type ServersPage,
} from "@/lib/ipc";
import { logError } from "@/lib/report";

export type Filters = {
  hideFull: boolean;
  hideEmpty: boolean;
  min: number | null;
  max: number | null;
  region: string;
};
export const noFilters: Filters = {
  hideFull: false,
  hideEmpty: false,
  min: null,
  max: null,
  region: "",
};

export type SortKey = "players" | "ping" | "fps" | "region" | "kind" | "jobId";
export type Sort = { key: SortKey; dir: "asc" | "desc" } | null;
export type Regions = Record<string, Region | undefined>;

/** A region filter only keeps rows whose region is loaded and matches. */
export function filterRows(rows: ServerRow[], f: Filters, regions: Regions): ServerRow[] {
  const q = f.region.trim().toLowerCase();
  return rows.filter(
    (r) =>
      !(f.hideFull && r.playing >= r.maxPlayers) &&
      !(f.hideEmpty && r.playing === 0) &&
      (f.min === null || r.playing >= f.min) &&
      (f.max === null || r.playing <= f.max) &&
      (!q || !!regions[r.jobId]?.label.toLowerCase().includes(q)),
  );
}

const value = (r: ServerRow, key: SortKey, regions: Regions): number | string | null =>
  key === "players"
    ? r.playing
    : key === "ping"
      ? r.ping
      : key === "fps"
        ? r.fps
        : key === "region"
          ? (regions[r.jobId]?.label ?? null)
          : key === "kind"
            ? r.kind
            : r.jobId;

/** Stable sort; unknown values (no ping, region not loaded) always sort last. */
export function sortRows(rows: ServerRow[], s: Sort, regions: Regions): ServerRow[] {
  if (!s) return rows;
  const m = s.dir === "asc" ? 1 : -1;
  return [...rows].sort((a, b) => {
    const x = value(a, s.key, regions);
    const y = value(b, s.key, regions);
    if (x === y) return 0;
    if (x === null) return 1;
    if (y === null) return -1;
    return (x < y ? -1 : 1) * m;
  });
}

/** Next sort when a header is clicked: desc → asc → off for numbers, asc → desc → off for text. */
export function nextSort(cur: Sort, key: SortKey): Sort {
  const first = key === "region" || key === "kind" || key === "jobId" ? "asc" : "desc";
  if (cur?.key !== key) return { key, dir: first };
  return cur.dir === first ? { key, dir: first === "asc" ? "desc" : "asc" } : null;
}

type StreamEvent = { streamId: string; done: boolean; error: string | null };

/** One backend stream at a time: buffers events that beat the streamId reply, stops on unmount. */
function useStream<E extends StreamEvent>(event: string, onData: (e: E) => void) {
  const id = useRef<string | null>(null);
  const early = useRef<E[] | null>(null);
  const run = useRef(0);
  const cb = useRef(onData);
  cb.current = onData;

  const deliver = useCallback((e: E) => {
    if (e.streamId !== id.current) return;
    if (e.done || e.error) id.current = null;
    cb.current(e);
  }, []);
  useEffect(
    () => onEvent<E>(event, (e) => (early.current ? void early.current.push(e) : deliver(e))),
    [event, deliver],
  );

  const stop = useCallback(() => {
    run.current += 1;
    early.current = null;
    const s = id.current;
    id.current = null;
    if (s) call("servers_stop", { streamId: s }).catch((e) => logError(String(e)));
  }, []);

  const start = useCallback(
    async (open: () => Promise<{ streamId: string }>) => {
      stop();
      const my = run.current;
      early.current = [];
      try {
        const { streamId } = await open();
        if (my !== run.current) {
          void call("servers_stop", { streamId }).catch((e) => logError(String(e)));
          return;
        }
        id.current = streamId;
        const q = early.current ?? [];
        early.current = null;
        q.forEach(deliver);
      } catch (e) {
        if (my !== run.current) return;
        early.current = null;
        throw e;
      }
    },
    [stop, deliver],
  );

  useEffect(() => stop, [stop]);
  return { start, stop };
}

/** ponytail: hard cap so a huge place cannot exhaust memory; the stream stops at the cap.
 * Same as Rust's MAX_PAGES (100 pages of 100 = 10k); raise both together. */
export const MAX_ROWS = 10_000;

type ListState = {
  /** shared buffer, appended in place; `version` changes whenever it grows */
  rows: ServerRow[];
  version: number;
  pages: number;
  streaming: boolean;
  /** ended early (Stop, row cap, cancelled in Rust): the list is partial */
  stopped: boolean;
  error: string | null;
};
const idle = (): ListState => ({
  rows: [],
  version: 0,
  pages: 0,
  streaming: false,
  stopped: false,
  error: null,
});

export function useServerStream() {
  const [st, setSt] = useState(idle);
  const buf = useRef<ServerRow[]>([]);
  const seen = useRef(new Set<string>());
  const closeRef = useRef<() => void>(() => {});
  const { start: open, stop: close } = useStream<ServersPage>("servers://page", (p) => {
    for (const r of p.rows) {
      if (buf.current.length >= MAX_ROWS) break;
      if (seen.current.has(r.jobId)) continue;
      seen.current.add(r.jobId);
      buf.current.push(r);
    }
    const capped = buf.current.length >= MAX_ROWS && !p.done;
    if (capped) closeRef.current();
    setSt((x) => ({
      rows: buf.current,
      version: x.version + 1,
      pages: p.stopped ? x.pages : p.page,
      streaming: !p.done && !p.error && !capped,
      stopped: p.stopped || capped,
      error: p.error,
    }));
  });
  closeRef.current = close;
  const start = useCallback(
    async (placeId: number, kind: ServerKind, accountId: string | null) => {
      buf.current = [];
      seen.current = new Set();
      setSt({ ...idle(), rows: buf.current, streaming: true });
      try {
        await open(() => call("servers_stream", { placeId, kind, accountId }));
      } catch (e) {
        setSt({ ...idle(), rows: buf.current, error: errMessage(e) });
      }
    },
    [open],
  );
  const stop = useCallback(() => {
    close();
    setSt((x) => (x.streaming ? { ...x, streaming: false, stopped: true } : x));
  }, [close]);
  const has = useCallback((jobId: string) => seen.current.has(jobId), []);
  return { ...st, start, stop, has };
}

/** One phase, so "running and done" can't both hold. A cancelled scan goes back to idle. */
type FindPhase = "idle" | "running" | "done";
type FindState = {
  /** place the search ran in; "Use this server" sends this, not the current input */
  placeId: number | null;
  phase: FindPhase;
  scanned: number;
  found: string | null;
  error: string | null;
};
const findIdle: FindState = { placeId: null, phase: "idle", scanned: 0, found: null, error: null };

export function useFindPlayer() {
  const [st, setSt] = useState(findIdle);
  const { start: open, stop: close } = useStream<FindProgress>("servers://find", (p) =>
    setSt((x) =>
      p.stopped
        ? findIdle
        : {
            placeId: x.placeId,
            phase: p.done || p.error ? "done" : "running",
            scanned: p.pagesScanned,
            found: p.found?.jobId ?? null,
            error: p.error,
          },
    ),
  );
  const start = useCallback(
    async (placeId: number, username: string, accountId: string) => {
      setSt({ ...findIdle, placeId, phase: "running" });
      try {
        await open(() => call("servers_find_player", { placeId, username, accountId }));
      } catch (e) {
        setSt({ ...findIdle, phase: "done", error: errMessage(e) });
      }
    },
    [open],
  );
  const cancel = useCallback(() => {
    close();
    setSt(findIdle);
  }, [close]);
  return {
    ...st,
    running: st.phase === "running",
    done: st.phase === "done",
    start,
    cancel,
  };
}
