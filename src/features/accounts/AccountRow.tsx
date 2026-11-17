import * as Tooltip from "@radix-ui/react-tooltip";
import { useSortable } from "@dnd-kit/sortable";
import clsx from "clsx";
import { Folder, FolderOpen, GripVertical, Pin } from "lucide-react";
import { memo, useRef, useState, type CSSProperties, type MouseEvent } from "react";
import { Button } from "@/components/ui/Button";
import { runHere, useElsewhere, useMoving } from "@/features/devices/elsewhere";
import { fileSrc, type AccountView, type LaunchTarget, type PresenceState } from "@/lib/ipc";
import { t, type MsgKey } from "@/lib/i18n";
import { DestLabel } from "./DestLabel";
import { ago, initials, label } from "./model";
import { GroupMenu, MoreMenu, RowMenu } from "./AccountMenu";

export function Avatar({
  a,
  size = 32,
  round,
  className,
}: {
  a: AccountView;
  size?: number;
  /** circle (list rows, side panel) instead of the rounded square */
  round?: boolean;
  className?: string;
}) {
  const src = fileSrc(a.headshot);
  const [broken, setBroken] = useState(false);
  return (
    <span
      data-pii
      data-avatar={a.id}
      aria-hidden
      className={clsx(
        "inline-grid shrink-0 place-items-center overflow-hidden bg-surface-4 font-semibold text-text-2 shadow-[inset_0_0_0_1px_var(--hairline-strong)] select-none",
        round ? "rounded-full" : "rounded-control",
        className,
      )}
      // px at 100% UI scale, drawn in rem so it scales with the text
      style={{
        width: `${size / 16}rem`,
        height: `${size / 16}rem`,
        fontSize: `${(size * 0.36) / 16}rem`,
      }}
    >
      {src && !broken ? (
        <img
          src={src}
          alt=""
          loading="lazy"
          decoding="async"
          draggable={false}
          onError={() => setBroken(true)}
          className="size-full object-cover"
        />
      ) : (
        initials(a)
      )}
    </span>
  );
}

const dot: Record<PresenceState, string | null> = {
  online: "bg-online",
  in_game: "bg-online",

  in_studio: "bg-studio",
  offline: "bg-text-3",
};
export const presenceKey: Record<PresenceState, MsgKey> = {
  online: "presence.online",
  in_game: "presence.inGame",
  in_studio: "presence.inStudio",
  offline: "presence.offline",
};

export function SessionBadge({ a }: { a: AccountView }) {
  if (a.session !== "expired" && a.session !== "pending") return null;
  return (
    <span
      className={clsx(
        "shrink-0 rounded-chip border px-1.5 py-px text-12 font-medium whitespace-nowrap",
        a.session === "expired" ? "border-warn/40 text-warn" : "border-hairline-strong text-text-2",
      )}
    >
      {a.session === "expired" ? t("session.expired") : t("session.pending")}
    </span>
  );
}

/** Group header height at 100% UI scale (3.25rem): 40px row plus the gap above it. */
export const HEAD_H = 52;

/** Sortable wrapper styles: virtual offset plus the drag transform. */
function place(start: number, transform: { y: number } | null, dragging: boolean): CSSProperties {
  return {
    transform: `translate3d(0, ${start + (transform?.y ?? 0)}px, 0)`,
    zIndex: dragging ? 2 : undefined,
  };
}

