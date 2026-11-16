import * as ToggleGroup from "@radix-ui/react-toggle-group";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { Copy, LayoutGrid, List, MapPin, Play, Search, Server, Star, X } from "lucide-react";
import { useState } from "react";
import { useUi } from "@/app/store";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { usePlay } from "@/features/playbar/usePlay";
import { call, type GameTile } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { reportError } from "@/lib/report";
import {
  copyText,
  ErrorLine,
  FAV_KEY,
  fmtCount,
  fmtPct,
  PanelEmpty,
  PanelTitle,
  pickedCls,
  Quiet,
  Split,
  sendToBar,
  Thumb,
  useDebounced,
  useDiscover,
} from "./shared";

export function useGames(query: string) {
  return useInfiniteQuery({
    queryKey: ["games", query],
    queryFn: ({ pageParam }) =>
      query
        ? call("games_search", { query, pageToken: pageParam })
        : call("games_discover").then((games) => ({ games, nextPageToken: null })),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextPageToken,
    staleTime: 10 * 60_000,
    meta: { silent: true },
  });
}

export function Games() {
  const [q, setQ] = useState("");
  const [pick, setPick] = useState<number>();
  const query = useDebounced(q.trim(), 350);
  const mode = useDiscover((d) => d.gamesView);
  const res = useGames(query);
  const games = res.data?.pages.flatMap((p) => p.games) ?? [];
  const cur = games.find((g) => g.placeId === pick) ?? games[0];
  const sel = { pick: cur?.placeId, set: setPick };

  let body;
  if (res.isPending) body = mode === "grid" ? <ShelfSkeleton /> : <ListSkeleton />;
  else if (res.isError && !games.length)
    body = <ErrorLine message={t("games.loadErr")} retry={() => void res.refetch()} />;
  else if (!games.length)
    body = <Quiet>{query ? t("games.noMatch", { q: query }) : t("games.none")}</Quiet>;
  else
    body = (
      <>
        {mode === "grid" ? <Shelf games={games} sel={sel} /> : <Rows games={games} sel={sel} />}
        {res.isError && (
          <ErrorLine
            message={t("games.loadErr")}
            retry={() => void (res.hasNextPage ? res.fetchNextPage() : res.refetch())}
          />
        )}
        {res.hasNextPage && !res.isError && (
          <div className="flex justify-center pt-8">
            <Button disabled={res.isFetchingNextPage} onClick={() => void res.fetchNextPage()}>
              {res.isFetchingNextPage ? t("games.loadingMore") : t("games.loadMore")}
            </Button>
          </div>
        )}
      </>
    );

  const list = (
    <>
      <div className="flex h-11 shrink-0 items-center gap-2 pb-1">
        <div className="relative min-w-0 flex-1">
          <Search
            size={14}
            strokeWidth={1.75}
            aria-hidden
            className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-text-3"
          />
          <input
            type="search"
            aria-label={t("games.search")}
            placeholder={t("games.search")}
            maxLength={100}
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && q && (e.stopPropagation(), setQ(""))}
            className="h-8 w-full rounded-control bg-field pr-8 pl-8 text-13 text-text-1 shadow-[inset_0_0_0_1px_var(--hairline)] transition-colors placeholder:text-text-3 hover:shadow-[inset_0_0_0_1px_var(--hairline-strong)] focus-visible:shadow-[var(--focus-ring)] [&::-webkit-search-cancel-button]:hidden"
          />
          {q && (
            <button
              type="button"
              aria-label={t("games.clear")}
              onClick={() => setQ("")}
              className="absolute top-1/2 right-1 inline-grid size-6 -translate-y-1/2 place-items-center rounded-chip text-text-3 hover:bg-surface-4 hover:text-text-1"
            >
              <X size={12} strokeWidth={2} />
            </button>
          )}
        </div>
        <ToggleGroup.Root
          type="single"
          value={mode}
          onValueChange={(v) => v && useDiscover.setState({ gamesView: v as "grid" | "list" })}
          aria-label={t("games.view")}
          className="flex gap-0.5 rounded-full bg-surface-3 p-0.5 shadow-[inset_0_0_0_1px_var(--hairline)]"
        >
          {(
            [
              ["list", List, "games.viewList"],
              ["grid", LayoutGrid, "games.viewGrid"],
            ] as const
          ).map(([v, Icon, label]) => (
            <ToggleGroup.Item
              key={v}
              value={v}
              aria-label={t(label)}
              title={t(label)}
              className="inline-grid h-7 w-9 place-items-center rounded-full text-text-3 transition-colors hover:text-text-1 data-[state=on]:bg-surface-4 data-[state=on]:text-text-1"
            >
              <Icon size={14} strokeWidth={1.75} />
            </ToggleGroup.Item>
          ))}
        </ToggleGroup.Root>
      </div>
      <div aria-busy={res.isFetching} className="min-h-0 flex-1 overflow-auto pb-2">
        {res.isPending && (
          <span role="status" className="sr-only">
            {t("games.loading")}
          </span>
        )}
        {body}
      </div>
    </>
  );
  return (
    <Split
      label={t("games.results")}
      list={list}
      panel={
        cur ? <Details key={cur.placeId} g={cur} /> : <PanelEmpty>{t("discover.pick")}</PanelEmpty>
      }
    />
  );
}

