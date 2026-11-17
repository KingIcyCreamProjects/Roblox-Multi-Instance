import * as ContextMenu from "@radix-ui/react-context-menu";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import clsx from "clsx";
import { Check, ChevronRight, Ellipsis } from "lucide-react";
import { useRef, type ReactNode } from "react";
import { useSettings } from "@/app/useSettings";
import { useUi, type FocusField } from "@/app/store";
import { toast } from "@/components/Toasts";
import { runHere, useElsewhere, useMoving } from "@/features/devices/elsewhere";
import { openSite } from "./openSite";
import { targetFromBar, usePlayBar } from "@/features/playbar/store";
import { usePlay } from "@/features/playbar/usePlay";
import { inTauri, type AccountView, type CopyField } from "@/lib/ipc";
import { t, type MsgKey } from "@/lib/i18n";
import { logError } from "@/lib/report";
import { useAccountActions, useAccounts } from "./useAccounts";

// Both Radix menus share part names; the dropdown is driven through the context-menu type.
type M = typeof ContextMenu;
const Drop = DropdownMenu as unknown as M;

export const TAGS: MsgKey[] = [
  "color.1",
  "color.2",
  "color.3",
  "color.4",
  "color.5",
  "color.6",
  "color.7",
  "color.8",
];

export const profileUrl = (userId: number) => `https://www.roblox.com/users/${userId}/profile`;

export async function openExternal(url: string) {
  if (!inTauri) return void window.open(url, "_blank", "noopener");
  try {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
  } catch (e) {
    logError(`open url: ${String(e)}`);
  }
}

const content =
  "menu z-50 max-h-[calc(100vh-16px)] min-w-[220px] overflow-y-auto glass rounded-panel p-1 text-13 text-text-1";
const item = clsx(
  "flex h-8 cursor-default items-center gap-2 rounded-chip px-2.5 outline-none select-none",
  "focus-visible:shadow-none data-[highlighted]:bg-surface-4 data-[disabled]:text-text-3",
);

/** Play bar destination as a saveable target (`null` + toast when it can't be saved). */
export function useBarTarget() {
  const bar = usePlayBar();
  const get = () => {
    const target = targetFromBar(bar.destination, bar.server, bar.mode);
    if (!target) toast(t("menu.saveDestBad"));
    return target;
  };
  return { empty: !bar.destination.trim(), get };
}

/** "New group": the Inspector's group field takes the name and moves `ids` there. Nothing to move: hint. */
export function startNewGroup(ids: string[]) {
  if (!ids.length) return void toast(t("accounts.newGroupHint"));
  const ui = useUi.getState();
  const [one] = ids;
  ui.setSel(ids.length === 1 ? { ids, anchor: one, cursor: one } : { ...ui.sel, ids });
  ui.focusField("newGroup", ids.length === 1 ? one : "multi");
}

