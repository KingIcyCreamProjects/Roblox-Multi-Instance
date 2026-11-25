import { create } from "zustand";
import { emptySel, type Sel } from "@/features/accounts/model";

export type Route = "home" | "accounts" | "discover" | "automation" | "devices" | "settings";
export type AddTab = "signin" | "cookies" | "pending" | "import";
/** Inspector field to focus next (F2, "Edit alias", "New group…"). `owner`: account id, or "multi"
 * for the batch view, so an exiting inspector view never consumes a request meant for the next one. */
export type FocusField = "alias" | "note" | "newGroup";
export type FocusReq = { field: FocusField; owner: string } | null;

type UiState = {
  route: Route;
  /** Error banner text; `moreErrors` wait behind it (FIFO), each dismiss shows the next. */
  lastError: string | null;
  moreErrors: string[];
  sel: Sel;
  /** Group whose header was clicked; the side panel shows it while `sel` is still exactly its accounts. */
  selGroup: string | null;
  focusReq: FocusReq;
  /** `nonce` changes per open so prefilled textareas remount once, not on every edit */
  add: { open: boolean; tab: AddTab; prefill: string; prefillFor: AddTab | null; nonce: number };
  signinWaiting: boolean;
  importOpen: boolean;
  toolsOpen: boolean;
  paletteOpen: boolean;
  /** A remote session is on screen: it lives in the Devices page, so the route stays put. */
  remoteOpen: boolean;
  /** Settings row to open next (a catalog path, or `#section`); the Settings page consumes it. */
  settingsJump: string | null;
  /** Collapsed account groups; here, not in the screen, so a route switch keeps them. */
  collapsed: ReadonlySet<string>;
  /** A message queues behind the shown one (duplicates dropped); `null` dismisses the shown one. */
  setError: (e: string | null) => void;
  go: (r: Route) => void;
  setSel: (s: Sel) => void;
  /** `collapsed`: its rows are hidden, so the keyboard cursor stays off them. */
  selectGroupOf: (name: string, ids: string[], collapsed?: boolean) => void;
  focusField: (field: FocusField, owner: string) => void;
  openAdd: (tab?: AddTab, prefill?: string) => void;
};

export const useUi = create<UiState>((set) => ({
  route: "accounts",
  lastError: null,
  moreErrors: [],
  sel: emptySel,
  selGroup: null,
  focusReq: null,
  add: { open: false, tab: "signin", prefill: "", prefillFor: null, nonce: 0 },
  signinWaiting: false,
  importOpen: false,
  toolsOpen: false,
  paletteOpen: false,
  remoteOpen: false,
  settingsJump: null,
  collapsed: new Set(),
  setError: (e) =>
    set((u) => {
      if (e === null)
        return { lastError: u.moreErrors[0] ?? null, moreErrors: u.moreErrors.slice(1) };
      if (u.lastError === null) return { lastError: e };
      if (e === u.lastError || u.moreErrors.includes(e)) return {};
      // ponytail: at most 5 waiting; older ones are in the log
      return { moreErrors: [...u.moreErrors, e].slice(-5) };
    }),
  go: (route) => set((u) => (u.remoteOpen ? {} : { route })),
  setSel: (sel) => set({ sel, selGroup: null }),
  selectGroupOf: (name, ids, collapsed) =>
    set({
      sel: { ids, anchor: ids[0] ?? null, cursor: collapsed ? null : (ids[0] ?? null) },
      selGroup: name,
    }),
  focusField: (field, owner) => set({ focusReq: { field, owner } }),
  openAdd: (tab = "signin", prefill = "") =>
    set((s) => ({
      add: { open: true, tab, prefill, prefillFor: prefill ? tab : null, nonce: s.add.nonce + 1 },
    })),
}));

/** Shortcut letter of a key press. A non-Latin layout reports its own letter in `key` (Ctrl+L on a
 * Russian layout is "д"), so fall back to the physical key there; Latin layouts keep `key`. */
export const keyLetter = (e: KeyboardEvent) =>
  !/^[a-z]$/i.test(e.key) && /^Key[A-Z]$/.test(e.code)
    ? e.code.slice(3).toLowerCase()
    : e.key.toLowerCase();

/** The modal surfaces. At most one is open: opening one closes the rest, whoever opened it. */
const MODALS = ["add", "importOpen", "toolsOpen", "paletteOpen"] as const;
type Modal = (typeof MODALS)[number];
const isOpen = (u: UiState, m: Modal) => (m === "add" ? u.add.open : u[m]);
useUi.subscribe((u, prev) => {
  const opened = MODALS.find((m) => isOpen(u, m) && !isOpen(prev, m));
  if (!opened) return;
  const others = MODALS.filter((m) => m !== opened && isOpen(u, m));
  if (others.length)
    useUi.setState(
      Object.fromEntries(others.map((m) => [m, m === "add" ? { ...u.add, open: false } : false])),
    );
});
