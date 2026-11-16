import {
  closestCenter,
  DndContext,
  DragOverlay,
  PointerSensor,
  useSensor,
  useSensors,
  type CollisionDetection,
  type DragEndEvent,
  type Modifier,
} from "@dnd-kit/core";
import { arrayMove, SortableContext, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { useVirtualizer } from "@tanstack/react-virtual";
import clsx from "clsx";
import { FolderPlus, Plus, Search, X } from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
} from "react";
import { useSettings } from "@/app/useSettings";
import { useUi } from "@/app/store";
import { toast } from "@/components/Toasts";
import { Button } from "@/components/ui/Button";
import { EmptyState } from "@/features/home/EmptyState";
import { usePlay } from "@/features/playbar/usePlay";
import type { AccountView, AccountsList } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { AccountRow, GroupHeader, HEAD_H, SkeletonRows } from "./AccountRow";
import { ImportSheet } from "@/features/import/ImportSheet";
import { ToolsSheet } from "@/features/tools/ToolsSheet";
import { startNewGroup } from "./AccountMenu";
import { AddSheet } from "./AddSheet";
import { Inspector, PANEL_W } from "./Inspector";
import {
  buildRows,
  cookiesIn,
  emptySel,
  move,
  pendingIn,
  select,
  selectGroup,
  type Row,
} from "./model";
import { useAccountActions, useAccounts } from "./useAccounts";

// Mirrors --row-h in tokens.css (comfortable 3rem / compact 2.5rem) at 100% UI scale; group
// headers are HEAD_H. Both are multiplied by the UI scale for the virtualizer.
const ROW_H = { comfortable: 48, compact: 40 } as const;

type UAData = { userAgentData?: { platform?: string } };
const isMac = /mac/i.test(
  (navigator as Navigator & UAData).userAgentData?.platform ?? navigator.userAgent,
);
const NONE: string[] = [];
// Group drags collapse the list, so the header's rect moves; keep the overlay on the pointer.
const onPointerY: Modifier = ({ activatorEvent, draggingNodeRect, transform }) =>
  activatorEvent instanceof MouseEvent && draggingNodeRect
    ? {
        ...transform,
        y:
          transform.y + activatorEvent.clientY - draggingNodeRect.top - draggingNodeRect.height / 2,
      }
    : transform;

const editable = (el: EventTarget | null) =>
  el instanceof HTMLElement &&
  (el.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName));

export function AccountsScreen() {
  const { data: list, isPending, refetch } = useAccounts();
  const openAdd = useUi((u) => u.openAdd);

  let body;
  if (isPending) {
    // the loaded layout's frame (list + side panel), so only the content fades in
    body = (
      <div className="flex h-full gap-4 px-7 pt-1 pb-3">
        <div role="status" aria-label={t("accounts.loading")} className="min-w-0 flex-1">
          <SkeletonRows />
        </div>
        <div aria-hidden className={clsx("glass-panel", PANEL_W)} />
      </div>
    );
  } else if (!list) {
    body = (
      <Column>
        <div role="alert" className="flex flex-col items-start gap-3 px-6 pt-10">
          <p className="text-14 text-text-1">{t("accounts.loadErr")}</p>
          <Button variant="primary" onClick={() => void refetch()}>
            {t("error.retry")}
          </Button>
        </div>
      </Column>
    );
  } else if (!list.accounts.length) {
    body = (
      <EmptyState
        onSignIn={() => openAdd("signin")}
        onImport={() => useUi.setState({ importOpen: true })}
      />
    );
  } else {
    body = <Loaded list={list} />;
  }
  return body;
}

/** Mounted once by the Shell: Home switching from empty to loaded must not unmount a sheet mid-flow. */
export function AccountSheets() {
  return (
    <>
      <AddSheet />
      <ImportSheet />
      <ToolsSheet />
    </>
  );
}

function Column({ children }: { children: React.ReactNode }) {
  return <div className="flex h-full flex-col px-7 pt-2">{children}</div>;
}