// rem floor: tiles grow with the text at a bigger UI scale, so the stats line keeps its numbers
const SHELF = "grid grid-cols-[repeat(auto-fill,minmax(11rem,1fr))] gap-3";

/** Likes and players as two separate spans (no middot chains). */
function Stats({ g, className }: { g: GameTile; className?: string }) {
  if (g.likeRatio === null && g.playing === null) return null;
  return (
    <p className={clsx("flex gap-3 tnum", className)}>
      {g.likeRatio !== null && (
        <span className="min-w-0 truncate">{t("games.likes", { pct: fmtPct(g.likeRatio) })}</span>
      )}
      {g.playing !== null && (
        <span className="min-w-0 truncate">{t("games.playing", { n: fmtCount(g.playing) })}</span>
      )}
    </p>
  );
}

type Sel = { pick: number | undefined; set: (id: number) => void };

function Shelf({ games, sel }: { games: GameTile[]; sel: Sel }) {
  return (
    <ul className={SHELF} aria-label={t("games.results")}>
      {games.map((g, i) => (
        <li key={`${g.placeId}-${i}`}>
          <button
            type="button"
            aria-pressed={sel.pick === g.placeId}
            onClick={() => sel.set(g.placeId)}
            className={clsx(pickedCls(sel.pick === g.placeId), "block w-full p-2")}
          >
            <Thumb src={g.thumbnail} className="aspect-video rounded-chip" />
            <span className="mt-2 block truncate text-13 font-medium text-text-1">{g.name}</span>
            <Stats g={g} className="truncate text-12 text-text-3" />
          </button>
        </li>
      ))}
    </ul>
  );
}

function Rows({ games, sel }: { games: GameTile[]; sel: Sel }) {
  return (
    <ul aria-label={t("games.results")} className="flex flex-col gap-0.5">
      {games.map((g, i) => (
        <li key={`${g.placeId}-${i}`}>
          <button
            type="button"
            aria-pressed={sel.pick === g.placeId}
            onClick={() => sel.set(g.placeId)}
            className={clsx(
              pickedCls(sel.pick === g.placeId),
              "flex h-14 w-full items-center gap-3 px-3",
            )}
          >
            <Thumb src={g.icon ?? g.thumbnail} className="size-9 shrink-0 rounded-chip" />
            <span className="min-w-0 flex-1">
              <span className="block truncate text-14 font-medium text-text-1">{g.name}</span>
              {g.playing !== null && (
                <span className="block truncate text-12 text-text-3 tnum">
                  {t("games.playing", { n: fmtCount(g.playing) })}
                </span>
              )}
            </span>
            {g.likeRatio !== null && (
              <span className="text-13 text-text-2 tnum">
                {t("games.likes", { pct: fmtPct(g.likeRatio) })}
              </span>
            )}
          </button>
        </li>
      ))}
    </ul>
  );
}

