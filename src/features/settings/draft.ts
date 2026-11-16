import { create } from "zustand";
import { merge } from "@/app/useSettings";
import { nest } from "./catalog";

/** Unsaved edits by dotted settings path. Lives outside the page so a remount keeps them. */
type Draft = {
  edits: Record<string, unknown>;
  /** Record an edit; one that equals the saved value drops the entry. */
  edit: (path: string, value: unknown, saved: unknown) => void;
  setAll: (edits: Record<string, unknown>) => void;
  clear: () => void;
  /** After a save: drop only edits that went out unchanged; ones typed meanwhile stay. */
  dropSent: (sent: Record<string, unknown>) => void;
};

export const useDraft = create<Draft>((set) => ({
  edits: {},
  edit: (path, value, saved) =>
    set(({ edits }) => {
      const rest = { ...edits };
      delete rest[path];
      return { edits: Object.is(value, saved) ? rest : { ...rest, [path]: value } };
    }),
  setAll: (edits) => set({ edits }),
  clear: () => set({ edits: {} }),
  dropSent: (sent) =>
    set(({ edits }) => ({
      edits: Object.fromEntries(
        Object.entries(edits).filter(([p, v]) => !(p in sent && Object.is(v, sent[p]))),
      ),
    })),
}));

/** One nested `settings_set` patch holding every edit. */
export const draftPatch = (edits: Record<string, unknown>) =>
  Object.entries(edits).reduce<unknown>((acc, [p, v]) => merge(acc, nest(p, v)), {});
