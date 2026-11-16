// Pure account-list logic: search, grouping into a flat virtual list, selection.
import type { AccountView, AccountsList, LaunchTarget } from "@/lib/ipc";
import { LOCALE, t } from "@/lib/i18n";

export const label = (a: AccountView) => a.alias || a.displayName || a.username;

/** Short text for a saved destination; `name` = the game's name once resolved (else "Place N"). */
export function destText(d: LaunchTarget | null | undefined, name?: string | null) {
  if (!d) return null;
  const game = name || t("dest.place", { place: d.placeId ?? "?" });
  const extra = d.privateLink
    ? t("dest.private")
    : d.jobId
      ? t("dest.job", { job: d.jobId.slice(0, 8) })
      : d.mode
        ? t(d.mode === "random" ? "mode.random" : "mode.smallest")
        : null;
  if (d.privateLink && !name) return extra;
  return extra ? `${game} · ${extra}` : game;
}

export function matches(a: AccountView, q: string) {
  const s = q.trim().toLowerCase();
  if (!s) return true;
  return [a.alias, a.username, a.displayName, a.note, a.group].some((f) =>
    f?.toLowerCase().includes(s),
  );
}

export type Row =
  | { kind: "group"; key: string; name: string; ids: string[]; total: number; collapsed: boolean }
  | { kind: "account"; key: string; a: AccountView };

export const groupKey = (g: string) => `group:${g}`;

/** Groups in display order, pinned first then manual order; empty groups hidden while searching. */
export function buildRows(list: AccountsList, q: string, collapsed: ReadonlySet<string>): Row[] {
  const names = [...list.groups];
  for (const a of list.accounts) if (!names.includes(a.group)) names.push(a.group);
  const rows: Row[] = [];
  for (const name of names) {
    const all = list.accounts.filter((a) => a.group === name);
    const hits = all
      .filter((a) => matches(a, q))
      .sort((x, y) => Number(y.pinned) - Number(x.pinned) || x.order - y.order);
    if (!hits.length && (q.trim() || !all.length)) continue;
    const isCollapsed = collapsed.has(name) && !q.trim();
    rows.push({
      kind: "group",
      key: groupKey(name),
      name,
      ids: hits.map((a) => a.id),
      total: all.length,
      collapsed: isCollapsed,
    });
    if (!isCollapsed) for (const a of hits) rows.push({ kind: "account", key: a.id, a });
  }
  return rows;
}

export type Sel = { ids: string[]; anchor: string | null; cursor: string | null };
export const emptySel: Sel = { ids: [], anchor: null, cursor: null };

/** Click semantics: plain replaces, toggle (Ctrl/Cmd, Space) flips one, range (Shift) spans from anchor. */
export function select(
  sel: Sel,
  order: string[],
  id: string,
  mode: "replace" | "toggle" | "range",
): Sel {
  if (mode === "toggle") {
    const on = sel.ids.includes(id);
    return { ids: on ? sel.ids.filter((x) => x !== id) : [...sel.ids, id], anchor: id, cursor: id };
  }
  if (mode === "range" && sel.anchor && order.includes(sel.anchor)) {
    const [i, j] = [order.indexOf(sel.anchor), order.indexOf(id)].sort((a, b) => a - b);
    return { ids: order.slice(i, j + 1), anchor: sel.anchor, cursor: id };
  }
  return { ids: [id], anchor: id, cursor: id };
}

/** Arrow keys: move the cursor; `extend` (Shift) grows a range, `only` (Ctrl/Cmd) leaves selection alone. */
export function move(
  sel: Sel,
  order: string[],
  delta: number,
  how: "replace" | "range" | "cursor",
): Sel {
  if (!order.length) return sel;
  const at = sel.cursor ? order.indexOf(sel.cursor) : -1;
  const next =
    order[clamp((at < 0 ? (delta < 0 ? order.length : -1) : at) + delta, 0, order.length - 1)];
  if (how === "cursor") return { ...sel, cursor: next };
  return select(how === "range" && !sel.anchor ? { ...sel, anchor: next } : sel, order, next, how);
}

/** Header checkbox: all of the group selected → remove them, else add the rest. */
export function selectGroup(sel: Sel, groupIds: string[]): Sel {
  const all = groupIds.length > 0 && groupIds.every((id) => sel.ids.includes(id));
  const ids = all
    ? sel.ids.filter((id) => !groupIds.includes(id))
    : [...sel.ids, ...groupIds.filter((id) => !sel.ids.includes(id))];
  return { ids, anchor: groupIds[0] ?? sel.anchor, cursor: sel.cursor ?? groupIds[0] ?? null };
}

export const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));

/** username:password lines in dropped text (only used when it holds no cookies). */
export const pendingIn = (text: string) =>
  text
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => /^[A-Za-z0-9_]{3,20}:\S/.test(l));

/** Cookie-shaped tokens in arbitrary dropped text, one per line. */
export const cookiesIn = (text: string) => text.match(/_\|WARNING:[^\s"';,]+/g) ?? [];

const rtf = new Intl.RelativeTimeFormat(LOCALE, { numeric: "auto" });
export function ago(iso: string, now = Date.now()) {
  const s = (new Date(iso).getTime() - now) / 1000;
  const steps: [Intl.RelativeTimeFormatUnit, number][] = [
    ["year", 31536000],
    ["month", 2592000],
    ["day", 86400],
    ["hour", 3600],
    ["minute", 60],
  ];
  for (const [unit, sec] of steps)
    if (Math.abs(s) >= sec) return rtf.format(Math.round(s / sec), unit);
  return rtf.format(0, "second");
}

export const initials = (a: AccountView) =>
  label(a)
    .replace(/[^A-Za-z0-9 ]/g, "")
    .split(/\s+|(?=[A-Z])/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0].toUpperCase())
    .join("") || "?";
