import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { Clock, Lock, TriangleAlert, User } from "lucide-react";
import { useEffect, useState } from "react";
import { presenceKey } from "@/features/accounts/AccountRow";
import { call, errMessage, type Preview } from "@/lib/ipc";
import { fmtInt, t, type MsgKey } from "@/lib/i18n";

function useDebounced<T>(v: T, ms: number) {
  const [d, setD] = useState(v);
  useEffect(() => {
    const id = setTimeout(() => setD(v), ms);
    return () => clearTimeout(id);
  }, [v, ms]);
  return d;
}

/** Live preview of a destination text (game name, players, private server, @user). */
export function Chip({ id, input }: { id: string; input: string }) {
  const q = useDebounced(input.trim(), 350);
  const typing = q !== input.trim();
  const { data, error, isFetching } = useQuery({
    queryKey: ["dest", q],
    queryFn: () => call("destination_resolve", { input: q }),
    enabled: !!q,
    staleTime: 60_000,
    meta: { silent: true },
  });
  if (!input.trim()) return null;
  if (typing || (isFetching && !data))
    return (
      <span
        id={id}
        data-testid="dest-loading"
        className="h-3 w-24 shrink-0 animate-pulse rounded-full bg-[var(--skeleton)]"
      />
    );
  const wrap = "flex max-w-full min-w-0 items-center gap-1.5 overflow-hidden text-12";
  if (error || !data || data.error || data.kind === "invalid")
    return (
      <span id={id} className={clsx(wrap, "text-error")}>
        <TriangleAlert size={13} strokeWidth={1.75} aria-hidden className="shrink-0" />
        <span className="truncate">
          {error ? errMessage(error) : (data?.error ?? t("pv.invalid"))}
        </span>
      </span>
    );
  return (
    <span id={id} className={clsx(wrap, "text-text-2")}>
      {chipBody(data)}
    </span>
  );
}

function chipBody(p: Preview) {
  if (p.kind === "follow" && p.follow) {
    const st = p.follow.presence?.state ?? "offline";
    return (
      <>
        <span className="inline-grid size-5 shrink-0 place-items-center rounded-full bg-surface-4">
          <User size={12} strokeWidth={2} aria-hidden />
        </span>
        <span data-pii className="truncate text-text-1">
          @{p.follow.username}
        </span>
        <span className="truncate">{p.follow.presence?.gameName ?? t(presenceKey[st])}</span>
      </>
    );
  }
  if (p.kind === "query")
    return (
      <>
        <Clock size={13} strokeWidth={1.75} aria-hidden className="shrink-0" />
        <span className="truncate">{t("pv.query")}</span>
      </>
    );
  return (
    <>
      {p.icon ? (
        <img src={p.icon} alt="" className="size-5 shrink-0 rounded-chip object-cover" />
      ) : (
        <span aria-hidden className="size-5 shrink-0 rounded-chip bg-surface-4" />
      )}
      {/* narrow bar: name and "playing" stay for screen readers only */}
      <span className="min-w-0 truncate text-text-1">
        {p.name ?? t(KIND[p.kind] ?? "pv.place")}
      </span>
      {p.kind === "private" && <Lock size={12} strokeWidth={2} aria-label={t("pv.private")} />}
      {p.playing != null && (
        <span className="shrink-0 tnum">
          {fmtInt(p.playing)}
          <span className="@max-[52rem]:sr-only"> {t("pv.playing")}</span>
        </span>
      )}
    </>
  );
}
const KIND: Partial<Record<Preview["kind"], MsgKey>> = {
  private: "pv.private",
  share: "pv.share",
  job: "pv.job",
};
