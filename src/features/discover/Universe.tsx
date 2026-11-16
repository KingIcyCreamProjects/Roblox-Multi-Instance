import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { Box, Copy, MapPin, Search, Server } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { errText } from "@/features/accounts/useAccounts";
import { placeIdOf, usePlayBar } from "@/features/playbar/store";
import { call } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import {
  copyText,
  ErrorLine,
  inputCls,
  Kv,
  PanelEmpty,
  PanelTitle,
  pickedCls,
  Quiet,
  sendToBar,
  Split,
  useDiscover,
} from "./shared";

const barPlace = () => {
  const id = placeIdOf(usePlayBar.getState().destination);
  return id ? String(id) : "";
};

export function Universe() {
  const [input, setInput] = useState(barPlace);
  // force-mounted: also take the Play bar place each time the tab opens with an empty input
  const tab = useDiscover((d) => d.tab);
  const [prevTab, setPrevTab] = useState(tab);
  if (tab !== prevTab) {
    setPrevTab(tab);
    if (tab === "universe" && !input) setInput(barPlace());
  }
  const [placeId, setPlaceId] = useState<number | null>(null);
  const [bad, setBad] = useState(false);
  const [pick, setPick] = useState<number>();
  const q = useQuery({
    queryKey: ["universe", placeId],
    queryFn: async () => {
      const { universeId } = await call("universe_of", { placeId: placeId! });
      return { universeId, places: await call("universe_places", { universeId }) };
    },
    enabled: placeId !== null,
    staleTime: 10 * 60_000,
    meta: { silent: true },
  });
  const places = q.data?.places;
  const cur =
    places?.find((p) => p.placeId === pick) ?? places?.find((p) => p.isRoot) ?? places?.[0];

  let body;
  if (placeId === null) body = <Quiet>{t("universe.prompt")}</Quiet>;
  else if (q.isPending)
    body = (
      <div aria-hidden className="flex flex-col">
        {Array.from({ length: 5 }, (_, i) => (
          <div key={i} className="flex h-14 animate-pulse items-center gap-3 px-3">
            <span className="size-9 rounded-chip bg-[var(--skeleton)]" />
            <span className="h-3 w-1/3 rounded-full bg-[var(--skeleton)]" />
          </div>
        ))}
      </div>
    );
  else if (q.isError)
    body = <ErrorLine message={errText(q.error)} retry={() => void q.refetch()} />;
  else
    body = (
      <>
        <div className="flex h-8 items-center gap-2 px-1 text-12 text-text-3">
          {t("universe.id")}
          <span data-selectable className="text-text-2 tnum">
            {q.data.universeId}
          </span>
          <IconButton
            label={t("universe.copyId")}
            className="size-6"
            onClick={() => void copyText(String(q.data.universeId), t("universe.id"))}
          >
            <Copy size={12} strokeWidth={1.75} />
          </IconButton>
        </div>
        {q.data.places.length === 0 ? (
          <Quiet>{t("universe.none")}</Quiet>
        ) : (
          <ul aria-label={t("universe.places")} className="flex flex-col gap-0.5">
            {q.data.places.map((p) => (
              <li key={p.placeId}>
                <button
                  type="button"
                  aria-pressed={cur?.placeId === p.placeId}
                  onClick={() => setPick(p.placeId)}
                  className={clsx(
                    pickedCls(cur?.placeId === p.placeId),
                    "flex h-14 w-full items-center gap-3 px-3",
                  )}
                >
                  <span className="grid size-9 shrink-0 place-items-center rounded-chip bg-surface-3 text-text-2">
                    <Box size={16} strokeWidth={1.75} aria-hidden />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-14 font-medium text-text-1">{p.name}</span>
                    <span className="block font-mono text-12 text-text-3 tnum">{p.placeId}</span>
                  </span>
                  {p.isRoot && <span className="text-12 text-text-3">{t("universe.root")}</span>}
                </button>
              </li>
            ))}
          </ul>
        )}
      </>
    );

  const list = (
    <>
      <form
        className="flex h-11 shrink-0 items-center gap-2 pb-1"
        onSubmit={(e) => {
          e.preventDefault();
          const id = placeIdOf(input);
          setBad(id === null);
          // a bad id clears the previous results too; the same id again retries
          if (id !== null && id === placeId) void q.refetch();
          else setPlaceId(id);
        }}
      >
        <input
          aria-label={t("universe.place")}
          aria-invalid={bad || undefined}
          aria-describedby={bad ? "universe-err" : undefined}
          placeholder={t("universe.place")}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          className={clsx(inputCls, "min-w-0 flex-1 font-mono tnum placeholder:font-sans")}
        />
        <Button type="submit" disabled={!input.trim()}>
          <Search size={14} strokeWidth={1.75} />
          {t("universe.lookup")}
        </Button>
      </form>
      {bad && (
        <p id="universe-err" className="px-1 pb-1 text-13 text-error">
          {t("universe.bad")}
        </p>
      )}
      <div className="min-h-0 flex-1 overflow-auto pb-2">{body}</div>
    </>
  );
  return (
    <Split
      label={t("discover.universe")}
      list={list}
      panel={
        cur ? (
          <>
            <PanelTitle title={cur.name} sub={cur.isRoot ? t("universe.root") : undefined} />
            <Kv
              rows={[
                [t("servers.place"), cur.placeId],
                [t("universe.id"), q.data!.universeId],
              ]}
            />
            <div className="flex flex-wrap gap-2">
              <Button variant="primary" onClick={() => sendToBar(String(cur.placeId), cur.name)}>
                <MapPin size={14} strokeWidth={1.75} />
                {t("games.setDest")}
              </Button>
              <Button onClick={() => useDiscover.getState().viewServers(cur.placeId)}>
                <Server size={14} strokeWidth={1.75} />
                {t("games.viewServers")}
              </Button>
              <Button
                variant="ghost"
                onClick={() => void copyText(String(cur.placeId), t("games.placeIdWhat"))}
              >
                <Copy size={14} strokeWidth={1.75} />
                {t("games.copyPlace")}
              </Button>
            </div>
          </>
        ) : (
          // nothing to pick before a lookup: the list's prompt says what to do
          q.data && <PanelEmpty>{t("discover.pick")}</PanelEmpty>
        )
      }
    />
  );
}
