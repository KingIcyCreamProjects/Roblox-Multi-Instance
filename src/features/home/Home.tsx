import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { Compass, Play } from "lucide-react";
import type { MouseEvent } from "react";
import { useUi } from "@/app/store";
import { Button } from "@/components/ui/Button";
import { Avatar } from "@/features/accounts/AccountRow";
import { AccountsScreen } from "@/features/accounts/AccountsScreen";
import { useBloom } from "@/features/accounts/Inspector";
import { ago, label, select } from "@/features/accounts/model";
import { useAccounts } from "@/features/accounts/useAccounts";
import {
  ErrorLine,
  FAV_KEY,
  RECENTS_KEY,
  sendToBar,
  Thumb,
  useDiscover,
  useRoving,
} from "@/features/discover/shared";
import { lastLaunchSet } from "@/features/playbar/store";
import { usePlay } from "@/features/playbar/usePlay";
import { call, type AccountsList } from "@/lib/ipc";
import { t } from "@/lib/i18n";

/** Same platform blockers as the Play bar: the reason Play can't run, else null. */
function useLaunchBlock() {
  const { data: caps } = useQuery({
    queryKey: ["caps"],
    queryFn: () => call("platform_capabilities"),
  });
  const { data: rbx } = useQuery({
    queryKey: ["roblox"],
    queryFn: () => call("roblox_status"),
    staleTime: 30_000,
    meta: { silent: true },
  });
  if (caps && !caps.launch) return caps.launchReason ?? t("play.unsupported");
  if (rbx?.installed === false) return t("play.noRoblox");
  return null;
}

/** Home: the last played (or first favourite) game owns the frame; recents and online accounts below. */
export function Home() {
  const { data: list, isPending } = useAccounts();
  if (isPending) return <HeroSkeleton />;
  // No accounts yet (or the list failed): the accounts screen owns the empty and error states.
  if (!list?.accounts.length) return <AccountsScreen />;
  return <Loaded list={list} />;
}

type HeroGame = { name: string; thumbnail: string | null; dest: string; sub: string };

function Loaded({ list }: { list: AccountsList }) {
  const recents = useQuery({
    queryKey: RECENTS_KEY,
    queryFn: () => call("recents_list"),
    meta: { silent: true },
  });
  const favs = useQuery({
    queryKey: FAV_KEY,
    queryFn: () => call("favorites_list"),
    meta: { silent: true },
  });
  const r0 = recents.data?.[0];
  const f0 = favs.data?.[0];
  const hero: HeroGame | null = r0
    ? {
        name: r0.name,
        thumbnail: r0.thumbnail,
        dest: String(r0.placeId),
        sub: t("home.lastPlayed", { when: ago(r0.lastPlayedAt) }),
      }
    : f0
      ? {
          name: f0.name,
          thumbnail: f0.thumbnail,
          dest: f0.privateLink ?? String(f0.placeId),
          sub: t("home.favorite"),
        }
      : null;

  let top;
  if (recents.isPending || favs.isPending) top = <HeroSkeleton />;
  else if (!hero && (recents.isError || favs.isError))
    top = (
      <div className="px-8">
        <ErrorLine
          message={t("home.loadErr")}
          retry={() => {
            void recents.refetch();
            void favs.refetch();
          }}
        />
      </div>
    );
  else top = hero ? <Hero g={hero} list={list} /> : <NoHero />;

  return (
    <div className="pb-10">
      {top}
      {!!recents.data?.length && <JumpBackIn recents={recents.data} />}
      <OnlineNow list={list} />
    </div>
  );
}

const mix = (p: number) => `color-mix(in oklch, var(--bg) ${p}%, transparent)`;
const SCRIM = `linear-gradient(to top, var(--bg) 0%, ${mix(92)} 18%, ${mix(70)} 40%, ${mix(35)} 68%, transparent 100%)`;

