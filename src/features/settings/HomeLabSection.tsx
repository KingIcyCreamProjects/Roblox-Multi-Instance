import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/components/ui/Button";
import { openExternal } from "@/lib/events";
import { reportError } from "@/lib/report";
import { call, errMessage, hasCode } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { Row } from "./controls";

const REPO = "https://github.com/KingIcyCreamProjects/HomeLab/";
const RELEASES = `${REPO}releases`;

/** Latest HomeLab installer: version, download, GitHub link. Offline, rate limited and "not
 *  released yet" all show the same calm state. The lookup is cached until "Try again". */
export function HomeLabSection() {
  const {
    data: rel,
    error,
    isError,
    isFetching,
    refetch,
  } = useQuery({
    queryKey: ["homelabLatest"],
    queryFn: () => call("homelab_latest"),
    meta: { silent: true },
  });
  const { data: caps } = useQuery({
    queryKey: ["caps"],
    queryFn: () => call("platform_capabilities"),
  });
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ text: string; err: boolean } | null>(null);
  const win = caps?.os === "windows";

  const download = () => {
    setBusy(true);
    setMsg(null);
    call("homelab_download")
      .then(
        (path) => setMsg({ text: t("hl.saved", { path }), err: false }),
        (e) => setMsg({ text: t("hl.err", { err: errMessage(e) }), err: true }),
      )
      .finally(() => setBusy(false));
  };

  // only this repo's pages: the release JSON is not trusted to pick where the browser goes
  const page = rel?.page.startsWith(REPO) ? rel.page : RELEASES;
  const github = (
    <Button sm onClick={() => void openExternal(page).catch(reportError)}>
      {t("hl.github")}
    </Button>
  );

  return (
    <>
      {rel ? (
        <Row
          id="hl"
          label={t("hl.latest", { v: rel.version })}
          desc={t("hl.file", { file: rel.asset, mb: (rel.size / 1_000_000).toFixed(1) })}
        >
          <Button sm variant="primary" disabled={busy} onClick={download}>
            {busy ? t("hl.downloading") : t(win ? "hl.download" : "hl.downloadOnly")}
          </Button>
          {github}
        </Row>
      ) : hasCode(error, "HOMELAB_NO_BUILD") ? (
        <Row id="hl" label={t("hl.noBuild")} desc={" "}>
          {github}
        </Row>
      ) : isError ? (
        <Row id="hl" label={t("hl.offline")} desc={t("hl.offline.d")}>
          <Button sm disabled={isFetching} onClick={() => void refetch()}>
            {t("fs.retry")}
          </Button>
          {github}
        </Row>
      ) : (
        <Row id="hl" label={t("hl.checking")} desc={" "}>
          {github}
        </Row>
      )}
      {msg && (
        <p
          role={msg.err ? "alert" : "status"}
          className={msg.err ? "mt-2 text-13 text-error" : "mt-2 text-13 text-text-2"}
        >
          {msg.text}
        </p>
      )}
    </>
  );
}
