import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { onEvent } from "@/lib/ipc";
import { KEY_SET } from "./useSettings";

/** Rust `commands::state_changed(app, &["autoStatus", ...])`: the named query roots went stale. */
export const STATE_CHANGED = "state://changed";

/** Mount once (App, survives a lock): refetch every query under each root key Rust names. */
export function useStateSync() {
  const qc = useQueryClient();
  useEffect(() => {
    // Bursts (["snap"] every ~100 ms) must not cancel the fetch in flight, or a slow one never
    // lands: let it finish, then refetch once for whatever changed meanwhile.
    const busy = new Set<string>();
    const again = new Set<string>();
    const refresh = (k: string) => {
      if (busy.has(k)) return void again.add(k);
      busy.add(k);
      void qc.invalidateQueries({ queryKey: [k] }).finally(() => {
        busy.delete(k);
        if (again.delete(k)) refresh(k);
      });
    };
    return onEvent<string[]>(STATE_CHANGED, (keys) => {
      for (const k of keys) {
        // A settings save in flight refetches when the last one settles; a refetch now
        // would wipe its optimistic value.
        if (k === "settings" && qc.isMutating({ mutationKey: KEY_SET })) continue;
        refresh(k);
      }
    });
  }, [qc]);
}
