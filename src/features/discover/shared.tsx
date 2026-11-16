import clsx from "clsx";
import { RefreshCw } from "lucide-react";
import { useEffect, useState, type KeyboardEvent, type ReactNode } from "react";
import { create } from "zustand";
import { toast } from "@/components/Toasts";
import { Button } from "@/components/ui/Button";
import { placeIdOf, usePlayBar } from "@/features/playbar/store";
import { onEvent, type RateLimited } from "@/lib/ipc";
import { LOCALE, t } from "@/lib/i18n";

export const FAV_KEY = ["favorites"];
export const RECENTS_KEY = ["recents"];

export type DiscoverTab = "servers" | "games" | "favorites" | "outfits" | "universe";

type D = {
  tab: DiscoverTab;
  /** Servers tab place input */
  place: string;
  /** "View servers" asked the Servers tab to refresh; it clears this when it does (mounted
   * now or later, so the request never depends on the tab being force-mounted) */
  refreshPending: boolean;
  gamesView: "grid" | "list";
  setTab: (t: DiscoverTab) => void;
  setPlace: (p: string) => void;
  viewServers: (placeId: number) => void;
};

export const useDiscover = create<D>((set, get) => ({
  tab: "games",
  place: "",
  refreshPending: false,
  gamesView: "grid",
  setTab: (tab) => {
    // An empty place input picks up the Play bar destination's place id.
    const id = placeIdOf(usePlayBar.getState().destination);
    set({ tab, ...(tab === "servers" && !get().place && id ? { place: String(id) } : {}) });
  },
  setPlace: (place) => set({ place }),
  viewServers: (placeId) => set({ tab: "servers", place: String(placeId), refreshPending: true }),
}));

/** Put a destination in the Play bar and say so (the toast region is a live region). */
export function sendToBar(destination: string, name: string) {
  usePlayBar.getState().setDestination(destination);
  toast(t("dest.set", { name }));
}

export async function copyText(text: string, what: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast(t("copy.done", { what }));
  } catch (e) {
    toast(t("copy.failed", { reason: e instanceof Error ? e.message : String(e) }));
  }
}

export const joinLink = (placeId: number, jobId: string) =>
  `https://www.roblox.com/games/start?placeId=${placeId}&gameInstanceId=${jobId}`;

const compact = new Intl.NumberFormat(LOCALE, { notation: "compact", maximumFractionDigits: 1 });
export const fmtCount = (n: number) => compact.format(n);
export const fmtPct = (r: number) => `${Math.round(r * 100)}%`;

export function useDebounced<T>(value: T, ms: number) {
  const [v, setV] = useState(value);
  useEffect(() => {
    const h = setTimeout(() => setV(value), ms);
    return () => clearTimeout(h);
  }, [value, ms]);
  return v;
}

/** True while Roblox told us to back off (roblox://ratelimited), on any host: the longest
 * announced wait wins, so a short backoff on one host never hides a long one on another. */
export function useRateLimited() {
  const [until, setUntil] = useState(0);
  useEffect(
    () =>
      onEvent<RateLimited>("roblox://ratelimited", (e) =>
        setUntil((u) => Math.max(u, Date.now() + e.retryInMs)),
      ),
    [],
  );
  useEffect(() => {
    if (!until) return;
    const h = setTimeout(() => setUntil(0), Math.max(0, until - Date.now()));
    return () => clearTimeout(h);
  }, [until]);
  return until > 0;
}

/** Image with a quiet surface fallback when the url is missing or fails. Decorative (alt=""). */
export function Thumb({
  src,
  className,
  children,
}: {
  src: string | null;
  className?: string;
  children?: ReactNode;
}) {
  // the src that failed: a new src gets a fresh try
  const [failed, setFailed] = useState<string | null>(null);
  const broken = failed === src;
  return (
    <div className={clsx("relative overflow-hidden bg-surface-3", className)}>
      {src && !broken && (
        <img
          src={src}
          alt=""
          loading="lazy"
          decoding="async"
          draggable={false}
          onError={() => setFailed(src)}
          className="absolute inset-0 size-full object-cover"
        />
      )}
      {children}
    </div>
  );
}

