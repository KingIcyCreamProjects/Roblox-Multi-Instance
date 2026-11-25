import * as ContextMenu from "@radix-ui/react-context-menu";
import * as ToggleGroup from "@radix-ui/react-toggle-group";
import { useQueryClient } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import clsx from "clsx";
import {
  ArrowDown,
  ArrowUp,
  Copy,
  Link2,
  MapPin,
  Play,
  RefreshCw,
  Square,
  User,
  X,
} from "lucide-react";
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
} from "react";
import { useUi } from "@/app/store";
import { useSettings } from "@/app/useSettings";
import { toast } from "@/components/Toasts";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { placeIdOf, saveLaunchSet, usePlayBar } from "@/features/playbar/store";
import {
  clearPlaceholder,
  launchBlocked,
  launchBusy,
  useLaunch,
  usePlay,
} from "@/features/playbar/usePlay";
import { call, errMessage, type ServerKind, type ServerRow } from "@/lib/ipc";
import { fmtInt, t, type MsgKey } from "@/lib/i18n";
import { reportError } from "@/lib/report";
import {
  filterRows,
  nextSort,
  noFilters,
  sortRows,
  useFindPlayer,
  useServerStream,
  type Filters,
  type Regions,
  type Sort,
  type SortKey,
} from "./serverModel";
import {
  copyText,
  inputCls,
  joinLink,
  Kv,
  PanelEmpty,
  PanelTitle,
  Split,
  useDiscover,
  useRateLimited,
} from "./shared";

/** Row and header height at 100% UI scale (2.25rem); the virtualizer needs it in px. */
const ROW_H = 36;
const ROW_CSS = "2.25rem";
// rem tracks: they grow with the rem text at a bigger UI scale instead of overlapping
const COLS =
  "grid grid-cols-[minmax(6.5rem,1.2fr)_3.5rem_2.5rem_var(--region-col,minmax(7rem,1fr))_minmax(3.5rem,0.7fr)] items-center gap-2.5 px-3";

const HEADERS: [SortKey, MsgKey][] = [
  ["players", "servers.col.players"],
  ["ping", "servers.col.ping"],
  ["fps", "servers.col.fps"],
  ["region", "servers.col.region"],
  ["kind", "servers.col.type"],
];

const menuContent = "menu z-50 min-w-[13.75rem] glass rounded-panel p-1 text-13 text-text-1";
const menuItem =
  "flex h-8 cursor-default items-center gap-2 rounded-chip px-2.5 outline-none select-none focus-visible:shadow-none data-[highlighted]:bg-surface-4 data-[disabled]:text-text-3";

const num = (s: string) => (s.trim() === "" || !Number.isFinite(+s) ? null : Math.max(0, +s));