function Hero({ g, list }: { g: HeroGame; list: AccountsList }) {
  const sel = useUi((u) => u.sel.ids);
  const { play } = usePlay();
  const block = useLaunchBlock();
  const bloom = useBloom(g.thumbnail);
  const live = new Set(list.accounts.map((a) => a.id));
  const last = lastLaunchSet().filter((id) => live.has(id));
  // the current selection wins, like the tray's recents; else the last launched set
  const ids = sel.length ? sel : last;
  return (
    <section aria-labelledby="home-hero" className="@container relative isolate">
      <Thumb
        src={g.thumbnail}
        className="aspect-video max-h-[min(54vh,480px)] min-h-[260px] w-full [@media(max-height:700px)]:max-h-[44vh]"
      />
      {/* soft bottom scrim carries the text; theme-aware so both themes read */}
      <div
        aria-hidden
        className="absolute inset-x-0 bottom-0 h-3/4"
        style={{ background: SCRIM }}
      />
      {/* the one ambient bloom, tinted from the thumbnail; straddles the edge so it reads continuous */}
      <div
        aria-hidden
        className="pointer-events-none absolute inset-x-[6%] -bottom-36 h-72 rounded-full opacity-35 blur-[90px]"
        style={{
          background: `radial-gradient(closest-side, ${bloom ?? "color-mix(in oklch, var(--accent) 18%, var(--surface-4))"}, transparent)`,
        }}
      />
      <div className="absolute inset-x-0 bottom-0 px-8 pb-8">
        <h1
          id="home-hero"
          title={g.name}
          className="max-w-[20ch] line-clamp-2 text-44 leading-[1.02] font-semibold tracking-[-0.035em] text-balance text-text-1 @3xl:text-64"
        >
          {g.name}
        </h1>
        <p className="mt-3 text-14 text-text-2">{g.sub}</p>
        <button
          type="button"
          disabled={!ids.length}
          aria-disabled={block ? true : undefined}
          // play() toasts the block itself and refetches caps, so an install unblocks the next try
          onClick={() => play(ids, { here: true, dest: g.dest })}
          className="mt-6 inline-flex h-11 items-center gap-2 rounded-full bg-accent px-6 shadow-[inset_0_1px_0_rgb(255_255_255/0.3)] text-14 font-semibold text-accent-ink transition-colors duration-[var(--dur-quick)] hover:bg-accent-hover focus-visible:shadow-[var(--focus-ring)] active:bg-accent-press disabled:pointer-events-none disabled:bg-surface-3 disabled:text-text-1 disabled:opacity-40"
        >
          <Play size={16} strokeWidth={2.25} fill="currentColor" />
          {ids.length > 1 ? t("playbar.playN", { n: ids.length }) : t("playbar.play")}
        </button>
        {!ids.length && (
          <span className="ml-4 text-13 text-text-2">{t("home.playNeedsAccount")}</span>
        )}
      </div>
    </section>
  );
}

function NoHero() {
  const go = useUi((u) => u.go);
  return (
    <section className="flex min-h-[300px] flex-col justify-end px-8 pt-16 pb-10">
      <h1 className="max-w-[20ch] text-44 leading-[1.05] font-semibold tracking-[-0.03em] text-balance text-text-1">
        {t("home.empty.title")}
      </h1>
      <p className="mt-4 max-w-[46ch] text-16 text-text-2">{t("home.empty.body")}</p>
      <div className="mt-8">
        <Button
          className="h-10 rounded-full px-5 text-14"
          onClick={() => {
            useDiscover.getState().setTab("games");
            go("discover");
          }}
        >
          <Compass size={16} strokeWidth={1.75} />
          {t("home.empty.cta")}
        </Button>
      </div>
    </section>
  );
}

function HeroSkeleton() {
  return (
    <div
      aria-hidden
      className="aspect-video max-h-[min(54vh,480px)] min-h-[260px] w-full [@media(max-height:700px)]:max-h-[44vh] animate-pulse bg-[var(--skeleton)]"
    />
  );
}

const strip = "-mx-8 flex snap-x snap-mandatory scroll-px-8 gap-4 overflow-x-auto px-8 pt-1 pb-3";

