import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { call, type Settings } from "@/lib/ipc";

const isObj = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** Deep merge of a settings patch (objects merge, anything else replaces). */
export const merge = (a: unknown, b: unknown): unknown =>
  isObj(a) && isObj(b)
    ? { ...a, ...Object.fromEntries(Object.entries(b).map(([k, v]) => [k, merge(a[k], v)])) }
    : b;

/** Every settings write that holds an optimistic value runs under this mutation key. */
export const KEY_SET = ["settings_set"];

export function useSettings() {
  return useQuery({ queryKey: ["settings"], queryFn: () => call("settings_get") });
}

export function useSetSettings() {
  const qc = useQueryClient();
  return useMutation({
    mutationKey: KEY_SET,
    mutationFn: (patch: unknown) => call("settings_set", { patch }),
    // Optimistic: a second quick toggle reads the advanced value, not the pre-request one.
    onMutate: async (patch) => {
      await qc.cancelQueries({ queryKey: ["settings"] });
      qc.setQueryData<Settings>(["settings"], (s) => s && (merge(s, patch) as Settings));
    },
    // A later save still in flight: keep its optimistic value; the last one to finish syncs.
    onSuccess: (s: Settings) => {
      if (qc.isMutating({ mutationKey: KEY_SET }) <= 1) qc.setQueryData(["settings"], s);
    },
    // Also on error: a failed server restart still saved the settings.
    onSettled: () => {
      if (qc.isMutating({ mutationKey: KEY_SET }) <= 1)
        void qc.invalidateQueries({ queryKey: ["settings"] });
      void qc.invalidateQueries({ queryKey: ["autoStatus"] }); // ports / LAN / enabled may have changed
    },
  });
}