function Loaded({ list }: { list: AccountsList }) {
  const sel = useUi((u) => u.sel);
  const setSel = useUi((u) => u.setSel);
  const selGroup = useUi((u) => u.selGroup);
  const { data: s } = useSettings();
  const act = useAccountActions();
  const { play } = usePlay();
  const [query, setQuery] = useState("");
  const collapsed = useUi((u) => u.collapsed);
  const [dropping, setDropping] = useState(false);
  /** group being dragged: all groups render collapsed meanwhile, so only headers move */
  const [dragGroup, setDragGroup] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  const rows = useMemo(
    () => buildRows(list, query, dragGroup ? new Set(list.groups) : collapsed),
    [list, query, collapsed, dragGroup],
  );
  const selSet = useMemo(() => new Set(sel.ids), [sel.ids]);
  const order = useMemo(() => rows.flatMap((r) => (r.kind === "account" ? [r.a.id] : [])), [rows]);
  const keys = useMemo(() => rows.map((r) => r.key), [rows]);
  const searching = !!query.trim();
  const scale = (s?.ui.scale ?? 100) / 100;
  const rowH = ROW_H[s?.ui.density ?? "comfortable"] * scale;
  const headH = HEAD_H * scale;

  const v = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => (rows[i].kind === "group" ? headH : rowH),
    getItemKey: (i) => rows[i].key,
    overscan: 10,
  });
  useEffect(() => v.measure(), [rowH, headH, v]);

  // Keep the keyboard cursor in view (not on a rows rebuild: a presence update must not snap back).
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  useEffect(() => {
    if (!sel.cursor) return;
    const i = rowsRef.current.findIndex((r) => r.key === sel.cursor);
    if (i >= 0) v.scrollToIndex(i, { align: "auto" });
  }, [sel.cursor, v]);

  // Ctrl/Cmd+F, or just start typing, to search.
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      if (mod && !e.shiftKey && e.key.toLowerCase() === "f") {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
        return;
      }
      if (mod || e.altKey || e.key.length !== 1 || e.key === " " || editable(e.target)) return;
      if (document.querySelector('[role="dialog"],[role="menu"]')) return;
      e.preventDefault();
      setQuery((q) => q + e.key);
      searchRef.current?.focus();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const focusList = () => scrollRef.current?.focus({ preventScroll: true });

  const pick = useCallback(
    (e: MouseEvent, id: string, force?: "replace") => {
      const mode = force ?? (e.shiftKey ? "range" : e.ctrlKey || e.metaKey ? "toggle" : "replace");
      setSel(select(useUi.getState().sel, order, id, mode));
      scrollRef.current?.focus({ preventScroll: true });
    },
    [order, setSel],
  );
  const open = useCallback((id: string) => play([id]), [play]);

  const groupOrder = useMemo(() => {
    const g = [...list.groups];
    for (const a of list.accounts) if (!g.includes(a.group)) g.push(a.group);
    return g;
  }, [list]);
  const groupIds = (name: string) =>
    rows.find((r): r is Extract<Row, { kind: "group" }> => r.kind === "group" && r.name === name)
      ?.ids ?? [];

  const moveAccount = (id: string, d: number) => {
    const a = list.accounts.find((x) => x.id === id);
    if (!a || searching) return;
    const ids = groupIds(a.group);
    const i = ids.indexOf(id);
    const j = i + d;
    if (!Number.isFinite(d) || j < 0 || j >= ids.length) return;
    if (list.accounts.find((x) => x.id === ids[j])?.pinned !== a.pinned) return;
    void act.reorder(a.group, arrayMove(ids, i, j));
  };
  const moveGroup = (name: string, d: number) => {
    const i = groupOrder.indexOf(name);
    const j = i + d;
    if (searching || j < 0 || j >= groupOrder.length) return;
    void act.reorderGroups(arrayMove(groupOrder, i, j));
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.target !== e.currentTarget) return;
    const mod = e.ctrlKey || e.metaKey;
    const k = e.key;
    const cursor = sel.cursor;
    // a search hides rows; never delete or launch selected accounts the user can't see
    const shown = searching ? sel.ids.filter((id) => order.includes(id)) : sel.ids;
    if (k === "ArrowDown" || k === "ArrowUp" || k === "Home" || k === "End") {
      e.preventDefault();
      const d = k === "ArrowDown" ? 1 : k === "ArrowUp" ? -1 : k === "End" ? Infinity : -Infinity;
      if (e.altKey && cursor) return moveAccount(cursor, d);
      setSel(move(sel, order, d, e.shiftKey ? "range" : mod ? "cursor" : "replace"));
    } else if (k === " " && cursor) {
      e.preventDefault();
      setSel(select(sel, order, cursor, "toggle"));
    } else if (k === "Enter") {
      const ids = shown.length ? shown : cursor && order.includes(cursor) ? [cursor] : [];
      if (ids.length) play(ids);
    } else if (k === "Delete" || (k === "Backspace" && e.metaKey)) {
      e.preventDefault();
      void act.remove(shown);
    } else if (k === "F2" && cursor) {
      e.preventDefault();
      setSel(select(sel, order, cursor, "replace"));
      useUi.getState().focusField("alias", cursor);
    } else if (mod && k.toLowerCase() === "a") {
      e.preventDefault();
      setSel({ ids: order, anchor: order[0] ?? null, cursor: sel.cursor ?? order[0] ?? null });
    } else if (k === "Escape" && sel.ids.length) {
      setSel({ ...emptySel, cursor });
    }
  };

  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 4 } }));
  const byId = useMemo(() => new Map(list.accounts.map((a) => [a.id, a])), [list]);
  // Rows only sort within their own group and pinned block; headers only among headers.
  const collide: CollisionDetection = (args) => {
    const from = String(args.active.id);
    const me = byId.get(from);
    return closestCenter({
      ...args,
      droppableContainers: args.droppableContainers.filter((c) => {
        const id = String(c.id);
        if (from.startsWith("group:")) return id.startsWith("group:");
        const o = byId.get(id);
        return !!me && !!o && o.group === me.group && o.pinned === me.pinned;
      }),
    });
  };
  const onDragEnd = ({ active, over }: DragEndEvent) => {
    setDragGroup(null);
    if (!over || active.id === over.id) return;
    const from = String(active.id);
    const to = String(over.id);
    const groupOf = (key: string) =>
      key.startsWith("group:") ? key.slice(6) : list.accounts.find((a) => a.id === key)?.group;
    if (from.startsWith("group:")) {
      const target = groupOf(to);
      if (!target) return;
      void act.reorderGroups(
        arrayMove(groupOrder, groupOrder.indexOf(from.slice(6)), groupOrder.indexOf(target)),
      );
      return;
    }
    const g = groupOf(from);
    // ponytail: drops across groups are ignored; "Move to group" covers it
    if (to.startsWith("group:") || !g || groupOf(to) !== g) return;
    const ids = groupIds(g);
    void act.reorder(g, arrayMove(ids, ids.indexOf(from), ids.indexOf(to)));
  };

  const newGroup = () => startNewGroup(useUi.getState().sel.ids);
  const total = list.accounts.length;
  const groupMode = selGroup !== null && sel.ids.every((id) => byId.get(id)?.group === selGroup);
  const destOf = (a: AccountView) => a.launchTarget ?? list.groupTargets[a.group];
  return (
    <div className="flex h-full gap-4 px-7 pt-1 pb-3">
      <section aria-label={t("nav.accounts")} className="@container flex min-w-0 flex-1 flex-col">
        <div className="flex h-11 shrink-0 items-center gap-2 pb-1">
          <div className="relative w-[16.25rem] min-w-[7.5rem] shrink">
            <Search
              size={14}
              strokeWidth={1.75}
              aria-hidden
              className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-text-3"
            />
            <input
              ref={searchRef}
              type="search"
              value={query}
              aria-label={t("accounts.search")}
              placeholder={t("accounts.searchShort")}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  setQuery("");
                  focusList();
                } else if (e.key === "ArrowDown") {
                  e.preventDefault();
                  focusList();
                  if (!sel.cursor) setSel(move(sel, order, 1, "replace"));
                }
              }}
              className="h-[var(--control-h)] w-full rounded-control bg-field pr-2 pl-8 @[34rem]:pr-14 text-13 text-text-1 shadow-[inset_0_0_0_1px_var(--hairline)] transition-colors placeholder:text-text-3 hover:shadow-[inset_0_0_0_1px_var(--hairline-strong)] focus-visible:shadow-[var(--focus-ring)] [&::-webkit-search-cancel-button]:hidden"
            />
            {query ? (
              <button
                type="button"
                aria-label={t("accounts.clearSearch")}
                onClick={() => {
                  setQuery("");
                  searchRef.current?.focus();
                }}
                className="absolute top-1/2 right-1 inline-grid size-6 -translate-y-1/2 place-items-center rounded-chip text-text-3 hover:bg-surface-4 hover:text-text-1"
              >
                <X size={14} strokeWidth={1.75} />
              </button>
            ) : (
              <kbd className="pointer-events-none absolute hidden @[34rem]:block top-1/2 right-2.5 -translate-y-1/2 font-sans text-12 text-text-3">
                {isMac ? "⌘F" : "Ctrl F"}
              </kbd>
            )}
          </div>
          <span className="text-12 text-text-3 tnum">
            {searching ? t("accounts.countOf", { n: order.length, total }) : total}
          </span>
          <Button className="ml-auto shrink-0" onClick={newGroup}>
            <FolderPlus size={14} strokeWidth={1.75} />
            <span className="sr-only @[34rem]:not-sr-only">{t("accounts.newGroup")}</span>
          </Button>
          <Button className="shrink-0" onClick={() => useUi.getState().openAdd("signin")}>
            <Plus size={14} strokeWidth={1.75} />
            <span className="sr-only @[34rem]:not-sr-only">{t("accounts.add")}</span>
          </Button>
        </div>

        <div
          className={clsx(
            "relative min-h-0 flex-1",
            dropping && "outline-2 -outline-offset-4 outline-accent-text outline-dashed",
          )}
          onDragOver={(e) => {
            if (!e.dataTransfer.types.includes("text/plain")) return;
            e.preventDefault();
            setDropping(true);
          }}
          onDragLeave={(e) => {
            if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropping(false);
          }}
          onDrop={(e) => {
            setDropping(false);
            const text = e.dataTransfer.getData("text/plain");
            if (!text) return;
            e.preventDefault();
            const cookies = cookiesIn(text);
            const pairs = cookies.length ? [] : pendingIn(text);
            if (cookies.length) useUi.getState().openAdd("cookies", cookies.join("\n"));
            else if (pairs.length) useUi.getState().openAdd("pending", pairs.join("\n"));
            else toast(t("drop.none"));
          }}
        >
          {rows.length === 0 ? (
            <div className="flex flex-col items-start gap-3 px-6 pt-10">
              <p className="text-14 text-text-1">{t("accounts.noMatch", { q: query.trim() })}</p>
              <Button onClick={() => setQuery("")}>{t("accounts.clearSearch")}</Button>
            </div>
          ) : (
            <div
              ref={scrollRef}
              role="grid"
              aria-label={t("accounts.listLabel")}
              aria-multiselectable
              aria-rowcount={rows.length}
              aria-activedescendant={sel.cursor ? `row-${sel.cursor}` : undefined}
              tabIndex={0}
              onKeyDown={(e) => {
                delete e.currentTarget.dataset.mouse;
                onKeyDown(e);
              }}
              onPointerDown={(e) => (e.currentTarget.dataset.mouse = "1")}
              onFocus={(e) => {
                // first arrow press picks the first row; with a selection, resume there
                if (e.target !== e.currentTarget || sel.cursor) return;
                // first shown selected row (a collapsed group's rows are hidden)
                const cursor = rows.find((r) => r.kind === "account" && sel.ids.includes(r.a.id));
                if (cursor?.kind === "account") setSel({ ...sel, cursor: cursor.a.id });
              }}
              className="accounts-grid h-full overflow-y-auto pb-2 outline-none"
            >
              <DndContext
                sensors={sensors}
                collisionDetection={collide}
                onDragStart={({ active }) => {
                  const id = String(active.id);
                  if (id.startsWith("group:")) setDragGroup(id.slice(6));
                }}
                onDragCancel={() => setDragGroup(null)}
                onDragEnd={onDragEnd}
              >
                <SortableContext items={keys} strategy={verticalListSortingStrategy}>
                  <div role="rowgroup" className="relative" style={{ height: v.getTotalSize() }}>
                    {v.getVirtualItems().map((vi) => {
                      const r = rows[vi.index];
                      if (r.kind === "group") {
                        return (
                          <GroupHeader
                            key={r.key}
                            name={r.name}
                            index={vi.index}
                            count={r.ids.length}
                            total={r.total}
                            collapsed={r.collapsed}
                            start={vi.start}
                            selected={selGroup === r.name && r.ids.every((id) => selSet.has(id))}
                            dragDisabled={searching}
                            onToggle={() => {
                              const cur = useUi.getState().sel;
                              if (!r.collapsed && cur.cursor && r.ids.includes(cur.cursor))
                                setSel({ ...cur, cursor: null });
                              useUi.setState(({ collapsed: c }) => {
                                const n = new Set(c);
                                if (n.has(r.name)) n.delete(r.name);
                                else n.add(r.name);
                                return { collapsed: n };
                              });
                            }}
                            // Ctrl/Cmd adds or removes the whole group, so several groups can play together
                            onSelect={(e) =>
                              e.ctrlKey || e.metaKey
                                ? setSel(selectGroup(useUi.getState().sel, r.ids))
                                : useUi.getState().selectGroupOf(r.name, r.ids, r.collapsed)
                            }
                            onMove={(d) => moveGroup(r.name, d)}
                            onNewGroup={newGroup}
                            onPlay={() => play(r.ids)}
                            onRename={(to) => void act.renameGroup(r.name, to)}
                          />
                        );
                      }
                      const selected = selSet.has(r.a.id);
                      return (
                        <AccountRow
                          key={r.key}
                          a={r.a}
                          index={vi.index}
                          start={vi.start}
                          selected={selected}
                          active={sel.cursor === r.a.id}
                          // unselected rows only use [a.id]: a stable array keeps their memo
                          selIds={selected ? sel.ids : NONE}
                          dest={destOf(r.a)}
                          calm={groupMode}
                          dragDisabled={searching}
                          onPick={pick}
                          onOpen={open}
                        />
                      );
                    })}
                  </div>
                </SortableContext>
                <DragOverlay dropAnimation={null} modifiers={[onPointerY]}>
                  {dragGroup && (
                    <div
                      data-pii
                      className="glass ml-3 inline-flex h-7 items-center rounded-control px-3 text-12 font-semibold text-text-1"
                    >
                      {dragGroup}
                    </div>
                  )}
                </DragOverlay>
              </DndContext>
            </div>
          )}
        </div>
      </section>
      <Inspector list={list} />
    </div>
  );
}
