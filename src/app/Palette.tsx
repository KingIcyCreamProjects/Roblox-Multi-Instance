import * as Dialog from "@radix-ui/react-dialog";
import clsx from "clsx";
import { ArrowDownToLine, CornerDownLeft, Eye, Lock, Navigation, Play } from "lucide-react";
import { useEffect, useId, useState, type ReactNode } from "react";
import { label } from "@/features/accounts/model";
import { useAccounts } from "@/features/accounts/useAccounts";
import { useDiscover, type DiscoverTab } from "@/features/discover/shared";
import { usePlayBar } from "@/features/playbar/store";
import { usePlay } from "@/features/playbar/usePlay";
import { useLock } from "@/features/vault/useVault";
import { t, type MsgKey } from "@/lib/i18n";
import { useUi, type Route } from "./store";
import { useSettings, useSetSettings } from "./useSettings";

type Cmd = { id: string; text: string; hint?: string; icon: ReactNode; run: () => void };

const ROUTES: [Route, MsgKey][] = [
  ["home", "nav.home"],
  ["accounts", "nav.accounts"],
  ["discover", "nav.discover"],
  ["automation", "nav.automation"],
  ["devices", "nav.devices"],
  ["settings", "nav.settings"],
];
const TABS: [DiscoverTab, MsgKey][] = [
  ["servers", "discover.servers"],
  ["games", "discover.games"],
  ["favorites", "discover.favorites"],
  ["outfits", "discover.outfits"],
  ["universe", "discover.universe"],
];

/** Subsequence match ("plorb" finds "Play Orbit"); lower score = tighter match. */
function score(text: string, q: string): number | null {
  const s = text.toLowerCase();
  let at = -1;
  let gaps = 0;
  for (const ch of q) {
    const i = s.indexOf(ch, at + 1);
    if (i < 0) return null;
    gaps += i - at - 1;
    at = i;
  }
  return gaps;
}