function Items({ M, a, ids }: { M: M; a: AccountView; ids: string[] }) {
  const { data: list } = useAccounts();
  const { data: s } = useSettings();
  const act = useAccountActions();
  const { play } = usePlay();
  const bar = useBarTarget();
  const saveDest = () => {
    const target = bar.get();
    if (!target) return;
    void act
      .updateMany(ids, { launchTarget: target })
      .then((ok) => ok && toast(t("menu.saveDestDone")));
  };
  const ui = useUi.getState();
  const single = ids.length === 1;
  const targets = list?.accounts.filter((x) => ids.includes(x.id)) ?? [a];
  const allPinned = targets.every((x) => x.pinned);
  const dev = !!s?.dev.enabled;
  const where = useElsewhere();
  const away = targets.filter((x) => x.userId !== null && where.has(x.userId));
  const moving = useMoving((m) => away.some((x) => m.ids.includes(x.id)));

  const focus = (field: FocusField) => {
    ui.setSel({ ids: [a.id], anchor: a.id, cursor: a.id });
    ui.focusField(field, a.id);
  };

  const sub = (labelKey: MsgKey, children: ReactNode) => (
    <M.Sub>
      <M.SubTrigger className={item}>
        <span className="flex-1">{t(labelKey)}</span>
        <ChevronRight size={14} strokeWidth={1.75} className="text-text-3" />
      </M.SubTrigger>
      <M.Portal>
        <M.SubContent className={content} sideOffset={4} alignOffset={-4}>
          {children}
        </M.SubContent>
      </M.Portal>
    </M.Sub>
  );
  const sep = <M.Separator className="mx-2 my-1 h-px bg-hairline" />;
  const copy = (field: CopyField) => (
    <M.Item className={item} onSelect={() => void act.copy(a.id, field)}>
      {t(`copy.${field}`)}
    </M.Item>
  );

  return (
    <>
      {away.length > 0 && (
        <M.Item className={item} disabled={moving} onSelect={() => void runHere(away)}>
          {moving
            ? t("elsewhere.moving")
            : away.length === 1
              ? t("elsewhere.runHere")
              : t("elsewhere.runNHere", { n: away.length })}
        </M.Item>
      )}
      <M.Item className={item} onSelect={() => play(ids)}>
        {single ? t("menu.play") : t("playbar.playN", { n: ids.length })}
      </M.Item>
      {sep}
      {single &&
        sub(
          "menu.copy",
          <>
            {copy("username")}
            {a.userId !== null && copy("userId")}
            {a.userId !== null && copy("profileLink")}
            {a.hasPassword && copy("password")}
            {dev && a.hasCookie && copy("cookie")}
          </>,
        )}
      {single && (
        <M.Item className={item} onSelect={() => focus("alias")}>
          {t("menu.editAlias")}
        </M.Item>
      )}
      {single && (
        <M.Item className={item} onSelect={() => focus("note")}>
          {t("menu.editNote")}
        </M.Item>
      )}
      {sub(
        "menu.moveTo",
        <>
          {(list?.groups ?? []).map((g) => (
            <M.Item
              key={g}
              className={item}
              onSelect={() => void act.updateMany(ids, { group: g })}
            >
              <span className="flex-1 truncate">{g}</span>
              {targets.every((x) => x.group === g) && <Check size={14} strokeWidth={2} />}
            </M.Item>
          ))}
          {sep}
          <M.Item className={item} onSelect={() => startNewGroup(ids)}>
            {t("menu.newGroup")}
          </M.Item>
        </>,
      )}
      {sub(
        "menu.setColor",
        <>
          <M.Item className={item} onSelect={() => void act.updateMany(ids, { color: null })}>
            <span aria-hidden className="size-3 rounded-full border border-hairline-strong" />
            {t("color.none")}
          </M.Item>
          {TAGS.map((k, i) => (
            <M.Item
              key={k}
              className={item}
              onSelect={() => void act.updateMany(ids, { color: i + 1 })}
            >
              <span
                aria-hidden
                className="size-3 rounded-full"
                style={{ background: `var(--tag-${i + 1})` }}
              />
              <span className="flex-1">{t(k)}</span>
              {single && a.color === i + 1 && <Check size={14} strokeWidth={2} />}
            </M.Item>
          ))}
        </>,
      )}
      <M.Item className={item} onSelect={() => void act.updateMany(ids, { pinned: !allPinned })}>
        {allPinned ? t("menu.unpin") : t("menu.pin")}
      </M.Item>
      {sep}
      <M.Item className={item} disabled={bar.empty} onSelect={saveDest}>
        {t("menu.saveDest")}
      </M.Item>
      {targets.some((x) => x.launchTarget) && (
        <M.Item className={item} onSelect={() => void act.updateMany(ids, { launchTarget: null })}>
          {t("menu.clearDest")}
        </M.Item>
      )}
      {sep}
      {single && a.userId !== null && (
        <M.Item className={item} onSelect={() => void openExternal(profileUrl(a.userId!))}>
          {t("menu.openProfile")}
        </M.Item>
      )}
      {single && (
        <M.Item className={item} onSelect={() => openSite(a.id)}>
          {t("menu.openSite")}
        </M.Item>
      )}
      {ids.length > 0 && (
        <M.Item
          className={item}
          onSelect={() =>
            useUi.setState({ sel: { ids, anchor: a.id, cursor: a.id }, toolsOpen: true })
          }
        >
          {t("menu.tools")}
        </M.Item>
      )}
      {single && (
        <M.Item className={item} onSelect={() => void act.signIn(a.id, a.username)}>
          {t("menu.signInAgain")}
        </M.Item>
      )}
      {sep}
      <M.Item
        className={clsx(item, "text-error data-[highlighted]:text-error")}
        onSelect={() => void act.remove(ids)}
      >
        {single ? t("menu.remove") : t("menu.removeN", { n: ids.length })}
      </M.Item>
    </>
  );
}

