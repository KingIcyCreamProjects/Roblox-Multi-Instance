import {
  closestCenter,
  DndContext,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  type DragCancelEvent,
  type DragEndEvent,
  type DragOverEvent,
  type DragStartEvent,
  type UniqueIdentifier,
} from "@dnd-kit/core";
import {
  arrayMove,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { ArrowDownToLine, GripVertical, Link2, MapPin, Trash2 } from "lucide-react";
import { useState, type KeyboardEvent } from "react";
import { toast } from "@/components/Toasts";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { ago } from "@/features/accounts/model";
import { call, type Favorite, type Recent } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { reportError } from "@/lib/report";
import {
  ErrorLine,
  FAV_KEY,
  FieldLabel,
  inputCls,
  PanelEmpty,
  PanelTitle,
  pickedCls,
  Quiet,
  RECENTS_KEY,
  sendToBar,
  Split,
  Thumb,
} from "./shared";

/** Move `from` to where `to` sits; `order` follows the new positions. */
export function reorderFavs(list: Favorite[], from: string, to: string): Favorite[] {
  const i = list.findIndex((f) => f.id === from);
  const j = list.findIndex((f) => f.id === to);
  if (i < 0 || j < 0 || i === j) return list;
  return arrayMove(list, i, j).map((f, order) => ({ ...f, order }));
}

/** Undo of a remove: today's order (edits since the remove kept) with the restored id put
 * back at its old position. */
export function undoOrder(qc: QueryClient, at: number, id: string): string[] {
  const ids = (qc.getQueryData<Favorite[]>(FAV_KEY) ?? []).map((x) => x.id).filter((x) => x !== id);
  ids.splice(Math.max(0, Math.min(at, ids.length)), 0, id);
  return ids;
}

/** "Removed {name}" with Undo, which adds it back at its old position `at`. Every remove of a
 *  favourite (this tab, the Games star) gives the same way back. */
export function removedToast(qc: QueryClient, f: Favorite, at: number) {
  toast(t("favs.removed", { name: f.name }), {
    label: t("accounts.undo"),
    run: () =>
      void call("favorites_add", { placeId: f.placeId, name: f.name, privateLink: f.privateLink })
        .then((back) => call("favorites_reorder", { ids: undoOrder(qc, at, back.id) }))
        .catch(reportError)
        .finally(() => void qc.invalidateQueries({ queryKey: FAV_KEY })),
  });
}

export function Favorites() {
  const qc = useQueryClient();
  const [pick, setPick] = useState<string | null>(null);
  const q = useQuery({
    queryKey: FAV_KEY,
    queryFn: () => call("favorites_list"),
    meta: { silent: true },
  });
  const cur = q.data?.find((f) => f.id === pick) ?? q.data?.[0];
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  const resync = (e: unknown) => {
    reportError(e);
    void qc.invalidateQueries({ queryKey: FAV_KEY });
  };
  /** Optimistic edit: an in-flight refetch (sent before the edit) must not overwrite it. */
  const optimistic = (fn: (l: Favorite[] | undefined) => Favorite[] | undefined) => {
    void qc.cancelQueries({ queryKey: FAV_KEY });
    qc.setQueryData<Favorite[]>(FAV_KEY, fn);
  };
  const onDragEnd = ({ active, over }: DragEndEvent) => {
    if (!q.data || !over) return;
    const next = reorderFavs(q.data, String(active.id), String(over.id));
    if (next === q.data) return;
    optimistic(() => next);
    call("favorites_reorder", { ids: next.map((f) => f.id) }).catch(resync);
  };
  // screen readers hear the game's name in catalog text, not dnd-kit's English and raw ids
  const nameOf = (id: UniqueIdentifier) => q.data?.find((f) => f.id === id)?.name ?? "";
  const accessibility = {
    screenReaderInstructions: { draggable: t("favs.dnd.help") },
    announcements: {
      onDragStart: ({ active }: DragStartEvent) => t("favs.dnd.start", { name: nameOf(active.id) }),
      onDragOver: ({ active, over }: DragOverEvent) =>
        over ? t("favs.dnd.over", { name: nameOf(active.id), to: nameOf(over.id) }) : undefined,
      onDragEnd: ({ active, over }: DragEndEvent) =>
        over
          ? t("favs.dnd.end", { name: nameOf(active.id), to: nameOf(over.id) })
          : t("favs.dnd.cancel", { name: nameOf(active.id) }),
      onDragCancel: ({ active }: DragCancelEvent) =>
        t("favs.dnd.cancel", { name: nameOf(active.id) }),
    },
  };
  const update = (f: Favorite, patch: { name?: string; privateLink?: string | null }) => {
    optimistic((l) => l?.map((x) => (x.id === f.id ? { ...x, ...patch } : x)));
    call("favorites_update", { id: f.id, patch }).catch(resync);
  };
  const remove = (f: Favorite) => {
    const at = (q.data ?? []).findIndex((x) => x.id === f.id);
    optimistic((l) => l?.filter((x) => x.id !== f.id));
    call("favorites_remove", { id: f.id }).then(() => removedToast(qc, f, at), resync);
  };

  let body;
  if (q.isPending)
    body = (
      <div aria-hidden className="flex flex-col gap-1">
        {Array.from({ length: 4 }, (_, i) => (
          <div key={i} className="flex h-14 animate-pulse items-center gap-3 px-2">
            <span className="size-9 rounded-chip bg-[var(--skeleton)]" />
            <span className="h-3 w-1/3 rounded-full bg-[var(--skeleton)]" />
          </div>
        ))}
      </div>
    );
  else if (q.isError)
    body = <ErrorLine message={t("favs.loadErr")} retry={() => void q.refetch()} />;
  else if (!q.data.length) body = <Quiet>{t("favs.empty")}</Quiet>;
  else
    body = (
      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        onDragEnd={onDragEnd}
        accessibility={accessibility}
      >
        <SortableContext items={q.data.map((f) => f.id)} strategy={verticalListSortingStrategy}>
          <ul className="flex flex-col gap-0.5" aria-label={t("favs.title")}>
            {q.data.map((f) => (
              <FavRow key={f.id} f={f} on={cur?.id === f.id} onPick={() => setPick(f.id)} />
            ))}
          </ul>
        </SortableContext>
      </DndContext>
    );

  const list = (
    <div className="min-h-0 flex-1 overflow-auto pb-2">
      <section aria-labelledby="favs-h">
        <h2 id="favs-h" className="mb-2 px-1 text-12 font-medium text-text-3">
          {t("favs.title")}
        </h2>
        {body}
      </section>
      <Recents />
    </div>
  );
  return (
    <Split
      label={t("favs.title")}
      list={list}
      panel={
        cur ? (
          <div key={cur.id} className="flex flex-1 flex-col gap-4">
            <PanelTitle title={cur.name} sub={t("favs.title")} />
            <label className="flex flex-col gap-1.5">
              <FieldLabel>{t("favs.name")}</FieldLabel>
              <InlineText
                label={t("favs.name")}
                maxLength={100}
                value={cur.name}
                onCommit={(name) => name.trim() && update(cur, { name: name.trim() })}
              />
            </label>
            <label className="flex flex-col gap-1.5">
              <FieldLabel>{t("favs.link")}</FieldLabel>
              <InlineText
                label={t("favs.link")}
                placeholder={t("favs.linkPlaceholder")}
                value={cur.privateLink ?? ""}
                onCommit={(v) => update(cur, { privateLink: v.trim() || null })}
              />
            </label>
            <div className="flex">
              <Button
                variant="primary"
                onClick={() => sendToBar(cur.privateLink ?? String(cur.placeId), cur.name)}
              >
                <MapPin size={14} strokeWidth={1.75} />
                {t("games.setDest")}
              </Button>
            </div>
            <Button
              variant="ghost"
              className="mt-auto self-start text-text-2 hover:text-error"
              onClick={() => remove(cur)}
            >
              <Trash2 size={14} strokeWidth={1.75} />
              {t("favs.remove")}
            </Button>
          </div>
        ) : (
          <PanelEmpty>{t("discover.pick")}</PanelEmpty>
        )
      }
    />
  );
}

function FavRow({ f, on, onPick }: { f: Favorite; on: boolean; onPick: () => void }) {
  const s = useSortable({ id: f.id });
  return (
    <li
      ref={s.setNodeRef}
      style={{
        transform: s.transform ? `translate3d(0, ${s.transform.y}px, 0)` : undefined,
        transition: s.transition,
      }}
      className={clsx(
        "flex h-14 items-center gap-1 pl-1",
        pickedCls(on),
        s.isDragging && "relative z-10 bg-surface-3 shadow-float",
      )}
    >
      <button
        type="button"
        aria-label={t("favs.drag", { name: f.name })}
        className="inline-grid size-7 shrink-0 cursor-grab place-items-center rounded-chip text-text-3 hover:bg-surface-4 hover:text-text-1 active:cursor-grabbing"
        {...s.attributes}
        {...s.listeners}
      >
        <GripVertical size={14} strokeWidth={1.75} />
      </button>
      <button
        type="button"
        aria-pressed={on}
        onClick={onPick}
        className="flex h-full min-w-0 flex-1 items-center gap-3 pr-3 text-left"
      >
        <Thumb src={f.thumbnail} className="size-9 shrink-0 rounded-chip" />
        <span className="min-w-0 flex-1 truncate text-14 font-medium text-text-1">{f.name}</span>
        {f.privateLink && (
          <span className="flex items-center gap-1 text-12 text-text-3">
            <Link2 size={12} strokeWidth={1.75} aria-hidden />
            {t("dest.private")}
          </span>
        )}
      </button>
    </li>
  );
}

/** Field that saves on blur or Enter; Esc reverts. */
function InlineText({
  label,
  value,
  placeholder,
  className,
  maxLength = 200,
  onCommit,
}: {
  label: string;
  value: string;
  placeholder?: string;
  className?: string;
  maxLength?: number;
  onCommit: (v: string) => void;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const commit = () => {
    if (draft !== null && draft !== value) onCommit(draft);
    setDraft(null);
  };
  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") e.currentTarget.blur();
    if (e.key === "Escape") {
      e.stopPropagation();
      setDraft(null);
      const el = e.currentTarget;
      requestAnimationFrame(() => el.blur());
    }
  };
  return (
    <input
      aria-label={label}
      placeholder={placeholder}
      value={draft ?? value}
      maxLength={maxLength}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={onKey}
      className={clsx(inputCls, "w-full", className)}
    />
  );
}

function Recents() {
  const qc = useQueryClient();
  const q = useQuery({
    queryKey: RECENTS_KEY,
    queryFn: () => call("recents_list"),
    meta: { silent: true },
  });
  const [clearing, setClearing] = useState(false);
  const [asking, setAsking] = useState(false);
  const clear = async () => {
    setClearing(true);
    try {
      await call("recents_clear");
      qc.setQueryData<Recent[]>(RECENTS_KEY, []);
    } catch (e) {
      reportError(e);
    } finally {
      setClearing(false);
    }
  };

  let body;
  if (q.isPending)
    body = <div aria-hidden className="h-24 animate-pulse rounded-panel bg-[var(--skeleton)]" />;
  else if (q.isError)
    body = <ErrorLine message={t("recents.loadErr")} retry={() => void q.refetch()} />;
  else if (!q.data.length) body = <Quiet>{t("recents.empty")}</Quiet>;
  else
    body = (
      <ul className="flex flex-col gap-0.5" aria-label={t("recents.title")}>
        {q.data.map((r) => (
          <li
            key={`${r.placeId}-${r.lastPlayedAt}`}
            className="flex h-14 items-center gap-3 rounded-control pr-2 pl-9 hover:bg-surface-3/60"
          >
            <Thumb src={r.thumbnail} className="size-9 shrink-0 rounded-chip" />
            <span className="min-w-0 flex-1 truncate text-14 text-text-1">{r.name}</span>
            <span className="text-12 text-text-3">{ago(r.lastPlayedAt)}</span>
            <IconButton
              label={t("games.setDest")}
              onClick={() => sendToBar(String(r.placeId), r.name)}
            >
              <ArrowDownToLine size={15} strokeWidth={1.75} />
            </IconButton>
          </li>
        ))}
      </ul>
    );

  return (
    <section aria-labelledby="recents-h" className="mt-6">
      <div className="mb-2 flex items-center gap-3 px-1">
        <h2 id="recents-h" className="flex-1 text-12 font-medium text-text-3">
          {t("recents.title")}
        </h2>
        {/* the whole history, no undo: asks first, like Unblock all */}
        {asking ? (
          <>
            <span role="status" className="text-13 text-text-2">
              {t("recents.clearAsk")}
            </span>
            <Button
              sm
              variant="danger"
              onClick={() => {
                setAsking(false);
                void clear();
              }}
            >
              {t("recents.clear")}
            </Button>
            <Button autoFocus sm variant="ghost" onClick={() => setAsking(false)}>
              {t("set.cancel")}
            </Button>
          </>
        ) : (
          <Button
            sm
            variant="ghost"
            disabled={!q.data?.length || clearing}
            onClick={() => setAsking(true)}
          >
            {clearing ? t("recents.clearing") : t("recents.clear")}
          </Button>
        )}
      </div>
      {body}
    </section>
  );
}
