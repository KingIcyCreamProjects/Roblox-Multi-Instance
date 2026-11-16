import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback } from "react";
import { call, errMessage } from "@/lib/ipc";
import { reportError } from "@/lib/report";

export function useVaultStatus() {
  return useQuery({ queryKey: ["vault"], queryFn: () => call("vault_status") });
}

/** Locks the vault, then refetches status so the gate swaps to Unlock. Failures are reported; status stays truthful. */
export function useLock() {
  const qc = useQueryClient();
  return useCallback(async () => {
    try {
      await call("vault_lock");
    } catch (e) {
      reportError(e);
    }
    await qc.invalidateQueries({ queryKey: ["vault"] });
  }, [qc]);
}

/** Vault error → catalog text (errMessage maps the codes); unknown ones pass through. */
export const vaultError = errMessage;
