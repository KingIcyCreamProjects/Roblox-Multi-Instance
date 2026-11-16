import { useEffect } from "react";
import { toast, useToasts } from "@/components/Toasts";
import { namesHidden } from "@/features/accounts/useAccounts";
import { onEvent } from "@/lib/events";
import { t } from "@/lib/i18n";
import { call } from "@/lib/ipc";
import { queryClient } from "@/lib/queryClient";
import { logError } from "@/lib/report";

const LONG = 8000;

/** Show the automation notice Rust holds, if any (it keeps one until taken, so none is lost).
 * Locked: left in Rust, taken on unlock (a toast now would be dropped). */
const takeNotice = async () =>
  useToasts.getState().locked ||
  call("automation_notice_take").then(
    (key) =>
      key &&
      void toast(
        t(key === "auto.tokenMemoryOnly" ? "auto.tokenMemoryOnly" : "auto.tokenReset"),
        undefined,
        LONG,
      ),
    (e) => logError(`automation_notice_take failed: ${String(e)}`),
  );

/** Mount once (App, so it survives a lock): toast when the local API read a cookie, or an automation notice. */
export function useSecretToast() {
  useEffect(() => {
    const offs = [
      onEvent<{ account: string }>("automation://secret", ({ account }) => {
        // locked or privacy mode: no username in the toast
        const name = namesHidden(queryClient) ? t("auto.anAccount") : account;
        toast(t("auto.secretToast", { name }), undefined, LONG);
      }),
      onEvent("automation://notice", () => void takeNotice()),
      useToasts.subscribe((s, p) => void (p.locked && !s.locked && takeNotice())),
    ];
    void takeNotice(); // emitted before this listener attached (startup, slow webview)
    return () => offs.forEach((off) => off());
  }, []);
}