/** Memoised: scrolling re-renders the list, rows only re-render when their own props change. */
export const AccountRow = memo(function AccountRow({
  a,
  index,
  start,
  selected,
  active,
  selIds,
  dest,
  calm,
  dragDisabled,
  onPick,
  onOpen,
}: {
  a: AccountView;
  /** 0-based position in the flat list (aria-rowindex is 1-based) */
  index: number;
  start: number;
  selected: boolean;
  /** keyboard cursor: aria-activedescendant target, and the row whose "…" is tabbable */
  active: boolean;
  /** current selection when this row is part of it (menus act on it); else any stable array */
  selIds: string[];
  /** short saved-destination label: its own, else its group's */
  dest: LaunchTarget | null | undefined;
  /** its group header is the selection: the header carries the highlight, not the rows */
  calm: boolean;
  dragDisabled: boolean;
  onPick: (e: MouseEvent, id: string, force?: "replace") => void;
  onOpen: (id: string) => void;
}) {
  const s = useSortable({ id: a.id, disabled: dragDisabled });
  const menuIds = selected ? selIds : [a.id];
  const state = a.presence?.state ?? "offline";
  const game = state === "in_game" ? a.presence?.gameName : null;
  // a row menu is open: no "Last played" tooltip on top of it
  const [menu, setMenu] = useState(false);
  // running on a paired computer: presence still says "In game", so say where too
  const where = useElsewhere().get(a.userId ?? NaN);
  const moving = useMoving((m) => m.ids.includes(a.id));
  return (
    <div
      ref={s.setNodeRef}
      className={clsx("absolute inset-x-0 top-0", s.isDragging && "opacity-90")}
      style={{
        ...place(start, s.transform, s.isDragging),
        height: "var(--row-h)",
        transition: s.transition,
      }}
    >
      <Tooltip.Root delayDuration={700} open={menu ? false : undefined}>
        <RowMenu a={a} ids={menuIds} onOpenChange={setMenu}>
          <Tooltip.Trigger asChild>
            <div
              id={`row-${a.id}`}
              role="row"
              aria-rowindex={index + 1}
              aria-selected={selected}
              onMouseDown={(e) => e.button === 0 && e.preventDefault()}
              onClick={(e) => onPick(e, a.id)}
              onDoubleClick={() => onOpen(a.id)}
              // replace: macOS Ctrl+click is a right-click and must not toggle into the selection
              onContextMenu={(e) => !selected && onPick(e, a.id, "replace")}
              className={clsx(
                "group/row relative mx-2 flex h-full items-center gap-3 rounded-control pr-2 pl-[1.875rem] transition-colors duration-[var(--dur-quick)]",
                selected && !calm
                  ? "bg-surface-3 shadow-[inset_0_0_0_1px_var(--hairline-strong)]"
                  : "hover:bg-surface-2",
                active && "row-active",
                s.isDragging && "bg-surface-3 shadow-float",
              )}
            >
              {a.color && (
                <span
                  aria-hidden
                  className="absolute top-3 bottom-3 left-1.5 w-[3px] rounded-full"
                  style={{ background: `var(--tag-${a.color})` }}
                />
              )}
              {/* the name keeps room first: presence text and destination give way when narrow */}
              <span role="gridcell" className="flex min-w-[6rem] flex-1 items-center gap-3">
                <span className="relative shrink-0">
                  <Avatar a={a} size={28} round />
                  <span
                    aria-hidden
                    className={clsx(
                      "absolute -right-px -bottom-px size-2 rounded-full shadow-[0_0_0_2px_var(--glass)]",
                      dot[state],
                    )}
                  />
                </span>
                <span className="flex min-w-0 flex-1 items-baseline gap-3">
                  <span data-pii className="truncate text-[0.84375rem] font-semibold text-text-1">
                    {label(a)}
                  </span>
                  {a.pinned && (
                    <Pin
                      size={12}
                      strokeWidth={2}
                      className="shrink-0 self-center text-text-3"
                      aria-label={t("accounts.pinned")}
                    />
                  )}
                  <span className="shrink-0 text-12 text-text-3 @max-[26rem]:sr-only">
                    {t(presenceKey[state])}
                  </span>
                  {game && <span className="sr-only">, {game}</span>}
                  {where && (
                    <span className="shrink-0 text-12 text-text-3 @max-[26rem]:sr-only">
                      {t("elsewhere.on", { device: where.device })}
                    </span>
                  )}
                </span>
              </span>
              <span
                role="gridcell"
                className="flex max-w-[40%] min-w-0 shrink-[3] items-center gap-2"
              >
                {a.friendPaused && (
                  <span className="shrink-0 rounded-chip border border-warn/40 px-1.5 py-px text-12 font-medium whitespace-nowrap text-warn">
                    {t("friend.paused")}
                  </span>
                )}
                <SessionBadge a={a} />
                {dest && (
                  <span className="min-w-0 truncate text-[0.71875rem] text-text-3 @max-[30rem]:hidden">
                    <DestLabel d={dest} />
                  </span>
                )}
              </span>
              <span role="gridcell" className="flex shrink-0 items-center">
                {where && (
                  <Button
                    sm
                    variant="primary"
                    className="mr-1"
                    tabIndex={active ? 0 : -1}
                    disabled={moving}
                    onClick={(e) => {
                      e.stopPropagation();
                      void runHere([a]);
                    }}
                    onDoubleClick={(e) => e.stopPropagation()}
                  >
                    {t(moving ? "elsewhere.moving" : "elsewhere.runHere")}
                  </Button>
                )}
                <button
                  type="button"
                  ref={s.setActivatorNodeRef}
                  {...s.listeners}
                  tabIndex={-1}
                  aria-hidden
                  disabled={dragDisabled}
                  onClick={(e) => e.stopPropagation()}
                  className="inline-grid h-7 w-5 cursor-grab touch-none place-items-center text-text-3 opacity-0 group-hover/row:opacity-100 hover:text-text-1 disabled:hidden"
                >
                  <GripVertical size={14} strokeWidth={1.75} />
                </button>
                <MoreMenu a={a} ids={menuIds} tabbable={active} onOpenChange={setMenu} />
              </span>
            </div>
          </Tooltip.Trigger>
        </RowMenu>
        <Tooltip.Portal>
          <Tooltip.Content
            side="top"
            align="end"
            sideOffset={-6}
            className="glass z-50 rounded-chip px-2 py-1 text-12 text-text-1 tnum"
          >
            {a.lastLaunchedAt
              ? t("accounts.lastPlayed", { when: ago(a.lastLaunchedAt) })
              : t("accounts.neverPlayed")}
          </Tooltip.Content>
        </Tooltip.Portal>
      </Tooltip.Root>
    </div>
  );
});