/** Ctrl/Cmd+K command palette: go to a screen, play an account, set the destination, privacy, lock. */
export function Palette() {
  const open = useUi((u) => u.paletteOpen);
  const setOpen = (o: boolean) => useUi.setState({ paletteOpen: o });
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const listId = useId();
  const { data: list } = useAccounts();
  const { data: s } = useSettings();
  const set = useSetSettings();
  const { play } = usePlay();
  const lock = useLock();
  // Any close (Esc, Enter on a command, Ctrl+K, another modal opening): start fresh next time.
  useEffect(() => {
    if (open) return;
    setQ("");
    setActive(0);
  }, [open]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        const { paletteOpen } = useUi.getState();
        // never stack on another open dialog (sheet, tools, import)
        if (!paletteOpen && document.querySelector('[role="dialog"]')) return;
        useUi.setState({ paletteOpen: !paletteOpen });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      useUi.setState({ paletteOpen: false }); // unmounted on lock: stay closed after unlock
    };
  }, []);

  const go = (r: Route) => useUi.getState().go(r);
  const query = q.trim();
  const cmds: Cmd[] = [
    ...ROUTES.filter(([r]) => r !== "automation" || s?.automation.enabled).map(([r, k]) => ({
      id: `go-${r}`,
      text: t("palette.goTo", { place: t(k) }),
      icon: <Navigation size={14} strokeWidth={1.75} />,
      run: () => go(r),
    })),
    ...TABS.map(([tab, k]) => ({
      id: `tab-${tab}`,
      text: t("palette.goTo", { place: `${t("nav.discover")} › ${t(k)}` }),
      icon: <Navigation size={14} strokeWidth={1.75} />,
      run: () => {
        useDiscover.getState().setTab(tab);
        go("discover");
      },
    })),
    ...(list?.accounts ?? []).map((a) => ({
      id: `play-${a.id}`,
      text: t("palette.play", { name: label(a) }),
      hint: `@${a.username}`,
      icon: <Play size={14} strokeWidth={1.75} />,
      run: () => play([a.id]),
    })),
    {
      id: "privacy",
      text: s?.ui.showUsernames === false ? t("title.privacyOn") : t("title.privacyOff"),
      icon: <Eye size={14} strokeWidth={1.75} />,
      run: () => s && set.mutate({ ui: { showUsernames: !s.ui.showUsernames } }),
    },
    {
      id: "lock",
      text: t("title.lock"),
      icon: <Lock size={14} strokeWidth={1.75} />,
      run: () => void lock(),
    },
  ];
  const ranked = query
    ? cmds
        // the hint (@username) counts too: an aliased account is still found by its username
        .map((c) => ({ c, n: score(`${c.text} ${c.hint ?? ""}`, query.toLowerCase()) }))
        .filter((x) => x.n !== null)
        .sort((a, b) => a.n! - b.n!)
        .map((x) => x.c)
    : cmds;
  // anything typed can also become the Play bar destination
  const shown: Cmd[] = query
    ? [
        ...ranked,
        {
          id: "dest",
          text: t("palette.setDest", { dest: query }),
          icon: <ArrowDownToLine size={14} strokeWidth={1.75} />,
          run: () => usePlayBar.getState().setDestination(query),
        },
      ]
    : ranked;
  const cur = Math.min(active, shown.length - 1);
  // the list scrolls; keep the keyboard highlight visible
  useEffect(() => {
    if (open) document.getElementById(`${listId}-${cur}`)?.scrollIntoView({ block: "nearest" });
  }, [open, cur, listId]);

  const runAt = (i: number) => {
    const c = shown[i];
    if (!c) return;
    setOpen(false);
    c.run();
  };

  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Portal>
        <Dialog.Overlay className="sheet-fade fixed inset-0 z-40 bg-[var(--scrim)]" />
        <Dialog.Content
          aria-describedby={undefined}
          className="menu fixed top-[calc(var(--titlebar-h)+48px)] left-1/2 z-50 flex max-h-[min(460px,70vh)] w-[min(560px,calc(100vw-32px))] -translate-x-1/2 flex-col overflow-hidden glass rounded-panel"
        >
          <Dialog.Title className="sr-only">{t("palette.title")}</Dialog.Title>
          <input
            role="combobox"
            aria-expanded
            aria-controls={listId}
            aria-activedescendant={shown[cur] ? `${listId}-${cur}` : undefined}
            aria-label={t("title.search")}
            placeholder={t("title.search")}
            value={q}
            spellCheck={false}
            onChange={(e) => {
              setQ(e.target.value);
              setActive(0);
            }}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                const d = e.key === "ArrowDown" ? 1 : -1;
                setActive((cur + d + shown.length) % shown.length);
              } else if (e.key === "Enter") {
                e.preventDefault();
                runAt(cur);
              }
            }}
            className="h-12 shrink-0 border-b border-hairline bg-transparent px-4 text-16 text-text-1 placeholder:text-text-3 focus:shadow-none focus:outline-none"
          />
          <ul
            id={listId}
            role="listbox"
            aria-label={t("palette.title")}
            className="overflow-auto p-1.5"
          >
            {shown.map((c, i) => (
              <li
                key={c.id}
                id={`${listId}-${i}`}
                role="option"
                aria-selected={i === cur}
                onMouseMove={() => i !== cur && setActive(i)}
                onClick={() => runAt(i)}
                className={clsx(
                  "flex h-9 cursor-default items-center gap-3 rounded-control px-2.5 text-14",
                  i === cur ? "bg-surface-3 text-text-1" : "text-text-2",
                )}
              >
                <span aria-hidden className="shrink-0 text-text-3">
                  {c.icon}
                </span>
                <span data-pii={c.hint ? "" : undefined} className="min-w-0 truncate">
                  {c.text}
                </span>
                {c.hint && (
                  <span data-pii className="min-w-0 truncate text-13 text-text-3">
                    {c.hint}
                  </span>
                )}
                {i === cur && (
                  <CornerDownLeft
                    size={14}
                    strokeWidth={1.75}
                    aria-hidden
                    className="ml-auto shrink-0 text-text-3"
                  />
                )}
              </li>
            ))}
          </ul>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
