import { useQuery } from "@tanstack/react-query";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useRef } from "react";
import { create } from "zustand";
import { useUi, type Route } from "@/app/store";
import { useSettings, useSetSettings } from "@/app/useSettings";
import { Sheet } from "@/components/ui/Sheet";
import { tourSteps } from "@/features/onboarding/Tour";
import { call } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { UPDATES, type Update } from "./log";

/** `pick`: version shown in detail; null = the list. */
export const useUpdateLog = create<{ open: boolean; pick: string | null }>(() => ({
  open: false,
  pick: null,
}));
export const openUpdateLog = (pick: string | null = null) =>
  useUpdateLog.setState({ open: true, pick });

const day = (d: string) =>
  new Date(`${d}T00:00:00`).toLocaleDateString(undefined, {
    year: "numeric",
    month: "long",
    day: "numeric",
  });

const SECTIONS = [
  ["added", "updates.added"],
  ["improved", "updates.improved"],
  ["fixed", "updates.fixed"],
] as const;

const TABS: readonly string[] = ["home", "accounts", "discover", "automation", "devices"];

/** Close the log and open where the line points: a tab, or a settings row. */
function show(to: string) {
  useUpdateLog.setState({ open: false });
  if (TABS.includes(to)) useUi.getState().go(to as Route);
  else {
    useUi.setState({ settingsJump: to });
    useUi.getState().go("settings");
  }
}

function Detail({ u }: { u: Update }) {
  return (
    <article className="flex flex-col gap-4">
      <button
        type="button"
        onClick={() => useUpdateLog.setState({ pick: null })}
        className="-ml-2 inline-flex items-center gap-1 self-start rounded-control px-2 py-1 text-13 font-medium text-text-2 hover:bg-surface-3 hover:text-text-1"
      >
        <ChevronLeft size={14} strokeWidth={1.75} aria-hidden />
        {t("updates.all")}
      </button>
      <header>
        <p className="text-12 text-text-3">
          {t("updates.version", { v: u.version })} · {day(u.date)}
        </p>
        <h3 className="text-16 font-semibold text-text-1">{u.title}</h3>
        <p className="mt-1 text-14 text-text-2">{u.summary}</p>
      </header>
      {SECTIONS.map(([k, label]) =>
        u[k]?.length ? (
          <section key={k}>
            <h4 className="mb-1.5 text-13 font-semibold text-text-1">{t(label)}</h4>
            <ul className="flex list-disc flex-col gap-1 pl-5 text-13 text-text-2 marker:text-text-3">
              {u[k].map((line) =>
                typeof line === "string" ? (
                  <li key={line}>{line}</li>
                ) : (
                  <li key={line.text}>
                    {line.text}{" "}
                    <button
                      type="button"
                      onClick={() => show(line.to)}
                      className="inline-flex items-center gap-0.5 rounded-chip font-medium whitespace-nowrap text-accent-text hover:underline"
                    >
                      {t("updates.show")}
                      <ChevronRight size={12} strokeWidth={2} aria-hidden />
                    </button>
                  </li>
                ),
              )}
            </ul>
          </section>
        ) : null,
      )}
    </article>
  );
}

function List() {
  return (
    <ul className="flex flex-col gap-2">
      {UPDATES.map((u, i) => (
        <li key={u.version}>
          <button
            type="button"
            onClick={() => useUpdateLog.setState({ pick: u.version })}
            className="flex w-full items-center gap-3 rounded-control bg-surface-2 px-4 py-3 text-left shadow-[inset_0_0_0_1px_var(--hairline)] hover:bg-surface-3"
          >
            <span className="min-w-0 flex-1">
              <span className="flex items-center gap-2 text-12 text-text-3">
                {t("updates.version", { v: u.version })} · {day(u.date)}
                {i === 0 && (
                  <span className="rounded-full bg-accent/15 px-2 py-0.5 text-12 font-semibold text-accent-text">
                    {t("updates.newest")}
                  </span>
                )}
              </span>
              <span className="block text-14 font-semibold text-text-1">{u.title}</span>
              <span className="block truncate text-13 text-text-2">{u.summary}</span>
            </span>
            <ChevronRight size={16} strokeWidth={1.75} aria-hidden className="text-text-3" />
          </button>
        </li>
      ))}
    </ul>
  );
}

export function UpdateLog() {
  const { open, pick } = useUpdateLog();
  const u = UPDATES.find((x) => x.version === pick);
  return (
    <Sheet
      open={open}
      onOpenChange={(o) => useUpdateLog.setState({ open: o })}
      title={t("updates.title")}
      description={t("updates.desc")}
      width={520}
      centered
    >
      {u ? <Detail u={u} /> : <List />}
    </Sheet>
  );
}

/** Once per new version, open its notes. Not on first run (the tour covers that).
 * After an update that added tour steps, wait for that tour to close: no stacked dialogs. */
export function useWhatsNew() {
  const { data: s } = useSettings();
  const { data: info } = useQuery({ queryKey: ["info"], queryFn: () => call("app_info") });
  const { data: caps } = useQuery({
    queryKey: ["caps"],
    queryFn: () => call("platform_capabilities"),
  });
  const { mutate } = useSetSettings();
  const v = info?.version;
  // once per version per mount: a failed save refetches the old `seen` and must not loop
  const done = useRef<string | null>(null);
  useEffect(() => {
    if (!s || !v || s.updates.seen === v || done.current === v) return;
    if (s.onboarding.tourDone && (!caps || tourSteps(s, caps).length)) return; // tour first
    done.current = v;
    mutate({ updates: { seen: v } });
    if (s.onboarding.tourDone && UPDATES.some((u) => u.version === v)) openUpdateLog(v);
  }, [s, v, caps, mutate]);
}
