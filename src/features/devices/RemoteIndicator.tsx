import { useQuery } from "@tanstack/react-query";
import { MonitorUp } from "lucide-react";
import { Button } from "@/components/ui/Button";
import { call } from "@/lib/ipc";
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
  const s = data?.session;
  return (
    <div
      data-tauri-drag-region
      className="flex h-screen items-center gap-3 border border-error/40 bg-bg px-4 text-text-1"
    >
      <MonitorUp size={18} strokeWidth={1.75} className="shrink-0 text-error" aria-hidden />
      <p role="status" data-tauri-drag-region className="min-w-0 flex-1 truncate text-13">
        {s ? t(s.control ? "rs.liveControl" : "rs.liveView", { peer: s.peer }) : t("ri.title")}
      </p>
      <Button sm variant="danger" onClick={() => void call("remote_stop")}>
        {t("rs.stop")}
      </Button>
    </div>
  );
}