/** List on the left, glass side panel on the right (the Accounts layout). */
export function Split({
  list,
  panel,
  label,
}: {
  list: ReactNode;
  panel: ReactNode;
  label: string;
}) {
  return (
    <div className="flex h-full min-h-0 gap-4 px-7 pt-1 pb-3">
      <section aria-label={label} className="flex min-w-0 flex-1 flex-col">
        {list}
      </section>
      <aside
        aria-label={t("discover.details")}
        className="glass-panel flex w-[clamp(300px,34%,25rem)] shrink-0 flex-col gap-4 overflow-y-auto rounded-panel p-[1.375rem]"
      >
        {panel}
      </aside>
    </div>
  );
}

export const PanelEmpty = ({ children }: { children: ReactNode }) => (
  <p className="px-4 py-6 text-center text-13 text-text-3">{children}</p>
);

export function PanelTitle({ title, sub }: { title: string; sub?: ReactNode }) {
  return (
    <div className="min-w-0">
      <h2 className="truncate text-[1.125rem] leading-tight font-semibold text-text-1">{title}</h2>
      {sub && <p className="mt-0.5 truncate text-12 text-text-3">{sub}</p>}
    </div>
  );
}

/** Label and value rows of a side panel. */
export function Kv({ rows }: { rows: [string, ReactNode][] }) {
  return (
    <dl className="grid grid-cols-[4.5rem_1fr] gap-x-3 gap-y-1.5 text-13">
      {rows.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="text-text-3">{k}</dt>
          <dd data-selectable className="min-w-0 truncate text-text-1 tnum">
            {v}
          </dd>
        </div>
      ))}
    </dl>
  );
}

export const FieldLabel = ({ children }: { children: ReactNode }) => (
  <span className="text-12 font-medium text-text-2">{children}</span>
);

/** A list row that can be the picked one. */
export const pickedCls = (on: boolean) =>
  clsx(
    "rounded-control text-left transition-colors",
    on
      ? "bg-surface-3 shadow-[inset_0_0_0_1px_var(--hairline)] focus-visible:shadow-[var(--focus-ring)]"
      : "hover:bg-surface-3/60",
  );

export const inputCls =
  "h-[var(--control-h)] min-w-0 rounded-control border border-transparent bg-surface-3 px-2.5 text-13 text-text-1 placeholder:text-text-3 transition-colors hover:bg-surface-4 focus:border-hairline-strong disabled:opacity-40";

export function ErrorLine({ message, retry }: { message: string; retry: () => void }) {
  return (
    <div role="alert" className="flex flex-col items-start gap-3 py-10">
      <p className="text-14 text-text-1">{message}</p>
      <Button onClick={retry}>
        <RefreshCw size={14} strokeWidth={2} />
        {t("error.retry")}
      </Button>
    </div>
  );
}

export function Quiet({ children }: { children: ReactNode }) {
  return <p className="py-10 text-14 text-text-2">{children}</p>;
}

/** Roving tabindex for a horizontal strip: one Tab stop, ←/→ (↑/↓, Home/End) move between items. */
export function useRoving(count: number) {
  const [cur, setCur] = useState(0);
  const at = Math.min(cur, Math.max(0, count - 1));
  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    const items = [...e.currentTarget.querySelectorAll<HTMLElement>("[data-strip-item]")];
    const i = items.indexOf(document.activeElement as HTMLElement);
    if (i < 0) return;
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
    const next = e.key === "Home" ? 0 : e.key === "End" ? items.length - 1 : step ? i + step : null;
    if (next === null) return;
    e.preventDefault();
    items[Math.min(items.length - 1, Math.max(0, next))]?.focus();
  };
  /** spread on each item */
  const item = (i: number) => ({
    "data-strip-item": true,
    tabIndex: i === at ? 0 : -1,
    onFocus: () => setCur(i),
  });
  return { onKeyDown, item };
}