/** Right-click menu for a row. `ids`: the selection when the row is part of it, else just the row. */
export function RowMenu({
  a,
  ids,
  children,
  onOpenChange,
}: {
  a: AccountView;
  ids: string[];
  children: ReactNode;
  onOpenChange?: (open: boolean) => void;
}) {
  return (
    <ContextMenu.Root modal={false} onOpenChange={onOpenChange}>
      <ContextMenu.Trigger asChild>{children}</ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content className={content} collisionPadding={8}>
          <Items M={ContextMenu} a={a} ids={ids} />
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}

/** The "…" button: same items as the context menu. Tabbable only on the keyboard-cursor row. */
export function MoreMenu({
  a,
  ids,
  tabbable,
  onOpenChange,
}: {
  a: AccountView;
  ids: string[];
  tabbable: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  return (
    <DropdownMenu.Root modal={false} onOpenChange={onOpenChange}>
      <DropdownMenu.Trigger
        aria-label={t("menu.more")}
        tabIndex={tabbable ? 0 : -1}
        onClick={(e) => e.stopPropagation()}
        onDoubleClick={(e) => e.stopPropagation()}
        className="inline-grid size-7 place-items-center rounded-control text-text-3 opacity-0 group-hover/row:opacity-100 hover:bg-surface-4 hover:text-text-1 focus-visible:opacity-100 data-[state=open]:bg-surface-4 data-[state=open]:opacity-100"
      >
        <Ellipsis size={16} strokeWidth={1.75} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          className={content}
          align="end"
          sideOffset={4}
          collisionPadding={8}
          // React events bubble through the portal into the row (its onClick re-selects just this row)
          onClick={(e) => e.stopPropagation()}
          onKeyDown={(e) => e.stopPropagation()}
        >
          <Items M={Drop} a={a} ids={ids} />
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

/** Group header "…" button: hover-revealed like the row one. Acts on the group it sits on. */
export function GroupMenu({
  name,
  count,
  collapsed,
  onRename,
  onSelect,
  onPlay,
  onToggle,
  onNewGroup,
}: {
  name: string;
  count: number;
  collapsed: boolean;
  onRename: () => void;
  onSelect: () => void;
  onPlay: () => void;
  onToggle: () => void;
  onNewGroup: () => void;
}) {
  // Rename: the header's input takes focus, the menu must not hand it back to "…"
  const keepFocus = useRef(false);
  const sep = <DropdownMenu.Separator className="mx-2 my-1 h-px bg-hairline" />;
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger
        aria-label={t("group.menu", { name })}
        className="inline-grid size-7 place-items-center rounded-control text-text-3 opacity-0 group-hover/head:opacity-100 hover:bg-surface-4 hover:text-text-1 focus-visible:opacity-100 data-[state=open]:bg-surface-4 data-[state=open]:opacity-100"
      >
        <Ellipsis size={16} strokeWidth={1.75} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          className={content}
          align="end"
          sideOffset={4}
          collisionPadding={8}
          onClick={(e) => e.stopPropagation()}
          onCloseAutoFocus={(e) => {
            if (keepFocus.current) e.preventDefault();
            keepFocus.current = false;
          }}
        >
          <DropdownMenu.Item className={item} disabled={!count} onSelect={onPlay}>
            {count === 1 ? t("menu.play") : t("playbar.playN", { n: count })}
          </DropdownMenu.Item>
          <DropdownMenu.Item className={item} disabled={!count} onSelect={onSelect}>
            {t("group.selectAll")}
          </DropdownMenu.Item>
          {sep}
          <DropdownMenu.Item
            className={item}
            onSelect={() => {
              keepFocus.current = true;
              onRename();
            }}
          >
            {t("group.rename")}
          </DropdownMenu.Item>
          <DropdownMenu.Item className={item} onSelect={onToggle}>
            {t(collapsed ? "group.expand" : "group.collapse")}
          </DropdownMenu.Item>
          {sep}
          <DropdownMenu.Item className={item} onSelect={onNewGroup}>
            {t("menu.newGroup")}
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
