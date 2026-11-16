import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Copy } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/Button";
import { Row } from "@/features/settings/controls";
import { call } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { reportError } from "@/lib/report";
import { curlCmd, maskToken, wsUrl } from "./lib";

/** Rust copies the token (`template`: `{token}` filled in) as a secret: kept out of clipboard
 *  history, cleared after 30 s and on lock. "done" only once it resolves; errors go to the banner. */
function useCopy(template: string | null) {
  const [done, setDone] = useState(0);
  // Keyed on the copy time: a repeat click restarts the 1.5 s, unmount clears it.
  useEffect(() => {
    if (!done) return;
    const id = setTimeout(() => setDone(0), 1500);
    return () => clearTimeout(id);
  }, [done]);
  const copy = () =>
    call("automation_token_copy", { template }).then(() => setDone(Date.now()), reportError);
  return [done > 0, copy] as const;
}

/** Copies the token, or `template` with `{token}` filled in by Rust. */
export function CopyButton({
  label,
  template = null,
  sm,
  ghost,
}: {
  label: string;
  template?: string | null;
  sm?: boolean;
  ghost?: boolean;
}) {
  const [done, copy] = useCopy(template);
  return (
    <Button
      sm={sm}
      variant={ghost ? "ghost" : undefined}
      aria-label={`${t("auto.copy")}: ${label}`}
      onClick={() => void copy()}
    >
      {done ? <Check size={14} strokeWidth={1.75} /> : <Copy size={14} strokeWidth={1.75} />}
      {done ? t("auto.copied") : t("auto.copy")}
    </Button>
  );
}

function CopyBlock({ label, shown, template }: { label: string; shown: string; template: string }) {
  return (
    <div className="flex flex-col gap-1 border-b border-hairline py-3.5">
      <span className="text-13 text-text-2">{label}</span>
      <div className="flex items-start gap-2">
        <code className="flex-1 rounded-control bg-surface-2 px-3 py-2 font-mono text-12 break-all text-text-2 select-text">
          {shown}
        </code>
        <CopyButton label={label} template={template} />
      </div>
    </div>
  );
}

/** Settings → Automation token rows. Masked on screen (last 4 visible); copies carry the real value. */
export function TokenRows() {
  const qc = useQueryClient();
  const { data: tok = "" } = useQuery({
    queryKey: ["autoToken"],
    queryFn: () => call("automation_token"),
  });
  const { data: st } = useQuery({
    queryKey: ["autoStatus"],
    queryFn: () => call("automation_status"),
  });
  const [confirm, setConfirm] = useState(false);

  // A failed regenerate keeps the confirm open and shows the error.
  const regenerate = () =>
    call("automation_token_regenerate").then((next) => {
      qc.setQueryData(["autoToken"], next);
      setConfirm(false);
    }, reportError);

  return (
    <>
      <Row
        id="token"
        label={t("settings.token")}
        desc={t("settings.tokenNote")}
        note={confirm ? t("set.regenConfirm") : null}
      >
        <code className="rounded-control bg-surface-2 px-3 py-2 font-mono text-13 text-text-1">
          {maskToken(tok)}
        </code>
        <CopyButton label={t("settings.token")} />
        {confirm ? (
          <span role="group" aria-label={t("set.regenConfirm")} className="flex items-center gap-2">
            <Button variant="danger" onClick={() => void regenerate()}>
              {t("set.regenYes")}
            </Button>
            <Button variant="ghost" onClick={() => setConfirm(false)}>
              {t("set.cancel")}
            </Button>
          </span>
        ) : (
          <Button onClick={() => setConfirm(true)}>{t("set.regenerate")}</Button>
        )}
      </Row>
      {st && (
        <>
          <CopyBlock
            label={t("set.curl")}
            shown={curlCmd(st.apiPort, maskToken(tok))}
            template={curlCmd(st.apiPort, "{token}")}
          />
          <CopyBlock
            label={t("set.wsExample")}
            shown={wsUrl(st.wsPort, maskToken(tok))}
            template={wsUrl(st.wsPort, "{token}")}
          />
        </>
      )}
    </>
  );
}
