import { useQuery } from "@tanstack/react-query";
import { useSettings } from "@/app/useSettings";
import { call } from "@/lib/ipc";
import { t, type MsgKey } from "@/lib/i18n";
import { SubHeading } from "./controls";

const time = (secs: number | null) =>
  secs === null ? "-" : new Date(secs * 1000).toLocaleTimeString();

/** Live clients and rule state from `watcher_status`, refreshed every scan interval. */
export function WatcherPanel() {
  const { data: s } = useSettings();
  const scan = Number((s?.watcher as { scanSec?: number } | undefined)?.scanSec ?? 6);
  const { data } = useQuery({
    queryKey: ["watcherStatus"],
    queryFn: () => call("watcher_status"),
    refetchInterval: Math.max(2, scan) * 1000,
  });
  return (
    <div className="pb-3.5">
      <SubHeading>{t("watcher.panel")}</SubHeading>
      {data && !data.running && <p className="py-2 text-13 text-text-3">{t("watcher.off")}</p>}
      {data?.running && data.clients.length === 0 && (
        <p className="py-2 text-13 text-text-3">{t("watcher.noClients")}</p>
      )}
      {data && data.clients.length > 0 && (
        <table className="my-2 w-full text-left text-13 text-text-1">
          <thead className="text-text-3">
            <tr>
              <th className="py-1 font-normal">{t("watcher.col.account")}</th>
              <th className="py-1 font-normal">{t("watcher.col.pid")}</th>
              <th className="py-1 font-normal">{t("watcher.col.memory")}</th>
              <th className="py-1 font-normal">{t("watcher.col.title")}</th>
              <th className="py-1 font-normal">{t("watcher.col.server")}</th>
            </tr>
          </thead>
          <tbody>
            {data.clients.map((c) => (
              <tr key={c.pid}>
                <td className="py-1" data-pii>
                  {c.gameClient ? (c.username ?? t("watcher.unknown")) : t("watcher.notGame")}
                </td>
                <td className="py-1 font-mono">{c.pid}</td>
                <td className="py-1">{t("vs.mb", { n: c.memoryMb })}</td>
                <td className="py-1" data-pii>
                  {c.title ?? "-"}
                </td>
                <td className="py-1">{time(c.lastServerEvent)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {data && (
        <ul className="flex flex-col gap-1 text-13">
          {data.rules.map((r) => (
            <li key={r.id} className="flex justify-between gap-4">
              <span className="text-text-1">
                {t(`watcher.rule.${r.id}` as MsgKey)}{" "}
                <span className="text-text-3">
                  {r.enabled ? t("watcher.ruleOn") : t("watcher.ruleOff")}
                </span>
              </span>
              <span className="text-text-2">
                {r.lastFiredAt === null
                  ? t("watcher.never")
                  : t("watcher.fired", { t: time(r.lastFiredAt) })}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
