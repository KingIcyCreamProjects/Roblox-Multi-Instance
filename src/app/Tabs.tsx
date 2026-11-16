import clsx from "clsx";
import {
  Bot,
  ChevronDown,
  ChevronUp,
  Compass,
  House,
  MonitorSmartphone,
  Settings,
  Users,
} from "lucide-react";
import { useEffect, useRef, type ComponentType } from "react";
import { useSetSettings, useSettings } from "@/app/useSettings";
import type { Settings as SettingsT } from "@/lib/ipc";
import { t, type MsgKey } from "@/lib/i18n";
import { useUi, type Route } from "./store";

type Item = {
  label: MsgKey;
  icon: ComponentType<{ size?: number; strokeWidth?: number }>;
};

const ITEMS: Record<Route, Item> = {
  home: { label: "nav.home", icon: House },
  accounts: { label: "nav.accounts", icon: Users },
  discover: { label: "nav.discover", icon: Compass },
  automation: { label: "nav.automation", icon: Bot },
  devices: { label: "nav.devices", icon: MonitorSmartphone },
  settings: { label: "nav.settings", icon: Settings },
};

export const DEFAULT_ORDER = Object.keys(ITEMS) as Route[];

/** Saved order, cleaned: unknown and repeated entries dropped, missing tabs appended. */
export function tabOrder(s: SettingsT | undefined): Route[] {
  const saved = (s?.ui.tabOrder ?? []).filter((r): r is Route => Object.hasOwn(ITEMS, r));
  return [...new Set([...saved, ...DEFAULT_ORDER])];
}

/** Top tab capsule in the saved order, Automation only while enabled. */
export function Tabs({ automation }: { automation: boolean }) {
  const route = useUi((u) => u.route);
  const go = useUi((u) => u.go);
  const remoteOpen = useUi((u) => u.remoteOpen);
  const { data: s } = useSettings();
  return (
    <nav
      aria-label={t("nav.main")}
      inert={remoteOpen}
      className="flex gap-0.5 rounded-full bg-surface-3 p-1 shadow-[inset_0_0_0_1px_var(--hairline),inset_0_1px_0_rgb(255_255_255/0.18)]"
    >
      {tabOrder(s)
        .filter((r) => r !== "automation" || automation)
        .map((r) => {
          const active = route === r;
          const { icon: Icon, label } = ITEMS[r];
          return (
            <button
              key={r}
              type="button"
              onClick={() => go(r)}
              aria-current={active ? "page" : undefined}
              title={t(label)}
              className={clsx(
                "flex h-8 shrink-0 items-center gap-1.5 rounded-full px-3.5 text-13 font-semibold transition-colors duration-[var(--dur-quick)]",
                active
                  ? "bg-accent text-accent-ink shadow-[inset_0_1px_0_rgb(255_255_255/0.35),0_4px_12px_rgb(30_80_255/0.35)]"
                  : "text-text-2 hover:bg-surface-3 hover:text-text-1",
                "focus-visible:shadow-[var(--focus-ring)]",
              )}
            >
              <Icon size={16} strokeWidth={1.75} />
              {/* icon-only when the title bar's centre gets too narrow (window width, UI scale) */}
              <span className={automation ? "@max-[37rem]:sr-only" : "@max-[31.5rem]:sr-only"}>
                {t(label)}
              </span>
            </button>
          );
        })}
    </nav>
  );
}

const arrow =
  "inline-grid size-8 place-items-center rounded-control text-text-2 hover:bg-surface-3 hover:text-text-1 aria-disabled:opacity-30";

/** Move tabs up and down; saves on every move. Used by the tour and Settings. */
export function TabOrder() {
  const { data: s } = useSettings();
  const set = useSetSettings();
  const order = tabOrder(s);
  // Keep keyboard focus on the moved row's arrow after the list reorders.
  const refocus = useRef<string | null>(null);
  const key = order.join();
  useEffect(() => {
    if (refocus.current) document.getElementById(refocus.current)?.focus();
    refocus.current = null;
  }, [key]);
  const move = (i: number, by: -1 | 1) => {
    if (i + by < 0 || i + by >= order.length) return;
    refocus.current = `tab-move-${order[i]}-${by}`;
    const next = [...order];
    [next[i], next[i + by]] = [next[i + by]!, next[i]!];
    set.mutate({ ui: { tabOrder: next } });
  };
  return (
    <ol aria-label={t("tabs.order")} className="flex flex-col">
      {order.map((r, i) => {
        const { icon: Icon, label } = ITEMS[r];
        const name = t(label);
        return (
          <li
            key={r}
            className="flex items-center gap-3 border-t border-[var(--hairline)] py-1.5 pr-1 pl-4 first:border-t-0"
          >
            <span className="w-4 text-13 text-text-3 tabular-nums">{i + 1}</span>
            <Icon size={16} strokeWidth={1.75} />
            <span className="flex-1 text-14 font-semibold text-text-1">{name}</span>
            <button
              type="button"
              id={`tab-move-${r}--1`}
              className={arrow}
              aria-disabled={i === 0}
              aria-label={t("tabs.up", { name })}
              onClick={() => move(i, -1)}
            >
              <ChevronUp size={16} aria-hidden />
            </button>
            <button
              type="button"
              id={`tab-move-${r}-1`}
              className={arrow}
              aria-disabled={i === order.length - 1}
              aria-label={t("tabs.down", { name })}
              onClick={() => move(i, 1)}
            >
              <ChevronDown size={16} aria-hidden />
            </button>
          </li>
        );
      })}
    </ol>
  );
}