function JumpBackIn({
  recents,
}: {
  recents: { placeId: number; name: string; thumbnail: string | null; lastPlayedAt: string }[];
}) {
  const rove = useRoving(recents.length);
  return (
    <section aria-labelledby="home-jump" className="relative mt-6 px-8">
      <h2 id="home-jump" className="mb-3 text-20 font-semibold tracking-[-0.01em] text-text-1">
        {t("home.jump")}
      </h2>
      <ul className={strip} onKeyDown={rove.onKeyDown}>
        {recents.map((r, i) => (
          <li key={`${r.placeId}-${r.lastPlayedAt}`} className="w-[232px] shrink-0 snap-start">
            <button
              type="button"
              {...rove.item(i)}
              onClick={() => sendToBar(String(r.placeId), r.name)}
              aria-label={t("home.jumpItem", { name: r.name })}
              className="group block w-full rounded-panel text-left"
            >
              <Thumb
                src={r.thumbnail}
                className="aspect-video rounded-panel [&_img]:transition-transform [&_img]:duration-[var(--dur-hero)] [&_img]:ease-[var(--ease-out)] group-hover:[&_img]:scale-[1.04]"
              />
              <span className="mt-2 block truncate text-14 font-medium text-text-1">{r.name}</span>
              <span className="block text-12 text-text-3">{ago(r.lastPlayedAt)}</span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

const dot = { online: "bg-online", in_game: "bg-ingame" } as const;

function OnlineNow({ list }: { list: AccountsList }) {
  const sel = useUi((u) => u.sel);
  const setSel = useUi((u) => u.setSel);
  const { play } = usePlay();
  const online = list.accounts.filter(
    (a) => a.presence?.state === "online" || a.presence?.state === "in_game",
  );
  const order = online.map((a) => a.id);
  const rove = useRoving(online.length);
  const pick = (e: MouseEvent, id: string) =>
    setSel(
      select(sel, order, id, e.ctrlKey || e.metaKey ? "toggle" : e.shiftKey ? "range" : "replace"),
    );
  return (
    <section aria-labelledby="home-online" className="relative mt-10 px-8">
      <h2 id="home-online" className="mb-3 text-20 font-semibold tracking-[-0.01em] text-text-1">
        {t("home.online")}
      </h2>
      {online.length === 0 ? (
        <p className="text-14 text-text-2">{t("home.onlineNone")}</p>
      ) : (
        <ul className={strip} onKeyDown={rove.onKeyDown}>
          {online.map((a, i) => {
            const on = sel.ids.includes(a.id);
            const state = a.presence!.state as keyof typeof dot;
            return (
              <li key={a.id} className="shrink-0 snap-start">
                <button
                  type="button"
                  {...rove.item(i)}
                  aria-pressed={on}
                  onClick={(e) => pick(e, a.id)}
                  onDoubleClick={() => play([a.id])}
                  title={t("home.onlineHint")}
                  className={clsx(
                    "flex w-[96px] flex-col items-center gap-2 rounded-panel px-1.5 py-2.5 transition-colors duration-[var(--dur-quick)]",
                    on ? "bg-surface-3" : "hover:bg-surface-3/60",
                  )}
                >
                  <span className="relative">
                    <Avatar
                      a={a}
                      size={56}
                      className={clsx(
                        on && "ring-2 ring-accent ring-offset-2 ring-offset-[var(--bg)]",
                      )}
                    />
                    <span
                      aria-hidden
                      className={clsx(
                        "absolute right-0 bottom-0 size-3 rounded-full ring-2 ring-[var(--bg)]",
                        dot[state],
                      )}
                    />
                  </span>
                  <span data-pii className="w-full truncate text-center text-13 text-text-1">
                    {label(a)}
                  </span>
                  <span className="w-full truncate text-center text-12 text-text-3">
                    {state === "in_game"
                      ? (a.presence!.gameName ?? t("presence.inGame"))
                      : t("presence.online")}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