export function Servers() {
  const place = useDiscover((d) => d.place);
  const setPlace = useDiscover((d) => d.setPlace);
  const refreshPending = useDiscover((d) => d.refreshPending);
  const selIds = useUi((u) => u.sel.ids);
  const account = selIds[0] ?? null;
  const { play } = usePlay();
  const qc = useQueryClient();
  const list = useServerStream();
  const find = useFindPlayer();
  const [kind, setKind] = useState<ServerKind>("public");
  const [f, setF] = useState<Filters>(noFilters);
  const [minS, setMinS] = useState("");
  const [maxS, setMaxS] = useState("");
  const [sort, setSort] = useState<Sort>(null);
  const [regions, setRegions] = useState<Regions>({});
  const [regionsBusy, setRegionsBusy] = useState(false);
  /** Epoch of the region load in flight, if any. */
  const regionsLoading = useRef<number | null>(null);
  const [username, setUsername] = useState("");
  const [activeId, setActiveId] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const gridId = useId();
  const placeId = placeIdOf(place);
  /** place the current rows belong to */
  const [rowsPlace, setRowsPlace] = useState<number | null>(null);

  const view = useMemo(
    () =>
      sortRows(
        filterRows(list.rows, { ...f, min: num(minS), max: num(maxS) }, regions),
        sort,
        regions,
      ),
    // list.rows is appended in place; version marks growth
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [list.rows, list.version, f, minS, maxS, regions, sort],
  );
  const indexOf = useMemo(() => new Map(view.map((r, i) => [r.jobId, i])), [view]);
  const active = activeId === null ? -1 : (indexOf.get(activeId) ?? -1);

  const rowH = (ROW_H * (useSettings().data?.ui.scale ?? 100)) / 100;
  const v = useVirtualizer({
    count: view.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowH,
    overscan: 12,
    getItemKey: (i) => view[i].jobId,
    // rows sit below the sticky header (same height) inside the same scroller
    scrollMargin: rowH,
    scrollPaddingStart: rowH,
  });
  useEffect(() => v.measure(), [v, rowH]);
  const reveal = (i: number) => v.scrollToIndex(i, { align: "auto" });

  const canStream = placeId !== null && (kind === "public" || !!account);
  /** Bumped whenever the region map is cleared: a load started before must not merge in. */
  const regionsEpoch = useRef(0);
  const clearRegions = () => {
    regionsEpoch.current += 1;
    setRegions({});
    // An older load may still run (Rust cancels it when the next one starts): don't wait for it.
    setRegionsBusy(false);
  };
  const refresh = (k: ServerKind = kind, p: number | null = placeId) => {
    if (p === null || (k === "private" && !account)) return;
    clearRegions();
    setActiveId(null);
    setRowsPlace(p);
    void list.start(p, k, account);
  };
  /** Rows belong to the kind they were fetched as: switching kind refetches. */
  const pickKind = (k: ServerKind) => {
    setKind(k);
    if (rowsPlace !== null || list.streaming) refresh(k, placeId ?? rowsPlace);
  };
  // A lookup-provider or format change makes the shown regions wrong (Rust caches by both).
  const { data: settings } = useSettings();
  const geo = JSON.stringify((settings as { geo?: unknown } | undefined)?.geo ?? null);
  const geoOn = (settings as { geo?: { enabled?: boolean } } | undefined)?.geo?.enabled !== false;
  const geoSeen = useRef(geo);
  useEffect(() => {
    if (geo === geoSeen.current) return;
    geoSeen.current = geo;
    clearRegions();
  }, [geo]);

  // "View servers" from Games: refresh for the new place (now, or when this tab mounts).
  useEffect(() => {
    if (!refreshPending) return;
    useDiscover.setState({ refreshPending: false });
    refresh();
    // refresh reads the latest place on this render
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshPending]);

  // Found a player: highlight and scroll to the row.
  useEffect(() => {
    if (!find.found) return;
    setActiveId(find.found);
    const i = indexOf.get(find.found);
    if (i !== undefined) v.scrollToIndex(i, { align: "center" });
    // only when a new match arrives
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [find.found]);

  const row = active >= 0 ? view[active] : undefined;

  /** Place first (it resets server and mode), then the server. */
  const toBar = (jobId: string, at: number | null = rowsPlace) => {
    const bar = usePlayBar.getState();
    if (at !== null) bar.setDestination(String(at));
    bar.setServer(jobId);
    bar.setMode("specific");
    toast(t("servers.serverSet"));
  };
  const join = (r: ServerRow) => {
    // before the bar changes: a refused launch must not leave the server in it
    const why = launchBlocked(qc) ?? (launchBusy() ? t("launch.busy") : null);
    if (why) return void toast(why);
    toBar(r.jobId);
    play(selIds, { here: true });
  };
  /** Reserves the server per account (sub-places need it), then the batch joins it.
   * Its batch shows on the Play button once its events arrive. */
  const reserveJoin = (r: ServerRow) => {
    if (rowsPlace === null || !selIds.length) return;
    const why = launchBlocked(qc);
    if (why) return void toast(why);
    if (launchBusy()) return void toast(t("launch.busy"));
    saveLaunchSet(selIds); // Home's hero Play reads it
    // Busy while the reservations run (Rust returns only after one gamejoin per account), so
    // Play and Join can't start a second batch. The real batch shows once its events arrive.
    useLaunch.setState({
      batch: { id: null, ids: selIds, done: null, reserve: true },
      focus: false,
    });
    call("servers_reserve_join", { placeId: rowsPlace, jobId: r.jobId, accountIds: selIds }).then(
      () => {
        clearPlaceholder(selIds);
        toast(t("servers.reserveStarted"));
      },
      (e) => {
        clearPlaceholder(selIds);
        toast(errMessage(e));
      },
    );
  };
  /** Every region load (button, context menu) goes through here: one at a time. */
  const loadRegions = async (jobIds: string[]) => {
    const epoch = regionsEpoch.current;
    // one at a time per list: a load from before a refresh doesn't count
    if (!account || rowsPlace === null || !jobIds.length || regionsLoading.current === epoch)
      return;
    regionsLoading.current = epoch;
    setRegionsBusy(true);
    try {
      const got = await call("servers_regions", { placeId: rowsPlace, jobIds, accountId: account });
      if (epoch === regionsEpoch.current) setRegions((r) => ({ ...r, ...got }));
    } catch (e) {
      // a load from before a refresh or place change is stale: its error too
      if (epoch === regionsEpoch.current) reportError(e);
    } finally {
      if (regionsLoading.current === epoch) {
        regionsLoading.current = null;
        setRegionsBusy(false);
      }
    }
  };
  // A region filter hides rows whose region isn't loaded: then look up the next unloaded
  // rows that pass the other filters (50: Rust MAX_REGION_JOBS), not just the shown ones.
  const visibleIds = () =>
    f.region.trim()
      ? filterRows(list.rows, { ...f, region: "", min: num(minS), max: num(maxS) }, regions)
          .filter((r) => !(r.jobId in regions))
          .slice(0, 50)
          .map((r) => r.jobId)
      : v
          .getVirtualItems()
          .map((i) => view[i.index]?.jobId)
          .filter((j): j is string => !!j && !(j in regions));

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget) return; // header sort buttons handle their own keys
    const page = Math.max(1, Math.floor((scrollRef.current?.clientHeight ?? 360) / rowH) - 1);
    const step: Record<string, number> = {
      ArrowDown: 1,
      ArrowUp: -1,
      PageDown: page,
      PageUp: -page,
      Home: -Infinity,
      End: Infinity,
    };
    if (e.key in step) {
      e.preventDefault();
      if (!view.length) return;
      const i = Math.min(view.length - 1, Math.max(0, (active < 0 ? -1 : active) + step[e.key]));
      setActiveId(view[i].jobId);
      reveal(i);
    } else if (e.key === "Enter" && row) {
      e.preventDefault();
      toBar(row.jobId);
    } else if (row && (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "c") {
      e.preventDefault();
      void copyText(row.jobId, t("copy.jobId"));
    }
  };

  // a 429 mid-scan can stall it for minutes: say so instead of a frozen count
  const slowed = useRateLimited();
  const findErr =
    find.error ?? (find.done && !find.found ? t("servers.find.none", { n: find.scanned }) : null);

  const findForm = (
    <form
      className="flex flex-col gap-2"
      onSubmit={(e) => {
        e.preventDefault();
        if (find.running) return find.cancel();
        if (placeId !== null && account && username.trim())
          void find.start(placeId, username.trim(), account);
      }}
    >
      <label htmlFor={`${gridId}-find`} className="text-12 font-medium text-text-2">
        {t("servers.find.label")}
      </label>
      <div className="flex gap-2">
        <div className="relative min-w-0 flex-1">
          <User
            size={14}
            strokeWidth={1.75}
            aria-hidden
            className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-text-3"
          />
          <input
            id={`${gridId}-find`}
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            className={clsx(inputCls, "w-full pl-8")}
            data-pii
          />
        </div>
        <span title={!account ? t("servers.needsAccount") : undefined}>
          <Button
            type="submit"
            disabled={!find.running && (placeId === null || !account || !username.trim())}
          >
            {find.running ? (
              <>
                <X size={14} strokeWidth={1.75} />
                {t("servers.find.cancel")}
              </>
            ) : (
              t("servers.find.go")
            )}
          </Button>
        </span>
      </div>
      <p role="status" className="text-12 text-text-2 tnum">
        {find.running
          ? t(slowed ? "servers.find.slowed" : "servers.find.scanned", { n: find.scanned })
          : find.found
            ? indexOf.has(find.found)
              ? t("servers.find.found", { n: find.scanned })
              : find.placeId === rowsPlace && list.has(find.found)
                ? t("servers.find.foundHidden", { job: find.found.slice(0, 8) })
                : t("servers.find.foundUnlisted", { job: find.found.slice(0, 8) })
            : (findErr ?? (!account ? t("servers.needsAccount") : null))}
      </p>
      {find.found && !find.running && row?.jobId !== find.found && (
        <Button className="self-start" onClick={() => toBar(find.found!, find.placeId)}>
          {t("servers.setServer")}
        </Button>
      )}
    </form>
  );

  const region = row ? regions[row.jobId] : undefined;
  const panel = (
    <>
      {row ? (
        <>
          <PanelTitle
            title={
              region
                ? region.label
                : row.kind === "private"
                  ? (row.name ?? t("servers.private"))
                  : row.jobId.slice(0, 8)
            }
            sub={t("servers.playersOf", { n: row.playing, max: row.maxPlayers })}
          />
          <Kv
            rows={[
              [
                t("servers.col.ping"),
                row.ping === null ? t("servers.unknown") : t("servers.ms", { n: row.ping }),
              ],
              [t("servers.col.fps"), row.fps === null ? t("servers.unknown") : row.fps.toFixed(0)],
              [
                t("servers.col.type"),
                row.kind === "private" ? (row.name ?? t("servers.private")) : t("servers.public"),
              ],
              [
                t("servers.col.jobId"),
                <span className="font-mono text-12 break-all whitespace-normal">{row.jobId}</span>,
              ],
            ]}
          />
          <div className="flex flex-col gap-2">
            <Button variant="primary" onClick={() => toBar(row.jobId)}>
              <MapPin size={14} strokeWidth={1.75} />
              {t("servers.setServer")}
            </Button>
            <Button disabled={!selIds.length} onClick={() => join(row)}>
              <Play size={14} strokeWidth={1.75} />
              {selIds.length > 1 ? t("servers.joinN", { n: selIds.length }) : t("servers.join")}
            </Button>
            <Button
              disabled={!selIds.length || rowsPlace === null}
              onClick={() => reserveJoin(row)}
            >
              <Link2 size={14} strokeWidth={1.75} />
              {t("servers.reserveJoin")}
            </Button>
            <div className="flex flex-wrap gap-1">
              <Button variant="ghost" onClick={() => void copyText(row.jobId, t("copy.jobId"))}>
                <Copy size={14} strokeWidth={1.75} />
                {t("servers.copyJob")}
              </Button>
              <Button
                variant="ghost"
                disabled={rowsPlace === null}
                onClick={() =>
                  void copyText(joinLink(rowsPlace!, row.jobId), t("servers.joinLinkWhat"))
                }
              >
                <Copy size={14} strokeWidth={1.75} />
                {t("servers.copyLink")}
              </Button>
            </div>
          </div>
        </>
      ) : (
        // nothing to pick before a lookup: the list's prompt says what to do
        list.rows.length > 0 && <PanelEmpty>{t("discover.pick")}</PanelEmpty>
      )}
      <div className="mt-auto border-t border-hairline pt-4">{findForm}</div>
    </>
  );

  return (
    <Split
      label={t("discover.servers")}
      panel={panel}
      list={
        <>
          <form
            className="flex h-11 shrink-0 items-center gap-2 pb-1"
            onSubmit={(e) => {
              e.preventDefault();
              refresh(); // Enter loads the typed place; the button alone stops
            }}
          >
            <input
              aria-label={t("servers.place")}
              placeholder={t("servers.place")}
              value={place}
              onChange={(e) => setPlace(e.target.value)}
              className={clsx(inputCls, "min-w-0 flex-1 font-mono tnum placeholder:font-sans")}
              inputMode="numeric"
            />
            <ToggleGroup.Root
              type="single"
              value={kind}
              onValueChange={(k) => k && pickKind(k as ServerKind)}
              aria-label={t("servers.kind")}
              className="flex gap-0.5 rounded-full bg-surface-3 p-0.5 shadow-[inset_0_0_0_1px_var(--hairline)]"
            >
              {(["public", "private"] as const).map((k) => (
                <ToggleGroup.Item
                  key={k}
                  value={k}
                  disabled={k === "private" && !account}
                  title={k === "private" && !account ? t("servers.privateNeedsAccount") : undefined}
                  className="h-7 rounded-full px-3 text-12 font-medium text-text-2 transition-colors hover:text-text-1 disabled:opacity-40 data-[state=on]:bg-surface-4 data-[state=on]:text-text-1"
                >
                  {t(k === "public" ? "servers.public" : "servers.private")}
                </ToggleGroup.Item>
              ))}
            </ToggleGroup.Root>
            <IconButton
              type="button"
              onClick={() => (list.streaming ? list.stop() : refresh())}
              label={list.streaming ? t("servers.stop") : t("servers.refresh")}
              disabled={!list.streaming && !canStream}
              className="bg-surface-3"
            >
              {list.streaming ? (
                <Square size={12} strokeWidth={1.75} fill="currentColor" />
              ) : (
                <RefreshCw size={14} strokeWidth={1.75} />
              )}
            </IconButton>
            {/* announce once, when the stream ends, not on every page or filter change */}
            <p aria-live="polite" className="sr-only">
              {!list.streaming && list.rows.length > 0
                ? t(list.stopped ? "servers.stopped" : "servers.loaded", {
                    n: fmtInt(list.rows.length),
                  })
                : ""}
            </p>
          </form>

          {/* filters */}
          <div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-1.5 px-1 pb-2 text-12 text-text-2">
            {/* own line, so the filters below always start at the left edge */}
            {list.rows.length > 0 && (
              <p className="w-full text-text-3 tnum">
                {t(list.streaming ? "servers.countStreaming" : "servers.count", {
                  shown: fmtInt(view.length),
                  n: fmtInt(list.rows.length),
                  pages: list.pages,
                })}
              </p>
            )}
            {(
              [
                ["hideFull", "servers.hideFull"],
                ["hideEmpty", "servers.hideEmpty"],
              ] as const
            ).map(([k, label]) => (
              <label key={k} className="flex cursor-pointer items-center gap-1.5 hover:text-text-1">
                <input
                  type="checkbox"
                  checked={f[k]}
                  onChange={(e) => setF({ ...f, [k]: e.target.checked })}
                  className="size-3.5 accent-[var(--accent)]"
                />
                {t(label)}
              </label>
            ))}
            {/* min and max wrap together */}
            <span className="flex items-center gap-4">
              <label className="flex items-center gap-1.5">
                {t("servers.minPlayers")}
                <input
                  type="number"
                  min={0}
                  value={minS}
                  onChange={(e) => setMinS(e.target.value)}
                  className={clsx(inputCls, "h-7 w-14 text-12 tnum")}
                />
              </label>
              <label className="flex items-center gap-1.5">
                {t("servers.maxPlayers")}
                <input
                  type="number"
                  min={0}
                  value={maxS}
                  onChange={(e) => setMaxS(e.target.value)}
                  className={clsx(inputCls, "h-7 w-14 text-12 tnum")}
                />
              </label>
            </span>
            <input
              aria-label={t("servers.regionFilter")}
              placeholder={t("servers.regionFilter")}
              value={f.region}
              onChange={(e) => setF({ ...f, region: e.target.value })}
              className={clsx(inputCls, "h-7 w-28 text-12")}
            />
            <span
              title={
                !account ? t("servers.needsAccount") : !geoOn ? t("servers.geoOff") : undefined
              }
            >
              <Button
                sm
                variant="ghost"
                disabled={!account || !geoOn || !visibleIds().length || regionsBusy}
                onClick={() => void loadRegions(visibleIds())}
              >
                <MapPin size={14} strokeWidth={1.75} />
                {regionsBusy ? t("servers.loadingRegions") : t("servers.loadRegions")}
              </Button>
            </span>
            {/* why Load regions is off, readable without hovering */}
            {list.rows.length > 0 && (!account || !geoOn) && (
              <span className="text-text-3">
                {!account ? t("servers.needsAccount") : t("servers.geoOff")}
              </span>
            )}
          </div>

          {list.error && list.rows.length > 0 && (
            <div role="alert" className="flex items-center gap-3 text-13 text-text-1">
              <p>{t("servers.streamErr", { error: list.error })}</p>
              <Button onClick={() => refresh()}>{t("error.retry")}</Button>
            </div>
          )}

          {/* table: the scroller is the grid; the header row sticks inside it */}
          <ContextMenu.Root modal={false}>
            <ContextMenu.Trigger asChild>
              <div
                ref={scrollRef}
                role="grid"
                aria-label={t("servers.table")}
                aria-rowcount={view.length + 1}
                aria-colcount={HEADERS.length}
                aria-activedescendant={row ? `${gridId}-${row.jobId}` : undefined}
                tabIndex={0}
                onKeyDown={onKey}
                // blank scroller space: no menu for the previously active row (Radix skips a prevented event)
                onContextMenu={(e) => {
                  if (e.button === 2 && !(e.target as HTMLElement).closest("[role=row]"))
                    e.preventDefault();
                }}
                onFocus={(e) => {
                  if (e.target !== e.currentTarget || !view.length) return;
                  if (active < 0) setActiveId(view[0].jobId);
                  else reveal(active);
                }}
                className="servers-grid relative min-h-0 flex-1 overflow-auto pb-2 outline-none"
                // the Region column stays narrow until regions are loaded
                style={
                  {
                    "--region-col": Object.keys(regions).length ? undefined : "4rem",
                  } as CSSProperties
                }
              >
                <div
                  role="row"
                  aria-rowindex={1}
                  onContextMenu={(e) => e.stopPropagation()}
                  className={clsx(
                    COLS,
                    "sticky top-0 z-10 border-b border-hairline bg-[var(--glass)] backdrop-blur-md",
                  )}
                  style={{ height: ROW_CSS }}
                >
                  {HEADERS.map(([key, label]) => {
                    const on = sort?.key === key;
                    return (
                      <div
                        key={key}
                        role="columnheader"
                        aria-sort={on ? (sort.dir === "asc" ? "ascending" : "descending") : "none"}
                      >
                        <button
                          type="button"
                          onClick={() => setSort(nextSort(sort, key))}
                          className={clsx(
                            "-mx-1.5 inline-flex h-7 items-center gap-1 rounded-chip px-1.5 text-12 font-medium transition-colors hover:bg-surface-3 hover:text-text-1",
                            on ? "text-text-1" : "text-text-3",
                          )}
                        >
                          {t(label)}
                          {on &&
                            (sort.dir === "asc" ? (
                              <ArrowUp size={12} strokeWidth={1.75} />
                            ) : (
                              <ArrowDown size={12} strokeWidth={1.75} />
                            ))}
                        </button>
                      </div>
                    );
                  })}
                </div>
                {view.length === 0 ? (
                  <div role="row">
                    <div role="gridcell">
                      <Empty
                        streaming={list.streaming}
                        error={list.error}
                        hasRows={list.rows.length > 0}
                        retry={() => refresh()}
                      />
                    </div>
                  </div>
                ) : (
                  <div style={{ height: v.getTotalSize() - rowH }} className="relative">
                    {v.getVirtualItems().map((vi) => {
                      const r = view[vi.index];
                      return (
                        <Row
                          key={vi.key}
                          id={`${gridId}-${r.jobId}`}
                          r={r}
                          index={vi.index}
                          top={vi.start - rowH}
                          region={regions[r.jobId]}
                          active={vi.index === active}
                          found={r.jobId === find.found}
                          onPick={() => setActiveId(r.jobId)}
                          onOpen={() => toBar(r.jobId)}
                        />
                      );
                    })}
                  </div>
                )}
              </div>
            </ContextMenu.Trigger>
            <ContextMenu.Portal>
              {row && (
                <ContextMenu.Content className={menuContent} collisionPadding={8}>
                  <ContextMenu.Item
                    className={menuItem}
                    disabled={!selIds.length}
                    onSelect={() => join(row)}
                  >
                    {selIds.length > 1
                      ? t("servers.joinN", { n: selIds.length })
                      : t("servers.join")}
                  </ContextMenu.Item>
                  <ContextMenu.Item
                    className={menuItem}
                    disabled={!selIds.length || rowsPlace === null}
                    onSelect={() => reserveJoin(row)}
                  >
                    {t("servers.reserveJoin")}
                  </ContextMenu.Item>
                  <ContextMenu.Item className={menuItem} onSelect={() => toBar(row.jobId)}>
                    {t("servers.setServer")}
                  </ContextMenu.Item>
                  <ContextMenu.Separator className="mx-2 my-1 h-px bg-hairline" />
                  <ContextMenu.Item
                    className={menuItem}
                    onSelect={() => void copyText(row.jobId, t("copy.jobId"))}
                  >
                    {t("servers.copyJob")}
                  </ContextMenu.Item>
                  <ContextMenu.Item
                    className={menuItem}
                    disabled={rowsPlace === null}
                    onSelect={() =>
                      void copyText(joinLink(rowsPlace!, row.jobId), t("servers.joinLinkWhat"))
                    }
                  >
                    {t("servers.copyLink")}
                  </ContextMenu.Item>
                  <ContextMenu.Item
                    className={menuItem}
                    disabled={!account || !geoOn || regionsBusy}
                    onSelect={() => void loadRegions([row.jobId])}
                  >
                    {t("servers.loadRegion")}
                  </ContextMenu.Item>
                </ContextMenu.Content>
              )}
            </ContextMenu.Portal>
          </ContextMenu.Root>
        </>
      }
    />
  );
}