/** Group header row: click selects the whole group (its destination lives in the side panel);
 * the folder toggles collapse. */
export function GroupHeader({
  name,
  index,
  count,
  total,
  collapsed,
  start,
  selected,
  dragDisabled,
  onToggle,
  onSelect,
  onMove,
  onNewGroup,
  onPlay,
  onRename,
}: {
  name: string;
  index: number;
  count: number;
  total: number;
  collapsed: boolean;
  start: number;
  selected: boolean;
  dragDisabled: boolean;
  onToggle: () => void;
  onSelect: (e: { ctrlKey: boolean; metaKey: boolean }) => void;
  onMove: (delta: number) => void;
  onNewGroup: () => void;
  onPlay: () => void;
  onRename: (to: string) => void;
}) {
  const s = useSortable({ id: `group:${name}`, disabled: dragDisabled });
  const Icon = collapsed ? Folder : FolderOpen;
  const [renaming, setRenaming] = useState(false);
  // Enter/Esc then the unmount blur: only the first one counts
  const settled = useRef(false);
  const done = (v: string | null) => {
    if (settled.current) return;
    settled.current = true;
    setRenaming(false);
    const to = v?.trim();
    if (to && to !== name) onRename(to);
  };
  return (
    <div
      ref={s.setNodeRef}
      role="row"
      aria-rowindex={index + 1}
      className="group/head absolute inset-x-0 top-0 flex items-end px-2"
      style={{
        ...place(start, s.transform, s.isDragging),
        height: `${HEAD_H / 16}rem`,
        transition: s.transition,
      }}
    >
      <div
        role="gridcell"
        className={clsx(
          "flex h-10 flex-1 items-center gap-1 rounded-control pr-2 pl-2 transition-colors duration-[var(--dur-quick)]",
          selected
            ? "bg-surface-3 shadow-[inset_0_0_0_1px_var(--hairline-strong)]"
            : "hover:bg-surface-2",
          s.isDragging && "opacity-40",
        )}
      >
        <button
          type="button"
          aria-expanded={!collapsed}
          aria-label={t(collapsed ? "accounts.expandGroup" : "accounts.collapseGroup", { name })}
          onClick={onToggle}
          onKeyDown={(e) => {
            if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
              e.preventDefault();
              onMove(e.key === "ArrowUp" ? -1 : 1);
            }
          }}
          className="inline-grid size-8 shrink-0 place-items-center rounded-control text-text-2 hover:bg-surface-3 hover:text-text-1"
        >
          <Icon size={16} strokeWidth={1.75} aria-hidden />
        </button>
        {renaming ? (
          <input
            autoFocus
            data-pii
            maxLength={50}
            defaultValue={name}
            aria-label={t("group.renameLabel", { name })}
            onFocus={(e) => e.currentTarget.select()}
            onBlur={(e) => done(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") done(e.currentTarget.value);
              if (e.key === "Escape") {
                e.stopPropagation();
                done(null);
              }
            }}
            className="h-8 min-w-0 flex-1 rounded-control bg-field px-2 text-14 font-semibold text-text-1 shadow-[inset_0_0_0_1px_var(--hairline-strong)] focus-visible:shadow-[var(--focus-ring)]"
          />
        ) : (
          <button
            type="button"
            aria-pressed={selected}
            onClick={onSelect}
            className="flex h-8 min-w-0 flex-1 items-center gap-2.5 rounded-control px-1 text-left"
          >
            <span data-pii className="truncate text-14 font-semibold text-text-1">
              {name}
            </span>
            <span className="text-12 font-medium text-text-3 tnum">
              {count === total ? total : t("accounts.countOf", { n: count, total })}
            </span>
          </button>
        )}
        <button
          type="button"
          ref={s.setActivatorNodeRef}
          {...s.listeners}
          tabIndex={-1}
          aria-hidden
          disabled={dragDisabled}
          className="inline-grid h-7 w-5 cursor-grab touch-none place-items-center text-text-3 opacity-0 group-hover/head:opacity-100 hover:text-text-1 disabled:hidden"
        >
          <GripVertical size={14} strokeWidth={1.75} />
        </button>
        <GroupMenu
          name={name}
          count={count}
          collapsed={collapsed}
          onRename={() => {
            settled.current = false;
            setRenaming(true);
          }}
          onSelect={() => onSelect({ ctrlKey: false, metaKey: false })}
          onPlay={onPlay}
          onToggle={onToggle}
          onNewGroup={onNewGroup}
        />
      </div>
    </div>
  );
}

export function SkeletonRows({ n = 8 }: { n?: number }) {
  return (
    // same offsets as the loaded list: toolbar, then rows (round avatar, name, trailing label)
    <div aria-hidden className="flex flex-col pt-11">
      {Array.from({ length: n }, (_, i) => (
        <div
          key={i}
          className="mx-2 flex animate-pulse items-center gap-3 pr-2 pl-[1.875rem]"
          style={{ height: "var(--row-h)" }}
        >
          <span className="size-7 shrink-0 rounded-full bg-[var(--skeleton)]" />
          <span
            className="h-3 rounded-full bg-[var(--skeleton)]"
            style={{ width: `${22 + ((i * 17) % 20)}%` }}
          />
          <span
            className="ml-auto h-2.5 rounded-full bg-[var(--skeleton)]"
            style={{ width: `${12 + ((i * 11) % 12)}%` }}
          />
        </div>
      ))}
    </div>
  );
}
