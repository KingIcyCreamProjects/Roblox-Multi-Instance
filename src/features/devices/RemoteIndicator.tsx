import { useQuery } from "@tanstack/react-query";
import { MonitorUp } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/Button";
import { call, errMessage } from "@/lib/ipc";
import { t } from "@/lib/i18n";

/** The always-on-top bar Rust shows while another computer is connected (window
 *  `remote-indicator`): who, viewing or controlling, and Stop. Closing the window stops the
 *  session too. */
export function RemoteIndicator() {
  const { data } = useQuery({
    queryKey: ["remoteStatus"],
    queryFn: () => call("remote_status"),
    refetchInterval: 2000,
  });
  const [busy, setBusy] = useState(false);
  // no banner or toasts in this window: a failed Stop says so in the bar, Stop stays to retry
  const [err, setErr] = useState<string | null>(null);
  const stop = () => {
    setBusy(true);
    setErr(null);
    call("remote_stop")
      .catch((e: unknown) => setErr(errMessage(e)))
      .finally(() => setBusy(false));
  };
  const s = data?.session;
  return (
    <div
      data-tauri-drag-region
      className="flex h-screen items-center gap-3 border border-error/40 bg-bg px-4 text-text-1"
    >
      <MonitorUp size={18} strokeWidth={1.75} className="shrink-0 text-error" aria-hidden />
      {err ? (
        <p role="alert" className="min-w-0 flex-1 truncate text-13 text-error" title={err}>
          {err}
        </p>
      ) : (
        <p role="status" data-tauri-drag-region className="min-w-0 flex-1 truncate text-13">
          {s ? t(s.control ? "rs.liveControl" : "rs.liveView", { peer: s.peer }) : t("ri.title")}
        </p>
      )}
      <Button sm variant="danger" disabled={busy} onClick={stop}>
        {t("rs.stop")}
      </Button>
    </div>
  );
}