/** Side panel: Set as destination, Play now, Favorite, Copy place id, View servers. */
function Details({ g }: { g: GameTile }) {
  const qc = useQueryClient();
  const sel = useUi((u) => u.sel.ids);
  const { play } = usePlay();
  const {
    data: favs,
    isError: favsErr,
    refetch: refetchFavs,
  } = useQuery({
    queryKey: FAV_KEY,
    queryFn: () => call("favorites_list"),
    meta: { silent: true },
  });
  const fav = favs?.find((f) => f.placeId === g.placeId && !f.privateLink);
  const [favBusy, setFavBusy] = useState(false);
  const toggleFav = async () => {
    setFavBusy(true);
    try {
      if (fav) await call("favorites_remove", { id: fav.id });
      else await call("favorites_add", { placeId: g.placeId, name: g.name, privateLink: null });
    } catch (e) {
      reportError(e);
    }
    await qc.invalidateQueries({ queryKey: FAV_KEY });
    setFavBusy(false);
  };
  return (
    <>
      <Thumb src={g.thumbnail} className="aspect-video shrink-0 rounded-panel" />
      <PanelTitle title={g.name} sub={g.creator} />
      <Stats g={g} className="-mt-2 text-12 text-text-3" />
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="primary" onClick={() => sendToBar(String(g.placeId), g.name)}>
          <MapPin size={14} strokeWidth={1.75} />
          {t("games.setDest")}
        </Button>
        <Button
          disabled={!sel.length}
          aria-describedby={sel.length ? undefined : "games-play-why"}
          onClick={() => play(sel, { here: true, dest: String(g.placeId) })}
        >
          <Play size={14} strokeWidth={2} />
          {t("games.playNow")}
        </Button>
        <IconButton
          label={fav ? t("games.unfavorite") : t("games.favorite")}
          aria-busy={favBusy || undefined}
          disabled={favBusy || (!favs && !favsErr)}
          onClick={() => void (favs ? toggleFav() : refetchFavs())}
        >
          <Star size={16} strokeWidth={1.75} fill={fav ? "currentColor" : "none"} />
        </IconButton>
      </div>
      {/* visible, not a hover title: a disabled button can't be focused or hovered for it */}
      {!sel.length && (
        <p id="games-play-why" className="-mt-2 text-12 text-text-3">
          {t("games.playNowNeedsAccount")}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button onClick={() => useDiscover.getState().viewServers(g.placeId)}>
          <Server size={14} strokeWidth={1.75} />
          {t("games.viewServers")}
        </Button>
        <Button
          variant="ghost"
          onClick={() => void copyText(String(g.placeId), t("games.placeIdWhat"))}
        >
          <Copy size={14} strokeWidth={1.75} />
          {t("games.copyPlace")}
        </Button>
      </div>
    </>
  );
}

function ShelfSkeleton() {
  return (
    <div aria-hidden className={SHELF}>
      {/* mirrors a Shelf tile: p-2, rounded-chip thumb, name, stats */}
      {Array.from({ length: 12 }, (_, i) => (
        <div key={i} className="animate-pulse p-2">
          <div className="aspect-video rounded-chip bg-[var(--skeleton)]" />
          <div className="mt-2 h-3 w-3/4 rounded-full bg-[var(--skeleton)]" />
          <div className="mt-1.5 h-2.5 w-1/2 rounded-full bg-[var(--skeleton)]" />
        </div>
      ))}
    </div>
  );
}

function ListSkeleton() {
  return (
    <div aria-hidden className="flex flex-col">
      {Array.from({ length: 10 }, (_, i) => (
        <div key={i} className="flex h-14 animate-pulse items-center gap-3 px-3">
          <span className="size-9 rounded-chip bg-[var(--skeleton)]" />
          <span className="h-3 w-1/3 rounded-full bg-[var(--skeleton)]" />
        </div>
      ))}
    </div>
  );
}
