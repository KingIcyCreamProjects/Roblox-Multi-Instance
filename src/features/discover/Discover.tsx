import * as Tabs from "@radix-ui/react-tabs";
import { Hourglass } from "lucide-react";
import { t, type MsgKey } from "@/lib/i18n";
import { Favorites } from "./Favorites";
import { Games } from "./Games";
import { Outfits } from "./Outfits";
import { Servers } from "./Servers";
import { useDiscover, useRateLimited, type DiscoverTab } from "./shared";
import { Universe } from "./Universe";

const TABS: [DiscoverTab, MsgKey][] = [
  ["servers", "discover.servers"],
  ["games", "discover.games"],
  ["favorites", "discover.favorites"],
  ["outfits", "discover.outfits"],
  ["universe", "discover.universe"],
];

const content = "min-h-0 flex-1 outline-none data-[state=inactive]:hidden";

export function Discover() {
  const tab = useDiscover((d) => d.tab);
  const setTab = useDiscover((d) => d.setTab);
  const limited = useRateLimited();
  return (
    <Tabs.Root
      value={tab}
      onValueChange={(v) => setTab(v as DiscoverTab)}
      className="flex h-full flex-col"
    >
      <div className="flex h-12 shrink-0 items-center gap-6 px-7">
        <Tabs.List
          aria-label={t("nav.discover")}
          className="flex gap-0.5 rounded-full bg-surface-3 p-0.5 shadow-[inset_0_0_0_1px_var(--hairline)]"
        >
          {TABS.map(([v, k]) => (
            <Tabs.Trigger
              key={v}
              value={v}
              className="h-7 rounded-full px-3.5 text-13 font-medium whitespace-nowrap text-text-2 transition-colors hover:text-text-1 data-[state=active]:bg-surface-4 data-[state=active]:text-text-1"
            >
              {t(k)}
            </Tabs.Trigger>
          ))}
        </Tabs.List>
        <p role="status" className="ml-auto flex items-center gap-2 text-13 text-text-2">
          {limited && (
            <>
              <Hourglass size={14} strokeWidth={1.75} aria-hidden className="text-warn" />
              {t("discover.rateLimited")}
            </>
          )}
        </p>
      </div>
      {/* Servers, Games and Universe stay mounted so a running stream, rows and typed drafts survive tab switches. */}
      <Tabs.Content value="servers" forceMount className={content}>
        <Servers />
      </Tabs.Content>
      <Tabs.Content value="games" forceMount className={content}>
        <Games />
      </Tabs.Content>
      <Tabs.Content value="favorites" className={content}>
        <Favorites />
      </Tabs.Content>
      <Tabs.Content value="outfits" className={content}>
        <Outfits />
      </Tabs.Content>
      <Tabs.Content value="universe" forceMount className={content}>
        <Universe />
      </Tabs.Content>
    </Tabs.Root>
  );
}