function Empty({
  streaming,
  error,
  hasRows,
  retry,
}: {
  streaming: boolean;
  error: string | null;
  hasRows: boolean;
  retry: () => void;
}) {
  if (error)
    return (
      <div role="alert" className="flex flex-col items-start gap-3 px-3 py-8">
        <p className="text-14 text-text-1">{error}</p>
        <Button onClick={retry}>{t("error.retry")}</Button>
      </div>
    );
  if (streaming)
    return (
      <div aria-hidden className="flex flex-col">
        {Array.from({ length: 10 }, (_, i) => (
          <div key={i} className={clsx(COLS, "animate-pulse")} style={{ height: ROW_CSS }}>
            {[60, 30, 30, 50, 40].map((w, j) => (
              <span
                key={j}
                className="h-2.5 rounded-full bg-[var(--skeleton)]"
                style={{ width: `${w}%` }}
              />
            ))}
          </div>
        ))}
      </div>
    );
  return (
    <p className="px-3 py-8 text-14 text-text-2">
      {hasRows ? t("servers.noMatch") : t("servers.empty")}
    </p>
  );
}

function Row({
  id,
  r,
  index,
  top,
  region,
  active,
  found,
  onPick,
  onOpen,
}: {
  id: string;
  r: ServerRow;
  index: number;
  top: number;
  region: Regions[string];
  active: boolean;
  found: boolean;
  onPick: () => void;
  onOpen: () => void;
}) {
  const pct = r.maxPlayers ? Math.min(1, r.playing / r.maxPlayers) : 0;
  return (
    <div
      id={id}
      role="row"
      aria-rowindex={index + 2}
      aria-selected={active}
      onPointerDown={onPick}
      onContextMenu={onPick}
      onDoubleClick={onOpen}
      className={clsx(
        COLS,
        "absolute inset-x-0 text-13 text-text-1 tnum",
        found ? "bg-[var(--selection-bg)]" : active ? "bg-surface-3" : "hover:bg-surface-3/60",
        active && "row-active",
      )}
      style={{ height: ROW_CSS, transform: `translateY(${top}px)` }}
    >
      <div role="gridcell" className="flex min-w-0 items-center gap-2.5 overflow-hidden">
        <span className="h-1 w-10 shrink-0 overflow-hidden rounded-full bg-surface-4">
          <span
            className={clsx("block h-full rounded-full", pct >= 1 ? "bg-text-3" : "bg-text-2")}
            style={{ width: `${pct * 100}%` }}
          />
        </span>
        <span>
          {r.playing}
          <span className="text-text-3">/{r.maxPlayers}</span>
        </span>
      </div>
      <div role="gridcell" className={r.ping === null ? "text-text-3" : undefined}>
        {r.ping === null ? t("servers.unknown") : t("servers.ms", { n: r.ping })}
      </div>
      <div role="gridcell" className={r.fps === null ? "text-text-3" : undefined}>
        {r.fps === null ? t("servers.unknown") : r.fps.toFixed(0)}
      </div>
      <div role="gridcell" className="flex min-w-0 items-center gap-2">
        {region === undefined ? (
          <span className="text-text-3">{t("servers.unknown")}</span>
        ) : region === null ? (
          <span className="text-text-3">{t("servers.regionNone")}</span>
        ) : (
          <>
            {region.countryCode && (
              <span className="rounded-chip bg-surface-4 px-1.5 py-px font-mono text-12 text-text-1">
                {region.countryCode}
              </span>
            )}
            {/* the format is free text: show all of it, the chip only adds the country code */}
            <span className="truncate text-text-2">{region.label}</span>
          </>
        )}
      </div>
      <div role="gridcell" className="truncate text-text-2">
        {r.kind === "private" ? (r.name ?? t("servers.private")) : t("servers.public")}
      </div>
    </div>
  );
}
